//! Git smart-HTTP v2 只读客户端：按原版 lip 的做法从仓库取引用、文件树与单个 blob。
//!
//! 不再靠 raw.githubusercontent / jsDelivr 猜清单路径，也不整仓下载归档：
//! - 列引用：command=ls-refs（带 peel 与 ref-prefix）；
//! - 列文件树：command=fetch + deepen 1 + filter blob:none，只传 commit 与 tree；
//! - 取文件：按 object id 批量 want blob，packfile 里逐个校验 SHA-1。
//!
//! 这里只实现 lip 需要的最小只读子集；不支持在客户端做合并、推送或协议 v0。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use openssl::hash::{hash, MessageDigest};

use super::{Cancel, LipError};

/// 与官方客户端一致的 User-Agent，部分 git 服务端会按 UA 决定是否启用 v2。
const USER_AGENT: &str = "git/2.0 (CopperGolem-LIP)";
const GIT_PROTOCOL: &str = "version=2";
const DISCOVER_TIMEOUT: Duration = Duration::from_secs(20);
/// 列引用的响应很小，但要防服务端挂住。
const REFS_TIMEOUT: Duration = Duration::from_secs(30);
/// 文件树请求（blob:none）通常几百 KiB，慢网下给足时间。
const TREE_TIMEOUT: Duration = Duration::from_secs(90);
/// 单批 blob 请求上限。
const BLOB_BATCH_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_WANTS_PER_REQUEST: usize = 128;
const MAX_RESPONSE_BYTES: usize = 512 * 1024 * 1024;
const MAX_OBJECT_BYTES: usize = 256 * 1024 * 1024;
const MAX_LINE_LENGTH: usize = 65520;

/// 远端引用（tag / branch），peeled 为附注标签指向的提交。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRefEntry {
    pub name: String,
    pub oid: String,
    pub peeled: Option<String>,
}

impl GitRefEntry {
    /// 该引用最终指向的提交：附注标签取 peeled，轻量标签与分支取自身。
    pub fn commit(&self) -> &str {
        self.peeled.as_deref().unwrap_or(&self.oid)
    }
}

/// 文件树里的一个条目。mode 是 git 的八进制位串按十进制读出来的值（100644 / 100755 / 120000）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: String,
    pub mode: u32,
    pub oid: String,
}

struct RemoteInfo {
    endpoint: String,
    supports_filter: bool,
    supports_shallow: bool,
}

pub struct TreeCache {
    pub entries: Vec<TreeEntry>,
}

/// Git smart-HTTP v2 客户端。tree 与 remote 信息按仓库缓存，blob 不做缓存（安装只读一次）。
pub struct GitClient {
    http: reqwest::Client,
    remotes: Mutex<HashMap<String, RemoteInfo>>,
    trees: Mutex<HashMap<String, std::sync::Arc<TreeCache>>>,
}

pub fn repo_key(owner: &str, repo: &str) -> String {
    format!("https://github.com/{}/{}.git", owner.to_ascii_lowercase(), repo.to_ascii_lowercase())
}

