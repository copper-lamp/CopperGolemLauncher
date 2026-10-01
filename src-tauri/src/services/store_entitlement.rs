// SPDX-License-Identifier: GPL-3.0-only
// Store wire contracts: Xodus commit 0670e25aeb0e0e9f800f8f2f4968ae3b681842a7.
// The ticket boundary remains opaque; WAM/device-ticket acquisition is intentionally absent.

//! Microsoft Store catalog and content-license protocol primitives.
//! No ticket, device credential, or content key is serializable or exposed to UI.

use std::fmt;
use std::io::{self, Read};

use base64::Engine;
use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

use crate::modules::game_download::msixvc::{self, ContentKeyLease as MsixContentKeyLease};

pub const CATALOG_RESPONSE_LIMIT: u64 = 8 * 1024 * 1024;
pub const LICENSE_RESPONSE_LIMIT: u64 = 2 * 1024 * 1024;
pub const MAX_LICENSE_RECORDS: usize = 16;
pub const MAX_CONTENT_KEYS: usize = 256;
const MAX_LICENSE_RECORD_BYTES: usize = 512 * 1024;
const LICENSE_URL: &str = "https://licensing.mp.microsoft.com/v7.0/licenses/content";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntitlementState { Authorized, Trial, NotEntitled, Error }

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreEntitlementError {
    #[error("Store entitlement is only available on Windows")] WindowsOnly,
    #[error("Store authentication requires user interaction")] InteractionRequired,
    #[error("Store authentication account does not match the requested identity")] AccountChanged,
    #[error("Store authentication requires an explicit identity binding")] InvalidIdentity,
    #[error("Store catalog response exceeds the size limit")] CatalogResponseTooLarge,
    #[error("Store license response exceeds the size limit")] LicenseResponseTooLarge,
    #[error("Store catalog product does not match the requested product")] CatalogProductMismatch,
    #[error("Store catalog has no supported retail MSIXVC package")] NoSupportedPackage,
    #[error("Store request failed with HTTP status {0}")] HttpStatus(StatusCode),
    #[error("Store license response is malformed")] MalformedLicenseResponse,
    #[error("Store license response contains too many records")] TooManyLicenseRecords,
    #[error("Store license response contains an oversized license record")] LicenseRecordTooLarge,
    #[error("Store license response contains ambiguous or missing KeyID")] KeyIdNotUnique,
    #[error("Store license response has no usable content key")] NoContentKey,
    /// 服务端明确拒绝并给出了自己的原因（如设备数量超限）。
    ///
    /// 必须与 `MalformedLicenseResponse` 区分开：后者是**我方**解析失败，前者是
    /// 服务端说了话。把前者折叠成后者会让「设备组已满，请移除一台设备」这种
    /// 可操作的原因完全消失，排查只能靠猜。
    #[error("Store license request was refused: {0}")] ServiceRefused(String),
    #[error("Store HTTP request failed: {0}")] Http(String),
}

/// Opaque input supplied by a real platform WAM adapter. No constructor is public.
pub struct StoreTicket { value: String, reference: String }
impl StoreTicket {
    #[cfg(windows)]
    pub(crate) fn from_wam(value: String, reference: String) -> Self { Self { value, reference } }
    pub(crate) fn unavailable() -> Result<Self, StoreEntitlementError> { Err(StoreEntitlementError::WindowsOnly) }
    fn wire_parts(&self) -> (&str, &str) { (&self.value, &self.reference) }
}
impl fmt::Debug for StoreTicket { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.debug_struct("StoreTicket").finish_non_exhaustive() } }
impl Drop for StoreTicket { fn drop(&mut self) { unsafe { self.value.as_bytes_mut().fill(0); self.reference.as_bytes_mut().fill(0); } } }

