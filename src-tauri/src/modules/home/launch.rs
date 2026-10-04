//! 启动游戏：进程启动 + 运行检测 + 启动后监控。
//!
//! 两条启动路径（与 LeviLauncher 一致）：
//! - 版本已注册到 AppX（`meta.registered`）：通过 `minecraft://` 协议唤起；
//! - 未注册：直接启动版本目录下的 `Minecraft.Windows.exe`（可附加启动参数 / 环境变量）。
//!
//! 启动成功后由后台任务监控进程状态，确认游戏运行时广播 `game.launched`。

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex};

#[cfg(target_os = "android")]
use crate::modules::home::meta::AndroidVersionMeta;

use serde::Serialize;

use crate::error::KernelError;
use crate::registry::events::EventBus;
use crate::services::paths::Paths;
use crate::services::settings::SettingsService;
use crate::state::KernelContext;

use super::meta::{resolve_version_dir, VersionMeta};

/// 游戏主程序文件名。
pub const GAME_EXE: &str = "Minecraft.Windows.exe";
/// 启动后确认游戏进程运行的最长等待（秒）。
const LAUNCH_CONFIRM_TIMEOUT_SECS: u64 = 90;
/// 进程状态轮询间隔（毫秒）。
const POLL_INTERVAL_MS: u64 = 400;
/// 确认启动后观察生命周期的轮询间隔（毫秒）。
const WATCH_INTERVAL_MS: u64 = 1500;

/// 启动所需的内核能力集（意图处理器等 `'static` 场景捕获用）。
#[derive(Clone)]
pub struct LaunchCtx {
    pub paths: Arc<Paths>,
    pub settings: Arc<SettingsService>,
    pub runtime: tokio::runtime::Handle,
    pub events: Arc<EventBus>,
    /// 启动器主窗口句柄（窗口行为用；0 = 尚未就绪）。
    pub hwnd: isize,
}

impl LaunchCtx {
    /// 从内核上下文提取（克隆内部 Arc，可安全跨 'static 捕获）。
    pub fn from_kernel(kernel: &KernelContext) -> Self {
        Self {
            paths: kernel.paths().clone(),
            settings: kernel.settings().clone(),
            runtime: kernel.runtime().clone(),
            events: kernel.events().clone(),
            hwnd: kernel.window().get(),
        }
    }
}

/// 按路径查找运行中的游戏进程 pid（取第一个）。
///
/// 监控要用 pid 判定「窗口是否真的上了屏」；协议唤起拿不到启动 pid，只能
/// 这样反查。
pub fn find_pid_at_path(exe_path: &std::path::Path) -> Option<u32> {
    pids_at_path(exe_path).first().copied()
}

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
fn resolve_launch_dir(paths: &Paths, settings: &SettingsService, name: &str) -> Result<PathBuf, KernelError> {
    resolve_version_dir(&paths.versions_root(settings), name)
}

/// 检测某路径下是否有游戏进程在运行（Windows 进程枚举，路径归一化比对）。
#[cfg(windows)]
pub fn is_process_running_at_path(exe_path: &std::path::Path) -> bool {
    !pids_at_path(exe_path).is_empty()
}

/// 枚举「映像路径等于 `exe_path`」的游戏进程 pid。
///
/// 按**完整路径**比对而非仅进程名：多版本共存时进程名恒为 `Minecraft.Windows.exe`，
/// 只有路径能把某个实例的进程区分出来。运行检测与结束游戏共用这一份实现，
/// 避免两条链路对「哪个进程属于这个版本」的判断出现分歧。
#[cfg(windows)]
fn pids_at_path(exe_path: &std::path::Path) -> Vec<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let target = normalize_path(exe_path);
    if target.is_empty() {
        return Vec::new();
    }
    let mut pids = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return Vec::new();
        };
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut has_entry = Process32FirstW(snapshot, &mut entry).is_ok();
        while has_entry {
            let exe_name = windows::core::PWSTR(entry.szExeFile.as_ptr() as *mut u16);
            let name = exe_name.to_string().unwrap_or_default();
            if name.eq_ignore_ascii_case(GAME_EXE) {
                if let Ok(handle) =
                    OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, entry.th32ProcessID)
                {
                    let mut buf = [0u16; 1024];
                    let mut size = buf.len() as u32;
                    let ok = QueryFullProcessImageNameW(
                        handle,
                        PROCESS_NAME_WIN32,
                        windows::core::PWSTR(buf.as_mut_ptr()),
                        &mut size,
                    )
                    .is_ok();
                    let _ = CloseHandle(handle);
                    if ok {
                        let path = String::from_utf16_lossy(&buf[..size as usize]);
                        if normalize_path(std::path::Path::new(&path)) == target {
                            pids.push(entry.th32ProcessID);
                        }
                    }
                }
            }
            has_entry = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    pids
}

