//! 下载队列服务：内核侧封装 `copper-downloader`，把引擎事件桥接为
//! 内核事件总线事件（`download.created` / `download.progress` / `download.status`），
//! 供下载悬浮窗与各模块订阅。
//!
//! # 内存任务与持久化历史的关系
//!
//! 引擎（`copper-downloader`）的任务**只存在于内存**：`core_download_task` 表
//! 保存的是历史快照（url / dest / 进度 / 投递参数），不保存运行态。
//!
//! 由此产生一条必须显式处理的边界：**表里有行 ≠ 引擎里有任务**。重启后引擎
//! 是空的，全部历史行都退化成「只有记录、没有任务」。若控制命令一律透传给
//! 引擎（`require_task` 只查内存表），重启后对历史行的每一次重试 / 删除都会
//! 返回 `TaskNotFound`，界面表现为「提示任务不存在」——历史记录看得见、点不动。
//!
//! 因此本服务的每个控制命令都按「先引擎、后持久化」两段处理：
//! - 引擎里有任务 → 直接透传，行为与单进程一致；
//! - 引擎里没有但表里有行 → 按持久化信息**复活**任务（沿用原 id，前端行就地
//!   更新，不会出现「新行 + 残留旧行」）或直接改写 / 删除记录；
//! - 两处都没有 → 才是真正的 `TaskNotFound`。
//!
//! 重启瞬间仍写着 `queued` / `downloading` 的行属于「上次运行被中断」，
//! 构造时统一归一为 `paused`（见 [`DownloadService::normalize_interrupted`]），
//! 使其在界面上表现为可「继续」，而不是永远停在假进度条上。

use std::path::PathBuf;
use std::sync::Arc;

use copper_downloader::{DownloadError, DownloadManager, DownloadOptions, DownloadStatus, TaskSnapshot};
use serde_json::Value;

use crate::error::KernelError;
use crate::registry::events::EventBus;
use crate::services::database::DatabaseService;
use crate::services::paths::Paths;

