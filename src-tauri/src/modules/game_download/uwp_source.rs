//! UWP 整包获取：向 Windows Update（FE3）解析临时直链，并按官方摘要校验完整性。
//!
//! # 为什么不能像 GDK 那样存直链
//!
//! GDK 渠道的版本库给的是长期有效的 CDN 直链（`assets1.xboxlive.cn/...msixvc`），
//! 而 UWP 包在商店侧只有一个 **UpdateID**（UUID）。真正的下载地址必须每次向
//! `fe3.delivery.mp.microsoft.com` 的 `GetExtendedUpdateInfo2` 现取：返回的是
//! `tlu.dl.delivery.mp.microsoft.com` 上带签名与时间戳（`P1=...`）的临时地址，
//! 过期即 403。所以**每次下载 / 续传都必须重新解析**，不能把地址写进数据库或清单。
//!
//! # 为什么只认 tlu 主机
//!
//! 同一次响应里通常有两条 `FileLocation`：
//!
//! | 主机 | 内容 | 摘要长度 |
//! |---|---|---|
//! | `dl.delivery.mp.microsoft.com` | 差分下载用的 **CAB**（`MSCF` 魔数，通常 1 MiB） | SHA-1 |
//! | `tlu.dl.delivery.mp.microsoft.com` | **真正的安装包**（`.appx`，ZIP 魔数） | SHA-1 / SHA-256 |
//!
//! 实测（2026-10-25，`1.21.114.1`）：tlu 那条是 929,830,528 字节的 ZIP，dl 那条是
//! 1,100,177 字节的 CAB。按顺序取第一条会把 CAB 当成安装包下下来，解包必失败，
//! 而且错误看起来像「包损坏」。因此主机是硬条件，不是偏好。
//!
//! # 摘要
//!
//! 服务端给的 `FileDigest` 是 base64 的 SHA-1（20 字节）或 SHA-256（32 字节），
//! 覆盖整个文件。校验是**流式**的（GB 级包不进内存），并且与续传兼容：
//! 续传完成后仍要对最终文件整体校验一次。

use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::error::KernelError;

/// Windows Update 客户端 Web 服务端点。
pub const FE3_ENDPOINT: &str = "https://fe3.delivery.mp.microsoft.com/ClientWebService/client.asmx";
/// 请求里声明的客户端协议版本（与系统 Windows Update 代理一致）。
const CLIENT_PROTOCOL: &str = "1.81";
/// 请求 User-Agent（服务端按它判定客户端形态，缺了会返回错误文档）。
const CLIENT_USER_AGENT: &str = "Windows-Update-Agent/10.0.19041.2546 Client-Protocol/1.81";
/// 只接受这个主机的下载地址（见模块注释）。
const PACKAGE_HOST: &str = "tlu.dl.delivery.mp.microsoft.com";
/// 响应体上限。
const MAX_RESPONSE_BYTES: usize = 4 << 20;
/// 摘要校验的读缓冲。
const DIGEST_BUFFER: usize = 1024 * 1024;

/// 解析出来的一个下载位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLocation {
    pub url: String,
    pub digest: Vec<u8>,
}

/// 可直接投递给下载引擎的整包地址 + 官方摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPackage {
    pub url: String,
    pub digest: Vec<u8>,
}

impl ResolvedPackage {
    /// 数据库里保存的摘要描述（`sha1:<hex>` / `sha256:<hex>`）。
    pub fn digest_spec(&self) -> String {
        digest_spec(&self.digest)
    }
}

/// 把原始摘要转成可持久化的描述串。
pub fn digest_spec(digest: &[u8]) -> String {
    let algorithm = match digest.len() {
        20 => "sha1",
        32 => "sha256",
        _ => "unknown",
    };
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{algorithm}:{hex}")
}