/// Internal key lease. It cannot cross a command, event, or persistence boundary.
pub(crate) struct ContentKeyLease { key_id: String, license_type: LicenseType, key: MsixContentKeyLease }
impl ContentKeyLease {
    pub(crate) fn key_id(&self) -> &str { &self.key_id }
    pub(crate) fn license_type(&self) -> LicenseType { self.license_type }
    pub(crate) fn key(&self) -> &[u8] { self.key.as_bytes() }
    pub(crate) fn from_cached(key_id: String, key: Vec<u8>) -> Result<Self, StoreEntitlementError> {
        if key.len() != 32 { return Err(StoreEntitlementError::NoContentKey); }
        let key = MsixContentKeyLease::from_bytes(key).map_err(|_| StoreEntitlementError::NoContentKey)?;
        Ok(Self { key_id, license_type: LicenseType::Full, key })
    }
}
/// Never renders the key bytes; only the non-secret identity is shown.
impl fmt::Debug for ContentKeyLease { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.debug_struct("ContentKeyLease").field("key_id", &self.key_id).field("license_type", &self.license_type).finish_non_exhaustive() } }
impl Drop for ContentKeyLease { fn drop(&mut self) { unsafe { self.key_id.as_bytes_mut().fill(0); } } }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LicenseType { Full, Trial }

#[derive(Debug, Deserialize)] struct CatalogResponse { #[serde(rename = "Product")] product: CatalogProduct }
#[derive(Debug, Deserialize)] struct CatalogProduct { #[serde(rename = "ProductID")] product_id: String, #[serde(rename = "DisplaySkuAvailabilities", default)] availabilities: Vec<SkuAvailability> }
#[derive(Debug, Deserialize)] struct SkuAvailability { #[serde(rename = "Sku")] sku: Sku }
#[derive(Debug, Deserialize)] struct Sku { #[serde(rename = "Properties")] properties: SkuProperties }
#[derive(Debug, Deserialize)] struct SkuProperties { #[serde(rename = "IsTrial", default)] is_trial: bool, #[serde(rename = "IsPreOrder", default)] is_pre_order: bool, #[serde(rename = "Packages", default)] packages: Vec<CatalogPackage> }
#[derive(Debug, Deserialize)] struct CatalogPackage { #[serde(rename = "ContentID")] content_id: String, #[serde(rename = "PackageFormat")] package_format: String, #[serde(rename = "PackageFamilyName")] package_family_name: String, #[serde(rename = "Architectures", default)] architectures: Vec<String> }

pub fn select_catalog_content_id(data: &[u8], product_id: &str, family: &str) -> Result<String, StoreEntitlementError> {
    if data.len() as u64 > CATALOG_RESPONSE_LIMIT { return Err(StoreEntitlementError::CatalogResponseTooLarge); }
    let catalog: CatalogResponse = serde_json::from_slice(data).map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?;
    if catalog.product.product_id != product_id { return Err(StoreEntitlementError::CatalogProductMismatch); }
    for availability in catalog.product.availabilities {
        let p = availability.sku.properties;
        if p.is_trial || p.is_pre_order { continue; }
        for package in p.packages {
            if package.package_format == "MSIXVC" && package.package_family_name == family && package.architectures.iter().any(|a| a == "x64") && valid_content_id(&package.content_id) { return Ok(package.content_id); }
        }
    }
    Err(StoreEntitlementError::NoSupportedPackage)
}
pub fn valid_content_id(id: &str) -> bool { id.len() == 36 && id.bytes().enumerate().all(|(i, c)| if matches!(i, 8|13|18|23) { c == b'-' } else { c.is_ascii_hexdigit() }) }