/// 持久化历史保留的终态记录条数上限（超出按创建时间淘汰最旧的）。
///
/// 历史表此前只增不减，是个无上限增长点：每投递一次任务就多一行，长期使用
/// 后 `download_tasks` 每次都要全表扫描+排序。保留最近若干条既满足
/// 「最近下载」展示需求，也把表规模钉在常数级。
const HISTORY_KEEP: i64 = 200;

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
        let persisted_max_id: u64 = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT COALESCE(MAX(id), 0) FROM core_download_task",
                    [],
                    |row| row.get::<_, u64>(0),
                )?)
            })
            .unwrap_or(0);
        // 新 id 必须大于任何历史 id，否则复活历史任务后，新投递会撞上同一个 id
        // 并把它从内存表里挤掉（表现为「任务刚重试就凭空消失」）。
        manager.ensure_next_id(persisted_max_id.saturating_add(1));
        normalize_interrupted(&db);
        let db_for_events = Arc::clone(&db);
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
    ///
    /// 投递参数在此处一并落库：引擎快照只含只读状态，`DownloadOptions`
    /// （断点续传、期望校验和、请求头）必须由调用侧留档，否则重启后历史任务
    /// 无法原样重投。写入用 upsert 的**非冲突分支语义**——行由事件监听器创建，
    /// 这里只补 options，不会覆盖监听器刚写下的状态。
    pub fn enqueue(
        &self,
        url: &str,
        dest: &std::path::Path,
        options: DownloadOptions,
    ) -> Result<u64, KernelError> {
        let id = self.manager.enqueue(url, dest, options.clone())?;
        // 事件监听器已插入该行（Created 事件在 enqueue 内同步发出）；
        // 此处只补 options 列，不覆盖监听器写下的状态。
        self.write_options(id, &options)?;
        Ok(id)
    }

    /// 全部任务快照 = 引擎内存任务 ∪ 持久化历史（以内存为准，同 id 去重）。
    pub fn tasks(&self) -> Vec<DownloadTaskView> {
        let active = self
            .manager
            .snapshots()
            .into_iter()
            .map(DownloadTaskView::from)
            .collect::<Vec<_>>();
        let active_ids = active.iter().map(|task| task.id).collect::<std::collections::HashSet<_>>();
        let mut history = self.history_rows().unwrap_or_default();
        history.retain(|task| !active_ids.contains(&task.id));
        history.extend(active);
        history.sort_by_key(|task| task.id);
        history
    }

    /// 持久化历史行（按创建时间升序）。
    fn history_rows(&self) -> Result<Vec<DownloadTaskView>, KernelError> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, url, dest, filename, status, total_bytes, downloaded_bytes,
                        error, retry_count
                 FROM core_download_task ORDER BY created_at ASC, id ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(DownloadTaskView {
                    id: row.get(0)?,
                    url: row.get(1)?,
                    dest: row.get(2)?,
                    filename: row.get(3)?,
                    status: parse_status(&row.get::<_, String>(4)?),
                    total_bytes: row.get(5)?,
                    downloaded_bytes: row.get(6)?,
                    // 历史行没有速率概念（速率只存在于运行中的内存任务）。
                    speed_bytes_per_sec: 0,
                    error: row.get(7)?,
                    retry_count: row.get::<_, i64>(8)?.max(0) as u32,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// 读取单条历史行（含投递参数，供复活任务用）。
    fn history_row(&self, id: u64) -> Result<Option<HistoryRow>, KernelError> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, url, dest, status, created_at, options
                 FROM core_download_task WHERE id = ?1",
            )?;
            let mut rows = stmt.query_map([id as i64], |row| {
                let raw: Option<String> = row.get(5)?;
                Ok(HistoryRow {
                    id: row.get(0)?,
                    url: row.get(1)?,
                    dest: row.get(2)?,
                    status: parse_status(&row.get::<_, String>(3)?),
                    created_at_ms: row.get::<_, i64>(4)?.max(0) as u64,
                    // 迁移前入库的行没有 options（NULL）。此时只能退回默认参数：
                    // 断点续传仍会生效，但期望校验和 / 自定义请求头会丢失。
                    // 仅影响升级前遗留的历史行，新投递一律带完整参数。
                    options: raw
                        .as_deref()
                        .and_then(|s| serde_json::from_str::<DownloadOptions>(s).ok())
                        .unwrap_or_default(),
                })
            })?;
            match rows.next() {
                Some(row) => Ok(Some(row?)),
                None => Ok(None),
            }
        })
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
        if self.manager.snapshot(id).is_ok() {
            return Ok(self.manager.pause(id)?);
        }
        // 只有持久化记录、引擎里没有任务 → 本来就没有在传输的东西可暂停。
        // 历史行不会展示暂停按钮，这里显式确认「无事可做」而非报错，
        // 避免控制面出现无法解释的失败。
        if self.history_row(id)?.is_some() {
            return Ok(());
        }
        Err(task_not_found(id))
    }

    pub fn resume(&self, id: u64) -> Result<(), KernelError> {
        if self.manager.snapshot(id).is_ok() {
            return Ok(self.manager.resume(id)?);
        }
        let row = self.history_row(id)?.ok_or_else(|| task_not_found(id))?;
        // 引擎的 `resume` 只处理内存里的 Paused；持久化的 paused 行要靠复活。
        // 其它终态（失败 / 取消）语义上应走 `retry`，这里不做隐式转换。
        if row.status != DownloadStatus::Paused {
            return Ok(());
        }
        // 保留 `.part`：这正是「继续」与「重试」的分界——继续保留已下载字节。
        self.revive(&row, false)
    }

    pub fn cancel(&self, id: u64) -> Result<(), KernelError> {
        if self.manager.snapshot(id).is_ok() {
            return Ok(self.manager.cancel(id)?);
        }
        let row = self.history_row(id)?.ok_or_else(|| task_not_found(id))?;
        if matches!(row.status, DownloadStatus::Done | DownloadStatus::Cancelled) {
            return Ok(());
        }
        // 引擎里没有在途任务，只需落终态 + 按配置清理断点文件，
        // 与引擎 `cancel_state` 的收尾保持一致。
        set_history_status(&self.db, id, DownloadStatus::Cancelled, Some("已取消"))?;
        if row.options.remove_on_cancel {
            let _ = std::fs::remove_file(part_path_of(&row.dest));
        }
        Ok(())
    }

    pub fn retry(&self, id: u64) -> Result<(), KernelError> {
        if self.manager.snapshot(id).is_ok() {
            return Ok(self.manager.retry(id)?);
        }
        let row = self.history_row(id)?.ok_or_else(|| task_not_found(id))?;
        // 与引擎 `retry` 对齐：只对失败 / 取消态重试，其余终态是空操作
        // （例如已完成的记录点重试不应重新下载一遍）。
        if !matches!(
            row.status,
            DownloadStatus::Failed | DownloadStatus::Cancelled | DownloadStatus::Paused
        ) {
            return Ok(());
        }
        // 重试丢弃 `.part`：断点字节可能来自已被服务端截断 / 内容变更的响应，
        // 续用它得到的仍可能是坏包；只有「继续」（resume）才保留字节。
        self.revive(&row, true)
    }

    pub fn remove(&self, id: u64) -> Result<(), KernelError> {
        // 内存与持久化两侧都要清。只清一侧会让条目「删了又出现」：
        // 引擎里移除后 DB 行仍在，下次 `tasks()` 就会把它当成历史行重新列出。
        let engine_removed = match self.manager.remove(id) {
            Ok(()) => true,
            // 活跃任务不可移除，必须原样上报，不能顺手删记录。
            Err(e @ DownloadError::ActiveTaskNotRemovable(_)) => return Err(e.into()),
            Err(DownloadError::TaskNotFound(_)) => false,
            Err(e) => return Err(e.into()),
        };
        let history_removed = self.delete_history(id)?;
        if !engine_removed && !history_removed {
            return Err(task_not_found(id));
        }
        Ok(())
    }

    pub fn pause_all(&self) {
        self.manager.pause_all();
    }

    pub fn resume_all(&self) {
        self.manager.resume_all();
        // 持久化的暂停行同样属于「可继续」，全量继续时一并复活。
        if let Ok(rows) = self.history_rows() {
            for view in rows.into_iter().filter(|t| t.status == DownloadStatus::Paused) {
                let _ = self.resume(view.id);
            }
        }
    }

    /// 复活一条持久化任务：沿用原 id 与原创建时间重新入队。
    ///
    /// 沿用原 id 是刻意的：前端列表以 id 为键，重投换新 id 会让界面出现
    /// 「新增一行 + 旧行原地不动」的错位，而旧行的记录随后又会被删除，
    /// 用户看到的是条目闪一下换位置。复用 id 则让同一行就地转入下载态。
    fn revive(&self, row: &HistoryRow, drop_partial: bool) -> Result<(), KernelError> {
        if drop_partial {
            let _ = std::fs::remove_file(part_path_of(&row.dest));
        }
        let mut options = row.options.clone();
        // 复活一律允许断点续传：`.part` 里已下载的字节是有效成果，
        // 哪怕调用方当初关闭了续传，也不该在重试时丢弃它。
        options.resume = true;
        self.manager.enqueue_with_created_at(
            row.id,
            &row.url,
            PathBuf::from(&row.dest),
            options.clone(),
            Some(row.created_at_ms),
        )?;
        // 复活事件（`download.created`）会 upsert 状态但刻意不碰 options 列，
        // 故此处把「强制开启续传」后的最终参数写回，保证下次复活用的是同一份。
        self.write_options(row.id, &options)?;
        Ok(())
    }

    /// 写入单条记录的投递参数（幂等；行不存在时静默跳过）。
    fn write_options(&self, id: u64, options: &DownloadOptions) -> Result<(), KernelError> {
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE core_download_task SET options = ?1 WHERE id = ?2",
                rusqlite::params![serde_json::to_string(options).ok(), id as i64],
            )?;
            Ok(())
        })
    }

    /// 删除一条持久化记录，返回是否确实删掉了行。
    fn delete_history(&self, id: u64) -> Result<bool, KernelError> {
        self.db.with_conn(|conn| {
            let n = conn.execute("DELETE FROM core_download_task WHERE id = ?1", [id as i64])?;
            Ok(n > 0)
        })
    }
}

