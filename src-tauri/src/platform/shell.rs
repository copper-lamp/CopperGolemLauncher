//! 平台适配层 · 系统文件管理器定位。
//!
//! 下载条目上的「快捷方式」按钮要求的是**在文件管理器里定位**，而不是用默认
//! 程序打开文件：`.msixvc` / `.levipack` 这类产物双击没有意义（甚至会被当成
//! 压缩包弹一个不相干的关联程序），用户真正想要的是「这东西到底下到哪了」。
//!
//! 因此平台差异收敛到这里：
//! - Windows：`explorer.exe /select,<路径>` 打开所在目录并选中（目录则直接打开）；
//! - 其它桌面平台：退化为用默认文件管理器打开所在目录（没有跨平台的「选中」语义）；
//! - Android / iOS：没有文件管理器可唤起，**显式报不支持**，不假装成功。
//!
//! 路径不存在时明确报错：静默打开一个空目录或替用户猜一个父目录，都会让用户
//! 以为「点了没反应」或者「文件不见了」。

use std::path::Path;

/// `Command::raw_arg` 属于 Windows 扩展 trait，必须显式引入。
#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// 在系统文件管理器中定位 `path`。
///
/// 失败返回用户可读原因（供命令层透传到前端提示）。
pub fn reveal(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("路径不存在：{}", path.display()));
    }
    reveal_existing(path)
}

/// 已确认存在的路径 → 唤起文件管理器。
#[cfg(windows)]
fn reveal_existing(path: &Path) -> Result<(), String> {
    // **必须用 `raw_arg`**：`std::process::Command::arg` 会按 MSVC 规则再包一层
    // 引号并转义内部引号，`/select,"D:\My Pack\x.mcpack"` 会被拼成
    // `"/select,\"D:\My Pack\x.mcpack\""`，而 explorer.exe 用自己的一套命令行
    // 解析，根本认不 backslash 转义 —— 结果就是进程起来了、文件管理器没动，
    // 用户看到「点了没有任何作用」。`raw_arg` 让原始文本原样进命令行。
    let raw = if path.is_dir() {
        // 目录不能带尾随反斜杠：`explorer "C:\dir\"` 里的 `\"` 会被当成转义引号。
        let mut dir = path.to_string_lossy().into_owned();
        while dir.len() > 1 && dir.ends_with('\\') {
            dir.pop();
        }
        format!("\"{dir}\"")
    } else {
        // 逗号后紧跟路径；路径含空格时必须带引号，否则会被拆成多个参数。
        format!("/select,\"{}\"", path.display())
    };
    std::process::Command::new("explorer.exe")
        .raw_arg(&raw)
        // explorer 常驻，立刻返回是正常现象，不能等它退出。
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开文件管理器失败：{e}"))
}

/// 非 Windows 桌面：打开所在目录（无「选中」语义）。
#[cfg(all(not(windows), not(any(target_os = "android", target_os = "ios"))))]
fn reveal_existing(path: &Path) -> Result<(), String> {
    let target = parent_or_self(path);
    tauri_plugin_opener::open_path(&target, None::<&str>)
        .map_err(|e| format!("打开文件管理器失败：{e}"))
}

/// 移动端：没有可唤起的文件管理器。
#[cfg(any(target_os = "android", target_os = "ios"))]
fn reveal_existing(_path: &Path) -> Result<(), String> {
    Err("当前平台不支持在文件管理器中定位文件".into())
}

/// 文件取其父目录；目录取其自身。
#[cfg(all(not(windows), not(any(target_os = "android", target_os = "ios"))))]
fn parent_or_self(path: &Path) -> std::path::PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不存在的路径必须显式报错，而不是打开别的地方。
    #[test]
    fn missing_path_is_rejected() {
        let missing = std::env::temp_dir().join("copper_reveal_does_not_exist_9f3a");
        let _ = std::fs::remove_file(&missing);
        let err = reveal(&missing).unwrap_err();
        assert!(err.contains("路径不存在"), "意外文案: {err}");
    }
}
