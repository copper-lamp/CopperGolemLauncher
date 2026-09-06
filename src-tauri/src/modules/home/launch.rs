//! 启动游戏：进程启动 + 运行检测 + 启动后监控。
//!
//! 两条启动路径（与 LeviLauncher 一致）：
//! - 版本已注册到 AppX（`meta.registered`）：通过 `minecraft://` 协议唤起；
//! - 未注册：直接启动版本目录下的 `Minecraft.Windows.exe`（可附加启动参数 / 环境变量）。
//!
//! 启动成功后由后台任务监控进程状态，确认游戏运行时广播 `game.launched`。

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use serde::Serialize;

use crate::error::KernelError;
use crate::state::KernelContext;

use super::meta::{resolve_version_dir, VersionMeta};

/// 游戏主程序文件名。
const GAME_EXE: &str = "Minecraft.Windows.exe";
/// 启动后确认游戏进程运行的最长等待（秒）。
const LAUNCH_CONFIRM_TIMEOUT_SECS: u64 = 90;
/// 进程状态轮询间隔（毫秒）。
const POLL_INTERVAL_MS: u64 = 800;

/// 启动结果（供前端反馈）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchOutcome {
    /// 直接启动了游戏进程。
    Spawned,
    /// 通过 `minecraft://` 协议唤起（AppX 注册版本）。
    Protocol,
}

/// 校验启动名并解析版本目录（复用 meta 的防逃逸逻辑）。
fn resolve_launch_dir(kernel: &KernelContext, name: &str) -> Result<PathBuf, KernelError> {
    resolve_version_dir(kernel.paths().versions_dir(), name)
}

/// 检测某路径下是否有游戏进程在运行（Windows 进程枚举，路径归一化比对）。
#[cfg(windows)]
pub fn is_process_running_at_path(exe_path: &std::path::Path) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let target = normalize_path(exe_path);
    if target.is_empty() {
        return false;
    }
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return false;
        };
        let mut found = false;
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut has_entry = Process32FirstW(snapshot, &mut entry).is_ok();
        while has_entry {
            let exe_name = windows::core::PWSTR(entry.szExeFile.as_ptr());
            let name = unsafe { exe_name.to_string().unwrap_or_default() };
            if name.eq_ignore_ascii_case(GAME_EXE) {
                if let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, entry.th32ProcessID) {
                    let mut buf = [0u16; 1024];
                    let mut size = buf.len() as u32;
                    let ok = QueryFullProcessImageNameW(
                        handle,
                        0,
                        windows::core::PWSTR(buf.as_mut_ptr()),
                        &mut size,
                    )
                    .is_ok();
                    let _ = CloseHandle(handle);
                    if ok {
                        let path = String::from_utf16_lossy(&buf[..size as usize]);
                        if normalize_path(std::path::Path::new(&path)) == target {
                            found = true;
                            break;
                        }
                    }
                }
            }
            has_entry = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

/// 非 Windows 平台：不检测（返回 false，启动仍可执行，但无运行确认）。
#[cfg(not(windows))]
pub fn is_process_running_at_path(_exe_path: &std::path::Path) -> bool {
    false
}

/// 路径归一化（小写 + 清理 + 去 UNC 前缀），用于进程路径比对。
#[cfg(windows)]
fn normalize_path(p: &std::path::Path) -> String {
    let mut s = p
        .canonicalize()
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .to_lowercase();
    for prefix in ["\\\\?\\", "\\??\\"] {
        while let Some(stripped) = s.strip_prefix(prefix) {
            s = stripped.to_string();
        }
    }
    s
}