impl GitClient {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http, remotes: Mutex::new(HashMap::new()), trees: Mutex::new(HashMap::new()) }
    }

    /// 列出仓库里以给定前缀开头的引用。前缀为空表示全部引用。
    pub async fn list_refs(&self, owner: &str, repo: &str, prefixes: &[String], cancel: &Cancel) -> Result<Vec<GitRefEntry>, LipError> {
        cancel.check()?;
        let remote = self.discover(owner, repo, cancel).await?;
        let mut body = PktLineWriter::new();
        body.line("command=ls-refs");
        body.delimiter();
        body.line("peel");
        for prefix in prefixes {
            body.line(&format!("ref-prefix {prefix}"));
        }
        body.flush();

        let payload = self.post_command(&remote, &body.into_bytes(), REFS_TIMEOUT, cancel).await?;
        let mut reader = PktLineReader::new(&payload);
        let mut refs = Vec::new();
        loop {
            match reader.read()? {
                PktLine::Data(line) => {
                    let text = String::from_utf8_lossy(line);
                    let text = text.trim_end_matches('\n');
                    ensure_not_error(text)?;
                    refs.push(parse_ref(text)?);
                }
                PktLine::Flush | PktLine::End => break,
                PktLine::Delimiter => {
                    return Err(LipError::Network("git ls-refs: unexpected delimiter".into()));
                }
            }
        }
        if prefixes.is_empty() {
            return Ok(refs);
        }
        refs.retain(|entry| prefixes.iter().any(|prefix| entry.name.starts_with(prefix.as_str())));
        Ok(refs)
    }

    /// 列出某个提交的完整文件树。只请求 commit 与 tree，不传文件内容。
    pub async fn list_tree(&self, owner: &str, repo: &str, commit: &str, cancel: &Cancel) -> Result<std::sync::Arc<TreeCache>, LipError> {
        let oid = normalize_object_id(commit)?;
        let key = format!("{}@{oid}", repo_key(owner, repo));
        if let Ok(cache) = self.trees.lock() {
            if let Some(entries) = cache.get(&key) {
                return Ok(entries.clone());
            }
        }
        let remote = self.discover(owner, repo, cancel).await?;
        if !remote.supports_filter || !remote.supports_shallow {
            return Err(LipError::Network(format!("{owner}/{repo}: 服务端不支持 shallow / filter，无法按原版方式获取文件树")));
        }
        let pack = self.fetch_pack(&remote, &[oid.clone()], true, true, TREE_TIMEOUT, cancel).await?;
        let entries = walk_tree(&pack, &oid)?;
        let cache = std::sync::Arc::new(TreeCache { entries });
        if let Ok(mut guard) = self.trees.lock() {
            guard.insert(key, cache.clone());
        }
        Ok(cache)
    }

    /// 批量取 blob（按 object id）。返回 oid -> 内容，并逐个校验哈希。
    pub async fn fetch_blobs(&self, owner: &str, repo: &str, oids: &[String], cancel: &Cancel) -> Result<HashMap<String, Vec<u8>>, LipError> {
        let mut remaining: Vec<String> = Vec::new();
        for oid in oids {
            let oid = normalize_object_id(oid)?;
            if !remaining.contains(&oid) {
                remaining.push(oid);
            }
        }
        let mut result = HashMap::new();
        if remaining.is_empty() {
            return Ok(result);
        }
        let remote = self.discover(owner, repo, cancel).await?;
        for batch in remaining.chunks(MAX_WANTS_PER_REQUEST) {
            cancel.check()?;
            let pack = self.fetch_pack(&remote, batch, false, false, BLOB_BATCH_TIMEOUT, cancel).await?;
            for (oid, content) in &pack.objects {
                if batch.contains(oid) {
                    result.insert(oid.clone(), content.clone());
                }
            }
            let missing: Vec<&String> = batch.iter().filter(|oid| !result.contains_key(*oid)).collect();
            if !missing.is_empty() {
                return Err(LipError::Network(format!("{owner}/{repo}: 服务端没有返回 {} 个文件，例如 {}", missing.len(), missing[0])));
            }
        }
        Ok(result)
    }

    async fn discover(&self, owner: &str, repo: &str, cancel: &Cancel) -> Result<RemoteInfo, LipError> {
        let key = repo_key(owner, repo);
        if let Ok(cache) = self.remotes.lock() {
            if let Some(info) = cache.get(&key) {
                return Ok(RemoteInfo { endpoint: info.endpoint.clone(), supports_filter: info.supports_filter, supports_shallow: info.supports_shallow });
            }
        }
        cancel.check()?;
        let url = format!("{key}/info/refs?service=git-upload-pack");
        let response = tokio::time::timeout(DISCOVER_TIMEOUT, self.http.get(&url).header("Git-Protocol", GIT_PROTOCOL).header("User-Agent", USER_AGENT).send())
            .await
            .map_err(|_| LipError::Network(format!("{owner}/{repo}: 连接 git 服务端超时")))?
            .map_err(|error| LipError::Network(format!("{owner}/{repo}: {error}")))?;
        if !response.status().is_success() {
            return Err(LipError::Network(format!("{owner}/{repo}: HTTP {}", response.status())));
        }
        let payload = response.bytes().await.map_err(|error| LipError::Network(error.to_string()))?;
        let mut reader = PktLineReader::new(&payload);
        let mut line = reader.read()?;
        if let PktLine::Data(text) = line {
            if String::from_utf8_lossy(text).starts_with("# service=") {
                line = reader.read()?;
                if matches!(line, PktLine::Flush) {
                    line = reader.read()?;
                }
            }
        }
        let version_line = match line {
            PktLine::Data(text) => String::from_utf8_lossy(text).trim_end_matches('\n').to_string(),
            _ => String::new(),
        };
        if version_line != "version 2" {
            return Err(LipError::Network(format!("{owner}/{repo}: 服务端不支持 git 协议 v2")));
        }
        let mut fetch_features: Vec<String> = Vec::new();
        let mut supported = false;
        let mut format_ok = true;
        loop {
            match reader.read()? {
                PktLine::Data(text) => {
                    let text = String::from_utf8_lossy(text);
                    let text = text.trim_end_matches('\n');
                    let (name, value) = match text.split_once('=') {
                        Some((name, value)) => (name, Some(value)),
                        None => (text, None),
                    };
                    if name == "fetch" {
                        supported = true;
                        fetch_features = value.unwrap_or_default().split(' ').map(|item| item.to_string()).collect();
                    }
                    if name == "object-format" && value.unwrap_or_default() != "sha1" {
                        format_ok = false;
                    }
                }
                PktLine::Flush => break,
                other => {
                    return Err(LipError::Network(format!("{owner}/{repo}: 能力声明的 pkt-line 异常（{other:?}）")));
                }
            }
        }
        if !supported {
            return Err(LipError::Network(format!("{owner}/{repo}: 服务端没有 fetch 能力")));
        }
        if !format_ok {
            return Err(LipError::Network(format!("{owner}/{repo}: 仓库使用非 SHA-1 的对象格式")));
        }
        let info = RemoteInfo {
            endpoint: key,
            supports_filter: fetch_features.iter().any(|feature| feature == "filter"),
            supports_shallow: fetch_features.iter().any(|feature| feature == "shallow"),
        };
        if let Ok(mut guard) = self.remotes.lock() {
            guard.insert(
                repo_key(owner, repo),
                RemoteInfo { endpoint: info.endpoint.clone(), supports_filter: info.supports_filter, supports_shallow: info.supports_shallow },
            );
        }
        Ok(info)
    }

    async fn post_command(&self, remote: &RemoteInfo, body: &[u8], timeout: Duration, cancel: &Cancel) -> Result<Vec<u8>, LipError> {
        cancel.check()?;
        let url = format!("{}/git-upload-pack", remote.endpoint);
        let response = tokio::time::timeout(
            timeout,
            self.http
                .post(&url)
                .header("Git-Protocol", GIT_PROTOCOL)
                .header("User-Agent", USER_AGENT)
                .header("Accept", "application/x-git-upload-pack-result")
                .header("Content-Type", "application/x-git-upload-pack-request")
                .body(body.to_vec())
                .send(),
        )
        .await
        .map_err(|_| LipError::Network(format!("git {url}: 请求超时")))?
        .map_err(|error| LipError::Network(format!("git {url}: {error}")))?;
        if !response.status().is_success() {
            return Err(LipError::Network(format!("git {url}: HTTP {}", response.status())));
        }
        let payload = response.bytes().await.map_err(|error| LipError::Network(error.to_string()))?;
        if payload.len() > MAX_RESPONSE_BYTES {
            return Err(LipError::Network("git: 响应超过大小上限".into()));
        }
        Ok(payload.to_vec())
    }

    async fn fetch_pack(&self, remote: &RemoteInfo, wants: &[String], deepen: bool, filter_blobs: bool, timeout: Duration, cancel: &Cancel) -> Result<Pack, LipError> {
        let mut request = PktLineWriter::new();
        request.line("command=fetch");
        request.delimiter();
        request.line("no-progress");
        request.line("ofs-delta");
        if deepen {
            request.line("deepen 1");
        }
        if filter_blobs {
            request.line("filter blob:none");
        }
        for want in wants {
            request.line(&format!("want {want}"));
        }
        request.line("done");
        request.flush();
        let payload = self.post_command(remote, &request.into_bytes(), timeout, cancel).await?;
        let mut reader = PktLineReader::new(&payload);
        // 跳过 acknowledgments / shallow-info 等小节，直到 packfile 小节。
        loop {
            match reader.read()? {
                PktLine::Data(text) => {
                    let header = String::from_utf8_lossy(text).trim_end_matches('\n').to_string();
                    ensure_not_error(&header)?;
                    if header == "packfile" {
                        break;
                    }
                    loop {
                        match reader.read()? {
                            PktLine::Data(text) => {
                                let text = String::from_utf8_lossy(text).trim_end_matches('\n').to_string();
                                ensure_not_error(&text)?;
                            }
                            PktLine::Delimiter => break,
                            PktLine::Flush | PktLine::End => {
                                return Err(LipError::Network(format!("git: 响应在小节 {header} 之后缺少 packfile")));
                            }
                        }
                    }
                }
                PktLine::Delimiter => continue,
                PktLine::Flush | PktLine::End => {
                    return Err(LipError::Network("git: 响应缺少 packfile 小节".into()));
                }
            }
        }
        let mut pack_bytes: Vec<u8> = Vec::new();
        loop {
            match reader.read()? {
                PktLine::Flush | PktLine::End => break,
                PktLine::Data(data) => {
                    if data.is_empty() {
                        return Err(LipError::Network("git: packfile 小节出现空 side-band".into()));
                    }
                    match data[0] {
                        1 => {
                            pack_bytes.extend_from_slice(&data[1..]);
                            cancel.check()?;
                            if pack_bytes.len() > MAX_RESPONSE_BYTES {
                                return Err(LipError::Network("git: packfile 超过大小上限".into()));
                            }
                        }
                        2 => {}
                        3 => {
                            let message = String::from_utf8_lossy(&data[1..]).trim().to_string();
                            return Err(LipError::Network(format!("git 服务端报错：{message}")));
                        }
                        other => return Err(LipError::Network(format!("git: 非法的 side-band 通道 {other}"))),
                    }
                }
                PktLine::Delimiter => return Err(LipError::Network("git: packfile 小节出现分隔符".into())),
            }
        }
        parse_pack(&pack_bytes)
    }
}