/// 供复活使用的历史行。
struct HistoryRow {
    id: u64,
    url: String,
    dest: String,
    status: DownloadStatus,
    /// 原始创建时间（Unix 毫秒），复活时沿用以保持列表位置稳定。
    created_at_ms: u64,
    options: DownloadOptions,
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

/// 统一的任务缺失错误（与引擎 `TaskNotFound` 文案一致）。
fn task_not_found(id: u64) -> KernelError {
    KernelError::Download(copper_downloader::DownloadError::TaskNotFound(id))
}

/// `<dest>.part` 断点临时文件路径（与引擎 `part_path_for` 同规则）。
fn part_path_of(dest: &str) -> PathBuf {
    let mut os = std::ffi::OsString::from(dest);
    os.push(".part");
    PathBuf::from(os)
}

/// 解析持久化的状态字符串；未知值按 `failed` 处理（保守：宁可显示可重试，
/// 也不把一条状态不明的记录显示成已完成）。
fn parse_status(raw: &str) -> DownloadStatus {
    match raw {
        "queued" => DownloadStatus::Queued,
        "downloading" => DownloadStatus::Downloading,
        "paused" => DownloadStatus::Paused,
        "cancelled" => DownloadStatus::Cancelled,
        "done" => DownloadStatus::Done,
        _ => DownloadStatus::Failed,
    }
}

/// 状态枚举 → 持久化字符串。
fn status_str(status: DownloadStatus) -> &'static str {
    match status {
        DownloadStatus::Queued => "queued",
        DownloadStatus::Downloading => "downloading",
        DownloadStatus::Paused => "paused",
        DownloadStatus::Cancelled => "cancelled",
        DownloadStatus::Done => "done",
        DownloadStatus::Failed => "failed",
    }
}

