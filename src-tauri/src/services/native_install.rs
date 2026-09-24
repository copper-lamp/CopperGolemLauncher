// SPDX-License-Identifier: GPL-3.0-only
//
// Store-authorized native MSIXVC installation orchestration adapted from
// LeviLauncher nativeinstall (install_windows.go, device_windows.go,
// challenge_windows.go, license_windows.go) under GPL-3.0-only, which in turn
// credits the Store wire contracts to Xodus commit
// 0670e25aeb0e0e9f800f8f2f4968ae3b681842a7. See /THIRD_PARTY_NOTICES.

//! End-to-end orchestration for a Store-authorized native MSIXVC install.
//!
//! This module is the single entry point that turns a downloaded package into a
//! content key usable by the installer. It chains, in order:
//!
//! 1. package identity (`ContentID`/`KeyID`) read from the XVD metadata;
//! 2. a WAM user ticket bound to the caller-supplied XUID;
//! 3. a DPAPI-protected device credential, provisioned when absent;
//! 4. a device ticket derived from the device license through two RST calls;
//! 5. a Store content license request;
//! 6. SPLicense unwrapping bound to this device, then KeyID selection.
//!
//! Secret material (user ticket, device credential, wrapping key, RSA private
//! key and content key) never leaves this module except as the returned
//! [`ContentKeyLease`], which is cleared on drop and cannot be serialized.

use std::path::{Path, PathBuf};

#[cfg(windows)]
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::modules::game_download::msixvc;

pub(crate) use crate::services::store_entitlement::ContentKeyLease;

/// Markets are exactly two uppercase ASCII letters.
const MARKET_LEN: usize = 2;
/// Device license SPLicense block upper bound, matching the reference guard.
pub const MAX_DEVICE_LICENSE_BYTES: usize = 256 * 1024;
/// TLV identifiers inside the device SPLicense block.
const TLV_DEVICE_WRAPPING_KEY: u32 = 1;
const TLV_DEVICE_ID: u32 = 2;
const TLV_DEVICE_PRIVATE_KEY: u32 = 0x12d;
/// The device ID TLV is a 2-byte length prefix plus 8 identity bytes.
const DEVICE_ID_FIELD_BYTES: usize = 10;
const DEVICE_ID_BYTES: usize = 8;
/// Length of the CLEP payload holding the BCrypt RSA private blob.
const DEVICE_PRIVATE_KEY_CLEP_BYTES: usize = 544;
/// Device STS proof is a 48-byte CLEP record whose last 32 bytes are the secret.
const DEVICE_PROOF_CLEP_BYTES: usize = 48;
const DEVICE_PROOF_SECRET_START: usize = 12;
const DEVICE_PROOF_SECRET_END: usize = 44;
/// RST scopes: the first exchange targets the device STS, the second the site.
const DEVICE_SCOPE: &str = "http://Passport.NET/tb";
const SITE_SCOPE: &str = "www.microsoft.com";
/// Device credential cache file name inside the cache directory.
pub const DEVICE_CACHE_FILE: &str = "device.dpapi";
/// Advisory exclusive lock guarding the device cache across processes.
pub const DEVICE_LOCK_FILE: &str = "device.lock";

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum NativeInstallError {
    #[error("Store native installation is only available on Windows")]
    WindowsOnly,
    #[error("invalid market code")]
    InvalidMarket,
    #[error("Store authentication requires an explicit identity binding")]
    InvalidIdentity,
    #[error("package identity could not be read: {0}")]
    Package(String),
    #[error("Store authentication failed: {0}")]
    Auth(String),
    #[error("device credential cache is unusable: {0}")]
    DeviceCache(String),
    #[error("device provisioning failed: {0}")]
    DeviceProvision(String),
    #[error("device license is malformed")]
    MalformedDeviceLicense,
    #[error("device ticket acquisition failed: {0}")]
    DeviceTicket(String),
    #[error("content license request failed: {0}")]
    License(String),
    #[error("content license does not contain the package KeyID")]
    MissingContentKey,
}

/// Public, non-secret package identity used to request a license.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageIdentity {
    pub content_id: String,
    pub key_id: String,
}

/// Everything the licensing service needs besides the package identity.
#[derive(Debug, Clone)]
pub struct StoreInstallRequest {
    pub xuid: String,
    pub market: String,
    pub cache_dir: PathBuf,
}

