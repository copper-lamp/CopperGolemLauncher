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

impl MainWindow {
    pub fn new() -> Self {
        Self {
            hwnd: AtomicIsize::new(0),
        }
    }

    /// 登记主窗口句柄。
    ///
    /// 只接受内核从自身主窗口取得的句柄。窗口重建（重载、恢复）后应再次写入，
    /// 否则会留下已失效的句柄。
    pub fn set(&self, hwnd: isize) {
        self.hwnd.store(hwnd, Ordering::Release);
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
    fn set_makes_the_window_ready_and_readable() {
        let window = MainWindow::new();
        window.set(0x1234);
        assert_eq!(window.get(), 0x1234);
        assert!(window.is_ready());
    }

    #[test]
    fn later_write_replaces_a_stale_handle() {
        let window = MainWindow::new();
        window.set(0x1111);
        window.set(0x2222);
        assert_eq!(window.get(), 0x2222);
    }

    #[test]
    fn clearing_the_handle_marks_the_window_unready() {
        let window = MainWindow::new();
        window.set(0x1234);
        window.set(0);
        assert!(!window.is_ready());
    }
}
