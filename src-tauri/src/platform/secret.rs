//! 凭证安全存储抽象。
//!
//! 账户令牌（refresh_token / access_token）必须经操作系统级加密存储，**不得**
//! 明文落盘或入库。各平台的「安全存储」实现完全不同：
//!
//! | 平台 | 实现 |
//! |---|---|
//! | Windows | Credential Manager（经 `keyring` 的 `windows-native` 后端） |
//! | Linux | Secret Service / gnome-keyring（经 `keyring` 的 `linux-native` 后端） |
//! | Android | Android Keystore（需 JNI 桥接，见 docs/平台适配.md 3.3 TODO） |
//!
//! 因此调用方（账户服务）只依赖本 trait，不感知平台。
//!
//! 设计约束：**不提供任何退化为明文的实现**。平台缺少安全存储时必须显式报错
//! （[`UnsupportedStore`]），让「登录不可持久化」这件事大声失败，而不是静默地
//! 把用户令牌写到不安全的地方。

use std::sync::Arc;

/// 单条凭证的存储键：`service` 为应用命名空间，`account` 为账户标识。
///
/// 与系统密钥环（Windows Credential Manager / Secret Service）的「服务 + 账户」
/// 二级寻址一致，便于两平台实现语义对齐。
pub trait SecretStore: Send + Sync {
    /// 写入（覆盖）凭证。
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), String>;

    /// 读取凭证；不存在返回 `Ok(None)`（区别于「读取失败」）。
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, String>;

    /// 删除凭证；不存在视为成功（幂等）。
    fn delete(&self, service: &str, account: &str) -> Result<(), String>;
}

/// 共享句柄别名，避免调用方处理泛型。
pub type SharedSecretStore = Arc<dyn SecretStore>;

/// 系统密钥环实现（Windows Credential Manager / Linux Secret Service）。
///
/// 仅在桌面平台编译：`keyring` 3 无 Android 后端（见 docs/平台适配.md 2.3）。
#[cfg(not(target_os = "android"))]
pub struct KeyringStore;

#[cfg(not(target_os = "android"))]
impl KeyringStore {
    pub fn new() -> Self {
        Self
    }

    fn entry(service: &str, account: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(service, account).map_err(|e| format!("密钥环不可用: {e}"))
    }
}

#[cfg(not(target_os = "android"))]
impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_os = "android"))]
impl SecretStore for KeyringStore {
    fn set(&self, service: &str, account: &str, value: &str) -> Result<(), String> {
        Self::entry(service, account)?
            .set_password(value)
            .map_err(|e| format!("写入密钥环失败: {e}"))
    }

    fn get(&self, service: &str, account: &str) -> Result<Option<String>, String> {
        match Self::entry(service, account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            // 条目不存在不是错误：调用方据此走「请重新登录」分支。
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("读取密钥环失败: {e}")),
        }
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), String> {
        match Self::entry(service, account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("清除密钥环失败: {e}")),
        }
    }
}

/// 平台缺少安全存储时的显式失败实现（当前：Android，待接 Keystore）。
///
/// 所有操作统一返回「不支持」，**不做任何降级**——避免令牌被写入不安全位置。
/// 接入 Android Keystore 后由 `platform::mod` 按其替换本实现。
#[cfg(target_os = "android")]
pub struct UnsupportedStore;

#[cfg(target_os = "android")]
impl UnsupportedStore {
    pub fn new() -> Self {
        Self
    }

    const REASON: &'static str =
        "当前平台尚未接入系统安全存储（Android Keystore），凭证无法持久化";
}

#[cfg(target_os = "android")]
impl Default for UnsupportedStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "android")]
impl SecretStore for UnsupportedStore {
    fn set(&self, _service: &str, _account: &str, _value: &str) -> Result<(), String> {
        Err(Self::REASON.to_string())
    }

    fn get(&self, _service: &str, _account: &str) -> Result<Option<String>, String> {
        Err(Self::REASON.to_string())
    }

    fn delete(&self, _service: &str, _account: &str) -> Result<(), String> {
        Err(Self::REASON.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内存实现：验证 trait 契约（幂等删除、缺失返回 None）而不触碰真实密钥环。
    struct MemoryStore(parking_lot::Mutex<std::collections::HashMap<String, String>>);

    impl MemoryStore {
        fn new() -> Self {
            Self(parking_lot::Mutex::new(std::collections::HashMap::new()))
        }

        fn key(service: &str, account: &str) -> String {
            format!("{service}/{account}")
        }
    }

    impl SecretStore for MemoryStore {
        fn set(&self, service: &str, account: &str, value: &str) -> Result<(), String> {
            self.0
                .lock()
                .insert(Self::key(service, account), value.to_string());
            Ok(())
        }

        fn get(&self, service: &str, account: &str) -> Result<Option<String>, String> {
            Ok(self.0.lock().get(&Self::key(service, account)).cloned())
        }

        fn delete(&self, service: &str, account: &str) -> Result<(), String> {
            self.0.lock().remove(&Self::key(service, account));
            Ok(())
        }
    }

    #[test]
    fn missing_secret_reads_as_none() {
        let store = MemoryStore::new();
        assert_eq!(store.get("svc", "acct").unwrap(), None);
    }

    #[test]
    fn set_then_get_roundtrips() {
        let store = MemoryStore::new();
        store.set("svc", "acct", "token").unwrap();
        assert_eq!(store.get("svc", "acct").unwrap().as_deref(), Some("token"));
    }

    #[test]
    fn delete_is_idempotent() {
        let store = MemoryStore::new();
        store.set("svc", "acct", "token").unwrap();
        store.delete("svc", "acct").unwrap();
        store.delete("svc", "acct").unwrap();
        assert_eq!(store.get("svc", "acct").unwrap(), None);
    }
}