impl StoreInstallRequest {
    pub fn new(xuid: impl Into<String>, market: impl Into<String>, cache_dir: impl Into<PathBuf>) -> Self {
        Self { xuid: xuid.into(), market: market.into(), cache_dir: cache_dir.into() }
    }
}

/// Validates the market code before any network call is attempted.
pub fn validate_market(market: &str) -> Result<(), NativeInstallError> {
    let bytes = market.as_bytes();
    if bytes.len() != MARKET_LEN || !bytes.iter().all(|byte| byte.is_ascii_uppercase()) {
        return Err(NativeInstallError::InvalidMarket);
    }
    Ok(())
}

/// Whether `package` needs a Store content key, i.e. it is an MSIXVC container
/// with at least one encrypted region. Unreadable inputs report `false` so the
/// caller keeps the compatibility backend instead of failing the install.
pub fn requires_store_key(package: &Path) -> bool {
    msixvc::has_encrypted_regions(package).unwrap_or(false)
}

/// Reads the package identity the licensing service expects.
pub fn read_identity(package: &Path) -> Result<PackageIdentity, NativeInstallError> {
    let identifiers = msixvc::read_package_identifiers(package)
        .map_err(|error| NativeInstallError::Package(error.to_string()))?;
    Ok(PackageIdentity { content_id: identifiers.content_id, key_id: identifiers.key_id })
}

/// Device state persisted under DPAPI. Every field is secret except the PUID,
/// which is only used as the cache binding identity.
#[cfg(windows)]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeviceState {
    member: String,
    password: String,
    puid: String,
    license: Vec<u8>,
}

#[cfg(windows)]
impl Drop for DeviceState {
    fn drop(&mut self) {
        unsafe {
            self.member.as_bytes_mut().fill(0);
            self.password.as_bytes_mut().fill(0);
        }
        self.license.fill(0);
    }
}

/// Validated device material derived from the device license.
///
/// The manual `Debug` deliberately prints nothing: every field is secret.
#[cfg(windows)]
struct DeviceMaterial {
    /// RFC 3394 KEK used to unwrap content keys.
    wrapping_key: [u8; 16],
    /// 8-byte device identity the content license must be bound to.
    device_id: [u8; DEVICE_ID_BYTES],
    /// RSA key that signs the first RST request.
    private_key: openssl::rsa::Rsa<openssl::pkey::Private>,
}

#[cfg(windows)]
impl std::fmt::Debug for DeviceMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceMaterial").finish_non_exhaustive()
    }
}

#[cfg(windows)]
impl Drop for DeviceMaterial {
    fn drop(&mut self) {
        self.wrapping_key.fill(0);
        self.device_id.fill(0);
    }
}

/// Derives and validates the wrapping key, device ID and RSA key.
#[cfg(windows)]
fn derive_device_material(license: &[u8]) -> Result<DeviceMaterial, NativeInstallError> {
    if license.is_empty() || license.len() > MAX_DEVICE_LICENSE_BYTES {
        return Err(NativeInstallError::MalformedDeviceLicense);
    }
    let blocks = msixvc::parse_license_blocks(license)
        .map_err(|_| NativeInstallError::MalformedDeviceLicense)?;

    let wrapping_key = crate::services::store_rst::device_wrapping_key(
        blocks.get(&TLV_DEVICE_WRAPPING_KEY).ok_or(NativeInstallError::MalformedDeviceLicense)?,
    )
    .map_err(|_| NativeInstallError::MalformedDeviceLicense)?;

    // The device ID TLV is a little-endian 2-byte length prefix followed by the
    // identity bytes; the reference hard-fails on any other shape.
    let encoded_id = blocks.get(&TLV_DEVICE_ID).ok_or(NativeInstallError::MalformedDeviceLicense)?;
    if encoded_id.len() != DEVICE_ID_FIELD_BYTES
        || u16::from_le_bytes([encoded_id[0], encoded_id[1]]) as usize != DEVICE_ID_BYTES
    {
        return Err(NativeInstallError::MalformedDeviceLicense);
    }
    let mut device_id = [0u8; DEVICE_ID_BYTES];
    device_id.copy_from_slice(&encoded_id[2..DEVICE_ID_FIELD_BYTES]);

    let private_blob = crate::services::store_rst::decrypt_clep(
        blocks.get(&TLV_DEVICE_PRIVATE_KEY).ok_or(NativeInstallError::MalformedDeviceLicense)?,
        DEVICE_PRIVATE_KEY_CLEP_BYTES,
    )
    .map_err(|_| NativeInstallError::MalformedDeviceLicense)?;
    let private_key = crate::services::store_rst::parse_bcrypt_rsa_private(&private_blob)
        .map_err(|_| NativeInstallError::MalformedDeviceLicense)?;

    Ok(DeviceMaterial { wrapping_key, device_id, private_key })
}

