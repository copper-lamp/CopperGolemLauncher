//! 远端访问：按原版 lip 的做法用 git smart-HTTP 读仓库（引用、文件树、清单 blob），
//! 以及下载 zip / mcmod 资产与本地档案。
//!
//! 清单不再靠 raw.githubusercontent / jsDelivr 的 URL 猜测，也不整仓下载归档：
//! 先 ls-refs 找到 tag 对应的提交，再只取 commit/tree，最后按 object id 取清单文件。

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value as Json;
use tokio::io::AsyncWriteExt;

use super::archive::ArchiveView;
use super::git::{GitClient, GitRefEntry};
use super::manifest::{self, PackageFile};
use super::source::{GitRef, PackageSource, Source};
use super::{Cancel, Environment, LipError};

const USER_AGENT: &str = "CopperGolem-LIP/1.0 (+https://github.com/CopperGolem)";
const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// 单个候选地址的连接上限。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// 下载流两段数据之间的最长静默，超过即判定该源卡死、换下一个。
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// 内置的 GitHub 加速镜像（按 URL 前缀拼接，形如 `https://gh-proxy.com/https://github.com/...`）。
const BUILTIN_URL_MIRRORS: [&str; 3] = ["https://gh-proxy.com/", "https://ghfast.top/", "https://ghproxy.net/"];
/// 内置的 GitHub 加速镜像（按主机替换，形如 `https://github.bibk.top/owner/repo/...`）。
const BUILTIN_PATH_MIRRORS: [&str; 1] = ["https://github.bibk.top/"];

/// 下载进度回调：已下载字节与响应声明的总字节（服务端不给长度时为 None）。
///
/// 引擎不把百分比写死在下载层：一次安装有多个包、多个资产，整体百分比只有
/// 上层（workspace）知道该落在哪个区间，这里只报原始字节。
pub type BytesProgress<'a> = dyn Fn(u64, Option<u64>) + Send + Sync + 'a;

/// 远端或本地解析出的包。
pub struct RemotePackage {
    pub file: PackageFile,
    /// 规范化的 format 4 JSON（写入 state.json 的 manifest）。
    pub json: Json,
    /// 解析到的 git 引用（tag/branch/commit），本地包为 None。
    pub git_ref: Option<String>,
    /// 仅在包含 self 资产时加载；否则为 None。
    pub archive: Option<ArchiveView>,
}

pub struct RemoteClient {
    http: reqwest::Client,
    git: GitClient,
    cache_dir: PathBuf,
    mirrors: Vec<String>,
}