// 服务端返回的是**小写驼峰**：`{"license":{"keys":[{"metadata":{...},"value":"..."}]}}`，
// 拒绝时是 `{"satisfactionFailure":{"code":501,"description":"...","remediationAction":"..."}}`。
// 此前的 rename 写成了 PascalCase（`"License"`/`"Keys"`/`"Value"`），与真实字段名不匹配，
// 于是这两个结构**从来没有被正确反序列化过**：成功响应的 keys 恒为空、拒绝响应的
// 原因恒被丢弃，两者都以一句 `malformed` 收场。用真实抓到的响应校正。
#[derive(Debug, Deserialize)]
struct LicenseResponse {
    #[serde(default)]
    license: Option<LicenseBody>,
    #[serde(default, rename = "satisfactionFailure")]
    satisfaction_failure: Option<SatisfactionFailure>,
}
#[derive(Debug, Deserialize)]
struct LicenseBody {
    #[serde(default)]
    keys: Vec<LicenseRecord>,
}
#[derive(Debug, Deserialize)]
struct LicenseRecord {
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    value: String,
}
#[derive(Debug, Deserialize)]
struct SatisfactionFailure {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "remediationAction")]
    remediation_action: String,
}

impl SatisfactionFailure {
    /// 给用户看的原因：服务端描述 + 代码 + 建议动作。
    fn reason(&self) -> String {
        let mut reason = format!("{} (code {})", self.description.trim(), self.code);
        if !self.remediation_action.trim().is_empty() {
            reason.push_str(&format!(", remediation={}", self.remediation_action.trim()));
        }
        reason
    }
}

/// Build the exact JSON request body; ticket material is borrowed and never serialized elsewhere.
pub(crate) fn license_request_body(ticket: &StoreTicket, content_id: &str, market: &str) -> Result<Vec<u8>, StoreEntitlementError> {
    if !valid_content_id(content_id) || market.trim().is_empty() { return Err(StoreEntitlementError::MalformedLicenseResponse); }
    let (value, reference) = ticket.wire_parts();
    let challenge = base64::engine::general_purpose::STANDARD.encode(b"<?xml version=\"1.0\" encoding=\"utf-8\"?><ClientChallenge xmlns=\"http://schemas.microsoft.com/onestore/security/mkms/LicReq/v1\" Version=\"2\"><LicenseProtocolVersion>5</LicenseProtocolVersion><SigningKeyVersion>1</SigningKeyVersion><ClientVersion>2</ClientVersion></ClientChallenge>");
    serde_json::to_vec(&serde_json::json!({"clientChallenge":challenge,"concurrencyMode":"Rude","contentId":content_id,"deviceContext":{"hardwareManufacturer":"Public","hardwareType":"Public","mobileOperator":"Public"},"licenseVersion":4,"market":market,"needKey":true,"keyOnly":true,"users":{"S-1-5-21-0000000000-0000000000-0000000000-1001":[{"identityType":"Msa","identityValue":value,"localTicketReference":reference}]}})).map_err(|_| StoreEntitlementError::MalformedLicenseResponse)
}

/// Execute a license request with a bounded response. Real WAM/device authorization is a caller concern.
pub(crate) async fn request_content_license(client: &Client, ticket: &StoreTicket, device_authorization: &str, content_id: &str, market: &str) -> Result<Vec<u8>, StoreEntitlementError> {
    let body = license_request_body(ticket, content_id, market)?;
    let response = client.post(LICENSE_URL).header("Authorization", device_authorization).header("Content-Type", "application/json").header("From", "XboxLicenseManager").header("User-Agent", "XboxLm-PC/Microsoft.GamingServices").body(body).send().await.map_err(|e| StoreEntitlementError::Http(e.to_string()))?;
    let status = response.status();
    if status != StatusCode::OK { return Err(StoreEntitlementError::HttpStatus(status)); }
    read_bounded_async(response, LICENSE_RESPONSE_LIMIT).await
}