/// Encodes the SPLicense `DeviceInfo` component exactly like the reference
/// implementation: raw SMBIOS from the firmware table, per-version header
/// fields, then the obfuscation pass.
#[cfg(windows)]
mod device_info {
    use base64::Engine;

    const COMPONENT_BUFFER_BYTES: usize = 2048;
    const SMBIOS_COPY_BYTES: usize = 256;

    fn rotate_left_signed(value: u32, count: i32) -> u32 {
        value.rotate_left(count.rem_euclid(32) as u32)
    }

    fn challenge_round(index: u32, x: u32) -> u32 {
        let r = rotate_left_signed;
        match index {
            1 => 0x3243u32.wrapping_mul(r(x ^ 0x2418_1621, -22)).wrapping_sub(r(x, -8)),
            2 => 0x3243u32.wrapping_mul(r(x, -15) ^ 0x2418),
            3 => (x >> 9).wrapping_add(0x1621u32.wrapping_mul(r(x ^ 0x4139, 3))),
            4 => r(x, -28) ^ 0x4139u32.wrapping_mul(r(x ^ 0x2418_1621, -9)),
            5 => r(x, -12).wrapping_add(0x3243u32.wrapping_mul(r(x.wrapping_sub(0x2418_1621), -14))),
            6 => r(x, -11) ^ 0x2418u32.wrapping_mul(r(x ^ 0x1621, 2)),
            7 => x.wrapping_sub(0x4139_3243).wrapping_sub(0x1621),
            8 => 0x4139u32.wrapping_mul(r(x ^ 0x2418, 2)).wrapping_sub(r(x, -18)),
            0 => 0x3243u32.wrapping_mul(r(x.wrapping_sub(0x2418_1621), -18)).wrapping_sub(r(x, -9)),
            _ => 0x4139u32.wrapping_mul(r(x.wrapping_add(0x2418_1621), -10)).wrapping_sub(r(x, -29)),
        }
    }

