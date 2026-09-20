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
impl ContentKeyLease { pub(crate) fn key_id(&self) -> &str { &self.key_id } pub(crate) fn license_type(&self) -> LicenseType { self.license_type } pub(crate) fn key(&self) -> &[u8] { self.key.as_bytes() } }
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

#[derive(Debug, Deserialize)] struct LicenseResponse { #[serde(rename = "License")] license: Option<LicenseBody>, #[serde(rename = "SatisfactionFailure")] satisfaction_failure: Option<SatisfactionFailure> }
#[derive(Debug, Deserialize)] struct LicenseBody { #[serde(rename = "Keys", default)] keys: Vec<LicenseRecord> }
#[derive(Debug, Deserialize)] struct LicenseRecord { #[serde(rename = "Value")] value: String }
#[derive(Debug, Deserialize)] struct SatisfactionFailure { #[allow(dead_code)] code: i64, #[allow(dead_code)] description: String }

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

/// Strictly parse one decoded license XML: exactly one LicenseInfo and SPLicenseBlock under License.
fn parse_license_xml(data: &[u8]) -> Result<(LicenseType, Vec<u8>), StoreEntitlementError> {
    let mut reader = Reader::from_reader(data);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut root_seen = false;
    let mut root_closed = false;
    let mut info = None;
    let mut block = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let name = event.name().as_ref().to_vec();
                if root_closed || (depth == 0 && name.as_slice() != b"License") {
                    return Err(StoreEntitlementError::MalformedLicenseResponse);
                }
                if depth == 0 { root_seen = true; }
                if depth == 1 && name.as_slice() == b"LicenseInfo" {
                    if info.is_some() || block.is_some() { return Err(StoreEntitlementError::MalformedLicenseResponse); }
                    info = Some(parse_license_type(&event, reader.decoder())?);
                } else if depth == 1 && name.as_slice() == b"SPLicenseBlock" {
                    if block.is_some() || info.is_none() { return Err(StoreEntitlementError::MalformedLicenseResponse); }
                    // read_text consumes the matching End event; keep the License depth unchanged.
                    block = Some(reader.read_text(event.name()).map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?.into_owned());
                    buf.clear();
                    continue;
                } else if depth > 0 {
                    return Err(StoreEntitlementError::MalformedLicenseResponse);
                }
                depth += 1;
            }
            Ok(Event::Empty(event)) => {
                let name = event.name().as_ref().to_vec();
                if depth != 1 || name.as_slice() != b"LicenseInfo" || info.is_some() || block.is_some() {
                    return Err(StoreEntitlementError::MalformedLicenseResponse);
                }
                info = Some(parse_license_type(&event, reader.decoder())?);
            }
            Ok(Event::End(event)) => {
                if depth == 0 || (depth == 1 && event.name().as_ref() != b"License") { return Err(StoreEntitlementError::MalformedLicenseResponse); }
                depth -= 1;
                if depth == 0 { root_closed = true; }
            }
            Ok(Event::Decl(_) | Event::Comment(_) | Event::Text(_)) => {}
            Ok(Event::Eof) => break,
            Err(_) => return Err(StoreEntitlementError::MalformedLicenseResponse),
            _ => return Err(StoreEntitlementError::MalformedLicenseResponse),
        }
        buf.clear();
    }
    if !root_seen || !root_closed || depth != 0 { return Err(StoreEntitlementError::MalformedLicenseResponse); }
    let kind = info.ok_or(StoreEntitlementError::MalformedLicenseResponse)?;
    let encoded = block.ok_or(StoreEntitlementError::MalformedLicenseResponse)?;
    let encoded = encoded.into_inner();
    let encoded = std::str::from_utf8(&encoded).map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?;
    let blob = base64::engine::general_purpose::STANDARD.decode(encoded.trim()).map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?;
    Ok((kind, blob))
}

fn parse_license_type<'a>(event: &quick_xml::events::BytesStart<'a>, decoder: quick_xml::encoding::Decoder) -> Result<LicenseType, StoreEntitlementError> {
    let mut kind = None;
    for attribute in event.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?;
        if attribute.key.as_ref() != b"Type" || kind.is_some() {
            return Err(StoreEntitlementError::MalformedLicenseResponse);
        }
        kind = Some(attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder).map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?.into_owned());
    }
    match kind.as_deref() { Some("Full") => Ok(LicenseType::Full), Some("Trial") => Ok(LicenseType::Trial), _ => Err(StoreEntitlementError::MalformedLicenseResponse) }
}

pub fn classify_license_response(data: &[u8]) -> Result<EntitlementState, StoreEntitlementError> {
    let parsed: LicenseResponse = bounded_json(data)?;
    if parsed.satisfaction_failure.is_some() {
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
fn bounded_json<T: for<'de> Deserialize<'de>>(data: &[u8]) -> Result<T, StoreEntitlementError> { if data.len() as u64 > LICENSE_RESPONSE_LIMIT { return Err(StoreEntitlementError::LicenseResponseTooLarge); } serde_json::from_slice(data).map_err(|_| StoreEntitlementError::MalformedLicenseResponse) }

/// Decode, unwrap and select exactly one requested KeyID from all license records.
pub(crate) fn extract_content_key(data: &[u8], wanted_key_id: &str, device_id: &[u8], kek: &[u8]) -> Result<ContentKeyLease, StoreEntitlementError> {
    let parsed: LicenseResponse = bounded_json(data)?; let license = parsed.license.ok_or(StoreEntitlementError::MalformedLicenseResponse)?; if license.keys.is_empty() || license.keys.len() > MAX_LICENSE_RECORDS { return Err(StoreEntitlementError::MalformedLicenseResponse); }
    let mut candidates = Vec::new();
    for record in license.keys { if record.value.len() > MAX_LICENSE_RECORD_BYTES { return Err(StoreEntitlementError::LicenseRecordTooLarge); } let decoded = base64::engine::general_purpose::STANDARD.decode(record.value).map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?; let (kind, blob) = parse_license_xml(&decoded)?; let keys = msixvc::unpack_content_keys(&blob, device_id, kek).map_err(|_| StoreEntitlementError::MalformedLicenseResponse)?; for (id, key) in keys { candidates.push((id, kind, key)); if candidates.len() > MAX_CONTENT_KEYS { return Err(StoreEntitlementError::TooManyLicenseRecords); } } }
    let mut selected = None; for (id, kind, key) in candidates { if id.eq_ignore_ascii_case(wanted_key_id) { if selected.is_some() { return Err(StoreEntitlementError::KeyIdNotUnique); } selected = Some(ContentKeyLease { key_id: id, license_type: kind, key }); } } selected.ok_or(StoreEntitlementError::NoContentKey)
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
mod tests { use super::*; #[test] fn content_id_is_strict() { assert!(valid_content_id("01234567-89ab-cdef-0123-456789abcdef")); assert!(!valid_content_id("01234567-89ab-cdef-0123-456789abcdeZ")); } #[test] fn bounded_reader_rejects_extra_byte() { assert_eq!(read_bounded(&b"12345"[..], 4).unwrap_err().kind(), io::ErrorKind::InvalidData); } #[test] fn strict_xml_rejects_duplicate_info() { assert!(parse_license_xml(br#"<License><LicenseInfo Type="Full"/><LicenseInfo Type="Trial"/></License>"#).is_err()); } }
