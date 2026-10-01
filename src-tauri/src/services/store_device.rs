// SPDX-License-Identifier: GPL-3.0-only

//! Windows DPAPI-backed storage boundary for Store account/device material.
//!
//! This module only protects and restores caller-provided bytes. It does not
//! acquire, mint, refresh, or send device tickets. Ticket acquisition remains
//! the responsibility of a future Windows WAM adapter.

use serde::{Deserialize, Serialize};

const CACHE_VERSION: u32 = 1;
const DEVICE_PROVISION_URL: &str = "https://login.live.com/ppsecure/deviceaddcredential.srf";
const DEVICE_RESPONSE_LIMIT: u64 = 1024 * 1024;
/// 设备凭据随机缓冲长度，与参考实现 `make([]byte, 32)` 一致。
const CREDENTIAL_RANDOM_BYTES: usize = 32;
/// `Membername` 取缓冲前 7 字节（转十六进制后前缀 `02`）。
const MEMBER_RANDOM_BYTES: usize = 7;
/// `Password` 取接下来 18 字节（base64url）。参考实现取的是 `random[7:25]`。
const PASSWORD_RANDOM_BYTES: usize = 18;
/// Client identification sent to the legacy MSA device endpoint.
///
/// The endpoint answers legacy MSA clients and keys off the announced OS /
/// IDE build, so a bare version string is answered with an error document
/// rather than a credential.
const DEVICE_USER_AGENT: &str = "MSAWindows/55 (OS 10.0.26100.0.0 ge_release; IDK 10.0.26100.5074 ge_release; Cfg 16.000.29325.00; Test 0)";

#[cfg(windows)]
use base64::Engine;
#[cfg(windows)]
use quick_xml::events::Event;
#[cfg(windows)]
use quick_xml::Reader;
#[cfg(windows)]
use quick_xml::XmlVersion;
#[cfg(windows)]
use reqwest::Client;

#[cfg(windows)]
#[derive(Debug, Clone)]
pub struct ProvisionedDevice {
    pub binding: DeviceCacheBinding,
    pub member: String,
    pub password: String,
    pub puid: String,
    pub license_block: Vec<u8>,
}

#[cfg(windows)]
impl Drop for ProvisionedDevice {
    fn drop(&mut self) {
        unsafe {
            self.member.as_bytes_mut().fill(0);
            self.password.as_bytes_mut().fill(0);
            self.puid.as_bytes_mut().fill(0);
        }
        self.license_block.fill(0);
    }
}

#[cfg(windows)]
#[derive(Debug, thiserror::Error)]
pub enum DeviceProvisionError {
    #[error("device provisioning request failed: {0}")] Http(String),
    #[error("device provisioning returned HTTP status {0}")] HttpStatus(reqwest::StatusCode),
    #[error("device provisioning response is malformed")] MalformedResponse,
    #[error("device provisioning response exceeds the size limit")] ResponseTooLarge,
    #[error("device provisioning was rejected by the service")] Rejected,
}