    fn obfuscate(buffer: &mut [u8]) {
        const MAGIC: u32 = 0x2418_1621;
        if buffer.len() < 8 {
            return;
        }
        let mut a = 0u32;
        let mut c = !(0x4139u32.wrapping_mul(rotate_left_signed(MAGIC, -10)));
        for index in 1..=8u32 {
            let next_a = c;
            let next_c = a ^ challenge_round(index, c);
            a = next_a;
            c = next_c;
        }
        let iv = u32::from_le_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);
        let mut lo = c ^ iv;
        let mut hi = 0u32;
        let mut previous_lo = iv;
        let mut previous_hi = 0u32;
        buffer[4..8].copy_from_slice(&lo.to_le_bytes());
        let mut position = 8usize;
        while position + 8 <= buffer.len() {
            let x = u32::from_le_bytes([buffer[position], buffer[position + 1], buffer[position + 2], buffer[position + 3]]);
            let y = u32::from_le_bytes([buffer[position + 4], buffer[position + 5], buffer[position + 6], buffer[position + 7]]);
            let mut inner_a = lo ^ x;
            let mut inner_c = hi ^ y ^ challenge_round(0, lo ^ x);
            for index in (1..=8u32).rev() {
                let next_a = inner_c ^ challenge_round(index, inner_a);
                let next_c = inner_a;
                inner_a = next_a;
                inner_c = next_c;
            }
            lo = previous_lo ^ inner_a ^ challenge_round(9, inner_c);
            hi = previous_hi ^ inner_c;
            previous_lo = x;
            previous_hi = y;
            buffer[position..position + 4].copy_from_slice(&lo.to_le_bytes());
            buffer[position + 4..position + 8].copy_from_slice(&hi.to_le_bytes());
            position += 8;
        }
    }

    /// Reads the raw SMBIOS firmware table. No caller-supplied identity is used.
    pub fn raw_firmware_table() -> Result<Vec<u8>, String> {
        use windows::Win32::System::SystemInformation::{GetSystemFirmwareTable, RSMB};
        let size = unsafe { GetSystemFirmwareTable(RSMB, 0, None) };
        if !(8..=(1 << 20)).contains(&size) {
            return Err("firmware table size is unavailable".into());
        }
        let mut raw = vec![0u8; size as usize];
        let read = unsafe { GetSystemFirmwareTable(RSMB, 0, Some(raw.as_mut_slice())) };
        if read != size {
            return Err("firmware table read was short".into());
        }
        Ok(raw)
    }

    /// Builds the `DeviceInfo` element sent to `deviceaddcredential.srf`.
    pub fn device_info_xml() -> Result<String, String> {
        let raw = raw_firmware_table()?;
        if raw.len() <= 8 {
            return Err("firmware table is too small".into());
        }
        let available = (raw.len() - 8).min(SMBIOS_COPY_BYTES);
        let mut xml = String::from("<DeviceInfo Id=\"DeviceInfo\">");
        for version in [2u32, 4u32] {
            let mut buffer = vec![0u8; COMPONENT_BUFFER_BYTES];
            buffer[0..4].copy_from_slice(&version.to_le_bytes());
            buffer[4..4 + available].copy_from_slice(&raw[8..8 + available]);
            if version == 2 {
                buffer[328] = 1;
            } else {
                buffer[1231..1235].copy_from_slice(&1u32.to_le_bytes());
            }
            obfuscate(&mut buffer);
            let component = if version == 4 { 8197 } else { 8196 };
            xml.push_str(&format!(
                "<Component name=\"{component}\">{}</Component>",
                base64::engine::general_purpose::STANDARD.encode(&buffer)
            ));
        }
        xml.push_str("<Component name=\"4113\">AA==</Component><Component name=\"4145\">AQAAAA==</Component>");
        for component in [4100, 4101, 4102, 4160, 4161] {
            xml.push_str(&format!("<Component name=\"{component}\" error=\"-2147024894\"/>"));
        }
        xml.push_str("</DeviceInfo>");
        Ok(xml)
    }
}

/// Holds the cross-process device cache lock for the duration of one install.
/// Dropping it closes the handle and releases the exclusive share.
#[cfg(windows)]
struct DeviceLock {
    _handle: std::fs::File,
}

#[cfg(windows)]
impl DeviceLock {
    fn acquire(cache_dir: &Path) -> Result<Self, NativeInstallError> {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::create_dir_all(cache_dir)
            .map_err(|error| NativeInstallError::DeviceCache(error.to_string()))?;
        let path = cache_dir.join(DEVICE_LOCK_FILE);
        // dwShareMode = 0 makes a concurrent installer fail fast instead of
        // racing on the device credential.
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(&path)
            .map_err(|_| NativeInstallError::DeviceCache("another installation is using this device cache".into()))?;
        Ok(Self { _handle: handle })
    }
}

#[cfg(windows)]
fn save_device_state(cache_dir: &Path, binding: &crate::services::store_device::DeviceCacheBinding, state: &DeviceState) -> Result<(), NativeInstallError> {
    let plaintext = serde_json::to_vec(state)
        .map_err(|error| NativeInstallError::DeviceCache(error.to_string()))?;
    let cache = crate::services::store_device::ProtectedDeviceCache::protect(binding.clone(), &plaintext)
        .map_err(|error| NativeInstallError::DeviceCache(error.to_string()))?;
    let encoded = cache
        .encode()
        .map_err(|error| NativeInstallError::DeviceCache(error.to_string()))?;
    let path = cache_dir.join(DEVICE_CACHE_FILE);
    let staging = cache_dir.join(format!("{DEVICE_CACHE_FILE}.new"));
    std::fs::write(&staging, &encoded)
        .map_err(|error| NativeInstallError::DeviceCache(error.to_string()))?;
    std::fs::rename(&staging, &path)
        .map_err(|error| NativeInstallError::DeviceCache(error.to_string()))?;
    Ok(())
}

