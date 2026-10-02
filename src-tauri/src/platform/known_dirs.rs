//! 平台适配层 · 系统用户目录。
//!
//! 内容下载的兜底落点需要一个「用户能自己找到」的目录：游戏没装时，内容不该
//! 落进内核私有缓存（用户既打不开、也不知道东西去哪了），而该落进系统的
//! 「下载」目录。解析它必须走系统 API 而不是猜路径——
//! Windows 的下载目录**可以被用户改到任意盘符**（含 OneDrive 重定向、
//! 组策略下发、注册表 `Shell Folders` 手改），`%USERPROFILE%\Downloads`
//! 只是绝大多数机器的巧合。
//!
//! 平台差异收敛到这里：
//! - Windows：`SHGetKnownFolderPath(FOLDERID_Downloads)`，失败回退
//!   `%USERPROFILE%\Downloads`；
//! - 其它平台：**显式返回 `None`**，不伪造一个路径。伪造的后果是内核以为
//!   有兜底目录，实际把文件写到一个用户永远找不到的地方——那比没有兜底更糟。
//!
//! 刻意不引 `dirs` crate：`services/paths.rs` 已明确「不硬依赖 `directories`，
//! 否则 Android 交叉编译会断」，平台层必须遵守同一条约束。

use std::path::PathBuf;

/// 系统用户目录后端。
pub trait UserDirs: Send + Sync {
    /// 用户「下载」目录。平台不提供（或解析失败）时返回 `None`。
    fn downloads_dir(&self) -> Option<PathBuf>;
}

// ---------------------------------------------------------------- Windows

/// Windows：Known Folder API 优先，环境变量兜底。
#[cfg(windows)]
#[derive(Debug, Default)]
pub struct ShellUserDirs;

#[cfg(windows)]
impl UserDirs for ShellUserDirs {
    fn downloads_dir(&self) -> Option<PathBuf> {
        known_folder_downloads().or_else(userprofile_downloads)
    }
}

/// `SHGetKnownFolderPath(FOLDERID_Downloads)`。
///
/// 这是唯一能反映「用户在「下载」文件夹属性里改了位置」以及 OneDrive
/// 重定向的途径。返回的 `PWSTR` 由 shell 分配，必须 `CoTaskMemFree`，
/// 否则每次调用泄漏一次。
#[cfg(windows)]
fn known_folder_downloads() -> Option<PathBuf> {
    use windows::Win32::UI::Shell::{FOLDERID_Downloads, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath};

    unsafe {
        // `KF_FLAG_DEFAULT`（0）：不做路径规范化、不校验存在性。
        // 这里刻意不传 `KF_FLAG_CREATE`：内核不该在解析阶段就往用户目录下
        // 建目录——解析是只读动作，建目录由真正落盘的那一步负责。
        let raw = SHGetKnownFolderPath(&FOLDERID_Downloads, KNOWN_FOLDER_FLAG(0), None)
            .ok()?;
        // `SHGetKnownFolderPath` 返回的 `PWSTR` 由 shell 分配；`windows` crate
        // 的 wrapper 包装成拥有所有权的 `HSTRING`/`PWSTR` 返回值后，会在
        // 离开作用域时 `CoTaskMemFree`，因此这里不需要也不应该手动释放。
        raw.to_string()
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
    }
}

/// `%USERPROFILE%\Downloads` 兜底。
#[cfg(windows)]
fn userprofile_downloads() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty())?;
    let dir = PathBuf::from(home).join("Downloads");
    dir.is_dir().then_some(dir)
}

// ---------------------------------------------------------------- 其它平台

/// 非 Windows：显式不支持。
///
/// 后续接入 Linux（XDG user dirs）/ macOS 时各自替换本实现即可，
/// 调用方无需任何改动。
#[cfg(not(windows))]
#[derive(Debug, Default)]
pub struct UnsupportedUserDirs;

#[cfg(not(windows))]
impl UserDirs for UnsupportedUserDirs {
    fn downloads_dir(&self) -> Option<PathBuf> {
        None
    }
}

/// 按当前平台装配用户目录后端。
pub fn assemble() -> std::sync::Arc<dyn UserDirs> {
    #[cfg(windows)]
    {
        std::sync::Arc::new(ShellUserDirs)
    }
    #[cfg(not(windows))]
    {
        std::sync::Arc::new(UnsupportedUserDirs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 后端可装配且调用不 panic（不要求真有结果）。
    #[test]
    fn backend_assembles_and_answers() {
        let dirs = assemble();
        // 允许 None：这是「平台不提供」的正确表达，不是失败。
        let _ = dirs.downloads_dir();
    }

    /// Windows 上：有 `USERPROFILE` 时兜底路径必须落在该 profile 下。
    #[cfg(windows)]
    #[test]
    fn userprofile_fallback_is_under_profile() {
        let Some(home) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) else {
            return;
        };
        let Some(dir) = userprofile_downloads() else {
            // 目录不存在时不返回是正确的（不伪造）。
            return;
        };
        assert!(dir.starts_with(PathBuf::from(home)));
        assert_eq!(dir.file_name().and_then(|n| n.to_str()), Some("Downloads"));
    }
}
