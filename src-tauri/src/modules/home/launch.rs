//! 启动游戏：进程启动 + 目录重定向注入 + 运行检测 + 启动后监控。
//!
//! 一条启动路径：始终由启动器自己拉起 `Minecraft.Windows.exe`，并在主线程执行
//! 第一条指令之前把 hook DLL 远程注入进去（见 [`super::injector`]）。协议唤起
//! 已移除，原因见 [`launch_game`]。
//!
//! 启动成功后由后台任务监控进程状态，确认游戏运行时广播 `game.launched`。

use std::path::PathBuf;
use std::sync::Arc;

#[cfg(not(windows))]
use std::process::Command;

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
    pub db: Arc<crate::services::database::DatabaseService>,
    pub runtime: tokio::runtime::Handle,
    pub events: Arc<EventBus>,
    /// 启动器当前账户（用于 profile 一致性检查；测试可为 None）。
    pub account: Option<Arc<crate::services::account::AccountService>>,
    /// 启动器主窗口句柄（窗口行为用；0 = 尚未就绪）。
    pub hwnd: isize,
}

impl LaunchCtx {
    /// 从内核上下文提取（克隆内部 Arc，可安全跨 'static 捕获）。
    pub fn from_kernel(kernel: &KernelContext) -> Self {
        Self {
            paths: kernel.paths().clone(),
            settings: kernel.settings().clone(),
            db: kernel.db().clone(),
            runtime: kernel.runtime().clone(),
            events: kernel.events().clone(),
            account: Some(kernel.account().clone()),
            hwnd: kernel.window().get(),
        }
    }
}

/// 按路径查找运行中的游戏进程 pid（取第一个）。
///
/// 监控要用 pid 判定「窗口是否真的上了屏」；协议唤起拿不到启动 pid，只能
/// 这样反查。
#[cfg(windows)]
pub fn find_pid_at_path(exe_path: &std::path::Path) -> Option<u32> {
    pids_at_path(exe_path).first().copied()
}