/// 解析命令行参数字符串（支持引号包裹的空白）。
fn parse_launch_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    for c in input.chars() {
        match c {
            '"' => in_quote = !in_quote,
            c if c.is_whitespace() => {
                if in_quote {
                    current.push(c);
                } else if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// 解析环境变量文本（每行 `KEY=VALUE`）。
fn parse_env_vars(input: &str) -> Vec<(String, String)> {
    input
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (k, v) = line.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// 启动游戏。`check_running` 为 true 时，若该版本已在运行则返回错误。
pub fn launch_game(
    kernel: &KernelContext,
    name: &str,
    check_running: bool,
) -> Result<LaunchOutcome, KernelError> {
    let dir = resolve_launch_dir(kernel, name)?;
    let meta = VersionMeta::read(&dir)
        .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{name}` 元数据缺失")))?;
    let exe = dir.join(GAME_EXE);
    if !exe.is_file() {
        return Err(KernelError::InvalidArgument(format!(
            "版本 `{name}` 缺少 {GAME_EXE}"
        )));
    }

    // 已注册 AppX：协议唤起。
    if meta.registered {
        if check_running && is_process_running_at_path(&exe) {
            return Err(KernelError::InvalidArgument("游戏已在运行".into()));
        }
        let is_preview = meta.version_type.eq_ignore_ascii_case("preview");
        let protocol = if is_preview {
            "minecraft-preview://"
        } else {
            "minecraft://"
        };
        let url = if meta.enable_editor_mode {
            format!("{protocol}creator/?Editor=true")
        } else {
            protocol.to_string()
        };
        spawn_protocol(&url)?;
        monitor_after_launch(kernel, name, dir.clone());
        return Ok(LaunchOutcome::Protocol);
    }

    // 未注册：直接启动 exe。
    let args: Vec<String> = if meta.enable_editor_mode {
        vec!["-Editor".into(), "true".into()]
    } else {
        parse_launch_args(&meta.launch_args)
    };

    if check_running && is_process_running_at_path(&exe) {
        return Err(KernelError::InvalidArgument("游戏已在运行".into()));
    }

    let mut cmd = Command::new(&exe);
    cmd.args(&args);
    cmd.current_dir(&dir);
    let envs = parse_env_vars(&meta.env_vars);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    // 启动后丢弃子进程句柄：游戏进程脱离启动器独立运行，由后台监控接管。
    let _child = cmd.spawn().map_err(|e| {
        KernelError::InvalidArgument(format!("启动游戏失败: {e}"))
    })?;

    monitor_after_launch(kernel, name, dir);
    Ok(LaunchOutcome::Spawned)
}

/// 通过协议 URL 唤起游戏（`cmd /c start "" <url>`）。
#[cfg(windows)]
fn spawn_protocol(url: &str) -> Result<(), KernelError> {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("cmd");
    cmd.args(["/c", "start", "", url]);
    // 隐藏 cmd 窗口。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| KernelError::InvalidArgument(format!("唤起游戏失败: {e}")))
}

#[cfg(not(windows))]
fn spawn_protocol(url: &str) -> Result<(), KernelError> {
    let _ = url;
    Err(KernelError::InvalidArgument("协议启动仅支持 Windows".into()))
}

/// 后台监控：轮询游戏进程，确认运行后广播 `game.launched`。
/// 之后持续观察直到进程退出（释放监控任务）。
fn monitor_after_launch(kernel: &Arc<KernelContext>, name: &str, dir: PathBuf) {
    let kernel = kernel.clone();
    let name = name.to_string();
    kernel.runtime().spawn(async move {
        let exe = dir.join(GAME_EXE);
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(LAUNCH_CONFIRM_TIMEOUT_SECS);
        let mut confirmed = false;
        while std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
            if is_process_running_at_path(&exe) {
                if !confirmed {
                    confirmed = true;
                    kernel.events().publish(
                        "game.launched",
                        serde_json::json!({ "name": name }),
                    );
                }
                break;
            }
        }
        if confirmed {
            // 持续观察直到退出（保持语义：一次启动对应一个生命周期）。
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                if !is_process_running_at_path(&exe) {
                    break;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_handles_quotes() {
        assert_eq!(parse_launch_args(""), Vec::<String>::new());
        assert_eq!(parse_launch_args("-a -b"), vec!["-a", "-b"]);
        assert_eq!(parse_launch_args("-a \"b c\" -d"), vec!["-a", "b c", "-d"]);
    }

    #[test]
    fn parse_env_lines() {
        assert_eq!(parse_env_vars(""), Vec::<(String, String)>::new());
        assert_eq!(
            parse_env_vars("A=1\n\nB = x y\n"),
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "x y".to_string())
            ]
        );
        // 无 '=' 的行被忽略
        assert_eq!(parse_env_vars("JUST_TEXT"), Vec::<(String, String)>::new());
    }
}