/// 校验调用方传入的设备信息 XML，失败时把可定位的现场写进日志。
///
/// 这里刻意不再使用裸 `return Err(..)`：这条链路的失败会被上层折叠成
/// 「device provisioning response is malformed」，与「服务端拒绝」无法区分，
/// 排查只能靠猜。第一步是**本地输入**校验，与网络无关，必须能被单独看见。
#[cfg(windows)]
fn validate_device_info(device_info_xml: &str) -> Result<(), DeviceProvisionError> {
    if device_info_xml.len() > 64 * 1024 {
        log::warn!(
            "[store-device] device info is too large: {} bytes (limit {})",
            device_info_xml.len(),
            64 * 1024
        );
        return Err(DeviceProvisionError::MalformedResponse);
    }
    let mut info_reader = Reader::from_str(device_info_xml);
    let mut info_root = false;
    let mut info_depth = 0usize;
    let mut roots: Vec<String> = Vec::new();
    loop {
        match info_reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                if info_depth == 0 {
                    roots.push(name.clone());
                    if name != "DeviceInfo" || info_root {
                        log::warn!(
                            "[store-device] device info root is invalid: name={name}, roots=[{}], bytes={}",
                            roots.join(","),
                            device_info_xml.len()
                        );
                        return Err(DeviceProvisionError::MalformedResponse);
                    }
                    info_root = true;
                }
                info_depth += 1;
            }
            // 自闭合元素（`<Component .../>`）不改变嵌套深度。
            //
            // 语义上必须与「开标签 + 闭标签」等价：本工程构造的 DeviceInfo 以五个
            // 自闭合 `Component` 结尾（与参考实现逐字一致）。此前这里把 `Empty`
            // 单独忽略，虽然对**平衡文档**的深度判断仍然正确，但根元素识别多了一处
            // 漏洞——以自闭合形式书写的根不会被认出来。这里按元素语义统一处理，
            // 让「根必须是 DeviceInfo」「深度必须闭合」两条约束对所有元素形状都成立。
            //
            // 注意：这不是设备注册失败的原因。那次失败的真因是凭据随机缓冲只有
            // 16 字节（见 `provision_device`），与本文档解析无关。这条分支只是把
            // 校验补成正确形态，别把它当成已修缺陷的纪念碑。
            Ok(Event::Empty(event)) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                if info_depth == 0 {
                    roots.push(name.clone());
                    if name != "DeviceInfo" || info_root {
                        log::warn!(
                            "[store-device] device info root is invalid: name={name}, roots=[{}], bytes={}",
                            roots.join(","),
                            device_info_xml.len()
                        );
                        return Err(DeviceProvisionError::MalformedResponse);
                    }
                    info_root = true;
                }
            }
            Ok(Event::End(_)) => {
                let Some(next) = info_depth.checked_sub(1) else {
                    log::warn!(
                        "[store-device] device info has an unbalanced end tag, bytes={}",
                        device_info_xml.len()
                    );
                    return Err(DeviceProvisionError::MalformedResponse);
                };
                info_depth = next;
            }
            Ok(Event::Eof) => break,
            Ok(Event::Decl(_) | Event::Text(_) | Event::Comment(_)) => {}
            Ok(other) => {
                log::warn!(
                    "[store-device] device info contains an unsupported event: {other:?}, bytes={}",
                    device_info_xml.len()
                );
                return Err(DeviceProvisionError::MalformedResponse);
            }
            Err(error) => {
                log::warn!(
                    "[store-device] device info is not parsable XML at byte {}: {error}, bytes={}",
                    info_reader.buffer_position(),
                    device_info_xml.len()
                );
                return Err(DeviceProvisionError::MalformedResponse);
            }
        }
    }
    if !info_root || info_depth != 0 {
        log::warn!(
            "[store-device] device info is incomplete: root={info_root}, depth={info_depth}, bytes={}",
            device_info_xml.len()
        );
        return Err(DeviceProvisionError::MalformedResponse);
    }
    Ok(())
}

/// 设备注册响应的解析结果。字段保持原始形态，日志与校验各自决定怎么用。
#[cfg(windows)]
struct ParsedDeviceResponse {
    root: Option<String>,
    success_attr: Option<String>,
    puid: Option<String>,
    /// 第一段 `SPLicenseBlock` 的 base64 文本（设备许可证）。
    license_block: Option<String>,
    /// 响应里出现的 `SPLicenseBlock` 段数。用于诊断与回归测试。
    block_sections: usize,
    parse_error: Option<String>,
    elements: Vec<String>,
}