/// 非 Windows 平台：不检测（返回 false，启动仍可执行，但无运行确认）。
#[cfg(not(windows))]
pub fn is_process_running_at_path(_exe_path: &std::path::Path) -> bool {
    false
}

/// 强制结束该版本目录下运行的游戏进程，返回结束的进程数。
///
/// 语义是「结束进程」而非「请求退出」：Minecraft 没有可用的退出 IPC，
/// 只能终止。进程消失后由 `monitor_after_launch` 的观察循环广播 `game.exited`，
/// 前端按钮因此不需要在命令返回时自行改状态（避免与真实进程状态脱节）。
#[cfg(windows)]
pub fn terminate_process_at_path(exe_path: &std::path::Path) -> Result<usize, KernelError> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, TerminateProcess, PROCESS_TERMINATE,
    };

    let pids = pids_at_path(exe_path);
    if pids.is_empty() {
        return Ok(0);
    }
    let mut killed = 0usize;
    let mut last_error: Option<String> = None;
    for pid in pids {
        unsafe {
            let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) else {
                last_error = Some(format!("进程 {pid} 拒绝访问"));
                continue;
            };
            let ok = TerminateProcess(handle, 1).is_ok();
            let _ = CloseHandle(handle);
            if ok {
                killed += 1;
            } else {
                last_error = Some(format!("进程 {pid} 结束失败"));
            }
        }
    }
    if killed == 0 {
        return Err(KernelError::InvalidArgument(
            last_error.unwrap_or_else(|| "结束游戏失败".into()),
        ));
    }
    Ok(killed)
}