/// Loads the cached device credential, provisioning a new one when absent.
#[cfg(windows)]
async fn ensure_device_state(
    client: &reqwest::Client,
    cache_dir: &Path,
    xuid: &str,
) -> Result<DeviceState, NativeInstallError> {
    // The PUID is only known after provisioning, so the on-disk binding is
    // validated against the stored PUID rather than assumed from the XUID.
    if let Some(state) = load_device_state(cache_dir, xuid)? {
        return Ok(state);
    }
    let info = device_info::device_info_xml().map_err(NativeInstallError::DeviceProvision)?;
    let provisioned = crate::services::store_device::provision_device(client, xuid.to_string(), &info)
        .await
        .map_err(|error| NativeInstallError::DeviceProvision(error.to_string()))?;
    let state = DeviceState {
        member: provisioned.member.clone(),
        password: provisioned.password.clone(),
        puid: provisioned.puid.clone(),
        license: provisioned.license_block.clone(),
    };
    // Never persist a device credential we could not actually use.
    drop(derive_device_material(&state.license)?);
    let binding = crate::services::store_device::DeviceCacheBinding {
        account_id: xuid.to_string(),
        device_id: state.puid.clone(),
    };
    save_device_state(cache_dir, &binding, &state)?;
    Ok(state)
}

/// Loads the cached device state for `xuid`, rejecting another account's cache.
#[cfg(windows)]
fn load_device_state(cache_dir: &Path, xuid: &str) -> Result<Option<DeviceState>, NativeInstallError> {
    let path = cache_dir.join(DEVICE_CACHE_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(NativeInstallError::DeviceCache(error.to_string())),
    };
    let cache = crate::services::store_device::ProtectedDeviceCache::decode(&bytes)
        .map_err(|error| NativeInstallError::DeviceCache(error.to_string()))?;
    if cache.binding().account_id != xuid {
        // A different account must not inherit another account's device.
        return Err(NativeInstallError::Auth("device cache belongs to a different account".into()));
    }
    let expected = crate::services::store_device::DeviceCacheBinding {
        account_id: cache.binding().account_id.clone(),
        device_id: cache.binding().device_id.clone(),
    };
    let plaintext = cache
        .unprotect(&expected)
        .map_err(|error| NativeInstallError::DeviceCache(error.to_string()))?;
    let state: DeviceState = serde_json::from_slice(&plaintext)
        .map_err(|_| NativeInstallError::DeviceCache("device cache payload is malformed".into()))?;
    if state.member.trim().is_empty() || state.password.len() < 16 || state.license.is_empty() {
        return Err(NativeInstallError::DeviceCache("device cache payload is incomplete".into()));
    }
    Ok(Some(state))
}

/// Full Store authorization chain for one downloaded package.
///
/// Returns the content key leased to the caller. The key is bound to the
/// package `KeyID`; a license for any other package is rejected.
#[cfg(windows)]
pub(crate) async fn acquire_package_content_key(
    client: &reqwest::Client,
    request: &StoreInstallRequest,
    package: &Path,
) -> Result<ContentKeyLease, NativeInstallError> {
    if request.xuid.trim().is_empty() {
        return Err(NativeInstallError::InvalidIdentity);
    }
    validate_market(&request.market)?;
    let identity = read_identity(package)?;

    // Serialize device-credential access for the whole operation.
    let _lock = DeviceLock::acquire(&request.cache_dir)?;

    let ticket = crate::services::store_wam::acquire_store_ticket_for_xuid(&request.xuid)
        .map_err(|error| NativeInstallError::Auth(error.to_string()))?;

    let state = ensure_device_state(client, &request.cache_dir, &request.xuid).await?;
    let material = derive_device_material(&state.license)?;

    let device_authorization =
        acquire_device_ticket(client, &state.member, &material.private_key).await?;

    let license_bytes = crate::services::store_entitlement::request_content_license(
        client,
        &ticket,
        &device_authorization,
        &identity.content_id,
        &request.market,
    )
    .await
    .map_err(|error| NativeInstallError::License(error.to_string()))?;

    crate::services::store_entitlement::extract_content_key(
        &license_bytes,
        &identity.key_id,
        &material.device_id,
        &material.wrapping_key,
    )
    .map_err(|error| match error {
        crate::services::store_entitlement::StoreEntitlementError::NoContentKey
        | crate::services::store_entitlement::StoreEntitlementError::KeyIdNotUnique => {
            NativeInstallError::MissingContentKey
        }
        other => NativeInstallError::License(other.to_string()),
    })
}