fn ensure_not_error(line: &str) -> Result<(), LipError> {
    if let Some(rest) = line.strip_prefix("ERR ") {
        return Err(LipError::Network(format!("git 服务端报错：{rest}")));
    }
    Ok(())
}

fn parse_ref(line: &str) -> Result<GitRefEntry, LipError> {
    let mut parts = line.split(' ');
    let oid = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if !is_object_id(oid) || name.is_empty() {
        return Err(LipError::Network(format!("git: 无法解析引用行 {line}")));
    }
    let peeled = parts.find_map(|part| part.strip_prefix("peeled:").filter(|value| is_object_id(value)).map(|value| value.to_ascii_lowercase()));
    Ok(GitRefEntry { name: name.to_string(), oid: oid.to_ascii_lowercase(), peeled })
}

pub fn is_object_id(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn normalize_object_id(value: &str) -> Result<String, LipError> {
    if is_object_id(value) {
        Ok(value.to_ascii_lowercase())
    } else {
        Err(LipError::Network(format!("git: {value} 不是 40 位对象 id")))
    }
}

pub fn object_id(kind: &str, content: &[u8]) -> String {
    let mut data = Vec::with_capacity(content.len() + 32);
    data.extend_from_slice(kind.as_bytes());
    data.push(b' ');
    data.extend_from_slice(content.len().to_string().as_bytes());
    data.push(0);
    data.extend_from_slice(content);
    let digest = hash(MessageDigest::sha1(), &data).expect("sha1");
    hex(digest.as_ref())
}

pub fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Commit,
    Tree,
    Blob,
    Tag,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Commit => "commit",
            Kind::Tree => "tree",
            Kind::Blob => "blob",
            Kind::Tag => "tag",
        }
    }

    fn from_code(code: u8) -> Result<Self, LipError> {
        match code {
            1 => Ok(Kind::Commit),
            2 => Ok(Kind::Tree),
            3 => Ok(Kind::Blob),
            4 => Ok(Kind::Tag),
            other => Err(LipError::Network(format!("git: 未知对象类型 {other}"))),
        }
    }
}