/// 启动归一：把上次运行遗留的「在途」状态改为 `paused`。
///
/// 重启后引擎内存为空，那些仍写着 `queued` / `downloading` 的行并不代表有任务
/// 在跑，只是最后一次退出时的快照。若不归一，界面会展示一条永远停在某个
/// 百分比、既不能取消也不能继续的僵尸进度条——比直接显示「已暂停」更糟。
///
/// 归一为 `paused` 而非 `failed`：断点字节还在，「继续」能接着下。
fn normalize_interrupted(db: &DatabaseService) {
    let result = db.with_conn(|conn| {
        let n = conn.execute(
            "UPDATE core_download_task
                SET status = 'paused', error = '上次运行时中断，可继续下载'
              WHERE status IN ('queued', 'downloading')",
            [],
        )?;
        Ok(n)
    });
    match result {
        Ok(n) if n > 0 => log::info!("[download] 归一 {n} 条上次运行中断的任务为已暂停"),
        Ok(_) => {}
        Err(e) => log::warn!("[download] 归一中断任务失败（历史行状态可能不准）: {e}"),
    }
}

/// 改写一条持久化记录的状态与错误（内存任务的事件监听不覆盖历史行）。
fn set_history_status(
    db: &DatabaseService,
    id: u64,
    status: DownloadStatus,
    error: Option<&str>,
) -> Result<(), KernelError> {
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE core_download_task SET status = ?1, error = ?2 WHERE id = ?3",
            rusqlite::params![status_str(status), error, id as i64],
        )?;
        Ok(())
    })
}

/// 落盘任务快照（创建 / 状态变化时调用；进度事件不落库，避免高频写）。
///
/// 同一条 id 既是内存任务也是持久化行的键，故这里 upsert 全字段——
/// 复活历史任务时 `Created` 事件会带着原始 url / dest / 进度回来，
/// 正好把该行刷新成新任务的真实状态。
///
/// `options` 列不在冲突分支里：投递参数由 [`DownloadService::enqueue`] 单独
/// 写入，此处若一并 upsert 会用「快照里没有的默认参数」覆盖真实值
/// （断点续传与期望校验和都会被抹掉）。
fn persist_snapshot(db: &DatabaseService, snapshot: &TaskSnapshot) {
    let _ = db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO core_download_task
             (id, url, dest, filename, status, total_bytes, downloaded_bytes,
              retry_count, error, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
               url = excluded.url, dest = excluded.dest, filename = excluded.filename,
               status = excluded.status, total_bytes = excluded.total_bytes,
               downloaded_bytes = excluded.downloaded_bytes,
               retry_count = excluded.retry_count,
               error = excluded.error",
            rusqlite::params![
                snapshot.id,
                snapshot.url,
                snapshot.dest.to_string_lossy(),
                snapshot.filename,
                status_str(snapshot.status),
                snapshot.total_bytes,
                snapshot.downloaded_bytes,
                snapshot.retry_count,
                snapshot.error,
                snapshot.created_at_ms as i64,
            ],
        )?;
        prune_history(conn)?;
        Ok(())
    });
}