/// Acquires the device ticket (MSA device authorization) through the two RST
/// exchanges the licensing service expects.
///
/// Phase one signs with the device RSA key and returns the device STS token plus
/// the CLEP-encrypted proof secret; phase two signs with the WS-SecureConversation
/// double-derived key and returns the site token used as the `Authorization`
/// header of the content-license request.
#[cfg(windows)]
async fn acquire_device_ticket(
    client: &reqwest::Client,
    member: &str,
    private_key: &openssl::rsa::Rsa<openssl::pkey::Private>,
) -> Result<String, NativeInstallError> {
    use crate::services::store_rst;
    use crate::services::store_rst_transport as transport;

    let context = transport::RstTransportContext::default();
    let signing_key = openssl::pkey::PKey::from_rsa(private_key.clone())
        .map_err(|error| NativeInstallError::DeviceTicket(error.to_string()))?;

    let first_request = transport::make_rst(
        member,
        DEVICE_SCOPE,
        None,
        Some(&signing_key),
        None,
    )
    .map_err(|error| NativeInstallError::DeviceTicket(error.to_string()))?;
    let first = transport::post_rst(client, &context, &first_request, None)
        .await
        .map_err(|error| NativeInstallError::DeviceTicket(error.to_string()))?;
    let device_token = first
        .requested_token_xml()
        .map_err(|error| NativeInstallError::DeviceTicket(error.to_string()))?;
    let proof = first
        .binary_secret()
        .map_err(|error| NativeInstallError::DeviceTicket(error.to_string()))?;

    let proof = base64::engine::general_purpose::STANDARD
        .decode(proof.trim())
        .map_err(|_| NativeInstallError::DeviceTicket("device proof is not valid base64".into()))?;
    let mut secret_data = store_rst::decrypt_clep(&proof, DEVICE_PROOF_CLEP_BYTES)
        .map_err(|_| NativeInstallError::DeviceTicket("device proof is not a usable CLEP record".into()))?;
    if secret_data.len() < DEVICE_PROOF_SECRET_END {
        secret_data.fill(0);
        return Err(NativeInstallError::DeviceTicket("device proof is too short".into()));
    }
    let secret = secret_data[DEVICE_PROOF_SECRET_START..DEVICE_PROOF_SECRET_END].to_vec();
    secret_data.fill(0);

    let second_request = transport::make_rst(
        "",
        SITE_SCOPE,
        Some(&device_token),
        None,
        Some(&secret),
    )
    .map_err(|error| NativeInstallError::DeviceTicket(error.to_string()))?;
    let second = transport::post_rst(client, &context, &second_request, Some(&secret))
        .await
        .map_err(|error| NativeInstallError::DeviceTicket(error.to_string()))?;
    second
        .binary_security_token()
        .map_err(|error| NativeInstallError::DeviceTicket(error.to_string()))
}

/// Non-Windows builds have no WAM, DPAPI or device provisioning.
#[cfg(not(windows))]
pub(crate) async fn acquire_package_content_key(
    _client: &reqwest::Client,
    _request: &StoreInstallRequest,
    _package: &Path,
) -> Result<ContentKeyLease, NativeInstallError> {
    Err(NativeInstallError::WindowsOnly)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_validation_is_strict() {
        assert!(validate_market("US").is_ok());
        assert!(validate_market("DE").is_ok());
        for bad in ["", "U", "USA", "us", "U1", "U ", " U"] {
            assert_eq!(validate_market(bad).unwrap_err(), NativeInstallError::InvalidMarket, "{bad}");
        }
    }

    #[test]
    fn request_rejects_empty_identity_before_platform_dispatch() {
        let request = StoreInstallRequest::new("  ", "US", std::env::temp_dir());
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let error = runtime
            .block_on(async {
                let client = reqwest::Client::new();
                acquire_package_content_key(&client, &request, Path::new("missing.pkg")).await
            })
            .unwrap_err();
        assert_eq!(error, NativeInstallError::InvalidIdentity);
    }

    #[cfg(windows)]
    #[test]
    fn malformed_device_licenses_are_rejected_without_panicking() {
        assert_eq!(derive_device_material(&[]).unwrap_err(), NativeInstallError::MalformedDeviceLicense);
        assert_eq!(derive_device_material(&[0; 8]).unwrap_err(), NativeInstallError::MalformedDeviceLicense);
        assert_eq!(
            derive_device_material(&vec![0u8; MAX_DEVICE_LICENSE_BYTES + 1]).unwrap_err(),
            NativeInstallError::MalformedDeviceLicense
        );
    }
}