enum Record {
    Base { kind: Kind, content: Vec<u8> },
    OfsDelta { base_index: usize, delta: Vec<u8> },
    RefDelta { base: String, delta: Vec<u8> },
}

/// 解析完成的 packfile：对象内容按 object id 索引。
pub struct Pack {
    pub objects: HashMap<String, Vec<u8>>,
}

fn parse_pack(bytes: &[u8]) -> Result<Pack, LipError> {
    if bytes.len() < 32 || &bytes[..4] != b"PACK" {
        return Err(LipError::Network("git: packfile 头非法".into()));
    }
    let version = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if version != 2 && version != 3 {
        return Err(LipError::Network(format!("git: 不支持的 packfile 版本 {version}")));
    }
    let count = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let body_end = bytes.len() - 20;
    let expected = hex(&bytes[body_end..]);
    let actual = hex(hash(MessageDigest::sha1(), &bytes[..body_end]).map_err(|error| LipError::Network(error.to_string()))?.as_ref());
    if expected != actual {
        return Err(LipError::Network("git: packfile 校验和不匹配".into()));
    }

    let mut position = 12usize;
    let mut records: Vec<Record> = Vec::with_capacity(count);
    let mut offsets: Vec<usize> = Vec::with_capacity(count);
    for _ in 0..count {
        let start = position;
        let mut byte = next_byte(bytes, &mut position)?;
        let type_code = (byte >> 4) & 7;
        let mut size = (byte & 0x0f) as usize;
        let mut shift = 4u32;
        while byte & 0x80 != 0 {
            if shift > 56 {
                return Err(LipError::Network("git: packfile 对象过大".into()));
            }
            byte = next_byte(bytes, &mut position)?;
            size |= ((byte & 0x7f) as usize) << shift;
            shift += 7;
        }
        if size > MAX_OBJECT_BYTES {
            return Err(LipError::Network(format!("git: packfile 对象 {size} 字节，超过上限")));
        }
        offsets.push(start);
        match type_code {
            6 => {
                let mut byte = next_byte(bytes, &mut position)?;
                let mut offset = (byte & 0x7f) as usize;
                while byte & 0x80 != 0 {
                    byte = next_byte(bytes, &mut position)?;
                    offset = ((offset + 1) << 7) | (byte & 0x7f) as usize;
                }
                if offset > start {
                    return Err(LipError::Network("git: packfile 增量基址越界".into()));
                }
                let base_offset = start - offset;
                let base_index = offsets
                    .iter()
                    .position(|value| *value == base_offset)
                    .ok_or_else(|| LipError::Network("git: packfile 增量基对象不在本包内".into()))?;
                let (delta, consumed) = inflate(bytes, position, size)?;
                position += consumed;
                records.push(Record::OfsDelta { base_index, delta });
            }
            7 => {
                if position + 20 > body_end {
                    return Err(LipError::Network("git: packfile 增量基对象 id 截断".into()));
                }
                let base = hex(&bytes[position..position + 20]);
                position += 20;
                let (delta, consumed) = inflate(bytes, position, size)?;
                position += consumed;
                records.push(Record::RefDelta { base, delta });
            }
            code => {
                let kind = Kind::from_code(code)?;
                let (content, consumed) = inflate(bytes, position, size)?;
                position += consumed;
                records.push(Record::Base { kind, content });
            }
        }
    }

    let mut resolved: Vec<Option<(Kind, Vec<u8>)>> = vec![None; records.len()];
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut objects: HashMap<String, Vec<u8>> = HashMap::new();

    for index in 0..records.len() {
        resolve_record(index, &records, &mut resolved, &mut by_id, &mut objects)?;
    }
    Ok(Pack { objects })
}