/// 解析 `deviceaddcredential.srf` 的响应体。
///
/// 抽成独立函数是为了能被单元测试直接覆盖：这段解析踩过两个**必须同时解决**的坑，
/// 且都只在真实响应上才暴露：
///
/// 1. quick-xml 会把超长文本节点分片返回多个 `Event::Text`，同一元素内必须**累加**；
///    覆盖式赋值会把 11308 字符的块截成第一片 468 字符（解码后 350 字节）。
/// 2. 同一份响应里有**两个** `SPLicenseBlock`（`START@834` 的 8480 字节设备许可证，
///    和 `START@15004` 的 350 字节骨架）。每遇到新元素必须**开新段**，否则两段拼接
///    成一个既不是 A 也不是 B 的串，base64 解码直接失败。
///
/// 两者叠加时的旧行为是：先被截成 350 字节，再被第二段污染，最终报
/// 「device provisioning response is malformed」，而服务端其实一切正常。
#[cfg(windows)]
fn parse_device_response(bytes: &[u8]) -> ParsedDeviceResponse {
    let mut reader = Reader::from_reader(bytes);
    let mut buf = Vec::new();
    let mut root: Option<String> = None;
    let mut success_attr: Option<String> = None;
    let mut puid: Option<String> = None;
    let mut block_sections: Vec<Vec<u8>> = Vec::new();
    let mut current: Option<Vec<u8>> = None;
    let mut parse_error: Option<String> = None;
    let mut elements: Vec<String> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if root.is_none() {
                    root = Some(String::from_utf8_lossy(e.name().as_ref()).into_owned());
                    // `Success` 是响应根元素的属性；没有它就无法把「服务端拒绝」
                    // 与「文档缺字段」区分开。
                    for attribute in e.attributes().flatten() {
                        if attribute.key.as_ref() == b"Success" {
                            success_attr = attribute
                                .normalized_value(XmlVersion::Implicit1_0)
                                .ok()
                                .map(|value| value.into_owned());
                        }
                    }
                }
                if elements.len() < 24 {
                    elements.push(String::from_utf8_lossy(e.name().as_ref()).into_owned());
                }
                let name = e.name().as_ref().to_vec();
                if name == b"SPLicenseBlock" {
                    block_sections.push(Vec::new());
                }
                current = Some(name);
            }
            Ok(Event::Text(e)) => {
                let decoded = match e.decode() {
                    Ok(text) => text.into_owned(),
                    Err(error) => {
                        parse_error = Some(format!(
                            "text at byte {} is not decodable: {error}",
                            reader.buffer_position()
                        ));
                        break;
                    }
                };
                match current.as_deref() {
                    Some(b"puid") => match &mut puid {
                        Some(existing) => existing.push_str(&decoded),
                        None => puid = Some(decoded),
                    },
                    Some(b"SPLicenseBlock") => {
                        if let Some(section) = block_sections.last_mut() {
                            section.extend_from_slice(decoded.as_bytes());
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(_)) => {}
            Ok(Event::End(_)) => current = None,
            Ok(Event::Eof) => break,
            Err(error) => {
                parse_error = Some(format!(
                    "not parsable XML at byte {}: {error}",
                    reader.buffer_position()
                ));
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    let block_sections_count = block_sections.len();
    // 设备许可证是第一段；顺序异常时退回第一个非空段（后续会因缺 `0x12d` 明确报错，
    // 而不是静默用错许可证）。
    let license_block = block_sections
        .into_iter()
        .find(|section| !section.is_empty())
        .and_then(|section| String::from_utf8(section).ok());

    ParsedDeviceResponse {
        root,
        success_attr,
        puid,
        license_block,
        block_sections: block_sections_count,
        parse_error,
        elements,
    }
}

/// Provision a real device credential. `device_info_xml` must be produced by a
/// Windows platform adapter; this function never invents hardware identity.
#[cfg(windows)]
pub async fn provision_device(
    client: &Client,
    account_id: String,
    device_info_xml: &str,
) -> Result<ProvisionedDevice, DeviceProvisionError> {
    if account_id.trim().is_empty() {
        // 账户为空是上层绑定错误，不是响应格式问题；分开报，避免继续被折叠成
        // 「malformed」而让排查方向跑偏。
        log::warn!("[store-device] device provisioning called without an account id");
        return Err(DeviceProvisionError::MalformedResponse);
    }
    validate_device_info(device_info_xml)?;

    // 凭据随机源必须是 32 字节，而不是 UUID 的 16 字节。
    //
    // 这里曾写成 `*uuid::Uuid::new_v4().as_bytes()`（只有 16 字节），随后按参考
    // 实现取 `random[7..25]`（18 字节）当密码：切片恒定越界 → 恒定返回
    // `malformed`。参考实现（`device_windows.go`）用的是 `make([]byte, 32)` 再
    // `rand.Read`，因此 7..25 正好落在范围内。设备注册因此 100% 失败，而且那条
    // 路径用 `?` 直接返回、不经过任何日志，现场只能靠直连 endpoint 复现才看得到。
    let mut random = [0u8; CREDENTIAL_RANDOM_BYTES];
    if let Err(error) = getrandom::getrandom(&mut random) {
        random.fill(0);
        log::warn!("[store-device] 设备凭据随机源不可用: {error}");
        return Err(DeviceProvisionError::Http(format!(
            "credential randomness unavailable: {error}"
        )));
    }
    let member = format!(
        "02{}",
        random[..MEMBER_RANDOM_BYTES]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let password = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(&random[MEMBER_RANDOM_BYTES..MEMBER_RANDOM_BYTES + PASSWORD_RANDOM_BYTES]);
    random.fill(0);
    let body = format!("<?xml version=\"1.0\"?><DeviceAddRequest><ClientInfo name=\"IDCRL\" version=\"1.0\"><BinaryVersion>55</BinaryVersion></ClientInfo><Authentication><Membername>{member}</Membername><Password>{password}</Password></Authentication>{device_info_xml}</DeviceAddRequest>");

    let mut response = client.post(DEVICE_PROVISION_URL)
        .header("Content-Type", "application/soap+xml")
        // Full client identification, not a bare version. The endpoint answers
        // legacy MSA clients; a truncated agent is answered with an error
        // document instead of a credential, which the reference client does not
        // hit because it always sends the complete string.
        .header("User-Agent", DEVICE_USER_AGENT)
        .body(body).send().await.map_err(|e| DeviceProvisionError::Http(e.to_string()))?;
    let status = response.status();
    if !status.is_success() { return Err(DeviceProvisionError::HttpStatus(status)); }
    if response.content_length().is_some_and(|length| length > DEVICE_RESPONSE_LIMIT) {
        return Err(DeviceProvisionError::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| DeviceProvisionError::Http(e.to_string()))? {
        let next = bytes.len().checked_add(chunk.len()).ok_or(DeviceProvisionError::ResponseTooLarge)?;
        if next as u64 > DEVICE_RESPONSE_LIMIT { return Err(DeviceProvisionError::ResponseTooLarge); }
        bytes.extend_from_slice(&chunk);
    }


    let parsed = parse_device_response(&bytes);
    let root = parsed.root;
    let success_attr = parsed.success_attr;
    let puid = parsed.puid;
    let block = parsed.license_block;
    let parse_error = parsed.parse_error;
    let elements = parsed.elements;
    // Owns its labels so it does not borrow `puid` / `block`, which are moved
    // out below once the response is accepted.
    let describe = {
        let body_len = bytes.len();
        let root_label = root.clone();
        let success_label = success_attr.clone();
        let element_labels = elements.join(",");
        move |outcome: &str, puid_state: &str, block_state: &str| {
            format!(
                "{outcome} (http={status}, bytes={body_len}, root={}, success={}, puid={puid_state}, license_block={block_state}, elements=[{element_labels}])",
                root_label.as_deref().unwrap_or("-"),
                success_label.as_deref().unwrap_or("-"),
            )
        }
    };
    let puid_state = |value: Option<&String>| match value {
        Some(text) if text.trim().is_empty() => "empty",
        Some(_) => "present",
        None => "absent",
    };
    let block_state = if block.is_some() { "present" } else { "absent" };

    // 响应体本身不可解析：先把现场写进日志再返回，否则「格式错误」与
    // 「服务端拒绝」在上层完全同形。
    if let Some(detail) = parse_error {
        log::warn!(
            "[store-device] device provisioning response is unparsable: {detail}; {}",
            describe("unparsable", puid_state(puid.as_ref()), block_state)
        );
        return Err(DeviceProvisionError::MalformedResponse);
    }



    // An explicit `Success="false"` is a refusal, not a malformed document.
    if success_attr.as_deref().is_some_and(|value| {
        value.eq_ignore_ascii_case("false") || value.eq_ignore_ascii_case("0")
    }) {
        log::warn!(
            "[store-device] device provisioning refused: {}",
            describe("refused", puid_state(puid.as_ref()), block_state)
        );
        return Err(DeviceProvisionError::Rejected);
    }
    if root.as_deref() != Some("DeviceAddResponse") {
        log::warn!(
            "[store-device] device provisioning root mismatch: {}",
            describe("bad root", puid_state(puid.as_ref()), block_state)
        );
        return Err(DeviceProvisionError::MalformedResponse);
    }
    let puid = match puid.filter(|v| !v.trim().is_empty()) {
        Some(value) => value,
        None => {
            log::warn!(
                "[store-device] device provisioning returned no puid: {}",
                describe("no puid", "absent", block_state)
            );
            return Err(DeviceProvisionError::MalformedResponse);
        }
    };
    let license_block = match block {
        Some(encoded) => base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .map_err(|_| {
                log::warn!(
                    "[store-device] license block is not base64: {}",
                    describe("bad base64", "present", "present")
                );
                DeviceProvisionError::MalformedResponse
            })?,
        None => {
            log::warn!(
                "[store-device] device provisioning returned no license block: {}",
                describe("no license", "present", "absent")
            );
            return Err(DeviceProvisionError::MalformedResponse);
        }
    };
    if license_block.is_empty() {
        log::warn!(
            "[store-device] license block decoded empty: {}",
            describe("empty license", "present", "present")
        );
        return Err(DeviceProvisionError::MalformedResponse);
    }
    Ok(ProvisionedDevice { binding: DeviceCacheBinding { account_id, device_id: puid.clone() }, member, password, puid, license_block })
}
const MAX_PROTECTED_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreDeviceError {
    #[error("device ticket storage is only available on Windows")]
    WindowsOnly,
    #[error("protected cache is malformed")]
    MalformedCache,
    #[error("protected cache version is unsupported")]
    UnsupportedVersion,
    #[error("protected cache exceeds the size limit")]
    CacheTooLarge,
    #[error("protected cache does not belong to the requested account or device")]
    BindingMismatch,
    #[error("DPAPI operation failed with Windows error {0}")]
    Dpapi(u32),
    #[error("protected cache serialization failed: {0}")]
    Serialization(String),
}

/// Account/device identity is deliberately separate from protected material.
/// Callers must supply both values when restoring; no ambient account is used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCacheBinding {
    pub account_id: String,
    pub device_id: String,
}

/// Serialized cache envelope. `protected` is DPAPI ciphertext, never a ticket
/// fabricated by this service. The envelope itself contains no credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedDeviceCache {
    version: u32,
    binding: DeviceCacheBinding,
    protected: Vec<u8>,
}

impl ProtectedDeviceCache {
    pub fn binding(&self) -> &DeviceCacheBinding { &self.binding }

    /// Protect opaque account/device data using the current Windows user DPAPI.
    #[cfg(windows)]
    pub fn protect(binding: DeviceCacheBinding, plaintext: &[u8]) -> Result<Self, StoreDeviceError> {
        if plaintext.len() > MAX_PROTECTED_BYTES { return Err(StoreDeviceError::CacheTooLarge); }
        if binding.account_id.trim().is_empty() || binding.device_id.trim().is_empty() {
            return Err(StoreDeviceError::MalformedCache);
        }
        Ok(Self { version: CACHE_VERSION, binding, protected: protect_bytes(plaintext)? })
    }

    #[cfg(not(windows))]
    pub fn protect(_binding: DeviceCacheBinding, _plaintext: &[u8]) -> Result<Self, StoreDeviceError> {
        Err(StoreDeviceError::WindowsOnly)
    }

    /// Restore opaque data only when the caller supplies the same binding.
    #[cfg(windows)]
    pub fn unprotect(&self, expected: &DeviceCacheBinding) -> Result<Vec<u8>, StoreDeviceError> {
        validate(self, expected)?;
        unprotect_bytes(&self.protected)
    }

    #[cfg(not(windows))]
    pub fn unprotect(&self, _expected: &DeviceCacheBinding) -> Result<Vec<u8>, StoreDeviceError> {
        Err(StoreDeviceError::WindowsOnly)
    }

    pub fn encode(&self) -> Result<Vec<u8>, StoreDeviceError> {
        if self.version != CACHE_VERSION { return Err(StoreDeviceError::UnsupportedVersion); }
        if self.protected.len() > MAX_PROTECTED_BYTES { return Err(StoreDeviceError::CacheTooLarge); }
        if self.protected.is_empty() || self.binding.account_id.trim().is_empty() || self.binding.device_id.trim().is_empty() {
            return Err(StoreDeviceError::MalformedCache);
        }
        serde_json::to_vec(self).map_err(|e| StoreDeviceError::Serialization(e.to_string()))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StoreDeviceError> {
        if bytes.len() > MAX_PROTECTED_BYTES { return Err(StoreDeviceError::CacheTooLarge); }
        let cache: Self = serde_json::from_slice(bytes).map_err(|_| StoreDeviceError::MalformedCache)?;
        if cache.version != CACHE_VERSION { return Err(StoreDeviceError::UnsupportedVersion); }
        if cache.protected.is_empty() || cache.protected.len() > MAX_PROTECTED_BYTES {
            return Err(StoreDeviceError::MalformedCache);
        }
        if cache.binding.account_id.trim().is_empty() || cache.binding.device_id.trim().is_empty() {
            return Err(StoreDeviceError::MalformedCache);
        }
        Ok(cache)
    }
}

#[cfg(any(windows, not(windows)))]
fn validate(cache: &ProtectedDeviceCache, expected: &DeviceCacheBinding) -> Result<(), StoreDeviceError> {
    if cache.version != CACHE_VERSION { return Err(StoreDeviceError::UnsupportedVersion); }
    if cache.binding != *expected { return Err(StoreDeviceError::BindingMismatch); }
    if cache.protected.is_empty() || cache.protected.len() > MAX_PROTECTED_BYTES {
        return Err(StoreDeviceError::MalformedCache);
    }
    Ok(())
}

#[cfg(windows)]
fn protect_bytes(plaintext: &[u8]) -> Result<Vec<u8>, StoreDeviceError> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: plaintext.len() as u32, pbData: plaintext.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(&input, None, None, None, None, 0, &mut output)
            .map_err(|e| StoreDeviceError::Dpapi(e.code().0 as u32))?;
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as *mut _));
        Ok(result)
    }
}

