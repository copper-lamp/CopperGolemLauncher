//! 下载队列服务：内核侧封装 `copper-downloader`，把引擎事件桥接为
//! 内核事件总线事件（`download.created` / `download.progress` / `download.status`），
//! 供下载悬浮窗与各模块订阅。

use std::sync::Arc;

use copper_downloader::{DownloadManager, DownloadOptions, DownloadStatus, TaskSnapshot};
use serde_json::Value;

use crate::error::KernelError;
use crate::registry::events::EventBus;
use crate::services::database::DatabaseService;
use crate::services::paths::Paths;

/// 下载队列服务。
pub struct DownloadService {
    manager: DownloadManager,
    db: Arc<DatabaseService>,
}

impl DownloadService {
    /// 创建下载服务。`concurrency` 为同时下载数（可从设置读取）。
    pub fn new(
        concurrency: usize,
        runtime: tokio::runtime::Handle,
        paths: Arc<Paths>,
        db: Arc<DatabaseService>,
        events: Arc<EventBus>,
    ) -> Self {
        // 引擎自建 reqwest 客户端，且编译时关闭了 reqwest 默认特性（不读环境变量代理），
        // 必须显式注入内核解析出的代理，否则 http:// CDN 下载在需要代理的网络下会失败。
        let manager = DownloadManager::new_with_proxy(
            concurrency,
            runtime,
            crate::services::http_client::resolved_proxy(),
        );
        let db_for_events = db.clone();
        manager.add_listener(move |ev| {
            let (name, snapshot) = match ev {
                copper_downloader::DownloadEvent::Created(s) => ("download.created", s),
                copper_downloader::DownloadEvent::Progress(s) => ("download.progress", s),
                copper_downloader::DownloadEvent::StatusChanged(s) => ("download.status", s),
            };
            if !matches!(name, "download.progress") {
                persist_snapshot(&db_for_events, &snapshot);
            }
            events.publish(name, serde_json::to_value(&snapshot).unwrap_or(Value::Null));
        });
        let _ = paths;
        Self { manager, db }
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
    pub fn tasks(&self) -> Vec<DownloadTaskView> {
        let active = self
            .manager
            .snapshots()
            .into_iter()
            .map(DownloadTaskView::from)
            .collect::<Vec<_>>();
        let active_ids = active.iter().map(|task| task.id).collect::<std::collections::HashSet<_>>();
        let mut history = self
            .db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, url, dest, filename, status, total_bytes, error, created_at
                     FROM core_download_task ORDER BY created_at ASC",
                )?;
                let rows = stmt.query_map([], |row| {
                    let status: DownloadStatus = match row.get::<_, String>(4)?.as_str() {
                        "queued" => DownloadStatus::Queued,
                        "downloading" => DownloadStatus::Downloading,
                        "paused" => DownloadStatus::Paused,
                        "cancelled" => DownloadStatus::Cancelled,
                        "done" => DownloadStatus::Done,
                        _ => DownloadStatus::Failed,
                    };
                    Ok(DownloadTaskView {
                        id: row.get::<_, u64>(0)?,
                        filename: row.get(3)?,
                        url: row.get(1)?,
                        dest: row.get(2)?,
                        total_bytes: row.get::<_, u64>(5)?,
                        downloaded_bytes: 0,
                        speed_bytes_per_sec: 0,
                        status,
                        error: row.get(6)?,
                        retry_count: 0,
                    })
                })?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap_or_default();
        history.retain(|task| !active_ids.contains(&task.id));
        history.extend(active);
        history.sort_by_key(|task| task.id);
        history
    }

    /// 当前并发上限（同时下载的任务数）。
    pub fn concurrency(&self) -> usize {
        self.manager.concurrency()
    }

    /// 调整并发上限（运行期即时生效，排队任务按新上限重新派发）。
    pub fn set_concurrency(&self, concurrency: usize) {
        self.manager.set_concurrency(concurrency);
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

fn persist_snapshot(db: &DatabaseService, snapshot: &TaskSnapshot) {
    let _ = db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO core_download_task
             (id, url, dest, filename, status, total_bytes, error, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
               url = excluded.url, dest = excluded.dest, filename = excluded.filename,
               status = excluded.status, total_bytes = excluded.total_bytes,
               error = excluded.error, created_at = excluded.created_at",
            rusqlite::params![
                snapshot.id,
                snapshot.url,
                snapshot.dest.to_string_lossy(),
                snapshot.filename,
                serde_json::to_string(&snapshot.status).unwrap_or_default().trim_matches('"'),
                snapshot.total_bytes,
                snapshot.error,
                snapshot.created_at_ms as i64,
            ],
        )?;
        Ok(())
    });
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