/// 非 Windows 平台（含 Android）：宿主不由本进程托管，不提供强制结束。
#[cfg(not(windows))]
pub fn terminate_process_at_path(_exe_path: &std::path::Path) -> Result<usize, KernelError> {
    Err(KernelError::InvalidArgument(
        "当前平台不支持强制结束游戏".into(),
    ))
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
///
/// 顺序固定为 **prepare → inject → 分流 → 监控**（`docs/启动链路与实例隔离.md` §2.7）：
/// 注入必须**先于**任何一种启动方式，否则协议唤起的实例里根本没有 hook DLL，
/// 隔离与加载器静默失效 —— 而这种失效没有任何报错。
pub fn launch_game(
    ctx: &LaunchCtx,
    name: &str,
    check_running: bool,
) -> Result<LaunchOutcome, KernelError> {
    let dir = resolve_launch_dir(&ctx.paths, &ctx.settings, name)?;
    let mut meta = VersionMeta::read(&dir)
        .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{name}` 元数据缺失")))?;
    #[cfg(target_os = "android")]
    if let let android) = meta.android.as_ref() {
        return launch_android(ctx, name, &dir, android);
    }
    let exe = game_exe_path(&dir);
    if !exe.is_file() {
        return Err(KernelError::InvalidArgument(format!(
            "版本 `{name}` 缺少 {GAME_EXE}"
        )));
    }
    if check_running && is_process_running_at_path(&exe) {
        return Err(KernelError::InvalidArgument("游戏已在运行".into()));
    }

    // 0) prepare：骨架目录 + 预加载清单 + 控制台子系统
    prepare_instance(ctx, name, &dir, &exe, &mut meta)?;

    // 1) inject：hook DLL 落位 + 导入表改写
    #[cfg(windows)]
    match super::inject::inject(&dir, &exe) {
        Ok(outcome) => {
            if outcome.dll_written || outcome.exe_patched {
                log::info!(
                    "[home/launch] 版本 `{name}` 已注入 hook（dll={} exe={}）",
                    outcome.dll_written,
                    outcome.exe_patched
                );
            }
        }
        Err(error) => {
            // 注入失败必须显式失败：继续启动等于给用户一个「数据写在启动器
            // 管不到的地方」的实例，比报错难收拾得多。
            return Err(error);
        }
    }

    // 2) 分流：已注册 AppX 走协议唤起，否则直接启动 exe。
    //
    // 协议唤起只在「该版本确实被注册进系统」时成立：`minecraft://` 的处理程序
    // 是全局唯一的，注册别的包（或压根没注册）时唤起起来的不是这个版本目录。
    // 协议失败一律回落到直接启动，并把 `registered` 就地纠正为 false。
    if meta.registered {
        if is_version_registered(&dir) {
            let is_preview = meta.version_type.eq_ignore_ascii_case("preview");
            let protocol = if is_preview {
                "minecraft-preview://"
            } else {
                "minecraft://"
            };
            let editor_supported =
                super::isolate::supports_editor_mode(&meta.game_version, &meta.version_type);
            let url = if meta.enable_editor_mode && editor_supported {
                format!("{protocol}creator/?Editor=true")
            } else {
                protocol.to_string()
            };
            if spawn_protocol(&url).is_ok() {
                monitor_after_launch(ctx, name, dir, 0);
                return Ok(LaunchOutcome::Protocol);
            }
            log::warn!("[home/launch] 版本 `{name}` 协议唤起失败，回落到直接启动");
        } else {
            log::info!("[home/launch] 版本 `{name}` 未在系统中注册，改走直接启动");
        }
        demote_registered(&dir);
    }

    // 3) 直启
    let args = launch_args(&meta, name);
    let mut cmd = Command::new(&exe);
    cmd.args(&args);
    cmd.current_dir(&dir);
    for (k, v) in parse_env_vars(&meta.env_vars) {
        cmd.env(k, v);
    }
    apply_hook_env(&mut cmd, &dir, &meta);
    // 启动后丢弃子进程句柄：游戏进程脱离启动器独立运行，由后台监控接管。
    // 但 pid 要留下 —— 它是唯一能区分「秒退」与「迟迟没起来」的依据。
    let pid = cmd
        .spawn()
        .map_err(|e| KernelError::InvalidArgument(format!("启动游戏失败: {e}")))?
        .id();

    monitor_after_launch(ctx, name, dir, pid);
    Ok(LaunchOutcome::Spawned)
}

/// 游戏主程序在实例目录下的路径。
pub fn game_exe_path(version_dir: &std::path::Path) -> PathBuf {
    version_dir.join(GAME_EXE)
}

/// 组装启动参数。
///
/// 编辑器模式走 `-Editor true`，但**先过版本门槛**：门槛以下硬塞 `-Editor`
/// 只会让游戏启动失败（正式版 1.21.50 / 预览版 1.19.80.20 起才有编辑器）。
fn launch_args(meta: &VersionMeta, name: &str) -> Vec<String> {
    let editor = meta.enable_editor_mode
        && super::isolate::supports_editor_mode(&meta.game_version, &meta.version_type);
    if meta.enable_editor_mode && !editor {
        log::warn!(
            "[home/launch] 版本 `{name}`（{} {}）不支持编辑器模式，已忽略该开关",
            meta.version_type,
            meta.game_version
        );
        return parse_launch_args(&meta.launch_args);
    }
    if editor {
        return vec!["-Editor".into(), "true".into()];
    }
    parse_launch_args(&meta.launch_args)
}

/// 启动前的实例准备：骨架目录、预加载清单、控制台子系统。
fn prepare_instance(
    ctx: &LaunchCtx,
    name: &str,
    dir: &std::path::Path,
    exe: &std::path::Path,
    meta: &mut VersionMeta,
) -> Result<(), KernelError> {
    // 隔离强制：无论元数据写的是什么，本次启动都按隔离实例处理。
    if super::isolate::enforce_isolation(meta) {
        // 自愈写盘失败不阻断启动：隔离本身不依赖这个字段。
        if let Err(error) = VersionMeta::write(dir, meta) {
            log::warn!("[home/launch] 归一隔离标记失败（不影响启动）: {error}");
        }
    }
    super::isolate::ensure_skeleton(dir, meta)?;

    // 预加载清单：探测原生模块 → 写清单 → 注入侧按清单加载。
    match super::preload::write_manifest_by_dir(ctx, name, dir) {
        Ok(summary) => {
            if summary.entry_count > 0 {
                log::info!(
                    "[home/launch] 版本 `{name}` 预加载清单：{} 项（加载器 {:?}）",
                    summary.entry_count,
                    summary.loader_name
                );
            }
        }
        Err(error) => log::warn!("[home/preload] 生成清单失败（不影响启动）: {error}"),
    }

    // 控制台子系统（只改 PE 的一个字段，随时可逆）。
    #[cfg(windows)]
    if let Err(error) = super::inject::apply_console(exe, meta.enable_console) {
        log::warn!("[home/launch] 应用控制台设置失败（不影响启动）: {error}");
    }
    Ok(())
}

/// 交接给注入侧的环境变量。
///
/// **数据根目录由启动器显式下发**（`COPPER_HOOK_DATA_DIR`）：注入侧不再自己
/// 推导目录规则，两侧共用 `isolate` 的同一份结论，避免 LeviLauncher 那种
/// 「Go 侧与 C++ 侧对 1.26+ 判断相反」的分叉。
fn apply_hook_env(cmd: &mut Command, dir: &std::path::Path, meta: &VersionMeta) {
    use copper_core_hook::contract as contract;
    let data_dir = super::isolate::data_dir(dir, meta);
    cmd.env(contract::ENV_DATA_DIR, &data_dir);
    cmd.env(contract::ENV_ROOT, dir);
    cmd.env(
        contract::ENV_CHANNEL,
        if super::isolate::is_preview(meta) {
            "preview"
        } else {
            "release"
        },
    );
    cmd.env(
        contract::ENV_PRELOAD,
        super::inject::manifest_path(dir),
    );
}

/// 启动后台监控（注入已完成后调用）。
fn monitor_after_launch(ctx: &LaunchCtx, name: &str, dir: PathBuf, launch_pid: u32) {
    let settings = ctx.settings.clone();
    let hwnd = ctx.hwnd;
    let on_launched: Option<Box<dyn FnOnce() + Send>> = {
        let settings = settings.clone();
        let action = window::AfterLaunch::parse(
            &settings
                .get_or::<String>("launch.after_launch".to_string(), "keep".to_string()),
        );
        Some(Box::new(move || {
            if let Err(error) = window::apply_launcher_action(hwnd, action) {
                log::warn!("[home/launch] 应用启动后窗口行为失败: {error}");
            }
        }))
    };
    let on_finished: Box<dyn FnOnce(monitor::ExitReason) + Send> = Box::new(move |reason| {
        let action = window::AfterGameExit::parse(
            &settings
                .get_or::<String>("launch.after_game_exit".to_string(), "keep".to_string()),
        );
        if let Err(error) = window::apply_exit_action(hwnd, action) {
            log::warn!("[home/launch] 应用游戏退出后窗口行为失败: {error}");
        }
        log::info!("[home/monitor] 会话结束：{reason:?}");
    });
    super::monitor::spawn_after_launch(
        &ctx.events,
        &ctx.runtime,
        name,
        dir,
        launch_pid,
        on_launched,
        on_finished,
    );
}

#[cfg(target_os = "android")]
fn launch_android(ctx: &LaunchCtx, name: &str, dir: &std::path::Path, android: &AndroidVersionMeta) -> Result<LaunchOutcome, KernelError> {
    let base = crate::modules::game_download::apk::base_apk_path(dir);
    if !base.is_file() {
        return Err(KernelError::InvalidArgument(format!(
            "实例 `{name}` 缺少 {}",
            crate::modules::game_download::apk::BASE_APK
        )));
    }
    crate::platform::android::request_prepare(&ctx.events, name, &android.package_name, android.version_name.as_str())?;
    // 安卓没有进程可轮询：Java 宿主收到 prepare 请求即接管启动，
    // 因此这里直接广播 `game.launched`。退出由宿主的文件信箱回传
    // （`useAndroidGameExit`），不经过本模块的监控任务。
    ctx.events
        .publish("game.launched", serde_json::json!({ "name": name }));
    Ok(LaunchOutcome::Spawned)
}

/// 纠正版本元数据：标记为「未注册」。
///
/// 只在协议唤起失败后调用（此时已证明该版本不受系统协议接管）。
/// 写失败只记日志 —— 启动路径不能因为元数据纠正失败而中断。
fn demote_registered(dir: &std::path::Path) {
    let Some(mut meta) = VersionMeta::read(dir) else {
        return;
    };
    if !meta.registered {
        return;
    }
    meta.registered = false;
    if let Err(error) = VersionMeta::write(dir, &meta) {
        log::warn!("[home/launch] 纠正 registered 标记失败: {error}");
    }
}

/// 判断该版本目录**当前是否真的被注册进系统**（存在 `InstallLocation` 为该目录的 AppX 包）。
///
/// 元数据里的 `registered` 是历史事实，会骗人：本启动器自己安装的版本全部只是解包，
/// 从未注册；但用户从 LeviLauncher 迁过来的版本目录里可能写着 `registered: true`，
/// 而系统上注册的是另一个包。此时唤起 `minecraft://` 起来的是**别人的游戏**，
/// 比启动失败更难排查。因此协议唤起前必须向系统核实一次。
#[cfg(windows)]
fn is_version_registered(dir: &std::path::Path) -> bool {
    let path = dir.to_string_lossy().to_string();
    // 脚本以单引号包裹字面量：出现引号 / 反引号即无法安全内联，直接判定未注册。
    if path.contains('\'') || path.contains('`') || path.contains('"') {
        return false;
    }
    let script = format!(
        "$p = Get-AppxPackage | Where-Object {{ $_.InstallLocation -eq '{path}' }} \
         | Select-Object -First 1 -ExpandProperty InstallLocation; \
         if ($p) {{ 'yes' }} else {{ 'no' }}"
    );
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).trim() == "yes"
        })
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn is_version_registered(_dir: &std::path::Path) -> bool {
    false
}

