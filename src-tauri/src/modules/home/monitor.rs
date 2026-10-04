//! 启动后监控：确认启动成功、区分失败原因、观察生命周期。
//!
//! 从 `launch.rs` 拆出来的原因：这段逻辑有独立的生命周期（后台任务）与独立的
//! 状态（去重登记表），和「怎么把进程拉起来」是两件事，混在一个文件里两边都难读。
//!
//! # 判定链
//!
//! ```text
//! 进程出现 ──► 等窗口呈现 ──► game.launched ──► 观察直到进程消失 ──► game.exited{closed}
//!    │
//!    ├─ 启动进程先死 ────────► game.exited{crashed}   （崩溃 / 缺 DLL / 被杀软拦下）
//!    └─ 超时也没出现 ────────► game.exited{timeout}   （迟迟起不来）
//! ```
//!
//! 「秒退」与「超时」必须分开：前者是**立刻可行动**的（换个加载器、关掉杀软、
//! 补依赖），后者是「再等等」。混成一个「启动失败」时，用户只能反复重试。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use crate::registry::events::EventBus;

use super::window;

/// 启动确认的最长等待（秒）。
pub const CONFIRM_TIMEOUT_SECS: u64 = 120;
/// 启动确认阶段的轮询间隔（毫秒）。
pub const POLL_INTERVAL_MS: u64 = 400;
/// 确认启动后观察生命周期的轮询间隔（毫秒）。
pub const WATCH_INTERVAL_MS: u64 = 1500;
/// 进程出现后等窗口呈现的最长等待（秒）。
pub const WINDOW_TIMEOUT_SECS: u64 = 60;

/// 退出原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// 正常退出。
    Closed,
    /// 启动进程起来后立刻消失。
    Crashed,
    /// 超时未见游戏进程。
    Timeout,
}

impl ExitReason {
    /// 事件负载里的 `reason` 字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Crashed => "crashed",
            Self::Timeout => "timeout",
        }
    }
}

/// 在跑的监控任务（按实例目录去重）。
static ACTIVE_MONITORS: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// 登记监控任务；已有同名任务时返回 `false`（调用方应放弃本次监控）。
pub fn register_monitor(dir: &Path) -> bool {
    match ACTIVE_MONITORS.lock() {
        Ok(mut set) => set.insert(dir.to_path_buf()),
        // 锁中毒时宁可重复监控，也不要让界面卡在「启动中」。
        Err(poisoned) => poisoned.into_inner().insert(dir.to_path_buf()),
    }
}

/// 释放监控任务登记。
pub fn unregister_monitor(dir: &Path) {
    match ACTIVE_MONITORS.lock() {
        Ok(mut set) => {
            set.remove(dir);
        }
        Err(poisoned) => {
            poisoned.into_inner().remove(dir);
        }
    }
}

/// 进程是否存活（秒退判定的依据）。
#[cfg(windows)]
pub fn is_process_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    /// 进程未退出时 `GetExitCodeProcess` 返回该值。
    const STILL_ACTIVE: u32 = 259;
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            // 打不开（已退出 / 权限不足）按「已退出」处理：此时游戏进程也不存在，
            // 再等下去只是白等满超时。
            return false;
        };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code).is_ok();
        let _ = CloseHandle(handle);
        ok && code == STILL_ACTIVE
    }
}

/// 非 Windows 平台没有 pid 概念：返回 `true` 表示「不判定为秒退」，
/// 监控退回纯超时语义（与该平台上没有进程检测能力的事实一致）。
#[cfg(not(windows))]
pub fn is_process_alive(_pid: u32) -> bool {
    true
}

