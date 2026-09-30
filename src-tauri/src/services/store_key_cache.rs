// SPDX-License-Identifier: GPL-3.0-only
//
// Local content-key cache adapted from LeviLauncher nativeinstall
// license_cache_windows.go under GPL-3.0-only. See /THIRD_PARTY_NOTICES.

//! 本地商店 content key 缓存：装过一次的包，下次**离线**直接解包。
//!
//! 设计对齐 LeviLauncher `license_cache_windows.go`：
//! - 只缓存 `Full` 许可（Trial 不缓存，试用过期后必须重新在线授权）；
//! - 文件名 = `key_id.dpapi`，内容经 Windows DPAPI 加密，**只绑定当前用户**；
//! - 明文里带账户 `xuid`，换账户登录时命中直接失效（防止跨账户复用授权）；
//! - key 不进日志、不进数据库、不进事件，仅在进程内租约中短暂存活。
//!
//! 非 Windows 平台返回 [`KeyCacheError::Unsupported`]（Store 授权链本身即 Windows-only）。

use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::services::store_device::{
    DeviceCacheBinding, ProtectedDeviceCache, StoreDeviceError,
};
use crate::services::store_entitlement::{ContentKeyLease, LicenseType};

/// 缓存文件魔数，识别「本模块写入的格式」，避免把未知文件当缓存解析。
const MAGIC: &[u8; 4] = b"CGK1";

#[derive(Debug, thiserror::Error)]
pub enum KeyCacheError {
    #[error("内容密钥缓存仅在 Windows 可用")]
    Unsupported,
    #[error("缓存文件损坏: {0}")]
    Corrupt(String),
    #[error("缓存账户与当前账户不一致")]
    AccountMismatch,
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("加密失败: {0}")]
    Crypto(String),
}

impl From<StoreDeviceError> for KeyCacheError {
    fn from(e: StoreDeviceError) -> Self {
        match e {
            StoreDeviceError::WindowsOnly => KeyCacheError::Unsupported,
            other => KeyCacheError::Crypto(other.to_string()),
        }
    }
}

/// 落盘明文（DPAPI 保护前）。
#[derive(Debug, Serialize, Deserialize)]
struct CachedKeyPlain {
    /// 绑定的账户 XUID（换账户即失效）。
    xuid: String,
    /// 32 字节 AES-XTS key，base64。
    key: String,
    /// 许可类型（仅 `full` 会被写入）。
    license: String,
}

/// 内容密钥缓存文件路径（`dir/<key_id 安全化>.dpapi`）。
pub fn cache_path(dir: &Path, key_id: &str) -> PathBuf {
    let safe: String = key_id
        .chars()
        .map(|c| if c.is_ascii_hexdigit() || c == '-' { c } else { '_' })
        .collect();
    dir.join(format!("{safe}.dpapi"))
}

/// 落盘一个 `Full` content key（DPAPI + 账户绑定）。Trial 不缓存。
pub(crate) fn save(dir: &Path, key_id: &str, lease: &ContentKeyLease, xuid: &str) -> Result<(), KeyCacheError> {
    if lease.license_type() != LicenseType::Full || xuid.trim().is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let plain = serde_json::to_vec(&CachedKeyPlain {
        xuid: xuid.to_string(),
        key: base64::engine::general_purpose::STANDARD.encode(lease.key()),
        license: "full".into(),
    })
    .map_err(|e| KeyCacheError::Crypto(e.to_string()))?;
    // 账户绑定到 `binding.account_id`（明文内部也带一份双保险）。
    let protected = ProtectedDeviceCache::protect(
        DeviceCacheBinding {
            account_id: xuid.to_string(),
            device_id: key_id.to_string(),
        },
        &plain,
    )?;
    let envelope = protected
        .encode()
        .map_err(|e| KeyCacheError::Crypto(e.to_string()))?;
    drop_zero(&mut { plain });
    let mut out = Vec::with_capacity(MAGIC.len() + envelope.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&envelope);
    let target = cache_path(dir, key_id);
    let tmp = target.with_extension("dpapi.tmp");
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, &target)?;
    drop_zero(&mut out);
    Ok(())
}

/// 读取缓存 key。命中且账户一致时返回租约；任何不一致/损坏都按「未命中」处理
/// （这里只返回 `Ok(None)` 表示可安全回退在线链）。
pub(crate) fn load(dir: &Path, key_id: &str, xuid: &str) -> Result<Option<ContentKeyLease>, KeyCacheError> {
    let path = cache_path(dir, key_id);
    if !path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read(&path)?;
    if raw.len() < MAGIC.len() || &raw[..MAGIC.len()] != MAGIC {
        // 格式不认识 → 当未命中，让在线链接管。
        return Ok(None);
    }
    let envelope = ProtectedDeviceCache::decode(&raw[MAGIC.len()..])
        .map_err(|e| KeyCacheError::Corrupt(e.to_string()))?;
    if envelope.binding().account_id != xuid || envelope.binding().device_id != key_id {
        return Err(KeyCacheError::AccountMismatch);
    }
    let mut plain = envelope
        .unprotect(&DeviceCacheBinding {
            account_id: xuid.to_string(),
            device_id: key_id.to_string(),
        })
        // DPAPI 失败（换机器/账户）→ 未命中，回退在线链。
        .map_err(|_| KeyCacheError::AccountMismatch)?;
    let parsed: CachedKeyPlain = match serde_json::from_slice(&plain) {
        Ok(v) => v,
        Err(e) => {
            drop_zero(&mut plain);
            return Err(KeyCacheError::Corrupt(e.to_string()));
        }
    };
    drop_zero(&mut plain);
    if parsed.xuid != xuid {
        return Err(KeyCacheError::AccountMismatch);
    }
    let key = base64::engine::general_purpose::STANDARD
        .decode(parsed.key.trim())
        .map_err(|e| KeyCacheError::Corrupt(e.to_string()))?;
    if key.len() != 32 {
        return Err(KeyCacheError::Corrupt("key 长度非 32 字节".into()));
    }
    ContentKeyLease::from_cached(key_id.to_string(), key)
        .map(Some)
        .map_err(|e| KeyCacheError::Corrupt(e.to_string()))
}

fn drop_zero(data: &mut [u8]) {
    for b in data.iter_mut() {
        *b = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_path_is_filesystem_safe() {
        let dir = Path::new("C:\\tmp");
        let p = cache_path(dir, "33EC8436-5A0E-4F0D-B1CE-3F29C3955039");
        assert!(p
            .to_string_lossy()
            .ends_with("33EC8436-5A0E-4F0D-B1CE-3F29C3955039.dpapi"));
        let p2 = cache_path(dir, "bad/key\\name");
        assert!(!p2.to_string_lossy().contains('/'));
        assert!(!p2.file_name().unwrap().to_string_lossy().contains('\\'));
    }

    #[test]
    fn missing_file_is_a_miss_not_an_error() {
        let dir = std::env::temp_dir().join(format!("copper_gd_kc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        assert!(matches!(
            load(&dir, "AAAA-BBBB", "xuid"),
            Ok(None) | Err(KeyCacheError::Unsupported)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
