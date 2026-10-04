//! 注入侧日志（平台中立核心 + 各平台落盘）。
//!
//! # 为什么不用 `log` / `env_logger`
//!
//! 那是给启动器内核用的（那里有全局单例、有 `%APPDATA%` 下的日志目录）。注入侧
//! 的处境完全不同：
//!
//! - 日志目标必须在**重定向之前**就确定 —— 而重定向之后 `%TEMP%` 本身也是被改的，
//!   按环境变量找临时目录会写进实例目录的 `temp`，被游戏清理掉；
//! - 它在游戏进程里，不能假设任何全局状态已被初始化；
//! - 它不能 panic，也不能分配失败即崩（宿主是游戏，不是我们自己）。
//!
//! 所以自己写一个 append-only 文本日志，带体积上限，且**任何失败都只是丢日志**。

use std::path::Path;

#[cfg(windows)]
use std::fs::{File, OpenOptions};
#[cfg(windows)]
use std::io::Write;
#[cfg(windows)]
use std::sync::{Mutex, OnceLock};

#[cfg(windows)]
use crate::contract::LOG_MAX_BYTES;

#[cfg(windows)]
static LOGGER: OnceLock<Mutex<Option<File>>> = OnceLock::new();
#[cfg(windows)]
static LOG_PATH: OnceLock<std::path::PathBuf> = OnceLock::new();

/// 初始化日志（幂等；第二次调用不换文件）。
///
/// 必须在 `DllMain` 里尽早调用 —— 但**只做「打开文件句柄」这一件事**，
/// 真正写内容放到工作线程。打开文件本身不会触发 DLL 加载（`CreateFileW`
/// 来自已加载的 kernel32），因此不构成 loader lock 风险。
#[cfg(windows)]
pub fn init(path: &Path) {
    let _ = LOG_PATH.set(path.to_path_buf());
    let _ = LOGGER.set(Mutex::new(open_log_file(path)));
}

#[cfg(not(windows))]
pub fn init(_path: &Path) {}

#[cfg(windows)]
fn open_log_file(path: &Path) -> Option<File> {
    if let Some(parent) = path.parent() {
        // 目录不存在（实例目录被删 / 权限异常）时只放弃日志，不影响重定向。
        let _ = std::fs::create_dir_all(parent);
    }
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(mut file) => {
            let _ = writeln!(file, "--- copper-core-hook 会话开始 ---");
            Some(file)
        }
        Err(_) => None,
    }
}

/// 写一行日志。**永不 panic、永不返回错误。**
///
/// 宿主是游戏：这里的一次 unwrap 失败就是整个游戏进程崩掉，而日志本身
/// 只是排障材料。所有 `Option` / `Result` 一律吞掉。
pub fn line(level: &str, message: &str) {
    write_line(level, message);
    #[cfg(windows)]
    if let Some(logger) = LOGGER.get() {
        // 超过体积上限就停写并留一行终止标记：日志在游戏进程里无锁写，
        // 一次异常循环就能把用户磁盘写满。
        if let Some(path) = LOG_PATH.get() {
            if !within_budget(path) {
                return;
            }
        }
        if let Ok(mut guard) = logger.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = writeln!(file, "[{level}] {message}");
                let _ = file.flush();
            }
        }
    }
}

/// 记录一条错误（与 [`line`] 同语义，单独命名只为调用点可读）。
pub fn error(message: impl AsRef<str>) {
    line("ERROR", message.as_ref());
}

/// 记录一条警告。
pub fn warn(message: impl AsRef<str>) {
    line("WARN", message.as_ref());
}

/// 记录一条信息。
pub fn info(message: impl AsRef<str>) {
    line("INFO", message.as_ref());
}

/// 写一行到标准输出（`debug_print` 风格）。
///
/// 不写文件时（未初始化 / 非 Windows）这是唯一的排障出口：游戏进程的
/// stdout 通常不可见，但 OutputDebugString 在调试器与事件查看器里可见。
pub fn write_line(level: &str, message: &str) {
    #[cfg(windows)]
    crate::sys::debug_output(&format!("[copper-hook][{level}] {message}\n"));
    #[cfg(not(windows))]
    {
        let _ = (level, message);
    }
}

/// 体积上限内的追加检查（超过即停止写文件，只留 debug 输出）。
#[cfg(windows)]
pub fn within_budget(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.len() < LOG_MAX_BYTES).unwrap_or(true)
}