/// 向 Windows Update 解析某 UpdateID 的安装包地址与摘要。
pub async fn resolve(
    client: &reqwest::Client,
    update_id: &str,
) -> Result<ResolvedPackage, KernelError> {
    if !is_uuid(update_id) {
        return Err(KernelError::InvalidArgument(format!(
            "UpdateID 非法: `{update_id}`"
        )));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let body = request_body(update_id, now);
    let response = client
        .post(FE3_ENDPOINT)
        .header("Content-Type", "application/soap+xml; charset=utf-8")
        .header("User-Agent", CLIENT_USER_AGENT)
        .body(body)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(KernelError::Config(format!(
            "Windows Update 解析失败：HTTP {status}"
        )));
    }
    let bytes = response.bytes().await?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(KernelError::Config(
            "Windows Update 响应体超过上限，拒绝解析".into(),
        ));
    }
    let text = String::from_utf8_lossy(&bytes);
    let locations = parse_file_locations(&text)?;
    select_package(&locations)
}

/// 构造 `GetExtendedUpdateInfo2` 请求体。
///
/// `created` / `expires` 为 Unix 秒（服务端用它判断请求新鲜度，实测过期时间戳
/// 也能拿到地址，但没有理由不按协议发当前时间）。
pub fn request_body(update_id: &str, created: i64) -> String {
    let created_at = format_utc(created);
    let expires_at = format_utc(created + 300);
    let message_id = uuid::Uuid::new_v4();
    format!(
        r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:a="http://www.w3.org/2005/08/addressing" xmlns:u="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd">
<s:Header><a:Action s:mustUnderstand="1">http://www.microsoft.com/SoftwareDistribution/Server/ClientWebService/GetExtendedUpdateInfo2</a:Action><a:MessageID>urn:uuid:{message_id}</a:MessageID><a:To>{endpoint}</a:To>
<o:Security s:mustUnderstand="1" xmlns:o="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd"><u:Timestamp><u:Created>{created_at}</u:Created><u:Expires>{expires_at}</u:Expires></u:Timestamp><t:WindowsUpdateTicketsToken xmlns:t="http://schemas.microsoft.com/msus/2014/10/WindowsUpdateAuthorization" u:id="ClientMSA"><t:TicketType Name="AAD" Version="1.0" Policy="MBI_SSL"/></t:WindowsUpdateTicketsToken></o:Security></s:Header>
<s:Body><GetExtendedUpdateInfo2 xmlns="http://www.microsoft.com/SoftwareDistribution/Server/ClientWebService"><protocolVersion>{protocol}</protocolVersion><updateIDs><UpdateIdentity><UpdateID>{update_id}</UpdateID><RevisionNumber>1</RevisionNumber></UpdateIdentity></updateIDs><infoTypes><XmlUpdateFragmentType>FileUrl</XmlUpdateFragmentType></infoTypes></GetExtendedUpdateInfo2></s:Body></s:Envelope>"#,
        endpoint = FE3_ENDPOINT,
        protocol = CLIENT_PROTOCOL,
    )
}

/// 解析响应里的全部 `FileLocation`。
pub fn parse_file_locations(xml: &str) -> Result<Vec<FileLocation>, KernelError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut out = Vec::new();
    let mut in_location = false;
    let mut current_url: Option<String> = None;
    let mut current_digest: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                let local = local_name(String::from_utf8_lossy(element.name().as_ref()).as_ref());
                match local.as_str() {
                    "FileLocation" => {
                        in_location = true;
                        current_url = None;
                        current_digest = None;
                    }
                    "FileDigest" if in_location => {
                        current_digest = Some(read_text(&mut reader, "FileDigest")?);
                    }
                    "Url" if in_location => {
                        current_url = Some(read_text(&mut reader, "Url")?);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(element)) => {
                let local = local_name(String::from_utf8_lossy(element.name().as_ref()).as_ref());
                if local == "FileLocation" {
                    in_location = false;
                    if let (Some(url), Some(digest)) = (current_url.take(), current_digest.take()) {
                        if let Ok(digest) = base64_decode(&digest) {
                            out.push(FileLocation { url, digest });
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(KernelError::Config(format!(
                    "Windows Update 响应解析失败: {error}"
                )))
            }
            _ => {}
        }
    }
    if out.is_empty() {
        return Err(KernelError::Config(
            "Windows Update 响应里没有可用的下载地址".into(),
        ));
    }
    Ok(out)
}

/// 在候选位置里挑出真正的安装包（`tlu` 主机 + 合法摘要）。
pub fn select_package(locations: &[FileLocation]) -> Result<ResolvedPackage, KernelError> {
    for location in locations {
        let Ok(url) = reqwest::Url::parse(location.url.trim()) else {
            continue;
        };
        // 只接受 http/https、无用户信息、无自定义端口：URL 由服务端签名下发，
        // 但拼装错误或中间人改写都会以「下载到一个奇怪的东西」暴露得很晚。
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            continue;
        }
        if !url
            .host_str()
            .map(|host| host.eq_ignore_ascii_case(PACKAGE_HOST))
            .unwrap_or(false)
        {
            continue;
        }
        if !matches!(location.digest.len(), 20 | 32) {
            continue;
        }
        // 原样返回服务端给的字符串，**不要**用 `Url` 重新序列化：签名覆盖的是查询串的
        // 原始字节，重新编码一次（`%2f` → `%2F` 之类）就可能让签名失效，症状是
        // 「解析成功但下载 403」。`Url::parse` 这里只用于校验。
        return Ok(ResolvedPackage {
            url: location.url.trim().to_string(),
            digest: location.digest.clone(),
        });
    }
    Err(KernelError::Config(
        "Windows Update 没有返回可用的安装包地址（只剩差分包）".into(),
    ))
}