fn resolve_record(
    index: usize,
    records: &[Record],
    resolved: &mut Vec<Option<(Kind, Vec<u8>)>>,
    by_id: &mut HashMap<String, usize>,
    objects: &mut HashMap<String, Vec<u8>>,
) -> Result<(Kind, Vec<u8>), LipError> {
    if let Some((kind, content)) = &resolved[index] {
        return Ok((*kind, content.clone()));
    }
    let (kind, content) = match &records[index] {
        Record::Base { kind, content } => (*kind, content.clone()),
        Record::OfsDelta { base_index, delta } => {
            let (base_kind, base_content) = resolve_record(*base_index, records, resolved, by_id, objects)?;
            (base_kind, apply_delta(&base_content, delta)?)
        }
        Record::RefDelta { base, delta } => {
            let base_index = by_id
                .get(base)
                .copied()
                .ok_or_else(|| LipError::Network(format!("git: packfile 缺少增量基对象 {base}")))?;
            let (base_kind, base_content) = resolve_record(base_index, records, resolved, by_id, objects)?;
            (base_kind, apply_delta(&base_content, delta)?)
        }
    };
    let oid = object_id(kind.name(), &content);
    by_id.insert(oid.clone(), index);
    objects.insert(oid, content.clone());
    resolved[index] = Some((kind, content.clone()));
    Ok((kind, content))
}

fn next_byte(bytes: &[u8], position: &mut usize) -> Result<u8, LipError> {
    let byte = *bytes.get(*position).ok_or_else(|| LipError::Network("git: packfile 截断".into()))?;
    *position += 1;
    Ok(byte)
}

