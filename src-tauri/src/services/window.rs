// SPDX-License-Identifier: GPL-3.0-only

//! 主窗口句柄登记。
//!
//! WAM 的交互式取票必须把系统账户界面归属到一个**真实存在**的窗口，否则系统
//! 不会呈现界面（这是桌面端 WAM 的硬性要求）。命令层能通过 `AppHandle` 直接取
//! 句柄，但安装链在后台任务里运行，拿不到 `AppHandle`——它同样需要在静默取票
//! 失败时回落到交互路径。
//!
//! 因此句柄在这里登记为内核能力：窗口就绪时写入一次，安装链与命令层读同一份。
//! 句柄只能由内核从**自身**窗口写入，前端无法注入，所以「归属本进程」由构造保证。

use std::sync::atomic::{AtomicIsize, Ordering};

/// 主窗口句柄登记（`0` 表示窗口尚未就绪）。
#[derive(Debug, Default)]
pub struct MainWindow {
    hwnd: AtomicIsize,
}

/// Windows 上校验并归一化句柄；其它平台原样返回。
#[cfg(windows)]
fn normalize(hwnd: isize) -> Option<isize> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetWindowThreadProcessId, GA_ROOT,
    };

    if hwnd == 0 {
        return None;
    }
    let raw = HWND(hwnd as *mut std::ffi::c_void);

    // SAFETY: 只读取窗口属性，不改变窗口状态。
    let mut owner_pid = 0u32;
    // SAFETY: 输出参数指向有效的 u32。
    let _ = unsafe { GetWindowThreadProcessId(raw, Some(&mut owner_pid)) };
    // SAFETY: PID 取自窗口本身。
    let self_pid = std::process::id();
    if owner_pid != 0 && owner_pid != self_pid {
        log::warn!(
            "[core] 主窗口句柄 0x{hwnd:x} 不属于本进程（owner pid={owner_pid}, self pid={self_pid}），交互式授权不可用"
        );
        return None;
    }

    // SAFETY: GetAncestor 只沿窗口链向上查询。
    let root = unsafe { GetAncestor(raw, GA_ROOT) };
    let normalized = if root.0.is_null() { raw } else { root };
    Some(normalized.0 as isize)
}

#[cfg(not(windows))]
fn normalize(hwnd: isize) -> Option<isize> {
    (hwnd != 0).then_some(hwnd)
}

impl MainWindow {
    pub fn new() -> Self {
        Self {
            hwnd: AtomicIsize::new(0),
        }
    }

    /// 登记主窗口句柄。
    ///
    /// 只接受内核从自身主窗口取得的句柄：先校验归属本进程，再归一化到**顶层
    /// 窗口**。WAM 的账户界面需要一个能真正拥有它的顶层窗口，拿到子窗口时
    /// 界面会归属失败、一闪即逝（表现为系统立刻返回用户取消）。窗口重建
    /// （重载、恢复）后应再次写入，否则会留下已失效的句柄。
    ///
    /// 校验不通过时**不写入**并返回 `false`：宁可让交互授权显式不可用，也不要
    /// 把一个不受本进程控制的句柄交给系统账户界面。
    pub fn set(&self, hwnd: isize) -> bool {
        match normalize(hwnd) {
            Some(value) => {
                if value != hwnd {
                    log::info!(
                        "[core] 主窗口句柄已归一化到顶层窗口：0x{hwnd:x} -> 0x{value:x}"
                    );
                }
                self.hwnd.store(value, Ordering::Release);
                true
            }
            None => {
                self.hwnd.store(0, Ordering::Release);
                false
            }
        }
    }

    /// 当前主窗口句柄；未就绪时为 `0`。
    pub fn get(&self) -> isize {
        self.hwnd.load(Ordering::Acquire)
    }

    /// 是否已有可承载系统账户界面的窗口。
    pub fn is_ready(&self) -> bool {
        self.get() != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_registry_reports_no_window() {
        let window = MainWindow::new();
        assert_eq!(window.get(), 0);
        assert!(!window.is_ready());
    }

    #[test]
    fn a_zero_handle_is_never_registered() {
        // A zero handle would reach WAM as "no window" and make the system
        // account UI unowned, so it must be refused rather than stored.
        let window = MainWindow::new();
        assert!(!window.set(0));
        assert!(!window.is_ready());
    }

    #[test]
    fn later_write_replaces_a_stale_handle() {
        let window = MainWindow::new();
        window.set(0x1111);
        let _ = window.set(0x2222);
        assert_eq!(window.get(), 0x2222);
    }

    #[test]
    fn clearing_the_handle_marks_the_window_unready() {
        let window = MainWindow::new();
        let _ = window.set(0x1234);
        window.set(0);
        assert!(!window.is_ready());
    }
}