/// 按 `sha1:<hex>` / `sha256:<hex>` 流式校验文件摘要。
///
/// `expected` 为空或算法未知时返回 `Err`：没有校验值就不该放行（与 GDK 渠道
/// 「清单缺 MD5 即拒绝安装」同一立场）。
pub fn verify_digest(
    path: &Path,
    expected: &str,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<bool, KernelError> {
    let (algorithm, hex) = expected
        .split_once(':')
        .ok_or_else(|| KernelError::Config(format!("摘要描述非法: `{expected}`")))?;
    let digest_type = match algorithm {
        "sha1" => openssl::hash::MessageDigest::sha1(),
        "sha256" => openssl::hash::MessageDigest::sha256(),
        other => {
            return Err(KernelError::Config(format!(
                "不支持的摘要算法: `{other}`"
            )))
        }
    };
    let expected_bytes = decode_hex(hex)
        .ok_or_else(|| KernelError::Config(format!("摘要值不是十六进制: `{hex}`")))?;
    let mut file = std::fs::File::open(path)?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut hasher = openssl::hash::Hasher::new(digest_type)
        .map_err(|e| KernelError::Config(format!("摘要器初始化失败: {e}")))?;
    let mut buffer = vec![0u8; DIGEST_BUFFER];
    let mut done: u64 = 0;
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        hasher
            .update(&buffer[..read])
            .map_err(|e| KernelError::Config(format!("摘要计算失败: {e}")))?;
        done += read as u64;
        on_progress(done, total);
    }
    let actual = hasher
        .finish()
        .map_err(|e| KernelError::Config(format!("摘要收尾失败: {e}")))?;
    Ok(actual.as_ref() == expected_bytes.as_slice())
}

/// 读一个元素的文本内容（跳过元素内部的子节点）。
fn read_text(reader: &mut Reader<&[u8]>, tag: &str) -> Result<String, KernelError> {
    // quick-xml 对超长文本会分片返回多个 `Text` 事件，必须累加而不是覆盖
    // （store-entitlement 的网络响应解析曾在这里踩过：只留第一片导致 base64 解码失败）。
    let mut text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Text(value)) => {
                let raw = String::from_utf8_lossy(value.as_ref());
                text.push_str(&raw);
            }
            // quick-xml 0.41 把实体引用拆成独立事件：`&amp;` 不在 `Text` 里。
            // 忽略它就会把 URL 里的 `&` 整个吃掉（`P1=..P2=..`），签名随之失效。
            Ok(Event::GeneralRef(reference)) => {
                let decoded = reference.decode().map_err(|error| {
                    KernelError::Config(format!(
                        "Windows Update 响应实体解析失败: {error}"
                    ))
                })?;
                match reference.resolve_char_ref() {
                    Ok(Some(ch)) => text.push(ch),
                    Ok(None) => match decoded.as_ref() {
                        "amp" => text.push('&'),
                        "lt" => text.push('<'),
                        "gt" => text.push('>'),
                        "quot" => text.push('"'),
                        "apos" => text.push('\''),
                        other => {
                            return Err(KernelError::Config(format!(
                                "Windows Update 响应含未知实体 `&{other};`"
                            )))
                        }
                    },
                    Err(error) => {
                        return Err(KernelError::Config(format!(
                            "Windows Update 响应字符引用解析失败: {error}"
                        )))
                    }
                }
            }
            Ok(Event::CData(value)) => {
                text.push_str(&String::from_utf8_lossy(value.as_ref()));
            }
            Ok(Event::End(end)) => {
                let local = local_name(String::from_utf8_lossy(end.name().as_ref()).as_ref());
                if local == tag {
                    return Ok(text.trim().to_string());
                }
            }
            Ok(Event::Eof) => {
                return Err(KernelError::Config(format!(
                    "Windows Update 响应在 `{tag}` 内意外结束"
                )))
            }
            Err(error) => {
                return Err(KernelError::Config(format!(
                    "Windows Update 响应解析失败: {error}"
                )))
            }
            Ok(_) => {}
        }
    }
}