/// 解压一段 zlib 流，返回解压结果与实际消耗的压缩字节数。
fn inflate(bytes: &[u8], position: usize, size: usize) -> Result<(Vec<u8>, usize), LipError> {
    use flate2::{Decompress, FlushDecompress, Status};
    let input = bytes.get(position..).ok_or_else(|| LipError::Network("git: packfile 截断".into()))?;
    // 对象头部已给出解压后的精确长度，先按该长度分配；解压后超出即数据损坏。
    let mut output = vec![0u8; size.max(1)];
    let mut decompress = Decompress::new(true);
    loop {
        let in_position = decompress.total_in() as usize;
        let out_position = decompress.total_out() as usize;
        if in_position >= input.len() {
            return Err(LipError::Network("git: zlib 流未结束".into()));
        }
        if out_position >= output.len() {
            if output.len() != size {
                break;
            }
            return Err(LipError::Network("git: zlib 解压结果超过对象头声明长度".into()));
        }
        let status = decompress
            .decompress(&input[in_position..], &mut output[out_position..], FlushDecompress::None)
            .map_err(|error| LipError::Network(format!("git: zlib 解压失败 {error}")))?;
        if status == Status::StreamEnd {
            break;
        }
    }
    let written = decompress.total_out() as usize;
    if written != size {
        return Err(LipError::Network(format!("git: zlib 解压得到 {written} 字节，对象头声明 {size}")));
    }
    output.truncate(written);
    Ok((output, decompress.total_in() as usize))
}

/// 应用 git delta：先读基对象与结果长度，再按指令复制或插入。
fn apply_delta(base: &[u8], delta: &[u8]) -> Result<Vec<u8>, LipError> {
    let mut position = 0usize;
    let base_size = read_delta_size(delta, &mut position)?;
    let result_size = read_delta_size(delta, &mut position)?;
    if base_size != base.len() {
        return Err(LipError::Network("git: delta 基对象长度不匹配".into()));
    }
    if result_size > MAX_OBJECT_BYTES {
        return Err(LipError::Network(format!("git: delta 结果 {result_size} 字节，超过上限")));
    }
    let mut result = vec![0u8; result_size];
    let mut written = 0usize;
    while position < delta.len() {
        let op = delta[position];
        position += 1;
        if op & 0x80 != 0 {
            let mut offset = 0usize;
            let mut size = 0usize;
            for i in 0..4 {
                if op & (1 << i) != 0 {
                    offset |= (*delta.get(position).ok_or_else(|| LipError::Network("git: delta 截断".into()))? as usize) << (8 * i);
                    position += 1;
                }
            }
            for i in 0..3 {
                if op & (0x10 << i) != 0 {
                    size |= (*delta.get(position).ok_or_else(|| LipError::Network("git: delta 截断".into()))? as usize) << (8 * i);
                    position += 1;
                }
            }
            if size == 0 {
                size = 0x10000;
            }
            if offset + size > base.len() || written + size > result.len() {
                return Err(LipError::Network("git: delta 复制越界".into()));
            }
            result[written..written + size].copy_from_slice(&base[offset..offset + size]);
            written += size;
        } else if op != 0 {
            let size = op as usize;
            if position + size > delta.len() || written + size > result.len() {
                return Err(LipError::Network("git: delta 插入越界".into()));
            }
            result[written..written + size].copy_from_slice(&delta[position..position + size]);
            position += size;
            written += size;
        } else {
            return Err(LipError::Network("git: delta 指令非法".into()));
        }
    }
    if written != result.len() {
        return Err(LipError::Network("git: delta 结果长度不匹配".into()));
    }
    Ok(result)
}

fn read_delta_size(delta: &[u8], position: &mut usize) -> Result<usize, LipError> {
    let mut value = 0usize;
    let mut shift = 0u32;
    loop {
        let byte = *delta.get(*position).ok_or_else(|| LipError::Network("git: delta 截断".into()))?;
        *position += 1;
        value |= ((byte & 0x7f) as usize) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        if shift > 56 {
            return Err(LipError::Network("git: delta 长度过大".into()));
        }
    }
}

