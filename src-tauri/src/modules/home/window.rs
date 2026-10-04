//! 窗口：游戏窗口「是否真的上了屏」的判定，以及启动器自身的窗口行为。
//!
//! # 为什么要判定窗口而不只看进程
//!
//! 进程出现 ≠ 用户看到游戏。MCBE 从启动到出画面还要加载资源、连认证服务，
//! 这期间进程一直活着但什么都没有。把「进程出现」当启动成功，用户会看到
//! 启动器收起、然后什么都没有。
//!
//! 判定必须比 `IsWindowVisible` 更严（对齐 LeviLauncher
//! `internal/launch/window_windows.go:36`）：
//! - 非最小化；
//! - 窗口矩形非空；
//! - **DWM 未标记 cloaked** —— 其它虚拟桌面上的窗口 `WS_VISIBLE` 为真但
//!   DWM 把它放在屏幕外，不查这一项就会出现「游戏在别的桌面，屏幕上空空如也」。
//!
//! 另外必须排除两个类名：`ConsoleWindowClass`（控制台）与
//! `CASCADIA_HOSTING_WINDOW_CLASS`（Windows Terminal）—— 启动器自己开的
//! 终端窗口会被误判成游戏窗口。

use crate::error::KernelError;

/// 启动器窗口行为：游戏启动后。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterLaunch {
    Keep,
    Minimize,
    Hide,
}

/// 启动器窗口行为：游戏退出后。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterGameExit {
    /// 保持启动器现状。
    Keep,
    /// 关闭启动器。
    Close,
    /// 重新显示启动器窗口（此前被隐藏/最小化时用）。
    Reopen,
}

impl AfterLaunch {
    /// 解析设置值（`launch.after_launch`），未知值退回 `Keep`。
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "minimize" => Self::Minimize,
            "hide" => Self::Hide,
            _ => Self::Keep,
        }
    }
}

impl AfterGameExit {
    /// 解析设置值（`launch.after_game_exit`），未知值退回 `Keep`。
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "close" => Self::Close,
            "reopen" => Self::Reopen,
            _ => Self::Keep,
        }
    }
}

/// 游戏进程是否已有「已呈现」的窗口。
#[cfg(windows)]
pub fn game_window_presented(pid: u32) -> bool {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId};

    /// 枚举上下文：目标 pid + 是否已找到。
    struct Ctx {
        pid: u32,
        found: bool,
    }

    /// 回调返回 FALSE 时停止枚举。
    unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> BOOL {
        let ctx = &mut *(param.0 as *mut Ctx);
        let mut owner = 0u32;
        if GetWindowThreadProcessId(hwnd, &mut owner) != 0
            && owner == ctx.pid
            && presented(hwnd)
        {
            ctx.found = true;
            return FALSE;
        }
        TRUE
    }

    let mut ctx = Ctx { pid, found: false };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM((&mut ctx as *mut Ctx) as isize));
    }
    ctx.found
}

#[cfg(windows)]
unsafe fn presented(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::Graphics::Dwm::DwmGetWindowAttribute;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetWindowRect, IsIconic, IsWindowVisible,
    };

    if !IsWindowVisible(hwnd).as_bool() {
        return false;
    }
    if IsIconic(hwnd).as_bool() {
        return false;
    }
    let mut rect = windows::Win32::Foundation::RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err()
        || rect.right <= rect.left
        || rect.bottom <= rect.top
    {
        return false;
    }
    // 排除控制台 / 终端窗口：启动器自己开的那些会被误认成游戏窗口。
    let mut class = [0u16; 64];
    let len = GetClassNameW(hwnd, &mut class);
    let class_text = String::from_utf16_lossy(&class[..len.max(0) as usize]);
    if class_text == "ConsoleWindowClass" || class_text == "CASCADIA_HOSTING_WINDOW_CLASS" {
        return false;
    }
    // DWM cloaked：其它虚拟桌面 / 被 DWM 收起的窗口不算「已呈现」。
    let mut cloaked = 0u32;
    let ok = DwmGetWindowAttribute(
        hwnd,
        windows::Win32::Graphics::Dwm::DWMWA_CLOAKED,
        (&mut cloaked as *mut u32).cast(),
        std::mem::size_of::<u32>() as u32,
    );
    ok.is_ok() && cloaked == 0
}

/// 非 Windows：无法判定（返回「已呈现」以免把启动流程卡在等待窗口上）。
#[cfg(not(windows))]
pub fn game_window_presented(_pid: u32) -> bool {
    true
}

/// 对启动器主窗口施加行为。
///
/// `hwnd` 由内核自己持有（前端无法注入窗口句柄）。
#[cfg(windows)]
pub fn apply_launcher_action(hwnd: isize, action: AfterLaunch) -> Result<(), KernelError> {
    use windows::Win32::UI::WindowsAndMessaging::{
        ShowWindow, SW_HIDE, SW_MINIMIZE, SW_RESTORE, SW_SHOW,
    };
    if hwnd == 0 {
        return Ok(());
    }
    let hwnd = windows::Win32::Foundation::HWND(hwnd as *mut std::ffi::c_void);
    unsafe {
        match action {
            AfterLaunch::Keep => ShowWindow(hwnd, SW_SHOW),
            AfterLaunch::Minimize => ShowWindow(hwnd, SW_MINIMIZE),
            AfterLaunch::Hide => ShowWindow(hwnd, SW_HIDE),
        };
    }
    Ok(())
}

#[cfg(windows)]
pub fn apply_exit_action(hwnd: isize, action: AfterGameExit) -> Result<(), KernelError> {
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_RESTORE, SW_SHOW};
    if hwnd == 0 {
        return Ok(());
    }
    let hwnd = windows::Win32::Foundation::HWND(hwnd as *mut std::ffi::c_void);
    unsafe {
        match action {
            AfterGameExit::Keep => ShowWindow(hwnd, SW_SHOW),
            AfterGameExit::Reopen => ShowWindow(hwnd, SW_RESTORE),
            AfterGameExit::Close => {}
        };
    }
    Ok(())
}

/// 非 Windows 平台：窗口行为由前端 Shell 处理，内核不下指令。
#[cfg(not(windows))]
pub fn apply_launcher_action(_hwnd: isize, _action: AfterLaunch) -> Result<(), KernelError> {
    Ok(())
}

#[cfg(not(windows))]
pub fn apply_exit_action(_hwnd: isize, _action: AfterGameExit) -> Result<(), KernelError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_window_actions() {
        assert_eq!(AfterLaunch::parse("minimize"), AfterLaunch::Minimize);
        assert_eq!(AfterLaunch::parse("HIDE"), AfterLaunch::Hide);
        assert_eq!(AfterLaunch::parse("keep"), AfterLaunch::Keep);
        // 未知值退回 Keep：设置被写坏时不该让启动器 disappearing
        assert_eq!(AfterLaunch::parse("???"), AfterLaunch::Keep);

        assert_eq!(AfterGameExit::parse("close"), AfterGameExit::Close);
        assert_eq!(AfterGameExit::parse("reopen"), AfterGameExit::Reopen);
        assert_eq!(AfterGameExit::parse("nonsense"), AfterGameExit::Keep);
    }
}
