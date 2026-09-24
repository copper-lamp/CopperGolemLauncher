//! 平台适配层：把「随平台变化」的能力收敛到可替换实现，业务代码只依赖接口。
//!
//! 设计意图见 docs/平台适配.md。核心原则：
//!
//! 1. **平台差异沉到底层**：业务（账户 / 开始页 / 下载）不写 `cfg(...)`，只调接口。
//! 2. **不支持要显式**：平台缺能力时返回「不支持」，让既有降级分支自然成立，
//!    而不是伪造一个假实现（项目规范禁止假实现）。
//! 3. **依赖按平台门控**：仅桌面可用的依赖（`keyring` / `self-replace`）在
//!    `Cargo.toml` 按 target 引入，避免拖累 Android 交叉编译。
//!
//! 当前已落地的后端：
//! - [`SecretStore`]：系统安全存储（桌面密钥环 / Android 待接 Keystore）。
//!
//! 尚未抽象（等对应平台实现就位再抽，避免空接口）：
//! GameBackend / AuthBackend / StoreBackend / ProcessBackend / UpdaterBackend，
//! 详见 docs/平台适配.md 3.3 TODO。

pub mod secret;

use std::sync::Arc;

use secret::{SecretStore, SharedSecretStore};
#[cfg(not(target_os = "android"))]
use secret::KeyringStore;
#[cfg(target_os = "android")]
use secret::UnsupportedStore;

/// 平台标识字符串（与 `cgl-models` / `cgl-libs` 的 `platforms` 枚举逐字一致）。
///
/// 供前端 Shell 布局适配与模块 `platforms` 过滤共用，避免两处各写一套探测。
pub fn current_platform_id() -> &'static str {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "windows-x86_64"
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        "windows-aarch64"
    } else if cfg!(all(target_os = "android", target_arch = "aarch64")) {
        "android-arm64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x86_64"
    } else {
        "unknown"
    }
}

/// 平台形态：决定前端 Shell 布局（桌面窗口 / 移动全屏）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormFactor {
    Desktop,
    Mobile,
}

impl FormFactor {
    pub fn current() -> Self {
        if cfg!(any(target_os = "android", target_os = "ios")) {
            Self::Mobile
        } else {
            Self::Desktop
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Mobile => "mobile",
        }
    }
}

/// 平台后端聚合体：启动阶段按 `cfg` 装配一次，注入 `KernelContext`。
///
/// 与其它内核服务一致，经 `Arc` 共享、内部状态自持（后端实现需 `Send + Sync`）。
pub struct Backends {
    secret: SharedSecretStore,
}

impl Backends {
    /// 按当前平台装配全部后端。
    pub fn assemble() -> Self {
        Self {
            secret: Self::assemble_secret_store(),
        }
    }

    /// 装配凭证存储：桌面走系统密钥环；Android 暂无安全存储，显式不支持。
    #[cfg(not(target_os = "android"))]
    fn assemble_secret_store() -> SharedSecretStore {
        Arc::new(KeyringStore::new())
    }

    #[cfg(target_os = "android")]
    fn assemble_secret_store() -> SharedSecretStore {
        // 未接入 Android Keystore 前，登录无法持久化；此处显式失败而非降级为
        // 明文存储（见 docs/平台适配.md 3.1 风险与 3.3 TODO）。
        Arc::new(UnsupportedStore::new())
    }

    /// 凭证安全存储。
    pub fn secret(&self) -> &SharedSecretStore {
        &self.secret
    }

    /// 平台标识（`kernel_info` 下发前端）。
    pub fn platform_id(&self) -> &'static str {
        current_platform_id()
    }

    /// 平台形态（`kernel_info` 下发前端）。
    pub fn form_factor(&self) -> FormFactor {
        FormFactor::current()
    }
}

impl Default for Backends {
    fn default() -> Self {
        Self::assemble()
    }
}

/// 便于直接以 `dyn SecretStore` 传参。
pub fn as_secret(backends: &Backends) -> Arc<dyn SecretStore> {
    backends.secret().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_id_is_never_empty() {
        assert!(!current_platform_id().is_empty());
    }

    /// 桌面平台上平台标识必须落在已知枚举内（防止 cfg 写错落到 unknown）。
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn desktop_platform_id_is_known() {
        let id = current_platform_id();
        assert!(
            [
                "windows-x86_64",
                "windows-aarch64",
                "linux-x86_64",
                "android-arm64"
            ]
            .contains(&id),
            "unexpected platform id: {id}"
        );
    }

    #[test]
    fn form_factor_matches_target_os() {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        assert_eq!(FormFactor::current(), FormFactor::Mobile);
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        assert_eq!(FormFactor::current(), FormFactor::Desktop);
    }
}