/// 从 pack 里按提交对象走到根 tree，递归列出所有非目录条目。
fn walk_tree(pack: &Pack, commit: &str) -> Result<Vec<TreeEntry>, LipError> {
    let mut current = commit.to_string();
    for _ in 0..8 {
        let content = pack.objects.get(&current).ok_or_else(|| LipError::Network(format!("git: 服务端没有返回对象 {current}")))?;
        let kind = detect_kind(pack, &current).ok_or_else(|| LipError::Network(format!("git: 对象 {current} 类型未知")))?;
        match kind {
            Kind::Tag => current = parse_header(content, "object")?,
            Kind::Commit => {
                let tree = parse_header(content, "tree")?;
                let mut entries = Vec::new();
                walk_tree_object(pack, &tree, "", &mut entries)?;
                return Ok(entries);
            }
            other => {
                return Err(LipError::Network(format!("git: {commit} 是 {}，不是提交", other.name())));
            }
        }
    }
    Err(LipError::Network(format!("git: {commit} 的标签链过深")))
}

fn detect_kind(pack: &Pack, oid: &str) -> Option<Kind> {
    let content = pack.objects.get(oid)?;
    for kind in [Kind::Commit, Kind::Tree, Kind::Blob, Kind::Tag] {
        if object_id(kind.name(), content) == oid {
            return Some(kind);
        }
    }
    None
}

fn parse_header(content: &[u8], key: &str) -> Result<String, LipError> {
    let end = content.windows(2).position(|window| window == b"\n\n").unwrap_or(content.len());
    let headers = String::from_utf8_lossy(&content[..end]);
    for line in headers.split('\n') {
        if let Some(rest) = line.strip_prefix(&format!("{key} ")) {
            let value = rest.trim();
            if is_object_id(value) {
                return Ok(value.to_ascii_lowercase());
            }
        }
    }
    Err(LipError::Network(format!("git: 对象缺少 {key} 头")))
}

fn walk_tree_object(pack: &Pack, tree: &str, prefix: &str, entries: &mut Vec<TreeEntry>) -> Result<(), LipError> {
    let content = pack.objects.get(tree).ok_or_else(|| LipError::Network(format!("git: 服务端没有返回 tree {tree}")))?;
    let mut position = 0usize;
    while position < content.len() {
        let space = content[position..].iter().position(|byte| *byte == b' ').map(|offset| position + offset);
        let nul = space.and_then(|space| content[space + 1..].iter().position(|byte| *byte == 0).map(|offset| space + 1 + offset));
        let (Some(space), Some(nul)) = (space, nul) else {
            return Err(LipError::Network("git: tree 条目截断".into()));
        };
        if nul + 21 > content.len() {
            return Err(LipError::Network("git: tree 条目截断".into()));
        }
        let mode_text = std::str::from_utf8(&content[position..space]).map_err(|_| LipError::Network("git: tree 模式非 ASCII".into()))?;
        let mode: u32 = mode_text.parse().map_err(|_| LipError::Network(format!("git: tree 模式非法 {mode_text}")))?;
        let name = String::from_utf8_lossy(&content[space + 1..nul]).to_string();
        let oid = hex(&content[nul + 1..nul + 21]);
        position = nul + 21;
        if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
            return Err(LipError::Network(format!("git: 仓库里存在非法文件名 {name}")));
        }
        let path = format!("{prefix}{name}");
        if mode == 40000 {
            walk_tree_object(pack, &oid, &format!("{path}/"), entries)?;
        } else if mode != 160000 {
            entries.push(TreeEntry { path, mode, oid });
        }
    }
    Ok(())
}

#[derive(Debug)]
enum PktLine<'a> {
    Data(&'a [u8]),
    Flush,
    Delimiter,
    End,
}

struct PktLineReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> PktLineReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn read(&mut self) -> Result<PktLine<'a>, LipError> {
        if self.position >= self.bytes.len() {
            return Ok(PktLine::End);
        }
        if self.position + 4 > self.bytes.len() {
            return Err(LipError::Network("git: pkt-line 头截断".into()));
        }
        let header = std::str::from_utf8(&self.bytes[self.position..self.position + 4]).map_err(|_| LipError::Network("git: pkt-line 头非法".into()))?;
        let length = usize::from_str_radix(header, 16).map_err(|_| LipError::Network(format!("git: 非法的 pkt-line 长度 {header}")))?;
        self.position += 4;
        match length {
            0 => Ok(PktLine::Flush),
            1 => Ok(PktLine::Delimiter),
            2 => Ok(PktLine::End),
            3 => Err(LipError::Network("git: 非法的 pkt-line 长度 3".into())),
            length if length > MAX_LINE_LENGTH => Err(LipError::Network(format!("git: pkt-line 过长 {length}"))),
            length => {
                let end = self.position + length - 4;
                if end > self.bytes.len() {
                    return Err(LipError::Network("git: pkt-line 负载截断".into()));
                }
                let payload = &self.bytes[self.position..end];
                self.position = end;
                Ok(PktLine::Data(payload))
            }
        }
    }
}