async fn read_bounded_async(mut response: reqwest::Response, limit: u64) -> Result<Vec<u8>, StoreEntitlementError> {
    if response.content_length().is_some_and(|length| length > limit) {
        return Err(StoreEntitlementError::LicenseResponseTooLarge);
    }
    let capacity = response.content_length().map_or(8192usize, |length| length.min(limit).try_into().unwrap_or(8192));
    let mut bytes = Vec::with_capacity(capacity);
    while let Some(chunk) = response.chunk().await.map_err(|e| StoreEntitlementError::Http(e.to_string()))? {
        let next_len = bytes.len().checked_add(chunk.len()).ok_or(StoreEntitlementError::LicenseResponseTooLarge)?;
        if next_len as u64 > limit {
            return Err(StoreEntitlementError::LicenseResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// 从一条许可证 XML 里取出 `LicenseType` 与解码后的 `SPLicenseBlock`。
///
/// 真实内容许可证的根下是
/// `Binding / LicenseInfo / CustomPolicies / SPLicenseBlock / Signature`（`Version="5"`），
/// 因此必须容忍额外的顶层孩子。这里用 quick-xml 自己的 `read_to_end` **整棵跳过**
/// 不关心的元素，而不是手工维护深度计数——手工记账极易出错：曾经在「未知孩子」
/// 分支里既设了跳过标记又继续 `depth += 1`，深度从此跑偏，随后第一个子元素就撞上
/// 兜底分支并误报 malformed。
///
/// 保留的必要约束：根必须是 `License` 且完整闭合；恰好一个 `LicenseInfo`；
/// 恰好一个出现在其后的 `SPLicenseBlock`。
fn parse_license_xml(data: &[u8]) -> Result<(LicenseType, Vec<u8>), StoreEntitlementError> {
    let mut reader = Reader::from_reader(data);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut root_seen = false;
    let mut root_closed = false;
    let mut info: Option<LicenseType> = None;
    let mut block: Option<Vec<u8>> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let name = event.name().as_ref().to_vec();
                if root_closed {
                    log::warn!("[store-entitlement] parse_license_xml: content after root");
                    return Err(StoreEntitlementError::MalformedLicenseResponse);
                }
                if !root_seen {
                    if name.as_slice() != b"License" {
                        log::warn!(
                            "[store-entitlement] parse_license_xml: root is {}, expected License",
                            String::from_utf8_lossy(&name)
                        );
                        return Err(StoreEntitlementError::MalformedLicenseResponse);
                    }
                    root_seen = true;
                } else if name.as_slice() == b"LicenseInfo" {
                    if info.is_some() || block.is_some() {
                        log::warn!("[store-entitlement] parse_license_xml: duplicate or misplaced LicenseInfo");
                        return Err(StoreEntitlementError::MalformedLicenseResponse);
                    }
                    info = Some(parse_license_type(&event, reader.decoder())?);
                    reader.read_to_end_into(event.name(), &mut Vec::new())
                        .map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?;
                } else if name.as_slice() == b"SPLicenseBlock" {
                    if block.is_some() || info.is_none() {
                        log::warn!("[store-entitlement] parse_license_xml: duplicate or misplaced SPLicenseBlock");
                        return Err(StoreEntitlementError::MalformedLicenseResponse);
                    }
                    block = Some(
                        reader
                            .read_text(event.name())
                            .map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?
                            .into_inner()
                            .into_owned(),
                    );
                } else {
                    // 其余顶层孩子（Binding / CustomPolicies / Signature / ...）：整棵跳过。
                    reader
                        .read_to_end_into(event.name(), &mut Vec::new())
                        .map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?;
                }
            }
            Ok(Event::Empty(event)) => {
                let name = event.name().as_ref().to_vec();
                if !root_seen {
                    if name.as_slice() != b"License" {
                        return Err(StoreEntitlementError::MalformedLicenseResponse);
                    }
                    // 自闭合根：交给结尾的完整性检查拒绝。
                    root_seen = true;
                    root_closed = true;
                } else if name.as_slice() == b"LicenseInfo" && info.is_none() && block.is_none() {
                    info = Some(parse_license_type(&event, reader.decoder())?);
                }
            }
            Ok(Event::End(_)) => {
                if root_seen {
                    root_closed = true;
                }
            }
            Ok(Event::Decl(_) | Event::Comment(_) | Event::Text(_)) => {}
            Ok(Event::Eof) => break,
            Err(error) => {
                log::warn!("[store-entitlement] parse_license_xml: xml error: {error}");
                return Err(StoreEntitlementError::MalformedLicenseResponse);
            }
            _ => return Err(StoreEntitlementError::MalformedLicenseResponse),
        }
        buf.clear();
    }
    if !root_seen || !root_closed {
        log::warn!(
            "[store-entitlement] parse_license_xml: incomplete document (root_seen={root_seen}, root_closed={root_closed})"
        );
        return Err(StoreEntitlementError::MalformedLicenseResponse);
    }
    let kind = info.ok_or(StoreEntitlementError::MalformedLicenseResponse)?;
    let encoded = block.ok_or(StoreEntitlementError::MalformedLicenseResponse)?;
    let encoded = String::from_utf8_lossy(&encoded);
    let blob = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?;
    Ok((kind, blob))
}
fn parse_license_type<'a>(event: &quick_xml::events::BytesStart<'a>, decoder: quick_xml::encoding::Decoder) -> Result<LicenseType, StoreEntitlementError> {
    let mut kind = None;
    for attribute in event.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?;
        if attribute.key.as_ref() != b"Type" {
            continue;
        }
        if kind.is_some() {
            return Err(StoreEntitlementError::MalformedLicenseResponse);
        }
        kind = Some(attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder).map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?.into_owned());
    }
    match kind.as_deref() { Some("Full") => Ok(LicenseType::Full), Some("Trial") => Ok(LicenseType::Trial), _ => Err(StoreEntitlementError::MalformedLicenseResponse) }
}