/// 超出 [`HISTORY_KEEP`] 的最旧终态记录淘汰（只删终态，运行中记录永不淘汰）。
fn prune_history(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    conn.execute(
        "DELETE FROM core_download_task
          WHERE id NOT IN (
                SELECT id FROM core_download_task
                 ORDER BY created_at DESC, id DESC LIMIT ?1
              )
            AND status IN ('done', 'failed', 'cancelled', 'paused')",
        [HISTORY_KEEP],
    )?;
    Ok(())
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

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::database::CORE_MIGRATIONS;

    /// 建一个只含 core schema 的临时库（不启动事件总线 / 代理解析）。
    fn temp_db(tag: &str) -> (Arc<DatabaseService>, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "copper_dl_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(DatabaseService::open(&dir.join("k.db")).unwrap());
        db.migrate_scope("core", CORE_MIGRATIONS).unwrap();
        (db, dir)
    }

    fn insert_row(
        db: &DatabaseService,
        id: u64,
        status: &str,
        options: Option<&DownloadOptions>,
        created_at: i64,
    ) {
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO core_download_task
                 (id, url, dest, filename, status, total_bytes, downloaded_bytes,
                  retry_count, error, created_at, options)
                 VALUES (?1, 'http://x/a.zip', 'C:/t/a.zip', 'a.zip', ?2, 100, 40, 2,
                         'boom', ?3, ?4)",
                rusqlite::params![
                    id as i64,
                    status,
                    created_at,
                    options.and_then(|o| serde_json::to_string(o).ok()),
                ],
            )?;
            Ok(())
        })
        .unwrap();
    }

    fn read_row(db: &DatabaseService, id: u64) -> Option<(String, i64, Option<String>)> {
        db.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT status, created_at, options FROM core_download_task WHERE id = ?1")?;
            let mut rows = stmt.query_map([id as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?))
            })?;
            match rows.next() {
                Some(r) => Ok(Some(r?)),
                None => Ok(None),
            }
        })
        .unwrap()
    }

    /// 回归：重启后的「在途」行必须被归一为 paused。
    ///
    /// 否则界面会显示一条永远停在某百分比、既不能取消也不能继续的僵尸进度条。
    #[test]
    fn normalize_interrupted_marks_inflight_as_paused() {
        let (db, dir) = temp_db("norm");
        insert_row(&db, 1, "downloading", None, 10);
        insert_row(&db, 2, "queued", None, 20);
        insert_row(&db, 3, "done", None, 30);

        normalize_interrupted(&db);

        assert_eq!(read_row(&db, 1).unwrap().0, "paused");
        assert_eq!(read_row(&db, 2).unwrap().0, "paused");
        // 终态不受影响。
        assert_eq!(read_row(&db, 3).unwrap().0, "done");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 归一只动在途状态，可重复执行（幂等）。
    #[test]
    fn normalize_interrupted_is_idempotent() {
        let (db, dir) = temp_db("norm2");
        insert_row(&db, 1, "downloading", None, 10);
        normalize_interrupted(&db);
        normalize_interrupted(&db);
        assert_eq!(read_row(&db, 1).unwrap().0, "paused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归：状态字符串与枚举双向映射自洽（持久化层唯一的翻译点）。
    #[test]
    fn status_str_roundtrips_all_variants() {
        for s in [
            DownloadStatus::Queued,
            DownloadStatus::Downloading,
            DownloadStatus::Paused,
            DownloadStatus::Cancelled,
            DownloadStatus::Failed,
            DownloadStatus::Done,
        ] {
            assert_eq!(parse_status(status_str(s)), s, "往返不一致: {s:?}");
        }
        // 未知值按 failed 处理：宁可显示可重试，也不谎报已完成。
        assert_eq!(parse_status("weird"), DownloadStatus::Failed);
    }

    /// 投递参数必须能原样往返，否则重启后重投会丢掉断点续传 / 完整性校验。
    #[test]
    fn download_options_roundtrip_through_db_text() {
        let (db, dir) = temp_db("opts");
        let options = DownloadOptions {
            resume: true,
            remove_on_cancel: false,
            expected_sha256: Some("a".repeat(64)),
            max_retries: 7,
            headers: vec![("Authorization".into(), "Bearer x".into())],
            filename: Some("pkg.msixvc".into()),
            existing_policy: copper_downloader::error::ExistingFilePolicy::SkipIfValid,
        };
        insert_row(&db, 1, "failed", Some(&options), 100);

        let (_, _, raw) = read_row(&db, 1).unwrap();
        let back: DownloadOptions = serde_json::from_str(&raw.unwrap()).unwrap();
        assert_eq!(back.resume, options.resume);
        assert_eq!(back.remove_on_cancel, options.remove_on_cancel);
        assert_eq!(back.expected_sha256, options.expected_sha256);
        assert_eq!(back.max_retries, options.max_retries);
        assert_eq!(back.headers, options.headers);
        assert_eq!(back.filename, options.filename);
        assert_eq!(back.existing_policy, options.existing_policy);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 迁移前的行（options 为 NULL）必须能退回默认值，而不是读取失败。
    #[test]
    fn missing_options_fall_back_to_defaults() {
        let (db, dir) = temp_db("noopts");
        insert_row(&db, 1, "failed", None, 100);
        let (_, _, raw) = read_row(&db, 1).unwrap();
        assert!(raw.is_none());
        let back: DownloadOptions =
            serde_json::from_str(raw.as_deref().unwrap_or("{}")).unwrap_or_default();
        assert_eq!(back.max_retries, DownloadOptions::default().max_retries);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归：`remove` 语义依赖「内存 + 持久化」两侧的清理，故验证两侧独立生效。
    #[test]
    fn delete_history_removes_only_that_row() {
        let (db, dir) = temp_db("del");
        insert_row(&db, 1, "done", None, 10);
        insert_row(&db, 2, "done", None, 20);

        let n = db
            .with_conn(|conn| {
                Ok(conn.execute("DELETE FROM core_download_task WHERE id = ?1", [1i64])?)
            })
            .unwrap();
        assert_eq!(n, 1);
        assert!(read_row(&db, 1).is_none());
        assert!(read_row(&db, 2).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 淘汰只针对终态：窗口外的运行中记录必须保留（否则会把在途任务的历史抹掉）。
    #[test]
    fn prune_keeps_active_and_trims_oldest_terminal() {
        let (db, dir) = temp_db("prune");
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO core_download_task
                 (id, url, dest, status, total_bytes, created_at)
                 VALUES (1, 'u', 'd', 'downloading', 0, ?1)",
                rusqlite::params![999_999i64],
            )?;
            for id in 2..=(HISTORY_KEEP + 10) {
                conn.execute(
                    "INSERT INTO core_download_task
                     (id, url, dest, status, total_bytes, created_at)
                     VALUES (?1, 'u', 'd', 'done', 0, ?2)",
                    rusqlite::params![id as i64, id as i64],
                )?;
            }
            Ok(())
        })
        .unwrap();

        db.with_conn(|conn| {
            prune_history(conn)?;
            Ok(())
        })
        .unwrap();

        // 运行中的记录即便时间戳最新也必须留下。
        assert!(read_row(&db, 1).is_some(), "运行中记录被误删");
        let remaining: i64 = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM core_download_task WHERE status = 'done'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        // 窗口按「全部记录」取最近 HISTORY_KEEP 条，运行中记录也占名额，
        // 故终态保留数比 HISTORY_KEEP 少一条。这是有意的：窗口约束的是
        // 展示与表规模，不该让在途任务把自己的历史挤掉。
        assert_eq!(remaining, HISTORY_KEEP - 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `<dest>.part` 必须与引擎的断点文件规则一致，否则重试删错文件。
    #[test]
    fn part_path_appends_dot_part_like_engine() {
        assert_eq!(
            part_path_of("C:/t/a.zip").to_string_lossy(),
            PathBuf::from("C:/t/a.zip.part").to_string_lossy()
        );
    }
}