struct PktLineWriter {
    bytes: Vec<u8>,
}

impl PktLineWriter {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn line(&mut self, text: &str) {
        let mut payload = text.as_bytes().to_vec();
        payload.push(b'\n');
        self.bytes.extend_from_slice(format!("{:04x}", payload.len() + 4).as_bytes());
        self.bytes.extend_from_slice(&payload);
    }

    fn delimiter(&mut self) {
        self.bytes.extend_from_slice(b"0001");
    }

    fn flush(&mut self) {
        self.bytes.extend_from_slice(b"0000");
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkt_line_roundtrip() {
        let mut writer = PktLineWriter::new();
        writer.line("command=fetch");
        writer.delimiter();
        writer.line("want abc");
        writer.flush();
        let bytes = writer.into_bytes();
        let mut reader = PktLineReader::new(&bytes);
        match reader.read().unwrap() {
            PktLine::Data(data) => assert_eq!(String::from_utf8_lossy(data).trim_end(), "command=fetch"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(reader.read().unwrap(), PktLine::Delimiter));
        match reader.read().unwrap() {
            PktLine::Data(data) => assert_eq!(String::from_utf8_lossy(data).trim_end(), "want abc"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(reader.read().unwrap(), PktLine::Flush));
        assert!(matches!(reader.read().unwrap(), PktLine::End));
    }

    #[test]
    fn tree_walk_lists_files() {
        let blob = b"hello".to_vec();
        let blob_id = object_id("blob", &blob);
        let mut tree = Vec::new();
        tree.extend_from_slice(b"100644 file.txt\0");
        tree.extend_from_slice(&unhex(&blob_id));
        let tree_id = object_id("tree", &tree);
        let commit = format!("tree {tree_id}\nauthor a <a@b> 0 +0000\ncommitter a <a@b> 0 +0000\n\nmsg\n").into_bytes();
        let commit_id = object_id("commit", &commit);
        let mut objects = HashMap::new();
        objects.insert(blob_id, blob);
        objects.insert(tree_id, tree);
        objects.insert(commit_id.clone(), commit);
        let pack = Pack { objects };
        let entries = walk_tree(&pack, &commit_id).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "file.txt");
        assert_eq!(entries[0].mode, 100644);
    }

    #[test]
    fn delta_copy_and_insert() {
        let base = b"hello world".to_vec();
        let mut delta = Vec::new();
        delta.push(base.len() as u8);
        delta.push(11u8);
        // 0x91：偏移与大小各带 1 字节；0x90 会把缺省大小当成 0x10000。
        delta.push(0x91);
        delta.push(0);
        delta.push(5);
        delta.push(6);
        delta.extend_from_slice(b" there");
        let result = apply_delta(&base, &delta).unwrap();
        assert_eq!(result, b"hello there");
    }

    #[tokio::test]
    #[ignore]
    async fn probe_github_git_protocol() {
        let http = reqwest::Client::builder().user_agent("git/2.0 (lip)").build().unwrap();
        let client = GitClient::new(http);
        let cancel = Cancel::new();
        let refs = client.list_refs("GroupMountain", "GMLIB-Release", &["refs/tags/".to_string()], &cancel).await.unwrap();
        let tag = refs.iter().find(|entry| entry.name.ends_with("/v26.51.3")).expect("tag v26.51.3");
        let commit = tag.commit().to_string();
        let tree = client.list_tree("GroupMountain", "GMLIB-Release", &commit, &cancel).await.unwrap();
        let manifest = tree.entries.iter().find(|entry| entry.path.ends_with("tooth.json")).expect("tooth.json");
        let blobs = client.fetch_blobs("GroupMountain", "GMLIB-Release", &[manifest.oid.clone()], &cancel).await.unwrap();
        let text = String::from_utf8(blobs[&manifest.oid].clone()).unwrap();
        let bin: Vec<_> = tree.entries.iter().filter(|entry| entry.path.starts_with("bin/DLL/GMLIB")).map(|entry| entry.path.clone()).collect();
        println!("PROBE commit={commit} refs={} tree_files={} manifest={} bytes bin_files={}", refs.len(), tree.entries.len(), text.len(), bin.len());
        assert!(text.contains("GMLIB"));
        assert!(!bin.is_empty());
    }

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap()).collect()
    }
}