impl RemoteClient {
    pub fn new(cache_dir: PathBuf, mirrors: Vec<String>) -> Result<Self, LipError> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::limited(8))
            .build()
            .map_err(|error| LipError::Network(error.to_string()))?;
        let git = GitClient::new(http.clone());
        Ok(Self { http, git, cache_dir, mirrors })
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub fn git_client(&self) -> &GitClient {
        &self.git
    }

    /// 解析一个包引用为可安装的包。
    pub async fn fetch_package(&self, source: &PackageSource, environment: &Environment, cancel: &Cancel, progress: Option<&BytesProgress<'_>>) -> Result<RemotePackage, LipError> {
        match &source.source {
            Source::Github { owner, repo } => self.fetch_github(owner, repo, source, environment, cancel, progress).await,
            Source::Local { path } => {
                let (archive, text) = if path.is_dir() {
                    let archive = ArchiveView::from_directory(path)?;
                    let text = read_manifest_from_dir(path)?;
                    (archive, text)
                } else {
                    let archive = ArchiveView::from_zip(path)?;
                    let text = read_manifest_from_archive(&archive)?;
                    (archive, text)
                };
                let (file, json) = self.parse_manifest_with_json(&text, environment)?;
                let needs_archive = file.variants.values().flatten().any(|entry| entry.assets.iter().any(|asset| matches!(asset, manifest::Asset::SelfArchive { .. })));
                let archive = if needs_archive { Some(archive) } else { None };
                Ok(RemotePackage { file, json, git_ref: None, archive })
            }
            Source::ArchiveUrl { url } => {
                cancel.check()?;
                let temp = self.cache_dir.join(format!("archive-{}.zip", short_hash(url)));
                self.download_zip(&[url.clone()], &temp, cancel, progress).await?;
                let archive = ArchiveView::from_zip(&temp)?;
                let text = read_manifest_from_archive(&archive)?;
                let (file, json) = self.parse_manifest_with_json(&text, environment)?;
                let needs_archive = file.variants.values().flatten().any(|entry| entry.assets.iter().any(|asset| matches!(asset, manifest::Asset::SelfArchive { .. })));
                Ok(RemotePackage { file, json, git_ref: None, archive: if needs_archive { Some(archive) } else { None } })
            }
        }
    }

    /// 列出包的可选版本（仅 GitHub 仓库；本地包返回自身版本）。
    ///
    /// 版本来源与官方实现一致：仓库的 tag 就是版本，按 semver 排序。
    pub async fn versions(&self, source: &PackageSource, cancel: &Cancel) -> Result<Vec<semver::Version>, LipError> {
        match &source.source {
            Source::Github { owner, repo } => {
                let refs = self.git.list_refs(owner, repo, &["refs/tags/".to_string()], cancel).await?;
                let mut versions: Vec<semver::Version> = refs
                    .iter()
                    .filter_map(|entry| entry.name.strip_prefix("refs/tags/"))
                    .filter_map(|name| semver::Version::parse(name.trim_start_matches('v')).ok())
                    .collect();
                versions.sort();
                versions.dedup();
                Ok(versions)
            }
            Source::Local { path } => {
                let environment = Environment::current();
                let text = if path.is_dir() {
                    read_manifest_from_dir(path)?
                } else {
                    let archive = ArchiveView::from_zip(path)?;
                    read_manifest_from_archive(&archive)?
                };
                let (file, _) = self.parse_manifest_with_json(&text, &environment)?;
                Ok(vec![file.version()?])
            }
            Source::ArchiveUrl { .. } => Ok(Vec::new()),
        }
    }

    async fn fetch_github(&self, owner: &str, repo: &str, source: &PackageSource, environment: &Environment, cancel: &Cancel, progress: Option<&BytesProgress<'_>>) -> Result<RemotePackage, LipError> {
        cancel.check()?;
        let refs = self
            .git
            .list_refs(owner, repo, &["refs/tags/".to_string(), "refs/heads/".to_string()], cancel)
            .await?;
        let (reference_name, commit) = match &source.reference {
            Some(GitRef::Commit(value)) => (value.clone(), value.clone()),
            Some(GitRef::Tag(name)) => {
                let entry = find_tag(&refs, name).ok_or_else(|| LipError::PackageNotFound(format!("{owner}/{repo}: 没有 tag {name}")))?;
                (entry.name.clone(), entry.commit().to_string())
            }
            Some(GitRef::Branch(name)) => {
                let full = format!("refs/heads/{}", name.trim_start_matches("refs/heads/"));
                let entry = refs.iter().find(|entry| entry.name == full).ok_or_else(|| LipError::PackageNotFound(format!("{owner}/{repo}: 没有分支 {name}")))?;
                (entry.name.clone(), entry.commit().to_string())
            }
            Some(GitRef::Version(version)) => {
                let entry = find_version_tag(&refs, version).ok_or_else(|| LipError::PackageNotFound(format!("{owner}/{repo}: 没有版本 {version}")))?;
                (entry.name.clone(), entry.commit().to_string())
            }
            Some(GitRef::Latest) | None => {
                let entry = latest_release(&refs).ok_or_else(|| LipError::PackageNotFound(format!("{owner}/{repo}: 仓库没有可用的 tag 或默认分支")))?;
                (entry.name.clone(), entry.commit().to_string())
            }
        };
        self.fetch_github_at(owner, repo, &reference_name, &commit, environment, cancel, progress).await
    }

    async fn fetch_github_at(&self, owner: &str, repo: &str, reference_name: &str, commit: &str, environment: &Environment, cancel: &Cancel, progress: Option<&BytesProgress<'_>>) -> Result<RemotePackage, LipError> {
        cancel.check()?;
        // 只取提交与文件树（filter blob:none），再按需取清单文件本身。
        let tree = self.git.list_tree(owner, repo, commit, cancel).await?;
        let manifest_entry = tree
            .entries
            .iter()
            .find(|entry| entry.path == manifest::PACKAGE_FILE)
            .or_else(|| tree.entries.iter().find(|entry| entry.path == manifest::LEGACY_PACKAGE_FILE))
            .or_else(|| tree.entries.iter().find(|entry| entry.path.ends_with(&format!("/{}", manifest::PACKAGE_FILE))))
            .or_else(|| tree.entries.iter().find(|entry| entry.path.ends_with(&format!("/{}", manifest::LEGACY_PACKAGE_FILE))))
            .ok_or_else(|| LipError::InvalidManifest(format!("{owner}/{repo}@{reference_name}: 仓库里没有 lip.toml 或 tooth.json")))?;
        let blobs = self.git.fetch_blobs(owner, repo, std::slice::from_ref(&manifest_entry.oid), cancel).await?;
        let bytes = blobs
            .get(&manifest_entry.oid)
            .ok_or_else(|| LipError::Network(format!("{owner}/{repo}: 没有取到清单文件 {}", manifest_entry.path)))?;
        let text = String::from_utf8(bytes.clone()).map_err(|_| LipError::InvalidManifest(format!("{} is not UTF-8", manifest_entry.path)))?;
        let (file, json) = self.parse_manifest_with_json(&text, environment)?;

        // self 资产不再整仓下载归档：官方实现是按要放置的路径从文件树里按需取 blob，
        // 整仓 zip 对大仓库（GMLIB 等）意味着几十 MB 的无谓下载。这里只记录提交，
        // 由 workspace 在真正放置文件时按需批量取 blob。
        let _ = progress;
        Ok(RemotePackage { file, json, git_ref: Some(commit.to_string()), archive: None })
    }

    /// 依次尝试候选地址（原始 URL 及其镜像），下载第一个成功的归档。
    pub async fn download_zip(&self, urls: &[String], dest: &Path, cancel: &Cancel, progress: Option<&BytesProgress<'_>>) -> Result<(), LipError> {
        let mut last_error = String::from("no url");
        for url in urls {
            for candidate in self.asset_candidates(url) {
                cancel.check()?;
                match self.download_one(&candidate, dest, cancel, progress).await {
                    Ok(()) => return Ok(()),
                    Err(error) => last_error = format!("{candidate}: {error}"),
                }
            }
        }
        Err(LipError::Network(format!("all download sources failed: {last_error}")))
    }

    /// 把一个 GitHub 资产地址展开成候选列表：原始地址在前，镜像在后。
    ///
    /// 只改写 github.com 上的地址；其它主机（自建分发、CDN）保持原样，避免把无关的
    /// 请求也送进第三方代理。镜像按「内置 + `COPPER_LIP_MIRROR` 自定义」拼接，自定义
    /// 里写 `{url}` 表示按模板替换，否则当作前缀直接拼接。
    fn asset_candidates(&self, url: &str) -> Vec<String> {
        let mut candidates = vec![url.to_string()];
        let Some(rest) = url.strip_prefix("https://github.com/").or_else(|| url.strip_prefix("http://github.com/")) else {
            return candidates;
        };
        let mut push = |candidate: String| {
            if !candidate.is_empty() && !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        };
        for mirror in &self.mirrors {
            if mirror.contains("{url}") {
                push(mirror.replace("{url}", url));
            } else {
                push(format!("{}{url}", if mirror.ends_with('/') { mirror.clone() } else { format!("{mirror}/") }));
            }
        }
        for prefix in BUILTIN_URL_MIRRORS {
            push(format!("{prefix}{url}"));
        }
        for prefix in BUILTIN_PATH_MIRRORS {
            push(format!("{prefix}{rest}"));
        }
        candidates
    }

    async fn download_one(&self, url: &str, dest: &Path, cancel: &Cancel, progress: Option<&BytesProgress<'_>>) -> Result<(), LipError> {
        let response = tokio::time::timeout(CONNECT_TIMEOUT, self.http.get(url).send())
            .await
            .map_err(|_| LipError::Network(format!("{url}: connect timed out")))?
            .map_err(|error| LipError::Network(error.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            // 404 是最常见的失败：上游打了 tag 却没上传发布文件（CoralFans 26.51.2 就是）。
            // 把判断写进错误文本，用户和日志都不用再猜。
            let hint = if status == reqwest::StatusCode::NOT_FOUND { "（上游该版本可能未上传发布文件）" } else { "" };
            return Err(LipError::Network(format!("HTTP {status}{hint}")));
        }
        let expected = response.content_length();
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(LipError::Io)?;
        }
        // 先写 .part 再改名：缓存目录里出现 asset-*.zip 就意味着该文件已完整校验收尾。
        // 直接写目标名会让「下载到一半进程被杀」留下半截 zip，下一次命中缓存时解包失败。
        let part = dest.with_extension("zip.part");
        let mut file = tokio::fs::File::create(&part).await.map_err(LipError::Io)?;
        let mut stream = response.bytes_stream();
        let mut total = 0u64;
        let mut reported = 0u64;
        let mut failure: Option<LipError> = None;
        if let Some(progress) = progress {
            progress(0, expected);
        }
        loop {
            let chunk = match tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next()).await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(_) => {
                    failure = Some(LipError::Network(format!("{url}: download stalled")));
                    break;
                }
            };
            if let Err(error) = cancel.check() {
                failure = Some(error);
                break;
            }
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    failure = Some(LipError::Network(error.to_string()));
                    break;
                }
            };
            total += chunk.len() as u64;
            if total > MAX_DOWNLOAD_BYTES {
                failure = Some(LipError::Network("download exceeds size limit".into()));
                break;
            }
            if let Err(error) = file.write_all(&chunk).await {
                failure = Some(LipError::Io(error));
                break;
            }
            // 每 256 KiB 报一次：太小会淹没事件通道，太大又看不出在动。
            if let Some(progress) = progress {
                if total - reported >= 256 * 1024 {
                    reported = total;
                    progress(total, expected);
                }
            }
        }
        if failure.is_none() {
            if let Some(progress) = progress {
                progress(total, expected.or(Some(total)));
            }
        }
        let flushed = if failure.is_none() { file.flush().await } else { Ok(()) };
        drop(file);
        if failure.is_none() {
            if let Err(error) = flushed {
                failure = Some(LipError::Io(error));
            }
        }
        if failure.is_none() {
            // 归档必须以 ZIP 本地文件头或空档头开始，避免把 HTML 错误页当 zip。
            match tokio::fs::read(&part).await {
                Ok(header) if header.starts_with(b"PK\x03\x04") || header.starts_with(b"PK\x05\x06") => {}
                Ok(_) => failure = Some(LipError::Network("response is not a zip archive".into())),
                Err(error) => failure = Some(LipError::Io(error)),
            }
        }
        if let Some(error) = failure {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(error);
        }
        tokio::fs::rename(&part, dest).await.map_err(LipError::Io)?;
        Ok(())
    }

    fn parse_manifest(&self, text: &str, environment: &Environment) -> Result<PackageFile, LipError> {
        Ok(self.parse_manifest_with_json(text, environment)?.0)
    }

    /// 自动区分 lip.toml 与 tooth.json。
    fn parse_manifest_with_json(&self, text: &str, environment: &Environment) -> Result<(PackageFile, Json), LipError> {
        let trimmed = text.trim_start();
        if trimmed.starts_with('{') {
            let mut json: Json = serde_json::from_str(text).map_err(|error| LipError::InvalidManifest(format!("invalid tooth.json: {error}")))?;
            render_tooth_templates(&mut json);
            let Some(file) = manifest::from_tooth_json(&json, environment)? else {
                return Err(LipError::InvalidManifest("unsupported legacy tooth format".into()));
            };
            let canonical = serde_json::to_value(&file).map_err(LipError::Json)?;
            Ok((file, canonical))
        } else {
            manifest::parse_package_file(text)
        }
    }
}

