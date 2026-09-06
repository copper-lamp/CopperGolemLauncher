//! 下载队列服务：内核侧封装 `copper-downloader`，把引擎事件桥接为
//! 内核事件总线事件（`download.created` / `download.progress` / `download.status`），
//! 供下载悬浮窗与各模块订阅。

use std::sync::Arc;

use copper_downloader::{DownloadManager, DownloadOptions, DownloadStatus, TaskSnapshot};
use serde_json::Value;

use crate::error::KernelError;
use crate::registry::events::EventBus;
use crate::services::paths::Paths;

/// 下载队列服务。
pub struct DownloadService {
    manager: DownloadManager,
}

impl DownloadService {
    /// 创建下载服务。`concurrency` 为同时下载数（可从设置读取）。
    pub fn new(
        concurrency: usize,
        runtime: tokio::runtime::Handle,
        paths: Arc<Paths>,
        events: Arc<EventBus>,
    ) -> Self {
        let manager = DownloadManager::new(concurrency, runtime);
        // 引擎事件 → 内核事件总线（含前端桥接）。
        manager.add_listener(move |ev| {
            let (name, snapshot) = match ev {
                copper_downloader::DownloadEvent::Created(s) => ("download.created", s),
                copper_downloader::DownloadEvent::Progress(s) => ("download.progress", s),
                copper_downloader::DownloadEvent::StatusChanged(s) => ("download.status", s),
            };
            events.publish(name, serde_json::to_value(&snapshot).unwrap_or(Value::Null));
        });
        let _ = paths; // 保留参数：未来下载缓存目录策略从这里取
        Self { manager }
    }

    /// 投递下载任务。`dest` 会按需创建父目录。
    pub fn enqueue(
        &self,
        url: &str,
        dest: &std::path::Path,
        options: DownloadOptions,
    ) -> Result<u64, KernelError> {
        Ok(self.manager.enqueue(url, dest, options)?)
    }

    /// 全部任务快照。
    pub fn tasks(&self) -> Vec<TaskSnapshot> {
        self.manager.snapshots()
    }

    /// 单个任务快照。
    pub fn task(&self, id: u64) -> Option<TaskSnapshot> {
        self.manager.snapshot(id).ok()
    }

    pub fn pause(&self, id: u64) -> Result<(), KernelError> {
        Ok(self.manager.pause(id)?)
    }

    pub fn resume(&self, id: u64) -> Result<(), KernelError> {
        Ok(self.manager.resume(id)?)
    }

    pub fn cancel(&self, id: u64) -> Result<(), KernelError> {
        Ok(self.manager.cancel(id)?)
    }

    pub fn retry(&self, id: u64) -> Result<(), KernelError> {
        Ok(self.manager.retry(id)?)
    }

    pub fn remove(&self, id: u64) -> Result<(), KernelError> {
        Ok(self.manager.remove(id)?)
    }

    pub fn pause_all(&self) {
        self.manager.pause_all();
    }

    pub fn resume_all(&self) {
        self.manager.resume_all();
    }
}

/// 供命令层序列化任务状态到前端的视图。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DownloadTaskView {
    pub id: u64,
    pub filename: Option<String>,
    pub url: String,
    pub dest: String,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub speed_bytes_per_sec: u64,
    pub status: DownloadStatus,
    pub error: Option<String>,
    pub retry_count: u32,
}

impl From<TaskSnapshot> for DownloadTaskView {
    fn from(s: TaskSnapshot) -> Self {
        Self {
            id: s.id,
            filename: s.filename,
            url: s.url,
            dest: s.dest.to_string_lossy().into_owned(),
            total_bytes: s.total_bytes,
            downloaded_bytes: s.downloaded_bytes,
            speed_bytes_per_sec: s.speed_bytes_per_sec,
            status: s.status,
            error: s.error,
            retry_count: s.retry_count,
        }
    }
}
