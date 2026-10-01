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

/// Provision a real device credential. `device_info_xml` must be produced by a
/// Windows platform adapter; this function never invents hardware identity.
#[cfg(windows)]
pub async fn provision_device(
    client: &Client,
    account_id: String,
    device_info_xml: &str,
) -> Result<ProvisionedDevice, DeviceProvisionError> {
    if account_id.trim().is_empty() || device_info_xml.len() > 64 * 1024 {
        return Err(DeviceProvisionError::MalformedResponse);
    }
    let mut info_reader = Reader::from_str(device_info_xml);
    let mut info_root = false;
    let mut info_depth = 0usize;
    loop {
        match info_reader.read_event() {
            Ok(Event::Start(event)) => {
                if info_depth == 0 {
                    if event.name().as_ref() != b"DeviceInfo" || info_root { return Err(DeviceProvisionError::MalformedResponse); }
                    info_root = true;
                }
                info_depth += 1;
            }
            Ok(Event::End(_)) => { info_depth = info_depth.checked_sub(1).ok_or(DeviceProvisionError::MalformedResponse)?; }
            Ok(Event::Eof) => break,
            Ok(Event::Decl(_) | Event::Text(_) | Event::Comment(_) | Event::Empty(_)) => {}
            Err(_) => return Err(DeviceProvisionError::MalformedResponse),
            _ => return Err(DeviceProvisionError::MalformedResponse),
        }
    }
    if !info_root || info_depth != 0 { return Err(DeviceProvisionError::MalformedResponse); }
    let random = *uuid::Uuid::new_v4().as_bytes();
    let member = format!("02{}", random[..7].iter().map(|b| format!("{b:02x}")).collect::<String>());
    let password = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random.get(7..25).ok_or(DeviceProvisionError::MalformedResponse)?);
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

    let mut reader = Reader::from_reader(bytes.as_slice());
    let mut buf = Vec::new();
    let mut root: Option<String> = None;
    let mut success_attr: Option<String> = None;
    let mut puid = None;
    let mut block = None;
    let mut current = None;
    // Element names only, capped: enough to recognize an error document while
    // keeping a hostile response from growing the log without bound.
    let mut elements: Vec<String> = Vec::new();
    loop { match reader.read_event_into(&mut buf) {
        Ok(Event::Start(e)) => {
            if root.is_none() {
                root = Some(String::from_utf8_lossy(e.name().as_ref()).into_owned());
                // `Success` is an attribute of the response root. Without it a
                // service-side refusal is indistinguishable from a missing
                // license, and every refusal gets reported as "malformed".
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
            current = Some(e.name().as_ref().to_vec());
        }
        Ok(Event::Text(e)) => { let text = e.decode().map_err(|_| DeviceProvisionError::MalformedResponse)?.into_owned(); match current.as_deref() { Some(b"puid") => puid = Some(text), Some(b"SPLicenseBlock") => block = Some(text), _ => {} } }
        Ok(Event::Empty(_)) => {}
        Ok(Event::End(_)) => { current = None; }
        Ok(Event::Eof) => break,
        Err(_) => return Err(DeviceProvisionError::MalformedResponse), _ => {}
    }; buf.clear(); }

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