fn find_tag<'a>(refs: &'a [GitRefEntry], name: &str) -> Option<&'a GitRefEntry> {
    if name.starts_with("refs/") {
        return refs.iter().find(|entry| entry.name == name);
    }
    let full = format!("refs/tags/{name}");
    refs.iter().find(|entry| entry.name == full)
}

fn find_version_tag<'a>(refs: &'a [GitRefEntry], version: &semver::Version) -> Option<&'a GitRefEntry> {
    let candidates = [format!("refs/tags/v{version}"), format!("refs/tags/{version}")];
    candidates.iter().find_map(|candidate| refs.iter().find(|entry| &entry.name == candidate))
}

/// 最新版本：优先 semver 最高的 tag，没有 tag 时退回默认分支。
fn latest_release(refs: &[GitRefEntry]) -> Option<&GitRefEntry> {
    let mut best: Option<(semver::Version, &GitRefEntry)> = None;
    for entry in refs.iter().filter(|entry| entry.name.starts_with("refs/tags/")) {
        let name = entry.name.trim_start_matches("refs/tags/");
        if let Ok(version) = semver::Version::parse(name.trim_start_matches('v')) {
            if best.as_ref().map(|(current, _)| version > *current).unwrap_or(true) {
                best = Some((version, entry));
            }
        }
    }
    if let Some((_, entry)) = best {
        return Some(entry);
    }
    for branch in ["refs/heads/main", "refs/heads/master"] {
        if let Some(entry) = refs.iter().find(|entry| entry.name == branch) {
            return Some(entry);
        }
    }
    refs.iter().find(|entry| entry.name.starts_with("refs/heads/"))
}