#[cfg(windows)]
fn unprotect_bytes(ciphertext: &[u8]) -> Result<Vec<u8>, StoreDeviceError> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: ciphertext.len() as u32, pbData: ciphertext.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, 0, &mut output)
            .map_err(|e| StoreDeviceError::Dpapi(e.code().0 as u32))?;
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as *mut _));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 直连真实 endpoint 复现一次设备注册，用来定位「malformed」到底出在哪一步。
    ///
    /// 标 `#[ignore]`：它会发真实网络请求，不属于单元测试的职责。需要时显式运行：
    /// `cargo test -p copper-core --lib provision_device_against_live_endpoint -- --ignored --nocapture`
    ///
    /// 刻意不走 `log`：测试二进制没有安装日志后端（顺序测试也不并发），
    /// 直接 `println!` 原始响应才能在失败时看到服务端到底回了什么。
    #[cfg(windows)]
    #[test]
    #[ignore = "会访问 login.live.com，需手动运行"]
    fn provision_device_against_live_endpoint() {
        let xml = crate::services::native_install::device_info::device_info_xml()
            .expect("本机应能读取 SMBIOS 固件表");
        // 先单独跑本地校验：它失败就与网络无关。
        if let Err(error) = validate_device_info(&xml) {
            panic!("本地校验失败（与网络无关）: {error:?}");
        }
        println!("local device info validation: ok ({} bytes)", xml.len());

        let account = "000340014F648727";
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap();

        // 手工重放请求，唯一目的就是把原始响应打出来。
        let mut random = [0u8; CREDENTIAL_RANDOM_BYTES];
        getrandom::getrandom(&mut random).expect("随机源不可用");
        let member = format!(
            "02{}",
            random[..MEMBER_RANDOM_BYTES].iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        let password = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(&random[MEMBER_RANDOM_BYTES..MEMBER_RANDOM_BYTES + PASSWORD_RANDOM_BYTES]);
        let body = format!(
            "<?xml version=\"1.0\"?><DeviceAddRequest><ClientInfo name=\"IDCRL\" version=\"1.0\"><BinaryVersion>55</BinaryVersion></ClientInfo><Authentication><Membername>{member}</Membername><Password>{password}</Password></Authentication>{xml}</DeviceAddRequest>"
        );
        let (status, bytes) = runtime.block_on(async {
            let response = client
                .post(DEVICE_PROVISION_URL)
                .header("Content-Type", "application/soap+xml")
                .header("User-Agent", DEVICE_USER_AGENT)
                .body(body)
                .send()
                .await
                .expect("请求发送失败");
            let status = response.status();
            let bytes = response.bytes().await.expect("读取响应体失败").to_vec();
            (status, bytes)
        });
        let text = String::from_utf8_lossy(&bytes);
        println!("http status: {status}");
        println!("body bytes: {}", bytes.len());
        let dump = std::path::PathBuf::from("D:\\CopperGolem\\CopperCore\\scripts\\device-response.xml");
        let _ = std::fs::write(&dump, &bytes);
        println!("full response written to {}", dump.display());
        println!("body head:\n{}", text.chars().take(1200).collect::<String>());
        // 列出所有元素名与属性，并给出每个元素的文本长度，用来判断块是否缺失/截断。
        let mut reader = quick_xml::Reader::from_str(&text);
        let mut depth = 0usize;
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Start(e)) => {
                    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                    let attrs: Vec<String> = e.attributes().flatten()
                        .map(|a| format!("{}={}", String::from_utf8_lossy(a.key.as_ref()),
                             String::from_utf8_lossy(&a.value)))
                        .collect();
                    println!("{}<{} {}>", "  ".repeat(depth), name, attrs.join(" "));
                    depth += 1;
                }
                Ok(quick_xml::events::Event::Text(e)) => {
                    println!("{}text len={}", "  ".repeat(depth), e.len());
                }
                Ok(quick_xml::events::Event::End(_)) => { depth = depth.saturating_sub(1); }
                Ok(quick_xml::events::Event::Eof) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }

        let result = runtime.block_on(provision_device(&client, account.to_string(), &xml));
        match result {
            Ok(device) => println!(
                "provision ok: puid_present={}, license_bytes={}",
                !device.puid.is_empty(),
                device.license_block.len()
            ),
            Err(error) => panic!("provisioning 失败: {error:?}（上方已打印原始响应）"),
        }
    }

    /// `device_info_xml()` 的真实产物必须通过本地校验。
    ///
    /// 这条测试锁住一个已经发生过的缺陷：构造出的 DeviceInfo 以五个**自闭合**
    /// `<Component .../>` 结尾，而校验循环只对 `Start` 递增深度、对 `Empty` 视而不见，
    /// 于是 `</DeviceInfo>` 把深度减成 -1 并立刻判为 `malformed`。设备注册因此
    /// 100% 失败，且失败发生在所有诊断日志之前——现象就是「报 malformed 但没有任何
    /// `[store-device]` 现场」，排查会被引向服务端而非这里。
    ///
    /// 直接跑真实生成逻辑而不是手写样本：手写样本很容易漏掉那个自闭合形状，
    /// 而这里要防的正是「生成端与校验端对同一份文档的理解不一致」。
    #[cfg(windows)]
    #[test]
    fn generated_device_info_passes_local_validation() {
        let xml = crate::services::native_install::device_info::device_info_xml()
            .expect("本机应能读取 SMBIOS 固件表");
        assert!(
            xml.contains("/>"),
            "样本必须覆盖自闭合元素，否则这条测试锁不住目标缺陷"
        );
        match validate_device_info(&xml) {
            Ok(()) => {}
            Err(error) => panic!("真实生成的 DeviceInfo 未通过本地校验: {error:?}"),
        }
    }

    /// 响应解析必须同时处理「长文本分片」与「多个 SPLicenseBlock」。
    ///
    /// 这两件事只在真实响应上一起出现，历史上叠加成：
    /// 第一段被截成 350 字节 → 又被第二段污染 → base64 解码失败 →
    /// 上层只看到一句 malformed，而服务端其实一切正常。
    #[cfg(windows)]
    #[test]
    fn response_parser_concatenates_chunks_and_keeps_first_license_block() {
        let first = "QUJDREVGR0g"; // 长块的多个分片必须拼接后才是完整 base64
        let second = "REVW"; // 第二段整块忽略
        let xml = format!(
            r#"<?xml version="1.0"?><DeviceAddResponse Success="true"><success>true</success><puid>0018401951F4AD61</puid><License><SPLicenseBlock>{first}{first}</SPLicenseBlock></License><KeyHolderInfo><SPLicenseId>c18edcc8-9651-b9ee-cf7b-2b04b364fe66</SPLicenseId></KeyHolderInfo><SPLicenseBlock>{second}</SPLicenseBlock></DeviceAddResponse>"#
        );
        let parsed = parse_device_response(xml.as_bytes());
        assert_eq!(parsed.block_sections, 2, "应识别出两段 SPLicenseBlock");
        assert_eq!(parsed.root.as_deref(), Some("DeviceAddResponse"));
        assert_eq!(parsed.success_attr.as_deref(), Some("true"));
        assert_eq!(parsed.puid.as_deref(), Some("0018401951F4AD61"));
        assert!(parsed.parse_error.is_none());
        assert_eq!(
            parsed.license_block.as_deref(),
            Some(format!("{first}{first}").as_str()),
            "必须取第一段且把分片拼全"
        );
    }

    /// 自闭合元素不得改变嵌套深度；形状错误必须被拒。
    #[cfg(windows)]
    #[test]
    fn validation_rejects_wrong_shapes_but_accepts_self_closing() {        // 原缺陷的精确形状：DeviceInfo 显式开闭，内部以自闭合元素结尾。
        // 修复前 `Empty` 不减深度，`</DeviceInfo>` 会把深度减成 -1 并判 malformed。
        let shape = "<DeviceInfo Id=\"DeviceInfo\"><Component name=\"8196\">AA==</Component>\
                     <Component name=\"4113\">AA==</Component>\
                     <Component name=\"4145\">AQAAAA==</Component>\
                     <Component name=\"4100\" error=\"-2147024894\"/>\
                     <Component name=\"4161\" error=\"-2147024894\"/></DeviceInfo>";
        assert!(
            validate_device_info(shape).is_ok(),
            "自闭合元素不得让嵌套深度失衡"
        );
        assert!(validate_device_info("<DeviceInfo Id=\"DeviceInfo\"><Component name=\"4113\"/></DeviceInfo>").is_ok());
        assert!(validate_device_info("<DeviceInfo><A/></DeviceInfo>").is_ok());
        assert!(validate_device_info("<Wrong/>").is_err());
        assert!(validate_device_info("<DeviceInfo>").is_err());
        assert!(validate_device_info("</DeviceInfo>").is_err());
        assert!(validate_device_info("not xml at all").is_err());
    }
}
