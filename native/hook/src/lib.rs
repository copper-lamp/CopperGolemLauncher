//! 铜傀儡注入侧 hook DLL。
//!
//! 这个 crate 产出两份东西，**同一份源码**：
//!
//! - `cdylib` → `copper_core_hook.dll`：被游戏进程按 PE 导入表加载，负责
//!   ①实例隔离（把游戏的 `%APPDATA%` / `%TEMP%` / `%LOCALAPPDATA%` 重定向到
//!   实例目录）②执行启动器生成的原生预加载清单。方案见
//!   `docs/启动链路与实例隔离.md`；
//! - `rlib` → 被 `src-tauri` 以普通路径依赖引用，让内核能
//!   ①在编译顺序上保证 DLL 先于内核产出 ②直接复用 [`contract`] 里的交接契约
//!   常量，避免两侧各写一份字符串而悄悄漂移。
//!
//! # 分层
//!
//! | 模块 | 平台 | 职责 |
//! |---|---|---|
//! | [`contract`] | 中立 | 交接契约常量（环境变量名 / 清单 schema / 锚点导出名 …） |
//! | [`manifest`] | 中立 | `copper-preload.json` 解析与版本闸门 |
//! | [`paths`] | 中立 | 重定向目标计算、清单路径逃逸校验 |
//! | [`meta`] | 中立 | `version.json` 回退解析（非本启动器启动的实例） |
//! | [`log`] | 中立接口 / Windows 落盘 | 追加写日志，带体积上限，永不 panic |
//! | [`sys`] | Windows | 十余个 Win32 符号的原始 FFI 声明（唯一外部依赖入口） |
//! | [`iat`] | Windows | 主映像导入表遍历与槽位替换 |
//! | [`preload`] | Windows | 按清单 `LoadLibraryExW` |
//! | [`ffi`] | Windows | `DllMain`、三个重定向函数、工作线程 |
//!
//! # 为什么平台中立部分这么多
//!
//! 隔离能不能用，最终取决于几个纯逻辑判断：布局分界在哪、清单路径有没有
//! 越界、schema 版本认不认得。这些在 Linux CI 上就能测，而 CI 上跑不了游戏。
//! 把它们留在平台中立层，是这个 crate 最重要的设计决定 —— `docs §2.9` 要求的
//! 「非 Windows 只编译平台中立核心」不是省事，是让核心逻辑有测试网。
//!
//! # 安全约束（docs §2.9，改成 C++ 也一样要守）
//!
//! 1. `DllMain` 内不 `LoadLibrary`、不等待工作线程；
//! 2. hook 必须在游戏首次查询目录之前装好；
//! 3. hook 内部调用原函数必须绕开已打补丁的 IAT（本实现用装载前保存的槽原值）；
//! 4. 任何 panic 都不得跨 FFI 边界 —— 因此 `ffi.rs` 里没有 `unwrap`；
//! 5. 模块 `pin`，detach 时不释放；
//! 6. 只 hook 主映像 IAT；delay-load 与 `GetProcAddress` 动态解析抓不到。

#![deny(rust_2018_idioms)]
#![warn(missing_debug_implementations)]

pub mod contract;
pub mod log;
pub mod manifest;
pub mod meta;
pub mod paths;

#[cfg(windows)]
pub mod ffi;
#[cfg(windows)]
pub mod iat;
#[cfg(windows)]
pub mod preload;
#[cfg(windows)]
pub mod sys;

/// 本 crate 的语义版本（注入侧 ABI 变更时递增）。
///
/// 与 [`contract::HOOK_ABI_VERSION`] 的区别：那个是**PE 契约**版本（内核按它
/// 判断导入的锚点是不是自己写的那个），这个是实现版本。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn crate_exposes_contract_to_the_kernel_side() {
        // 内核（`src-tauri`）就是靠这批常量与注入侧对齐的。字面量在这里钉死，
        // 改名会在编译期暴露，而不是在游戏里表现为「静默不生效」。
        use crate::contract;
        assert_eq!(contract::ENV_DATA_DIR, "COPPER_HOOK_DATA_DIR");
        assert_eq!(contract::ENV_ROOT, "COPPER_HOOK_ROOT");
        assert_eq!(contract::ENV_CHANNEL, "COPPER_HOOK_CHANNEL");
        assert_eq!(contract::ENV_PRELOAD, "COPPER_HOOK_PRELOAD");
        assert_eq!(contract::MANIFEST_FILE_NAME, "copper-preload.json");
        assert_eq!(contract::MANIFEST_SCHEMA_VERSION, 1);
        assert_eq!(contract::ANCHOR_EXPORT, "CopperCoreHookAnchor");
        assert_eq!(contract::HOOK_DLL_FILE_NAME, "CopperCoreHook.dll");
        assert_eq!(contract::BACKUP_SUFFIX, ".copperorig");
        assert_eq!(contract::HOOK_LOG_FILE_NAME, "copper-hook.log");
    }
}