/// tooth.json 里的模板变量由包自身字段提供。
fn render_tooth_templates(json: &mut Json) {
    let tooth = json.get("tooth").and_then(Json::as_str).unwrap_or_default().to_string();
    let version = json.get("version").and_then(Json::as_str).unwrap_or_default().to_string();
    fn walk(value: &mut Json, tooth: &str, version: &str) {
        match value {
            Json::String(text) => {
                if text.contains("{{") {
                    *text = text.replace("{{tooth}}", tooth).replace("{{version}}", version).replace("{{path}}", tooth);
                }
            }
            Json::Array(items) => items.iter_mut().for_each(|item| walk(item, tooth, version)),
            Json::Object(map) => map.values_mut().for_each(|item| walk(item, tooth, version)),
            _ => {}
        }
    }
    walk(json, &tooth, &version);
}

fn read_manifest_from_dir(directory: &Path) -> Result<String, LipError> {
    for name in [manifest::PACKAGE_FILE, manifest::LEGACY_PACKAGE_FILE] {
        let path = directory.join(name);
        if path.is_file() {
            return std::fs::read_to_string(&path).map_err(LipError::Io);
        }
    }
    Err(LipError::InvalidManifest(format!("{} has no lip.toml or tooth.json", directory.display())))
}

fn read_manifest_from_archive(archive: &ArchiveView) -> Result<String, LipError> {
    for name in [manifest::PACKAGE_FILE, manifest::LEGACY_PACKAGE_FILE] {
        if archive.contains_file(name) {
            return String::from_utf8(archive.read(name)?).map_err(|_| LipError::InvalidManifest(format!("{name} is not UTF-8")));
        }
    }
    // GitHub 归档通常带一层 <repo>-<ref>/ 前缀。
    if let Some(candidate) = archive.files().iter().find(|file| file.ends_with("/lip.toml") || file.ends_with("/tooth.json")) {
        let bytes = archive.read(candidate)?;
        return String::from_utf8(bytes).map_err(|_| LipError::InvalidManifest(format!("{candidate} is not UTF-8")));
    }
    Err(LipError::InvalidManifest("archive has no lip.toml or tooth.json".into()))
}

