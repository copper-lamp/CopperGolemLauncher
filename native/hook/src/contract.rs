//! 启动器侧与注入侧之间的**唯一契约**。
//!
//! # 为什么集中在这里
//!
//! 注入 DLL 与启动器内核是两份独立的 Rust 编译单元。交接依赖四类字符串：
//! 环境变量名、清单文件名与 schema 版本、注入用 DLL 文件名、导入表锚点导出名。
//! 任何一侧单独改掉其中之一，表现都是「静默不生效」——隔离不隔离、加载器加不加，
//! 用户看不出来，内核侧也不会报错。因此这些常量**只允许在本模块定义**，
//! 内核通过 `copper_core_hook::contract` 引用同一批值（`src/modules/home/`）。
//!
//! 本模块必须保持平台中立（可在 Linux CI 上编译与测试）。

/// 交接变量：实例数据根目录（`%APPDATA%` 的重定向目标，**绝对路径**）。
pub const ENV_DATA_DIR: &str = "COPPER_HOOK_DATA_DIR";
/// 交接变量：版本目录（扫 `mods/`、写日志的根，**绝对路径**）。
pub const ENV_ROOT: &str = "COPPER_HOOK_ROOT";
/// 交接变量：渠道（`release` / `preview`），供 DLL 自检与日志。
pub const ENV_CHANNEL: &str = "COPPER_HOOK_CHANNEL";
/// 交接变量：预加载清单路径（**绝对路径**）。
pub const ENV_PRELOAD: &str = "COPPER_HOOK_PRELOAD";

/// 版本目录下的实例元数据文件名（非本启动器启动时 DLL 的回退依据）。
pub const VERSION_META_FILE: &str = "version.json";

/// 预加载清单文件名（相对版本目录）。
pub const MANIFEST_FILE_NAME: &str = "copper-preload.json";
/// 预加载清单 schema 版本。DLL 只接受该版本，其余一律拒绝加载并记日志 ——
/// 宁可什么都不加载，也不要按错误的理解去 `LoadLibrary` 一个猜测出来的路径。
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// 注入 DLL 在版本目录下的落位文件名。
pub const HOOK_DLL_FILE_NAME: &str = "CopperCoreHook.dll";
/// 原始启动文件备份后缀（`Minecraft.Windows.exe.copperorig`）。
pub const BACKUP_SUFFIX: &str = ".copperorig";

/// 注入时追加的 PE 节名（8 字节，不含 NUL）。
pub const HOOK_SECTION_NAME: &[u8; 8] = b".copperh";

/// 导入表锚点导出名：PE 导入表需要一个真实存在的导出符号作为 thunk 指向，
/// 函数体什么都不做（调用它没有任何意义，也不会有调用者）。
pub const ANCHOR_EXPORT: &str = "CopperCoreHookAnchor";

/// DLL 自身的 ABI 版本号（由锚点导出返回，供内核与实机排查确认加载的是哪一份）。
pub const HOOK_ABI_VERSION: u32 = 1;

/// 允许注入的宿主可执行文件名（小写比较）。
///
/// 白名单是硬门槛：本 DLL 会重定向**整个进程的**目录 API，一旦被别的程序
/// （浏览器、游戏、命令行工具）加载，那个程序的 `%APPDATA%` 会被一起改掉。
/// 非白名单宿主一律立即返回，不装 hook、不加载任何原生模组。
pub const HOST_EXE_NAMES: &[&str] = &["minecraft.windows.exe", "minecraft.win10.dx11.exe"];

/// LiteLoader 系的外部预加载器标记文件；存在时本 DLL 让位（见 docs §2.4）。
pub const PRELOADER_MARKER: &str = "preloader.dll";

/// 日志文件名（相对版本目录；**不进 `%TEMP%`**，重定向之后 TMP 本身也是被改的）。
pub const HOOK_LOG_FILE_NAME: &str = "copper-hook.log";
/// 日志体积上限：超过后停止追加并写入一行终止标记。
///
/// 日志在游戏进程里写，无锁跨模块写；不设上限的话一次异常循环就能把用户磁盘写满。
pub const LOG_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// 正式渠道的数据目录名（与游戏内部 `sAppName` 一致）。
pub const RELEASE_DATA_DIR: &str = "Minecraft Bedrock";
/// 预览渠道的数据目录名。
pub const PREVIEW_DATA_DIR: &str = "Minecraft Bedrock Preview";

/// 扁平布局的最低游戏版本（四段）：`>= 1.26.0.0` 起数据目录少一层渠道目录。
///
/// 见 docs §1.3 F7：LeviLauncher 的 Go 侧与 C++ 侧在此处判断相反。本常量是
/// 注入侧的唯一口径，启动器侧必须与之一致（当前 `home/isolate.rs` 仍按
/// 「始终带渠道目录 + 读侧探测」实现，两者尚未合并，见 docs §4.6）。
pub const FLAT_LAYOUT_MIN: [u64; 4] = [1, 26, 0, 0];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_names_are_namespaced() {
        // 环境变量名是跨进程契约，改名等于静默失效；这里钉死字面量。
        for name in [ENV_DATA_DIR, ENV_ROOT, ENV_CHANNEL, ENV_PRELOAD] {
            assert!(name.starts_with("COPPER_HOOK_"), "{name} 未走统一前缀");
        }
    }

    #[test]
    fn section_name_fits_pe_header() {
        // PE 节名固定 8 字节且不以 NUL 开头，否则加载器/工具链会读出乱码。
        assert_eq!(HOOK_SECTION_NAME.len(), 8);
        assert!(!HOOK_SECTION_NAME.contains(&0));
    }

    #[test]
    fn host_whitelist_is_lowercase_and_covers_both_exes() {
        assert!(HOST_EXE_NAMES
            .iter()
            .all(|name| name.chars().all(|c| !c.is_ascii_uppercase())));
        assert!(HOST_EXE_NAMES.contains(&"minecraft.windows.exe"));
        assert!(HOST_EXE_NAMES.contains(&"minecraft.win10.dx11.exe"));
    }
}