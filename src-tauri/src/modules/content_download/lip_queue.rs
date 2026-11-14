//! LIP 安装的排队与并发闸门。
//!
//! 下载中心有一个「同时下载」设置（`download.concurrency`，1~5）。内容安装同样吃这个
//! 上限：同一个版本目录上的安装必须串行（引擎的工作区锁也会拦，但排队比报错友好），
//! 不同版本目录之间按设置并行。
//!
//! 这里只在进程内做调度，不落库：排队中的任务记录已经是 `installing`，界面上显示
//! 「排队中」，进程重启后由内容下载模块的中断恢复逻辑统一收尾。

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::Notify;

use crate::modules::lip::{Cancel, LipError};

struct QueueState {
    /// 当前真正在执行的作业数。
    active: usize,
    /// 正在执行的作业占用的目标目录（同一个目录只允许一个作业）。
    dests: HashSet<String>,
}

fn state() -> &'static Mutex<QueueState> {
    static STATE: OnceLock<Mutex<QueueState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(QueueState { active: 0, dests: HashSet::new() }))
}

fn doorbell() -> &'static Notify {
    static DOORBELL: OnceLock<Notify> = OnceLock::new();
    DOORBELL.get_or_init(Notify::new)
}

/// 一个已获得的执行许可；析构即释放，调用方保证作业结束（含失败 / 取消）时释放。
pub struct QueuePermit {
    dest: String,
}

impl Drop for QueuePermit {
    fn drop(&mut self) {
        if let Ok(mut state) = state().lock() {
            state.active = state.active.saturating_sub(1);
            state.dests.remove(&self.dest);
        }
        doorbell().notify_waiters();
    }
}

/// 申请执行许可：同一目录串行，全局受 `limit` 限制。
///
/// `limit` 每次等待都重新求值，用户在下载中心改了「同时下载」后，正在排队的作业会
/// 立刻按新上限放行。`waiting` 只在第一次真正进入等待时调用一次，用来把「排队中」
/// 这个状态告诉界面——重复调用会刷屏进度事件。
pub async fn acquire<F>(dest: &str, limit: F, cancel: &Cancel, mut waiting: impl FnMut()) -> Result<QueuePermit, LipError>
where
    F: Fn() -> usize,
{
    let mut announced = false;
    loop {
        cancel.check()?;
        {
            let mut state = state().lock().map_err(|_| LipError::WorkspaceLocked)?;
            if state.active < limit().clamp(1, 5) && !state.dests.contains(dest) {
                state.active += 1;
                state.dests.insert(dest.to_string());
                return Ok(QueuePermit { dest: dest.to_string() });
            }
        }
        if !announced {
            announced = true;
            waiting();
        }
        // 等有人释放，或等上限被调大；定时兜底是为了读到新的设置值。
        tokio::select! {
            _ = doorbell().notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(400)) => {}
        }
    }
}

/// 当前排队 / 执行数量（诊断用）。
pub fn snapshot() -> (usize, usize) {
    match state().lock() {
        Ok(state) => (state.active, state.dests.len()),
        Err(_) => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn limits_global_and_serializes_same_dest() {
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for index in 0..4 {
            let running = running.clone();
            let peak = peak.clone();
            handles.push(tokio::spawn(async move {
                let cancel = Cancel::new();
                let dest = if index < 2 { "same" } else { "other" };
                let permit = acquire(dest, || 2, &cancel, || {}).await.unwrap();
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                running.fetch_sub(1, Ordering::SeqCst);
                drop(permit);
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        assert!(peak.load(Ordering::SeqCst) <= 2, "global limit exceeded");
        assert_eq!(running.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancelled_while_waiting_returns_cancel_error() {
        let held_cancel = Cancel::new();
        let _held = acquire("only", || 1, &held_cancel, || {}).await.unwrap();
        let waiting = std::sync::Arc::new(Cancel::new());
        let buddy = waiting.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            buddy.cancel();
        });
        let result = acquire("only", || 1, &waiting, || {}).await;
        assert!(matches!(result, Err(LipError::Cancelled)));
    }
}