fn short_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    digest[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, oid: &str) -> GitRefEntry {
        GitRefEntry { name: name.to_string(), oid: oid.to_string(), peeled: None }
    }

    #[test]
    fn renders_tooth_templates() {
        let mut json = serde_json::json!({
            "tooth": "github.com/a/b",
            "version": "1.2.3",
            "asset_url": "https://{{tooth}}/releases/v{{version}}/a.zip"
        });
        render_tooth_templates(&mut json);
        assert_eq!(json["asset_url"], "https://github.com/a/b/releases/v1.2.3/a.zip");
    }

    #[test]
    fn latest_release_prefers_high_semver_tag() {
        let refs = vec![
            entry("refs/tags/v1.2.0", "a000000000000000000000000000000000000001"),
            entry("refs/tags/v1.10.0", "a000000000000000000000000000000000000002"),
            entry("refs/heads/main", "a000000000000000000000000000000000000003"),
        ];
        assert_eq!(latest_release(&refs).unwrap().name, "refs/tags/v1.10.0");
    }

    #[test]
    fn version_tag_matches_both_spellings() {
        let refs = vec![entry("refs/tags/26.51.3", "a000000000000000000000000000000000000001")];
        let version = semver::Version::parse("26.51.3").unwrap();
        assert_eq!(find_version_tag(&refs, &version).unwrap().name, "refs/tags/26.51.3");
    }
}