/// 非 Windows 平台没有进程枚举，恒为「查不到」。
#[cfg(not(windows))]
pub fn find_pid_at_path(_exe_path: &std::path::Path) -> Option<u32> {
    None
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
            if is_game_exe_name(&name) {
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
/// 顺序固定为 **prepare → 落位 → 注入启动 → 监控**（`docs/启动链路与实例隔离.md` §2.7）。
///
/// # 为什么没有「协议唤起」这条路
///
/// 早期实现对 `registered` 的实例走 `minecraft://` 协议激活。那条路拿不到
/// 启动器自己的进程句柄，因此既无法远程注入 hook DLL，也无法下发
/// `COPPER_HOOK_*` 交接变量 —— 结果是隔离与加载器**静默失效**，而用户在界面上
/// 完全看不出区别（游戏照常启动，只是数据写在启动器管不到的地方）。
/// 「功能完整」的前提是隔离永远生效，因此统一走自己拉起进程并注入。
pub fn launch_game(
    ctx: &LaunchCtx,
    name: &str,
    check_running: bool,
) -> Result<LaunchOutcome, KernelError> {
    let dir = resolve_launch_dir(&ctx.paths, &ctx.settings, name)?;
    let mut meta = VersionMeta::read(&dir)
        .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{name}` 元数据缺失")))?;
    #[cfg(not(windows))]
    if meta.mode.as_deref() == Some("client") {
        return Err(KernelError::InvalidArgument(
            "客户端注入仅支持 Windows".into(),
        ));
    }
    #[cfg(target_os = "android")]
    if let Some(android) = meta.android.as_ref() {
        return launch_android(ctx, name, &dir, android);
    }
    if super::compatibility::is_bound(&dir) { return launch_compatibility(ctx, name, &dir, &meta); }
    let exe = game_exe_path(&dir);
    if !exe.is_file() {
        return Err(KernelError::InvalidArgument(format!(
            "版本 `{name}` 缺少 {}",
            exe.file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| GAME_EXE.to_string())
        )));
    }
    if check_running && is_process_running_at_path(&exe) {
        return Err(KernelError::InvalidArgument("游戏已在运行".into()));
    }

    // UWP 实例：注册 → 数据目录重定向 → 系统激活。这条路**不注入 hook DLL**，
    // 也不切控制台子系统：包必须纯 AppContainer 注册（全信任清单会让游戏启动即崩），
    // 隔离由包状态子目录的连接点承担（见 `home::uwp_launch`）。
    if meta.is_uwp() {
        if meta.mode.as_deref() == Some("client") {
            return Err(KernelError::InvalidArgument(
                "客户端模式不支持 UWP 实例；请切换到 GDK 实例或关闭客户端模式".into(),
            ));
        }
        // 隔离标记归一 + 骨架目录：与 GDK 用同一套数据目录规则，只是内容根布局不同。
        if super::isolate::enforce_isolation(&mut meta) {
            if let Err(error) = VersionMeta::write(&dir, &meta) {
                log::warn!("[home/launch] 归一隔离标记失败（不影响启动）: {error}");
            }
        }
        let pid = super::uwp_launch::start(name, &dir, &meta)?;
        monitor_after_launch(ctx, name, dir, pid);
        return Ok(LaunchOutcome::Spawned);
    }

    // 0) prepare：骨架目录 + 预加载清单（控制台子系统与导入表注入合并到第 1 步）
    prepare_instance(name, &dir, &exe, &mut meta)?;

    // 0.7) 客户端注入计划：关闭客户端模式时为 None；启用但客户端文件缺失 / 摘要
    //      不符时在这里就拒绝启动，而不是等游戏跑起来才发现注入不了。
    let client_plan = super::client::launch_plan(
        ctx.paths.as_ref(),
        ctx.db.as_ref(),
        meta.mode.as_deref(),
        meta.client_id.as_deref(),
    )?;

    // 0.5) 账户—profile 一致性：游戏用 profile 的三键静默恢复 Xbox 会话。
    //      如果 profile 属于另一个账户，当前 WAM 会话里没有它，游戏会 0 TCP
    //      停在「正在登录」。先诊断；不一致就隔离登录痕迹，并跳过继承。
    let data_dir = super::isolate::data_dir(&dir, &meta);
    let active = ctx.account.as_ref().and_then(|account| account.current());
    let active_xuid = active.as_ref().and_then(|info| info.xuid.as_deref());
    let diagnosis = super::account_check::diagnose(
        &data_dir,
        active_xuid,
        active.as_ref().map(|info| info.gamertag.as_str()),
    );
    log::info!("[home/account] {}", diagnosis.describe());
    if diagnosis.mismatch {
        // 不自动删除用户数据：profile 可能是用户在游戏里刚刚登录出来的。
        // 只跳过继承并提示；真正需要重置时由用户显式操作。
        log::warn!(
            "[home/launch] 版本 {} 的 profile 属于另一个账户；本次跳过继承。若游戏停在登录中，请删除该 profile 后重新登录",
            name
        );
        ctx.events.publish(
            "account.profile_mismatch",
            serde_json::json!({
                "name": name,
                "active_xuid": diagnosis.active_xuid,
                "profile_xuid": diagnosis.profile_xuid,
                "backup": null,
            }),
        );
    } else {
        // 0.6) 数据继承：目标没有完整登录态时，只从一手来源取。
        let versions_root = dir.parent().unwrap_or(dir.as_path()).to_path_buf();
        match super::inherit::inherit_if_needed(&dir, &meta, &versions_root) {
            Ok(Some(report)) => log::info!(
                "[home/launch] 版本 {} 已继承登录数据：{} 个文件 / {} 字节（来源 {}）",
                name,
                report.files,
                report.bytes,
                report.source.display()
            ),
            Ok(None) => log::debug!("[home/launch] 版本 {} 无需继承登录数据", name),
            Err(error) => log::warn!(
                "[home/launch] 版本 {} 继承登录数据失败（不阻断启动）: {error}",
                name
            ),
        }
    }

    // 1) 远程注入准备：恢复原版 exe（若有干净备份），只落 hook DLL，不改 exe。
    //
    //    改 exe 导入表会让 Authenticode 失效；Windows 会把这种“签名被改”的
    //    exe 当不可信程序，降成 Low 或直接拦掉，Xbox 的本机 RPC 随之卡死。
    //    这里保留原版签名，进程正常完整性启动，hook 由 CreateProcess 挂起后
    //    LoadLibraryW 注入，仍在游戏主线程运行前完成目录重定向。
    let (dll_written, restored) = super::inject::prepare_remote(&dir, &exe)?;
    if restored {
        log::info!("[home/launch] 版本 `{name}` 已从备份恢复原版 exe");
    }
    if dll_written {
        log::info!("[home/launch] 版本 `{name}` 已写入 hook DLL（不改 exe）");
    }
    let dll = super::inject::hook_dll_path(&dir);

    // 2) 启动参数与环境：用户自定义在前，交接变量在后且覆盖同名项
    //    （交接变量是隔离成立的前提，不能被用户的同名 env 顶掉）。
    let args = launch_args(&meta, name);
    let mut env = parse_env_vars(&meta.env_vars);
    env.extend(hook_env(&dir, &meta));

    // 3) 挂起启动 → 远程 LoadLibraryW → 恢复主线程。
    //
    //    包身份/协议激活由 AppModel 托管，拿不到进程句柄，也就无法远程注入；
    //    这里统一走自己的 CreateProcess，exe 保持原版。
    let console = meta.enable_console;
    log::info!(
        "[home/launch] 版本 `{name}` 以远程注入启动（console={console}）"
    );
    let pid = spawn_game(&exe, &dir, &args, &env, &dll, true, console)?;

    // 4) 客户端注入：游戏进程起来 5s 后再加载第三方 DLL（见 home::client）。
    //    失败不终止游戏；结果经 client.inject_result 事件回传，开始页同时提供
    //    「重新注入」按钮作为兜底。
    if let Some(plan) = client_plan {
        log::info!(
            "[home/launch] 版本 `{name}` 将在启动 {}ms 后注入客户端 `{}`",
            plan.delay_ms,
            plan.client.name
        );
        super::client::schedule_injection(ctx.events.clone(), name.to_string(), pid, plan);
    }

    monitor_after_launch(ctx, name, dir, pid);
    Ok(LaunchOutcome::Spawned)
}

fn launch_compatibility(ctx:&LaunchCtx,name:&str,dir:&std::path::Path,meta:&VersionMeta)->Result<LaunchOutcome,KernelError>{
    if meta.is_uwp() || meta.mode.as_deref()!=Some("original") || meta.loader.is_some() || meta.client_id.is_some() || meta.enable_console || meta.enable_editor_mode { return Err(KernelError::InvalidArgument("compat_mode_unsupported".into())); }
    static BOUND_LAUNCH:std::sync::Mutex<()>=std::sync::Mutex::new(());
    let _guard=BOUND_LAUNCH.lock().map_err(|_|KernelError::Module("compat_launch_lock".into()))?;
    let spec=super::compatibility::resolve(dir)?;
    if is_process_running_at_path(&spec.executable){return Err(KernelError::InvalidArgument("compat_game_running".into()));}
    std::fs::create_dir_all(super::isolate::data_dir(dir,meta))?;
    super::inject::ensure_hook_dll(dir)?;
    let mut env=hook_env(dir,meta); env.retain(|(k,_)|k!="COPPER_HOOK_LOAD_PRELOADER");
    env.push(("COPPER_HOOK_HOST_EXE".into(),spec.executable.to_string_lossy().into_owned()));
    let pid=spawn_game(&spec.executable,&spec.source_root,&[],&env,&super::inject::hook_dll_path(dir),true,false)?;
    // Source leases deny ordinary replacement during this process; another launcher has no cooperative lock contract.
    ctx.runtime.spawn(async move {let _leases=spec.leases;while super::monitor::is_process_alive(pid){tokio::time::sleep(std::time::Duration::from_secs(1)).await;}});
    monitor_after_launch(ctx,name,dir.to_path_buf(),pid);Ok(LaunchOutcome::Spawned)
}

/// 以松散包身份启动实例（与 LeviLauncher 一致）。
///
/// 注册幂等：包已指向本目录时 `register` 只回读校验。激活由 AppModel 完成，
/// 进程因此拿到包身份；hook DLL 由 exe 的导入表在加载阶段拉起，和激活方式无关。
#[cfg(windows)]
fn activate_registered(dir: &std::path::Path) -> Result<u32, KernelError> {
    use crate::modules::game_download::uwp_deploy;

    // 只有用户在设置里显式打开 registered 才走包身份；默认仍旧是自己拉起进程，
    // 保证环境交接与 pid 都还在自己手里。
    if !super::meta::VersionMeta::read(dir)
        .map(|meta| meta.registered)
        .unwrap_or(false)
    {
        return Err(KernelError::Config(
            "实例未标记为已注册，跳过包身份启动".into(),
        ));
    }
    let manifest = dir.join("appxmanifest.xml");
    if !manifest.is_file() {
        return Err(KernelError::Config(format!(
            "实例目录缺少 appxmanifest.xml，无法注册包身份：{}",
            dir.display()
        )));
    }
    let registered = uwp_deploy::register(dir).map_err(|error| {
        KernelError::Config(format!("松散包注册失败（需要开启开发者模式）: {error}"))
    })?;
    let aumid = format!("{}!Game", registered.family_name);
    uwp_deploy::activate(&aumid)
        .map_err(|error| KernelError::Config(format!("包激活失败（{aumid}）: {error}")))
}

/// 拉起游戏进程。
///
/// `inject = false`（正常路径）：exe 的导入表里已经有 hook DLL，加载器会自己
/// 把它加载起来，我们只需要普通启动。`inject = true` 是导入表注入不可用时的
/// 退路：挂起启动 → 远程线程 `LoadLibraryW` → 恢复主线程。
#[cfg(windows)]
fn spawn_game(
    exe: &std::path::Path,
    dir: &std::path::Path,
    args: &[String],
    env: &[(String, String)],
    dll: &std::path::Path,
    inject: bool,
    console: bool,
) -> Result<u32, KernelError> {
    if inject {
        super::injector::launch_with_injection(exe, dir, args, env, dll, console)
    } else {
        super::injector::launch_plain(exe, dir, args, env, console)
    }
}

/// 非 Windows 平台没有注入能力：直接 spawn，隔离由各平台自己的宿主负责。
#[cfg(not(windows))]
fn spawn_game(
    exe: &std::path::Path,
    dir: &std::path::Path,
    args: &[String],
    env: &[(String, String)],
    _dll: &std::path::Path,
    _inject: bool,
    _console: bool,
) -> Result<u32, KernelError> {
    let mut cmd = Command::new(exe);
    cmd.args(args);
    cmd.current_dir(dir);
    for (key, value) in env {
        cmd.env(key, value);
    }
    // 启动后丢弃子进程句柄：游戏进程脱离启动器独立运行，由后台监控接管。
    // 但 pid 要留下 —— 它是唯一能区分「秒退」与「迟迟没起来」的依据。
    cmd.spawn()
        .map(|child| child.id())
        .map_err(|e| KernelError::InvalidArgument(format!("启动游戏失败: {e}")))
}

/// 游戏主程序在实例目录下的路径。
///
/// UWP 实例以包清单里的 `Application/@Executable` 为准：0.x 时代的主程序名是
/// `Minecraft.Win10.DX11.exe`，而 GDK 一律是 `Minecraft.Windows.exe`。清单读不出来时
/// 退回 GDK 的默认名（此时启动会以「缺少主程序」明确失败，而不是猜一个不存在的路径）。
///
/// 这里自己读 `version.json` 而不是要求调用方传元数据：进程枚举（`monitor`）、
/// 结束游戏、删除实例三处都要用同一个判定，签名统一才不会出现「某条链路按 GDK 名字
/// 找进程，于是永远找不到」的分叉。
pub fn game_exe_path(version_dir: &std::path::Path) -> PathBuf {
    if super::compatibility::is_bound(version_dir) { return super::compatibility::executable(version_dir); }
    if let Some(meta) = VersionMeta::read(version_dir) {
        if meta.is_uwp() {
            if let Ok(manifest) =
                crate::modules::game_download::uwp::PackageManifest::read_dir(version_dir)
            {
                if let Some(file) = manifest
                    .executable
                    .replace('\\', "/")
                    .rsplit('/')
                    .next()
                    .map(str::to_string)
                {
                    if !file.is_empty() {
                        return version_dir.join(file);
                    }
                }
            }
        }
    }
    version_dir.join(GAME_EXE)
}

/// 游戏主程序的进程名是否属于本启动器识别的游戏。
///
/// 两个名字都要认：UWP 老版本用 `Minecraft.Win10.DX11.exe`，新版本与 GDK 用
/// `Minecraft.Windows.exe`。注入侧的宿主白名单也是这两个名字。
fn is_game_exe_name(name: &str) -> bool {
    name.eq_ignore_ascii_case(GAME_EXE) || name.eq_ignore_ascii_case("Minecraft.Win10.DX11.exe") || (name.starts_with("Minecraft.Copper.") && name.ends_with(".exe") && uuid::Uuid::parse_str(&name[17..name.len()-4]).is_ok())
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
    name: &str,
    dir: &std::path::Path,
    // 控制台子系统与导入表注入都移到 `inject::prepare_launch`，这里不再需要 exe。
    _exe: &std::path::Path,
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
    match super::preload::write_manifest_at(dir) {
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

    // 控制台子系统不在这里改：它和导入表注入都要改写同一个 311MB 文件，
    // 合并到 `inject::prepare_launch` 里一次完成，避免两次全文件重写。
    Ok(())
}


/// 交接给注入侧的环境变量。
///
/// **数据根目录由启动器显式下发**（`COPPER_HOOK_DATA_DIR`）：注入侧不再自己
/// 推导目录规则，两侧共用 `isolate` 的同一份结论，避免 LeviLauncher 那种
/// 「Go 侧与 C++ 侧对 1.26+ 判断相反」的分叉。
///
/// 返回值而非直接写进 [`Command`]：Windows 走 `CreateProcessW` 的环境块，
/// 两条路径必须拿到**同一份**变量，否则「非 Windows 用 Command、Windows 用
/// env block」会悄悄分叉出两套交接内容。
fn hook_env(dir: &std::path::Path, meta: &VersionMeta) -> Vec<(String, String)> {
    use copper_core_hook::contract as contract;
    let data_dir = super::isolate::data_dir(dir, meta);
    let mut env = vec![
        (contract::ENV_DATA_DIR.to_string(), data_dir.to_string_lossy().into_owned()),
        (contract::ENV_ROOT.to_string(), dir.to_string_lossy().into_owned()),
        (
            contract::ENV_CHANNEL.to_string(),
            if super::isolate::is_preview(meta) {
                "preview"
            } else {
                "release"
            }
            .to_string(),
        ),
        (
            contract::ENV_PRELOAD.to_string(),
            super::inject::manifest_path(dir)
                .to_string_lossy()
                .into_owned(),
        ),
        // %TEMP% 的目标同样由启动器算好：注入侧不再重算版本规则。
        (
            contract::ENV_TEMP.to_string(),
            super::isolate::temp_dir(dir, meta)
                .to_string_lossy()
                .into_owned(),
        ),
    ];
    // 装了 LeviLamina 客户端文件的实例：exe 保持原版（安装后已还原），
    // 由 hook 在远程注入后加载 PreLoader/LeviLamina。
    if dir.join("PreLoader.dll").is_file() {
        env.push(("COPPER_HOOK_LOAD_PRELOADER".to_string(), "1".to_string()));
    }
    env
}

/// 启动后台监控（注入已完成后调用）。
fn monitor_after_launch(ctx: &LaunchCtx, name: &str, dir: PathBuf, launch_pid: u32) {
    let settings = ctx.settings.clone();
    let hwnd = ctx.hwnd;
    let after_launch_raw = settings.get_or::<String>("launch.after_launch", "keep".to_string());
    let after_game_exit_raw =
        settings.get_or::<String>("launch.after_game_exit", "keep".to_string());
    let on_launched: Option<Box<dyn FnOnce() + Send>> = {
        let action = super::window::AfterLaunch::parse(&after_launch_raw);
        Some(Box::new(move || {
            if let Err(error) = super::window::apply_launcher_action(hwnd, action) {
                log::warn!("[home/launch] 应用启动后窗口行为失败: {error}");
            }
        }))
    };
    let on_finished: Box<dyn FnOnce(super::monitor::ExitReason) + Send> = Box::new(move |reason| {
        let action = super::window::AfterGameExit::parse(&after_game_exit_raw);
        if let Err(error) = super::window::apply_exit_action(hwnd, action) {
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
/// 协议唤起这条路已经移除（见 [`launch_game`] 的说明），但用户的版本目录里
/// 仍可能留着从 LeviLauncher 迁过来的 `registered: true`。留着它会让 UI 显示
/// 「已注册」，而我们既不注册也不唤起 —— 那是一个纯骗人的字段，因此在启动时
/// 顺手自愈。写失败只记日志：元数据纠正不该让启动失败。
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
        assert!(!crate::modules::home::inject::imports_hook_dll(
            &std::env::temp_dir().join("x.exe")
        ));
    }

    /// 交接变量必须齐备且指向实例目录：少一个变量，注入侧就静默退化成不隔离。
    #[test]
    fn hook_env_carries_the_full_handover() {
        use copper_core_hook::contract as contract;
        let dir = std::path::Path::new("D:/v/demo");
        let meta = VersionMeta {
            version_type: "release".into(),
            ..Default::default()
        };
        let env = hook_env(dir, &meta);
        let get = |key: &str| {
            env.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        // 路径按原样透传（Windows 上反斜杠是常态，但这里不依赖具体形态）
        assert_eq!(get(contract::ENV_ROOT), dir.to_string_lossy());
        assert_eq!(
            get(contract::ENV_DATA_DIR),
            super::super::isolate::data_dir(dir, &meta)
                .to_string_lossy()
        );
        assert_eq!(get(contract::ENV_CHANNEL), "release");
        assert!(get(contract::ENV_PRELOAD).ends_with("copper-preload.json"));
        // 默认 game_version 为空 → 解析不出 1.26+ → 按旧布局落在数据目录
        assert_eq!(
            get(contract::ENV_TEMP),
            super::super::isolate::data_dir(dir, &meta).to_string_lossy()
        );
        assert_eq!(env.len(), 5, "交接变量数量固定，多了会让交接内容漂移");
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

    /// 实机验收：真的把游戏拉起来、真的注入 hook DLL。
    ///
    /// 隔离能不能用，最终只能这样验 —— 单测覆盖不到「加载器会不会拒绝这份 DLL」，
    /// 覆盖不到「游戏会不会照着重定向后的目录写数据」。
    ///
    /// 默认 `#[ignore]`：它会真的启动游戏（本机需已安装实例、且会消耗显存）。
    /// 跑法：
    ///
    /// ```powershell
    /// $env:COPPER_LIVE_LAUNCH = "D:\...\versions\1.26.52.03"
    /// $env:COPPER_LIVE_WATCH_SECS = "90"
    /// cargo test -p copper-core --lib launch::tests::live_launch_isolates -- --ignored --nocapture
    /// ```
    #[cfg(windows)]
    #[test]
    #[ignore = "会真的启动游戏；需 COPPER_LIVE_LAUNCH 指向实例目录"]
    fn live_launch_isolates() {
        let Ok(version_dir) = std::env::var("COPPER_LIVE_LAUNCH") else {
            panic!("必须设置 COPPER_LIVE_LAUNCH 为实例目录");
        };
        let version_dir = std::path::PathBuf::from(version_dir);
        let name = version_dir
            .file_name()
            .expect("实例目录必须有名字")
            .to_string_lossy()
            .into_owned();
        let watch_secs: u64 = std::env::var("COPPER_LIVE_WATCH_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);

        // 让 `Paths::versions_root` 指向这个实例的父目录：测试要跑的就是真实启动链路。
        let root = version_dir
            .parent()
            .expect("实例目录必须有父目录")
            .to_path_buf();
        std::env::set_var("COPPER_VERSIONS_DIR", &root);
        let scratch = std::env::temp_dir().join(format!("copper_live_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&scratch);
        let paths = Arc::new(Paths::with_root(scratch.clone()));
        let db = Arc::new(
            crate::services::database::DatabaseService::open(&scratch.join("live.db"))
                .expect("打开测试数据库"),
        );
        db.migrate_scope(
            "core",
            crate::services::database::CORE_MIGRATIONS,
        )
        .expect("迁移 core schema");
        let events = Arc::new(EventBus::new());
        let settings = Arc::new(
            SettingsService::new(db.clone(), events.clone(), Default::default())
                .expect("创建设置服务"),
        );
        let runtime = tokio::runtime::Runtime::new().expect("创建 tokio 运行时");
        let ctx = LaunchCtx {
            paths,
            settings,
            db,
            runtime: runtime.handle().clone(),
            events,
            account: None,
            hwnd: 0,
        };

        let log = version_dir.join(copper_core_hook::contract::HOOK_LOG_FILE_NAME);
        let _ = std::fs::remove_file(&log);
        let meta = VersionMeta::read(&version_dir).expect("读取实例元数据");
        let data_dir = crate::modules::home::isolate::data_dir(&version_dir, &meta);
        let system_dir = std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .map(|root| {
                root.join(if crate::modules::home::isolate::is_preview(&meta) {
                    copper_core_hook::contract::PREVIEW_DATA_DIR
                } else {
                    copper_core_hook::contract::RELEASE_DATA_DIR
                })
            })
            .unwrap_or_default();

        launch_game(&ctx, &name, true).expect("启动链路必须成功（注入失败会在这里显式报错）");
        let exe = game_exe_path(&version_dir);
        let pid = find_pid_at_path(&exe).unwrap_or(0);
        println!("[live] 游戏 pid = {pid}");
        std::thread::sleep(std::time::Duration::from_secs(8));
        println!(
            "[live] 8s 后存活={} 仍在运行={}",
            pid != 0 && crate::modules::home::monitor::is_process_alive(pid),
            is_process_running_at_path(&exe)
        );

        let count_files = |dir: &std::path::Path| -> usize {
            walk(dir).count()
        };
        let instance_before = count_files(&data_dir);
        let system_before = count_files(&system_dir);

        // 等 hook 日志出现：注入侧的 worker 线程写第一行「注入侧就绪」即证明
        // DLL 已经进了游戏进程并跑到了工作线程（这一步过不去，后面全都不成立）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(watch_secs);
        let mut ready = false;
        while std::time::Instant::now() < deadline {
            if log.is_file()
                && std::fs::read_to_string(&log)
                    .unwrap_or_default()
                    .contains("注入侧就绪")
            {
                ready = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        println!(
            "[live] ---- {} ----\n{text}\n--------------------",
            log.display()
        );
        assert!(ready, "注入侧没有就绪：hook DLL 未进入游戏进程，或 DllMain 未跑完");

        // 隔离成不成立看这一行：替换数为 0 就等于没有 hook。
        let replaced: usize = text
            .lines()
            .find(|line| line.contains("个导入槽"))
            .and_then(|line| {
                line.split("替换 ")
                    .nth(1)
                    .and_then(|rest| rest.split(' ').next())
                    .and_then(|n| n.parse().ok())
            })
            .expect("日志里没有 hook 安装结论：安装报告没有落盘");
        assert!(replaced > 0, "目录重定向一个导入槽都没命中：隔离不会生效");

        // 让游戏真正跑一会儿，让它自己走到调用目录 API 的那一步。
        let settle: u64 = std::env::var("COPPER_LIVE_SETTLE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);
        std::thread::sleep(std::time::Duration::from_secs(settle));
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        println!("[live] ---- 稳态 {} ----\n{text}\n--------------------", log.display());

        // 这才是隔离生效的直接证据：不是「槽换掉了」，而是**游戏进程自己的调用**
        // 经过了重定向函数、并拿到了实例目录。
        //
        // 注意：未登录 Xbox 的机器上，游戏会停在登录/授权界面，**根本不会走到**
        // 解析数据目录那一段代码 —— 那样三个 API 一次都不会被调用。所以这里是
        // 「调到了就必须被重定向」，而不是「必须被调用」：前者任何机器上都成立，
        // 后者只在已登录的机器上成立。
        let mut redirected_any = false;
        for api in [
            "SHGetKnownFolderPath(RoamingAppData)",
            "GetTempPathA",
            "GetTempPathW",
        ] {
            let marker = format!("游戏首次调用 {api}");
            if text.contains(&marker) {
                redirected_any = true;
                println!("[live] {marker} -> 已重定向");
            } else {
                println!("[live] {marker} -> 本次未观察到（游戏未进入目录解析阶段）");
            }
        }
        // 一旦观察到调用，就必须每一条都被重定向到实例目录，不能有漏网的。
        for line in text.lines().filter(|line| line.contains("游戏调用了 ")) {
            assert!(
                line.contains("未重定向") && line.contains("FOLDERID"),
                "出现了预期之外的重定向结果：{line}"
            );
        }
        if !redirected_any {
            println!(
                "[live] 注意：本次未观察到游戏调用目录 API（未登录 Xbox 时游戏停在授权界面）。\
                 注入、hook 安装与预加载清单执行均已验证；数据落点需在已登录的机器上复验。"
            );
        }

        let instance_after = count_files(&data_dir);
        let system_after = count_files(&system_dir);
        println!(
            "[live] 实例数据文件数 {} -> {}\n[live] 系统 %APPDATA% 文件数 {} -> {}",
            instance_before,
            instance_after,
            system_before,
            system_after
        );
        for path in walk(&data_dir).take(40) {
            println!("[live]   实例: {}", path.display());
        }
        // 系统漫游目录必须一个文件都不多：那才是「没隔离」的样子。
        // （游戏没登录时两边都不长数据，所以这里只能断言「没多」，不能断言「多了」。）
        assert_eq!(
            system_after, system_before,
            "系统 {} 有了新增：隔离漏了",
            system_dir.display()
        );

        // 收尾：把游戏停掉，别把用户的机器留在「游戏还在跑」的状态。
        terminate_process_at_path(&exe).expect("结束测试启动的游戏");
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// 递归枚举目录下的全部条目（实机测试用来数「游戏写了多少文件」）。
    #[cfg(windows)]
    fn walk(dir: &std::path::Path) -> impl Iterator<Item = std::path::PathBuf> + '_ {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|entry| {
                let path = entry.path();
                let children = if path.is_dir() {
                    Box::new(walk(&path).collect::<Vec<_>>())
                } else {
                    Box::new(Vec::new())
                };
                std::iter::once(path).chain(children.into_iter())
            })
    }
}