/// 启动监控的后台任务。
///
/// `launch_pid` 为直接启动时 `Minecraft.Windows.exe` 的 pid；协议唤起传 0
/// （协议由系统代为激活，拿不到游戏进程 pid，此路径下无法做秒退判定）。
pub fn spawn_after_launch(
    events: &Arc<EventBus>,
    runtime: &tokio::runtime::Handle,
    name: &str,
    dir: PathBuf,
    launch_pid: u32,
    on_launched: Option<Box<dyn FnOnce() + Send>>,
    on_finished: Box<dyn FnOnce(ExitReason) + Send>,
) {
    // 同一实例同一时刻只允许一个监控任务：强制启动（跳过运行中检查）能连续
    // 触发多次启动，重复监控会让事件成对广播多次，前端按钮在一次启动里被复位两次。
    if !register_monitor(&dir) {
        log::warn!("[home/monitor] 版本 `{name}` 已有监控任务在跑，本次启动不重复监控");
        return;
    }
    let events = events.clone();
    let name = name.to_string();
    runtime.spawn(async move {
        let exe = super::launch::game_exe_path(&dir);
        let reason = confirm_loop(&events, &name, &exe, launch_pid).await;
        if let Some(callback) = on_launched {
            callback();
        }
        if let Some(reason) = reason {
            // 启动未成功：没有生命周期可观察，直接收尾。
            publish_exited(&events, &name, reason);
            unregister_monitor(&dir);
            on_finished(reason);
            return;
        }
        watch_until_exit(&events, &name, &exe).await;
        publish_exited(&events, &name, ExitReason::Closed);
        unregister_monitor(&dir);
        on_finished(ExitReason::Closed);
    });
}

/// 确认阶段。返回 `Some(reason)` 表示启动失败。
///
/// 确认成功前**不**广播 `game.launched`：进程出现只是中间态，等窗口真的
/// 上了屏才算「用户看得见的启动成功」。
async fn confirm_loop(
    events: &EventBus,
    name: &str,
    exe: &Path,
    launch_pid: u32,
) -> Option<ExitReason> {
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(CONFIRM_TIMEOUT_SECS);
    loop {
        if super::launch::is_process_running_at_path(exe) {
            break;
        }
        // 启动进程已经死了但游戏没出现：秒退，立刻结束而不是等满两分钟。
        if launch_pid != 0 && !is_process_alive(launch_pid) {
            log::warn!("[home/monitor] 版本 `{name}` 的启动进程已退出且未见游戏进程");
            return Some(ExitReason::Crashed);
        }
        if std::time::Instant::now() >= deadline {
            return Some(ExitReason::Timeout);
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }

    // 进程在了，等窗口。游戏 pid 从路径反查（协议唤起拿不到 pid）。
    let game_pid = super::launch::find_pid_at_path(exe).unwrap_or(0);
    let window_deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(WINDOW_TIMEOUT_SECS);
    loop {
        if game_pid != 0 && window::game_window_presented(game_pid) {
            events.publish("game.launched", serde_json::json!({ "name": name }));
            return None;
        }
        if !super::launch::is_process_running_at_path(exe) {
            // 窗口还没出来进程就没了：仍然算失败（崩溃），而不是继续等。
            return Some(ExitReason::Crashed);
        }
        if std::time::Instant::now() >= window_deadline {
            // 进程活着但一直没出窗口：按「已启动」处理。
            //
            // 判成失败会有一个更难收拾的后果 —— 用户既没有游戏窗口，也没有启动器
            // 告诉他出了什么事（LeviLauncher 在这里也选择放行，
            // `internal/launch/launch.go:242`）。
            log::warn!("[home/monitor] 版本 `{name}` 的游戏窗口在 {}s 内未呈现，按已启动处理", WINDOW_TIMEOUT_SECS);
            events.publish("game.launched", serde_json::json!({ "name": name }));
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
}

/// 生命周期观察：直到进程消失。
async fn watch_until_exit(events: &EventBus, name: &str, exe: &Path) {
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(WATCH_INTERVAL_MS)).await;
        if !super::launch::is_process_running_at_path(exe) {
            return;
        }
    }
}

fn publish_exited(events: &EventBus, name: &str, reason: ExitReason) {
    events.publish(
        "game.exited",
        serde_json::json!({ "name": name, "reason": reason.as_str() }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_reasons_have_stable_wire_names() {
        // 这些字符串会进事件负载与前端文案，改名等于前后端语义漂移
        assert_eq!(ExitReason::Closed.as_str(), "closed");
        assert_eq!(ExitReason::Crashed.as_str(), "crashed");
        assert_eq!(ExitReason::Timeout.as_str(), "timeout");
    }

    #[test]
    fn monitor_registration_dedupes_per_instance() {
        let a = PathBuf::from("D:/v/A");
        let b = PathBuf::from("D:/v/B");
        assert!(register_monitor(&a));
        assert!(!register_monitor(&a));
        assert!(register_monitor(&b));
        unregister_monitor(&a);
        assert!(register_monitor(&a));
        unregister_monitor(&a);
        unregister_monitor(&b);
    }
}