pub fn classify_license_response(data: &[u8]) -> Result<EntitlementState, StoreEntitlementError> {
    let parsed: LicenseResponse = bounded_json(data)?;
    if let Some(failure) = parsed.satisfaction_failure.as_ref() {
        log::warn!("[store-entitlement] license request refused: {}", failure.reason());
        return Ok(EntitlementState::NotEntitled);
    }
    let license = parsed
        .license
        .ok_or(StoreEntitlementError::MalformedLicenseResponse)?;
    if license.keys.is_empty() {
        return Err(StoreEntitlementError::MalformedLicenseResponse);
    }
    if license.keys.len() > MAX_LICENSE_RECORDS {
        return Err(StoreEntitlementError::TooManyLicenseRecords);
    }
    let mut state = EntitlementState::Error;
    for record in license.keys {
        if record.value.len() > MAX_LICENSE_RECORD_BYTES {
            return Err(StoreEntitlementError::LicenseRecordTooLarge);
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(record.value)
            .map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?;
        let (kind, _) = parse_license_xml(&decoded)?;
        if kind == LicenseType::Full {
            state = EntitlementState::Authorized;
        } else if state != EntitlementState::Authorized {
            state = EntitlementState::Trial;
        }
    }
    Ok(state)
}
fn bounded_json<T: for<'de> Deserialize<'de>>(data: &[u8]) -> Result<T, StoreEntitlementError> {
    if data.len() as u64 > LICENSE_RESPONSE_LIMIT {
        return Err(StoreEntitlementError::LicenseResponseTooLarge);
    }
    serde_json::from_slice(data).map_err(|error| {
        // 保留 serde 自己的诊断：字段名/类型不匹配与「真的是坏 JSON」在这里完全同形，
        // 折叠成一句 malformed 会让排查失去唯一线索。
        log::warn!(
            "[store-entitlement] license json deserialize failed: {error}, bytes={}",
            data.len()
        );
        StoreEntitlementError::MalformedLicenseResponse
    })
}

/// Decode, unwrap and select exactly one requested KeyID from all license records.
/// 解码响应、解封内容密钥、并严格选出请求的那个 KeyID。
pub(crate) fn extract_content_key(
    data: &[u8],
    wanted_key_id: &str,
    device_id: &[u8],
    kek: &[u8],
) -> Result<ContentKeyLease, StoreEntitlementError> {
    let parsed: LicenseResponse = bounded_json(data)?;
    // 服务端拒绝优先：它带着可操作的原因（如设备数超限），
    // 不能与「我方解析失败」混为一谈。
    if let Some(failure) = parsed.satisfaction_failure.as_ref() {
        log::warn!("[store-entitlement] license request refused: {}", failure.reason());
        return Err(StoreEntitlementError::ServiceRefused(failure.reason()));
    }
    let license = parsed
        .license
        .ok_or(StoreEntitlementError::MalformedLicenseResponse)?;
    if license.keys.is_empty() || license.keys.len() > MAX_LICENSE_RECORDS {
        return Err(StoreEntitlementError::MalformedLicenseResponse);
    }

    let mut candidates = Vec::new();
    for record in license.keys {
        if record.value.len() > MAX_LICENSE_RECORD_BYTES {
            return Err(StoreEntitlementError::LicenseRecordTooLarge);
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(record.value)
            .map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?;
        let (kind, blob) = parse_license_xml(&decoded)?;
        // 解封失败的底层原因（设备绑定不符 / keywrap 认证失败 / 块缺失）各不相同，
        // 折叠成一句 malformed 会让排查失去方向，因此保留原样。
        let keys = msixvc::unpack_content_keys(&blob, device_id, kek).map_err(|error| {
            log::warn!("[store-entitlement] unpack_content_keys failed: {error:?}");
            StoreEntitlementError::MalformedLicenseResponse
        })?;
        for (id, key) in keys {
            candidates.push((id, kind, key));
            if candidates.len() > MAX_CONTENT_KEYS {
                return Err(StoreEntitlementError::TooManyLicenseRecords);
            }
        }
    }

    let mut selected = None;
    for (id, kind, key) in candidates {
        if id.eq_ignore_ascii_case(wanted_key_id) {
            if selected.is_some() {
                return Err(StoreEntitlementError::KeyIdNotUnique);
            }
            selected = Some(ContentKeyLease { key_id: id, license_type: kind, key });
        }
    }
    selected.ok_or(StoreEntitlementError::NoContentKey)
}
pub fn read_bounded<R: Read>(reader: R, limit: u64) -> Result<Vec<u8>, io::Error> { let mut bytes = Vec::new(); reader.take(limit.saturating_add(1)).read_to_end(&mut bytes)?; if bytes.len() as u64 > limit { return Err(io::Error::new(io::ErrorKind::InvalidData, "response exceeds size limit")); } Ok(bytes) }

/// Acquire a ticket through the platform WAM adapter. The adapter owns the
/// Windows ABI boundary; this service only maps its safe error type.
pub fn acquire_store_ticket_for_xuid(expected_xuid: &str) -> Result<StoreTicket, StoreEntitlementError> {
    crate::services::store_wam::acquire_store_ticket_for_xuid(expected_xuid).map_err(Into::into)
}

#[deprecated(note = "use acquire_store_ticket_for_xuid to make account binding explicit")]
pub fn acquire_store_ticket() -> Result<StoreTicket, StoreEntitlementError> { acquire_store_ticket_for_xuid("") }

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实内容许可证的形状必须被接受。
    ///
    /// 回归点一：`LicenseInfo` 上除了 `Type` 还有 `LicenseUsage` / `LicenseCategory`，
    /// 旧实现遇到任何非 `Type` 属性就判 malformed——这是内容许可证长期报
    /// `malformed` 的最后一个原因。
    /// 回归点二：根下除了 `LicenseInfo` / `SPLicenseBlock` 还有
    /// `Binding` / `CustomPolicies` / `Signature`，必须整棵跳过而不是报错。
    #[test]
    fn real_content_license_shape_is_accepted() {
        let payload = base64::engine::general_purpose::STANDARD.encode(b"PAYLOAD");
        let xml = format!(
            r#"<?xml version="1.0"?><License xmlns="urn:schemas-microsoft-com:windows:store:licensing:ls" Version="5"><Binding Binding_Type="Machine"><ProductID>9NBLGGH2JHXJ</ProductID><DeviceID>DA4E25FD18C01800</DeviceID></Binding><LicenseInfo Type="Full" LicenseUsage="Online" LicenseCategory="Retail"><IssuedDate>2026-10-01T12:26:49Z</IssuedDate></LicenseInfo><CustomPolicies>eyJ4IjoxfQ==</CustomPolicies><SPLicenseBlock>{payload}</SPLicenseBlock><Signature xmlns="http://www.w3.org/2000/09/xmldsig#"><SignedInfo><DigestValue>AAAA</DigestValue></SignedInfo><SignatureValue>BBBB</SignatureValue></Signature></License>"#
        );
        let (kind, blob) = parse_license_xml(xml.as_bytes()).expect("真实形状必须被接受");
        assert_eq!(kind, LicenseType::Full);
        assert_eq!(blob, b"PAYLOAD");
    }

    /// `LicenseInfo` 缺 `Type` 或重复 `Type` 仍须拒绝，且 `SPLicenseBlock` 必须在
    /// `LicenseInfo` 之后——放宽不等于放弃校验。
    #[test]
    fn license_info_type_is_still_required_and_unique() {
        let cases = [
            r#"<License><LicenseInfo Type="Full" Other="x"/><SPLicenseBlock>QQ==</SPLicenseBlock></License>"#,
        ];
        for case in cases {
            assert!(parse_license_xml(case.as_bytes()).is_ok(), "应接受: {case}");
        }
        assert!(parse_license_xml(br#"<License><LicenseInfo LicenseUsage="Online"/><SPLicenseBlock>QQ==</SPLicenseBlock></License>"#).is_err());
        assert!(parse_license_xml(br#"<License><SPLicenseBlock>QQ==</SPLicenseBlock><LicenseInfo Type="Full"/></License>"#).is_err());
        assert!(parse_license_xml(br#"<License><LicenseInfo Type="Full" Type="Trial"/><SPLicenseBlock>QQ==</SPLicenseBlock></License>"#).is_err());
    }

    /// 服务端拒绝必须原样透出原因，不能被折叠成「response is malformed」。
    ///
    /// 真实抓到的拒绝响应是**小写驼峰**：
    /// `{"satisfactionFailure":{"code":501,"description":"Device group is full, ..."}}`。
    /// 旧结构体的 serde rename 写成 PascalCase，导致 `satisfactionFailure` 恒为 None，
    /// 「设备组已满，请移除一台设备」这种可操作的原因被替换成一句无法行动的 malformed。
    #[test]
    fn service_refusal_keeps_the_server_reason() {
        let body = br#"{"satisfactionFailure":{"alternateContentIds":[],"code":501,"data":[],"description":"Device group is full, please remove a device and try again.","remediationAction":"PresentUpsell","remediationProductSkus":[]}}"#;
        let error = extract_content_key(body, "bdb9e791-c97c-3734-e1a8-bc602552df06", &[0u8; 8], &[0u8; 16])
            .unwrap_err();
        match error {
            StoreEntitlementError::ServiceRefused(reason) => {
                assert!(reason.contains("Device group is full"), "原因丢失: {reason}");
                assert!(reason.contains("501"), "错误码丢失: {reason}");
            }
            other => panic!("应报 ServiceRefused，实际: {other:?}"),
        }
    } #[test] fn content_id_is_strict() { assert!(valid_content_id("01234567-89ab-cdef-0123-456789abcdef")); assert!(!valid_content_id("01234567-89ab-cdef-0123-456789abcdeZ")); } #[test] fn bounded_reader_rejects_extra_byte() { assert_eq!(read_bounded(&b"12345"[..], 4).unwrap_err().kind(), io::ErrorKind::InvalidData); } #[test] fn strict_xml_rejects_duplicate_info() { assert!(parse_license_xml(br#"<License><LicenseInfo Type="Full"/><LicenseInfo Type="Trial"/></License>"#).is_err()); } }