fn local_name(raw: &str) -> String {
    raw.rsplit(':').next().unwrap_or(raw).to_string()
}

fn base64_decode(raw: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(raw.trim())
}

fn decode_hex(raw: &str) -> Option<Vec<u8>> {
    let raw = raw.trim();
    if raw.len() % 2 != 0 || raw.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(raw.len() / 2);
    for pair in raw.as_bytes().chunks(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out.push((high * 16 + low) as u8);
    }
    Some(out)
}

/// 是否为 UUID 形状（目录里存的 UpdateID 是小写带连字符）。
fn is_uuid(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            *byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

/// Unix 秒 → `YYYY-MM-DDTHH:MM:SSZ`。
///
/// 只用 `std::time`，不引第三方时间库：这里只需要把一个整数格式化成固定形状，
/// 为它加一个 chrono 依赖不值得（而且 workspace 里已经有一个 `time` 通过依赖链
/// 进来，直接用它会把版本耦合带进本模块）。
pub fn format_utc(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let rem = unix_secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant 的 `civil_from_days`：Unix 天数 → 公历年月日。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实响应（2026-10-25 实测 `1.21.114.1`，字段与顺序逐字保留）。
    const REAL_RESPONSE: &str = r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:a="http://www.w3.org/2005/08/addressing" xmlns:u="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd"><s:Header><a:Action s:mustUnderstand="1">http://www.microsoft.com/SoftwareDistribution/Server/ClientWebService/GetExtendedUpdateInfo2Response</a:Action><a:RelatesTo>urn:uuid:11111111-2222-3333-4444-555555555555</a:RelatesTo></s:Header><s:Body xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema"><GetExtendedUpdateInfo2Response xmlns="http://www.microsoft.com/SoftwareDistribution/Server/ClientWebService"><GetExtendedUpdateInfo2Result><FileLocations><FileLocation><FileDigest>oUHLzen+ADSe3ZiBD69i/mtV17o=</FileDigest><Url>http://dl.delivery.mp.microsoft.com/filestreamingservice/files/a0585ab5-12d6-4b01-bcd5-de55f7d6333f</Url></FileLocation><FileLocation><FileDigest>DjOXCoVbKuPaPIywtF613BpbpAI=</FileDigest><Url>http://tlu.dl.delivery.mp.microsoft.com/filestreamingservice/files/69fa4c18-37a4-425c-b692-773ffb380b45?P1=1792923879&amp;P2=404&amp;P3=2&amp;P4=XwA%2bzk6zPB4xsT4hmTOTqM%2f%2bf2agdKBYYwNGt73DpVtimw8EEcP%2fdtsKFE1GMR04DywvcXyHAhsDONotmizIsw%3d%3d</Url></FileLocation></FileLocations></GetExtendedUpdateInfo2Result></GetExtendedUpdateInfo2Response></s:Body></s:Envelope>"#;

    #[test]
    fn selects_tlu_package_not_differential_cab() {
        let locations = parse_file_locations(REAL_RESPONSE).unwrap();
        assert_eq!(locations.len(), 2);
        let resolved = select_package(&locations).unwrap();
        assert!(
            resolved
                .url
                .starts_with("http://tlu.dl.delivery.mp.microsoft.com/filestreamingservice/files/"),
            "必须选 tlu 主机: {}",
            resolved.url
        );
        // 签名查询串要原样保留（HTML 实体 `&amp;` 必须已解码）。
        assert!(
            resolved.url.contains("?P1=1792923879&P2=404"),
            "URL 未按预期解码: {}",
            resolved.url
        );
        assert_eq!(resolved.digest.len(), 20);
        assert_eq!(
            resolved.digest_spec(),
            "sha1:0e33970a855b2ae3da3c8cb0b45eb5dc1a5ba402"
        );
    }

    #[test]
    fn rejects_response_without_tlu() {
        let xml = r#"<Envelope><Body><FileLocations><FileLocation><FileDigest>oUHLzen+ADSe3ZiBD69i/mtV17o=</FileDigest><Url>http://dl.delivery.mp.microsoft.com/filestreamingservice/files/x</Url></FileLocation></FileLocations></Body></Envelope>"#;
        let locations = parse_file_locations(xml).unwrap();
        assert!(select_package(&locations).is_err());
    }

    #[test]
    fn rejects_suspicious_urls() {
        // 非 tlu 主机 / 带端口 / 带用户信息 / 摘要长度不对，全部跳过。
        let cases = [
            "http://evil.example.com/filestreamingservice/files/x",
            "http://tlu.dl.delivery.mp.microsoft.com:8443/filestreamingservice/files/x",
            "http://user@tlu.dl.delivery.mp.microsoft.com/filestreamingservice/files/x",
            "ftp://tlu.dl.delivery.mp.microsoft.com/filestreamingservice/files/x",
        ];
        for url in cases {
            let locations = vec![FileLocation {
                url: url.into(),
                digest: vec![0u8; 20],
            }];
            assert!(select_package(&locations).is_err(), "应拒绝 {url}");
        }
        let bad_digest = vec![FileLocation {
            url: "http://tlu.dl.delivery.mp.microsoft.com/filestreamingservice/files/x".into(),
            digest: vec![0u8; 5],
        }];
        assert!(select_package(&bad_digest).is_err());
    }

    #[test]
    fn request_body_carries_update_id_and_timestamps() {
        let body = request_body("3738c248-0603-4560-abf4-fdd66e7cd852", 1_700_000_000);
        assert!(body.contains("<UpdateID>3738c248-0603-4560-abf4-fdd66e7cd852</UpdateID>"));
        assert!(body.contains("<u:Created>2023-11-14T22:13:20Z</u:Created>"));
        assert!(body.contains("<u:Expires>2023-11-14T22:18:20Z</u:Expires>"));
        assert!(body.contains("<protocolVersion>1.81</protocolVersion>"));
        // 拒绝非法 UpdateID（不能把用户输入直接拼进 SOAP）。
        assert!(!is_uuid("not-a-uuid"));
    }

    #[test]
    fn utc_formatting_matches_known_instants() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn digest_verification_streams_and_compares() {
        let dir = std::env::temp_dir().join(format!("copper_uwp_digest_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("blob.bin");
        // 2.5 MiB：跨过 1 MiB 读缓冲，验证多次回调。
        let payload = vec![7u8; 2 * 1024 * 1024 + 512 * 1024];
        std::fs::write(&path, &payload).unwrap();

        // SHA-256(payload) 由 openssl 自己算一遍作为期望值（避免手写常量出错）。
        let mut hasher = openssl::hash::Hasher::new(openssl::hash::MessageDigest::sha256()).unwrap();
        hasher.update(&payload).unwrap();
        let expected = hasher.finish().unwrap();
        let spec = digest_spec(&expected);
        assert!(spec.starts_with("sha256:"));

        let mut samples = Vec::new();
        assert!(verify_digest(&path, &spec, &mut |done, total| samples.push((done, total))).unwrap());
        assert!(!samples.is_empty());
        assert_eq!(samples.last().unwrap().0, payload.len() as u64);
        assert_eq!(samples.last().unwrap().1, payload.len() as u64);

        // 期望值错误 → false（不是错误）。
        let wrong = format!("sha256:{}", "0".repeat(64));
        assert!(!verify_digest(&path, &wrong, &mut |_, _| {}).unwrap());
        // 未知算法 / 缺描述 → 错误（没有校验值就不放行）。
        assert!(verify_digest(&path, "md5:abcd", &mut |_, _| {}).is_err());
        assert!(verify_digest(&path, "", &mut |_, _| {}).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
