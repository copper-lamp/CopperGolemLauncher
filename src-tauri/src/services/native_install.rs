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
#[cfg(windows)]
use serde::{Deserialize, Serialize};

use crate::modules::game_download::msixvc;

pub(crate) use crate::services::store_entitlement::ContentKeyLease;
use crate::services::store_key_cache::{self, KeyCacheError};

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
    /// Own-process window that owns the system account UI, or `0` when none.
    ///
    /// Only used as the fallback owner for an interactive ticket: the silent
    /// token request can be refused by the system, and re-issuing the same
    /// request against a window is the one path known to present the system
    /// account UI. Without a window there is no fallback, and a refused silent
    /// request stays a hard failure rather than blocking on a dialog nobody can
    /// own.
    pub owner_window: isize,
}

impl StoreInstallRequest {
    pub fn new(xuid: impl Into<String>, market: impl Into<String>, cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            xuid: xuid.into(),
            market: market.into(),
            cache_dir: cache_dir.into(),
            owner_window: 0,
        }
    }

    /// Sets the window that owns the system account UI for the fallback path.
    pub fn with_owner_window(mut self, hwnd: isize) -> Self {
        self.owner_window = hwnd;
        self
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

/// Alias used by the game installer; keeps package identity terminology consistent.
pub fn read_identifiers(package: &Path) -> Result<PackageIdentity, NativeInstallError> {
    read_identity(package)
}

/// Load a cached Full content key bound to the current account.
pub(crate) fn load_cached_key(
    ctx: &crate::modules::game_download::installer::Ctx,
    key_id: &str,
    xuid: &str,
) -> Result<Option<ContentKeyLease>, KeyCacheError> {
    store_key_cache::load(&ctx.store_key_dir(), key_id, xuid)
}

/// Save a Full content key for offline reinstall.
pub(crate) fn save_cached_key(
    ctx: &crate::modules::game_download::installer::Ctx,
    lease: &ContentKeyLease,
    xuid: &str,
) -> Result<(), KeyCacheError> {
    store_key_cache::save(&ctx.store_key_dir(), lease.key_id(), lease, xuid)
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

/// `PUID` 是 16 位十六进制文本（形如 `0018401951F4AD61`），内容就是 8 字节设备 ID。
///
/// Xodus 的 `read_vec(reader, size)` 在 `DeviceLicenseDeviceId` 上直接拿到原始 ID 字节；
/// 服务端在 `<puid>` 里把它写成十六进制文本，因此这里必须解码而不是当字符串比较——
/// 拿 16 字节文本去比 8 字节绑定值会让内容许可的 `0xd2` 校验永远不匹配。
#[cfg(windows)]
fn device_id_from_puid(puid: &str) -> Result<[u8; DEVICE_ID_BYTES], NativeInstallError> {
    let text = puid.trim();
    if text.len() != DEVICE_ID_BYTES * 2 {
        return Err(NativeInstallError::MalformedDeviceLicense);
    }
    let mut out = [0u8; DEVICE_ID_BYTES];
    for (index, byte) in out.iter_mut().enumerate() {
        let pair = text.get(index * 2..index * 2 + 2).ok_or(NativeInstallError::MalformedDeviceLicense)?;
        *byte = u8::from_str_radix(pair, 16).map_err(|_| NativeInstallError::MalformedDeviceLicense)?;
    }
    Ok(out)
}

/// Derives and validates the wrapping key, device ID and RSA key.
///
/// `license` 是**已解码**的 SPLicense 原始字节：`store_device::provision_device`
/// 负责把响应里 `<SPLicenseBlock>` 的 base64 文本解开后再交到这里。
///
/// 块号与布局取自 Xodus（`crates/xodus/src/licensing/splicense.rs` 的 `BlockId`，
/// commit `0670e25a`），不是从 LeviLauncher 的 Go 移植猜出来的——那份移植在设备
/// 许可这一段把 `blocks[1]` 当 4096 字节用，而线上响应里 `0x1` 是 4096 字节、
/// `0x12d` 是 4096 字节，两者的语义只有 Xodus 说清了：
///
/// - `0x1`  `EncryptedDeviceKey`：解出 16 字节设备包装密钥；
/// - `0x12d` `ClepSignState`：解出 544 字节 BCrypt RSA 私钥（设备凭据本体）；
/// - `0x2`  `DeviceLicenseDeviceId`：原始设备 ID 字节。
#[cfg(windows)]
fn derive_device_material(
    license: &[u8],
    device_id: &[u8],
) -> Result<DeviceMaterial, NativeInstallError> {
    if license.is_empty() || license.len() > MAX_DEVICE_LICENSE_BYTES {
        return Err(NativeInstallError::MalformedDeviceLicense);
    }
    if device_id.len() != DEVICE_ID_BYTES {
        return Err(NativeInstallError::MalformedDeviceLicense);
    }
    let blocks = msixvc::parse_license_blocks(license)
        .map_err(|_| NativeInstallError::MalformedDeviceLicense)?;

    let wrapping_key = crate::services::store_rst::device_wrapping_key(
        blocks.get(&TLV_DEVICE_WRAPPING_KEY).ok_or(NativeInstallError::MalformedDeviceLicense)?,
    )
    .map_err(|_| NativeInstallError::MalformedDeviceLicense)?;

    let mut bound_id = [0u8; DEVICE_ID_BYTES];
    bound_id.copy_from_slice(device_id);

    let private_blob = crate::services::store_rst::encrypted_state_secret(
        blocks.get(&TLV_DEVICE_PRIVATE_KEY).ok_or(NativeInstallError::MalformedDeviceLicense)?,
        DEVICE_PRIVATE_KEY_CLEP_BYTES,
    )
    .map_err(|_| NativeInstallError::MalformedDeviceLicense)?;
    let private_key = crate::services::store_rst::parse_bcrypt_rsa_private(&private_blob)
        .map_err(|_| NativeInstallError::MalformedDeviceLicense)?;

    Ok(DeviceMaterial { wrapping_key, device_id: bound_id, private_key })
}

/// Encodes the SPLicense `DeviceInfo` component exactly like the reference
/// implementation: raw SMBIOS from the firmware table, per-version header
/// fields, then the obfuscation pass.
///
/// `pub(crate)` 是为了让 `store_device` 的回归测试能直接校验**真实产物**：
/// 生成端与校验端对同一份文档的理解必须一致，只测手写样本锁不住这类缺陷。
#[cfg(windows)]
pub(crate) mod device_info {
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
    // 诊断：必须能看出这一轮到底是「复用已注册设备」还是「又注册了一台」。
    match load_device_state(cache_dir, xuid) {
        Ok(Some(state)) => {
            log::info!("[native-install] reusing registered device: puid={}", state.puid);
            return Ok(state);
        }
        Ok(None) => log::info!("[native-install] no device cache at {}", cache_dir.display()),
        Err(error) => log::warn!("[native-install] device cache unusable: {error}"),
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
    // 设备凭据必须在**注册成功后立刻落盘**，而不是等整条链跑通才落盘。
    //
    // 服务端按账户限制可注册设备数（实测超限时返回
    // `satisfactionFailure code 501 "Device group is full"`）。旧顺序是「先校验
    // 许可证可用性、再保存」，于是只要后续任何一步失败，这次注册就被丢弃，
    // 下一次尝试又会**新注册一台设备**——反复重试会把账户的设备槽位吃满，
    // 最终连注册带授权一起失败，而用户看到的原因还被折叠成 malformed。
    //
    // 参考实现的顺序正是先注册先落盘（`ensureDevice()` 里 `saveProtected` 紧跟
    // 注册之后），后续复用缓存。这里对齐该顺序：先持久化，再校验可用性。
    // 校验失败不会留下「不可用却还被复用」的凭据——`derive_device_material`
    // 会在每次使用时重新校验，坏凭据会立刻报错并可被删除重注册。
    let binding = crate::services::store_device::DeviceCacheBinding {
        account_id: xuid.to_string(),
        device_id: state.puid.clone(),
    };
    save_device_state(cache_dir, &binding, &state)?;
    // 落盘后立刻自检：明显不可用的许可证不该被 silently 复用。
    drop(derive_device_material(&state.license, &device_id_from_puid(&state.puid)?)?);
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

    // WAM is a blocking WinRT call: it polls its async operation to completion.
    // Running it on a tokio worker would park that worker for the whole timeout,
    // so both paths go to the blocking pool, the same execution model as the
    // interactive sign-in command.
    let xuid = request.xuid.clone();
    let silent = tokio::task::spawn_blocking(move || {
        crate::services::store_wam::acquire_store_ticket_for_xuid(&xuid)
    })
    .await
    .map_err(|error| NativeInstallError::Auth(format!("WAM ticket task failed: {error}")))?;

    let ticket = match silent {
        Ok(ticket) => ticket,
        // The silent request can be refused by the system even though the same
        // parameters work interactively (observed as E_ACCESSDENIED with a valid
        // account already resolved). Re-issue it against our own window, which is
        // the one path that presents the system account UI. The ticket stays bound
        // to the same XUID, so an account switch is still rejected.
        Err(silent_error) => {
            if request.owner_window == 0 {
                return Err(NativeInstallError::Auth(format!(
                    "{silent_error} (no owner window available, cannot fall back to interactive)"
                )));
            }
            log::warn!(
                "[native-install] silent ticket refused, falling back to interactive: \
                 {silent_error} (owner_window=0x{:x})",
                request.owner_window
            );
            let hwnd = request.owner_window;
            let xuid = request.xuid.clone();
            tokio::task::spawn_blocking(move || {
                crate::services::store_wam::acquire_store_ticket_for_window(hwnd, &xuid)
            })
            .await
            .map_err(|error| {
                NativeInstallError::Auth(format!("WAM interactive ticket task failed: {error}"))
            })?
            .map_err(|error| {
                NativeInstallError::Auth(format!(
                    "{silent_error}; interactive fallback failed too: {error}"
                ))
            })?
        }
    };

    let state = ensure_device_state(client, &request.cache_dir, &request.xuid).await?;
    let material = derive_device_material(&state.license, &device_id_from_puid(&state.puid)?)?;

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
        let id = [0u8; DEVICE_ID_BYTES];
        assert_eq!(derive_device_material(&[], &id).unwrap_err(), NativeInstallError::MalformedDeviceLicense);
        // 任意文本字节不得被当成 TLV 读出一个看似合法的结果。
        assert_eq!(
            derive_device_material(b"not base64!!", &id).unwrap_err(),
            NativeInstallError::MalformedDeviceLicense
        );
        assert_eq!(derive_device_material(b"AAAA", &id).unwrap_err(), NativeInstallError::MalformedDeviceLicense);
        assert_eq!(
            derive_device_material(&vec![0u8; MAX_DEVICE_LICENSE_BYTES + 1], &id).unwrap_err(),
            NativeInstallError::MalformedDeviceLicense
        );
        // PUID 文本口径：16 位十六进制 <-> 8 字节。
        assert_eq!(device_id_from_puid("0018401951F4AD61").unwrap(), [0x00,0x18,0x40,0x19,0x51,0xF4,0xAD,0x61]);
        assert!(device_id_from_puid("0018").is_err());
        assert!(device_id_from_puid("zz18401951F4AD61").is_err());
    }

    /// 端到端跑一遍安装链真正会走的授权流程，逐段报告停在哪里。
    ///
    /// 这条测试的价值在于：不必让使用者反复点安装按钮——整条链（WAM 票 → 设备注册 →
    /// 设备许可证解密 → 设备 RST → 内容许可证 → 内容密钥）都会在这里被真实执行，
    /// 哪一段挂了一目了然。
    ///
    /// 标 `#[ignore]`：会访问 login.live.com 并读取本地整包。运行：
    /// `cargo test -p copper-core --lib authorization_chain_against_live_services -- --ignored --nocapture`
    #[cfg(windows)]
    #[test]
    #[ignore = "会访问在线服务并读取本地整包，需手动运行"]
    fn authorization_chain_against_live_services() {
        struct StdoutLogger;
        impl log::Log for StdoutLogger {
            fn enabled(&self, _: &log::Metadata<'_>) -> bool {
                true
            }
            fn log(&self, record: &log::Record<'_>) {
                println!("[{}][{}] {}", record.level(), record.target(), record.args());
            }
            fn flush(&self) {}
        }
        static LOGGER: StdoutLogger = StdoutLogger;
        let _ = log::set_logger(&LOGGER);
        log::set_max_level(log::LevelFilter::Info);

        let package = std::path::PathBuf::from(
            r"D:\CopperGolem\CopperCore\.devdata\versions\.download\1.26.52.03.msixvc",
        );
        assert!(package.is_file(), "本地整包不存在: {}", package.display());

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap();
        let xuid = "000340014F648727".to_string();

        let identity = read_identity(&package).expect("读取包身份失败");
        println!(
            "1. package identity: content_id={} key_id={}",
            identity.content_id, identity.key_id
        );

        let ticket = runtime
            .block_on(async {
                let xuid = xuid.clone();
                tokio::task::spawn_blocking(move || {
                    crate::services::store_wam::acquire_store_ticket_for_xuid(&xuid)
                })
                .await
                .unwrap()
            })
            .expect("WAM 静默取票失败");
        println!("2. WAM store ticket: ok");

        // 走 ensure_device_state（与真实安装链一致）：已有凭据就复用，
        // 绝不每次重新注册设备——服务端按账户限制设备数。
        let cache_dir = std::path::PathBuf::from(
            r"D:\CopperGolem\CopperCore\.devdata\data\cache\store-device",
        );
        std::fs::create_dir_all(&cache_dir).expect("创建设备缓存目录失败");
        let (member, private_key) = runtime.block_on(async {
            let state = ensure_device_state(&client, &cache_dir, &xuid)
                .await
                .expect("设备凭据获取失败");
            println!("3. device credential: puid={}", state.puid);
            let material = derive_device_material(
                &state.license,
                &device_id_from_puid(&state.puid).expect("PUID 解码失败"),
            )
            .expect("设备许可证解密失败");
            println!("4. device license decrypted: rsa key ready");
            (state.member.clone(), material.private_key.clone())
        });

        let device_authorization = match runtime.block_on(acquire_device_ticket(
            &client,
            &member,
            &private_key,
        )) {
            Ok(token) => {
                println!("5. device ticket (RST x2): ok, {} chars", token.len());
                token
            }
            Err(error) => panic!("设备票据获取失败: {error}"),
        };

        let license_bytes = runtime
            .block_on(crate::services::store_entitlement::request_content_license(
                &client,
                &ticket,
                &device_authorization,
                &identity.content_id,
                "US",
            ))
            .expect("内容许可证请求失败");
        println!("6. content license: {} bytes", license_bytes.len());

        // 诊断：把内容许可证响应的形状与每个记录的 XML 结构打出来。
        {
            let text = String::from_utf8_lossy(&license_bytes).into_owned();
            println!("license json head: {}", &text[..text.len().min(300)]);
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                let keys = value
                    .get("license")
                    .and_then(|license| license.get("keys"))
                    .and_then(|keys| keys.as_array())
                    .cloned()
                    .unwrap_or_default();
                println!("license.keys count={}", keys.len());
                for (index, record) in keys.iter().enumerate() {
                    let raw = record.get("value").and_then(|v| v.as_str()).unwrap_or("");
                    match base64::engine::general_purpose::STANDARD.decode(raw) {
                        Ok(decoded) => {
                            let mut reader = quick_xml::Reader::from_reader(decoded.as_slice());
                            let mut buf = Vec::new();
                            let mut depth = 0usize;
                            let mut children: Vec<String> = Vec::new();
                            loop {
                                match reader.read_event_into(&mut buf) {
                                    Ok(quick_xml::events::Event::Start(e)) => {
                                        if depth == 1 {
                                            children.push(String::from_utf8_lossy(e.name().as_ref()).into_owned());
                                        }
                                        depth += 1;
                                    }
                                    Ok(quick_xml::events::Event::End(_)) => depth = depth.saturating_sub(1),
                                    Ok(quick_xml::events::Event::Eof) => break,
                                    Ok(_) => {}
                                    Err(error) => { println!("  record {index} xml error: {error}"); break; }
                                }
                                buf.clear();
                            }
                            println!("  record {index}: decoded={} bytes, root children={children:?}", decoded.len());
                        }
                        Err(error) => println!("  record {index}: base64 failed: {error}"),
                    }
                }
            } else {
                println!("license json parse failed");
            }
        }

        // 跑两次：第二次必须复用已注册的设备凭据，而不是再注册一台。
        // 服务端按账户限制设备数，反复新注册会把槽位吃满并让后续全部失败。
        for attempt in 1..=2 {
            let request = StoreInstallRequest::new(xuid.clone(), "US", cache_dir.clone());
            let result = runtime.block_on(acquire_package_content_key(&client, &request, &package));
            match result {
                Ok(lease) => println!("7.{attempt} content key lease: key_id={}", lease.key_id()),
                Err(error) => println!("7.{attempt} 失败: {error}"),
            }
            assert!(
                cache_dir.join(DEVICE_CACHE_FILE).is_file(),
                "第 {attempt} 次尝试后设备凭据没有落盘：下次会重复注册设备"
            );
        }
        println!("device credential persisted and reused across attempts");
    }

    /// 直连真实 endpoint 走完「注册设备 → 解析设备许可证」，把每个 TLV 的 id 与长度打出来。
    ///
    /// 云端返回的真实许可证不落在仓库里，只有实跑一次才能看到它的真实形状；
    /// 单元测试用的手写样本很容易与真实形状脱节。
    ///
    /// 标 `#[ignore]`：会访问 login.live.com。手动运行：
    /// `cargo test -p copper-core --lib derive_device_material_from_live_provision -- --ignored --nocapture`
    #[cfg(windows)]
    #[test]
    #[ignore = "会访问 login.live.com，需手动运行"]
    fn derive_device_material_from_live_provision() {
        let xml = device_info::device_info_xml().expect("本机应能读取 SMBIOS 固件表");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap();
        let device = runtime
            .block_on(crate::services::store_device::provision_device(
                &client,
                "000340014F648727".to_string(),
                &xml,
            ))
            .expect("设备注册失败");
        println!("puid={} license_bytes={}", device.puid, device.license_block.len());

        let raw = &device.license_block;
        println!(
            "license bytes={} full hex:\n{}",
            raw.len(),
            raw.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        // 头部按 SPLicense 容器格式解析：前 8 字节是容器头，之后是 TLV。
        println!("container head u32x2 = {:?}", (
            u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]),
            u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]),
        ));
        let blocks = crate::modules::game_download::msixvc::parse_license_blocks(raw)
            .expect("许可证 TLV 解析失败");
        let mut ids: Vec<_> = blocks.iter().map(|(id, bytes)| (*id, bytes.len())).collect();
        ids.sort();
        for (id, len) in &ids {
            println!("tlv 0x{id:x} len={len}");
        }
        for (id, expected) in [
            (TLV_DEVICE_WRAPPING_KEY, 4096usize),
            (TLV_DEVICE_ID, DEVICE_ID_FIELD_BYTES),
            (TLV_DEVICE_PRIVATE_KEY, DEVICE_PRIVATE_KEY_CLEP_BYTES),
        ] {
            match blocks.get(&id) {
                Some(bytes) => println!("tlv 0x{id:x}: len={} expected={expected}", bytes.len()),
                None => println!("tlv 0x{id:x}: MISSING (expected len={expected})"),
            }
        }

        match derive_device_material(
            &device.license_block,
            &device_id_from_puid(&device.puid).unwrap(),
        ) {
            Ok(material) => println!("derive ok: device_id={:02x?}", material.device_id),
            Err(error) => panic!("derive_device_material 失败: {error:?}（上方为真实 TLV 形状）"),
        }
    }
}