/// 通过协议 URL 唤起游戏（`ShellExecuteW`，系统 URL 激活的标准入口）。
///
/// 此前用 `cmd /c start`：它只报告 cmd 自身启动成功，协议没有处理程序时
/// 照样返回成功，用户看到的是 Windows 的「服务器启动失败」弹窗而后端毫不知情。
/// `ShellExecuteW` 的返回值 ≤32 即激活失败，调用方据此回落直接启动。
#[cfg(windows)]
fn spawn_protocol(url: &str) -> Result<(), KernelError> {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let verb = HSTRING::from("open");
    let wide = HSTRING::from(url);
    let result = unsafe {
        ShellExecuteW(
            None,
            windows::core::PCWSTR(verb.as_ptr()),
            windows::core::PCWSTR(wide.as_ptr()),
            windows::core::PCWSTR::null(),
            windows::core::PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute 的惯例：>32 成功，≤32 为错误码。
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(KernelError::InvalidArgument(format!(
            "协议 `{url}` 唤起失败（ShellExecute 返回 {}）",
            result.0 as isize
        )))
    }
}

#[cfg(not(windows))]
fn spawn_protocol(url: &str) -> Result<(), KernelError> {
    let _ = url;
    Err(KernelError::InvalidArgument("协议启动仅支持 Windows".into()))
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

    /// 没有匹配进程时「结束游戏」是成功的空操作（幂等），不是错误。
    #[cfg(windows)]
    #[test]
    fn terminate_missing_process_is_noop() {
        let missing = std::env::temp_dir().join("copper_no_such_game_dir_9d3f/Minecraft.Windows.exe");
        assert_eq!(terminate_process_at_path(&missing).unwrap(), 0);
        assert!(!is_process_running_at_path(&missing));
    }

    /// 未注册的临时目录不得被判定为已注册（否则会去唤起别人的游戏）。
    #[cfg(windows)]
    #[test]
    fn temp_dir_is_not_registered() {
        assert!(!is_version_registered(&std::env::temp_dir()));
    }

    /// 纠正 `registered` 幂等，且只动这一个字段。
    #[test]
    fn demote_registered_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("copper_demote_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let meta = VersionMeta {
            name: "demo".into(),
            registered: true,
            ..Default::default()
        };
        VersionMeta::write(&dir, &meta).unwrap();

        demote_registered(&dir);
        let after = VersionMeta::read(&dir).unwrap();
        assert!(!after.registered);
        assert_eq!(after.name, "demo");

        // 再调一次不报错，也不改动其它字段
        demote_registered(&dir);
        assert!(!VersionMeta::read(&dir).unwrap().registered);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同一实例的监控任务必须去重：否则一次启动会广播多对
    /// `game.launched` / `game.exited`，前端按钮在一次启动里被复位两次。
    #[test]
    fn monitor_registration_dedupes_per_instance() {
        let a = std::path::PathBuf::from("D:/v/A");
        let b = std::path::PathBuf::from("D:/v/B");
        assert!(register_monitor(&a));
        // 同实例重复登记被拒；不同实例互不影响
        assert!(!register_monitor(&a));
        assert!(register_monitor(&b));

        unregister_monitor(&a);
        assert!(register_monitor(&a));

        // 清理，避免影响其它测试
        unregister_monitor(&a);
        unregister_monitor(&b);
        assert!(register_monitor(&a));
        unregister_monitor(&a);
    }

    /// 编辑器门槛以下的实例不得硬塞 `-Editor`（那只会让游戏启动失败）。
    #[test]
    fn editor_switch_is_ignored_below_threshold() {
        assert!(!super::super::isolate::supports_editor_mode(
            "1.21.40.10",
            "release"
        ));
        assert!(super::super::isolate::supports_editor_mode(
            "1.21.50.0",
            "release"
        ));
    }
}
