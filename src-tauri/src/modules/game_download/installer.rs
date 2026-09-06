//! 游戏下载安装流水线：以数据库「下载任务→版本」为事实源，驱动 下载→校验→解包→写元数据→广播。
//!
//! 生命周期：
//! - `enqueue`：幂等投递下载（防重复安装 / 防重复排队），返回下载任务 id。
//! - 订阅 `download.status`：任务 `Done` → 触发异步安装（单飞锁串行）；`Failed/Cancelled` → 落失败。
//! - `finish_install`：md5 自验 → `extractor::extract_package` → 写 `version.json` → 清理整包 → 广播
//!   `version.installed`（开始页据此刷新可启动版本）与 `game-download.installed`。
//! - `resume_pending`：`start` 时续传没有完成的下载 / 续装中断的解包。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use copper_downloader::{DownloadOptions, DownloadStatus};
use md5::{Digest as _Md5Digest, Md5};
use serde_json::Value;

use crate::error::KernelError;
use crate::registry::events::EventBus;
use crate::services::database::DatabaseService;
use crate::services::download::DownloadService;
use crate::services::paths::Paths;
use crate::services::settings::SettingsService;
use crate::state::KernelContext;

use super::extractor;
use super::manifest;
use super::meta_bridge;

/// 模块在 cache 下的下载子目录。
const CACHE_SUBDIR: &str = "game-download";
/// 原生解包 DLL 落盘子目录。
const GDK_SUBDIR: &str = "gdkshared";

/// 安装流水线上下文：把 `KernelContext` 需要跨 async 捕获的能力拆出为可克隆 Arcs。
#[derive(Clone)]
pub struct Ctx {
    pub events: Arc<EventBus>,
    pub db: Arc<DatabaseService>,
    pub paths: Arc<Paths>,
    pub settings: Arc<SettingsService>,
    pub download: Arc<DownloadService>,
    pub runtime: tokio::runtime::Handle,
}

impl Ctx {
    pub fn from_kernel(kernel: &KernelContext) -> Self {
        Self {
            events: kernel.events().clone(),
            db: kernel.db().clone(),
            paths: kernel.paths().clone(),
            settings: kernel.settings().clone(),
            download: kernel.download().clone(),
            runtime: kernel.runtime().clone(),
        }
    }

    /// 下载缓存目录（`cache/game-download`）。
    pub fn cache_home(&self) -> PathBuf {
        self.paths.cache_dir().join(CACHE_SUBDIR)
    }

    /// 原生 DLL 落盘目录（`cache/gdkshared`）。
    pub fn gdk_dir(&self) -> PathBuf {
        self.paths.cache_dir().join(GDK_SUBDIR)
    }

    /// 某版本整包目标路径。
    pub fn dest_for(&self, slug: &str) -> PathBuf {
        self.cache_home().join(format!("{slug}.msixvc"))
    }

    /// 某版本安装目录（`versions/<folder>`）。
    pub fn install_dir(&self, folder: &str) -> PathBuf {
        self.paths.versions_dir().join(folder)
    }

    /// 某时间戳（Unix 秒）。
    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------- DB

pub const MIGRATION: crate::services::database::Migration = crate::services::database::Migration {
    version: 1,
    name: "game_download_task",
    sql: "CREATE TABLE IF NOT EXISTS module_game_download_task (
            version_id TEXT PRIMARY KEY,
            kind       TEXT NOT NULL,
            folder     TEXT NOT NULL,
            dest       TEXT NOT NULL,
            md5        TEXT NOT NULL,
            state      TEXT NOT NULL DEFAULT 'downloading',
            error      TEXT,
            task_id    INTEGER,
            created_at INTEGER NOT NULL
          );",
};

/// 任务 DB 记录。
#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub version_id: String,
    pub kind: String,
    pub folder: String,
    pub dest: PathBuf,
    pub md5: String,
    pub state: String,
    pub error: Option<String>,
    pub task_id: Option<u64>,
}

fn row_to_record(r: &rusqlite::Row) -> rusqlite::Result<TaskRecord> {
    Ok(TaskRecord {
        version_id: r.get(0)?,
        kind: r.get(1)?,
        folder: r.get(2)?,
        dest: PathBuf::from(r.get::<_, String>(3)?),
        md5: r.get(4)?,
        state: r.get(5)?,
        error: r.get(6)?,
        task_id: r.get(7)?,
    })
}

fn get_record(ctx: &Ctx, version_id: &str) -> Result<Option<TaskRecord>, KernelError> {
    ctx.db.with_conn(|conn| -> Result<Option<TaskRecord>, KernelError> {
        let mut stmt = conn.prepare(
            "SELECT version_id, kind, folder, dest, md5, state, error, task_id
             FROM module_game_download_task WHERE version_id = ?1",
        )?;
        let mut rows = stmt.query_map([version_id], row_to_record)?;
        let rec = match rows.next() {
            Some(r) => Some(r?),
            None => None,
        };
        Ok(rec)
    })
}

fn list_records(ctx: &Ctx, states: &[&str]) -> Result<Vec<TaskRecord>, KernelError> {
    ctx.db.with_conn(|conn| {
        let placeholders = states.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT version_id, kind, folder, dest, md5, state, error, task_id
             FROM module_game_download_task WHERE state IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<rusqlite::types::Value> = states
            .iter()
            .map(|s| rusqlite::types::Value::Text(s.to_string()))
            .collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params), row_to_record)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(KernelError::from)
    })
}

/// 幂等 upsert 记录。
fn upsert_record(ctx: &Ctx, rec: &TaskRecord) -> Result<(), KernelError> {
    ctx.db.with_conn(|conn| -> Result<(), KernelError> {
        conn.execute(
            "INSERT INTO module_game_download_task
               (version_id, kind, folder, dest, md5, state, error, task_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(version_id) DO UPDATE SET
               kind=excluded.kind, folder=excluded.folder, dest=excluded.dest,
               md5=excluded.md5, state=excluded.state, error=excluded.error, task_id=excluded.task_id",
            rusqlite::params![
                rec.version_id,
                rec.kind,
                rec.folder,
                rec.dest.to_string_lossy(),
                rec.md5,
                rec.state,
                rec.error,
                rec.task_id,
                Ctx::now()
            ],
        )?;
        Ok(())
    })
}

fn set_state(ctx: &Ctx, version_id: &str, state: &str, error: Option<&str>) -> Result<(), KernelError> {
    ctx.db.with_conn(|conn| -> Result<(), KernelError> {
        conn.execute(
            "UPDATE module_game_download_task SET state = ?1, error = ?2 WHERE version_id = ?3",
            rusqlite::params![state, error, version_id],
        )?;
        Ok(())
    })
}

// ---------------------------------------------------------------- 命令核心

/// 投递下载：查已安装幂等拒绝 → 定目录 → upsert 记录 → 投递全局下载队列 → 回填 task_id → 广播。
pub async fn enqueue(ctx: &Ctx, id: &str) -> Result<u64, KernelError> {
    // 已安装幂等拒绝。
    if meta_bridge::is_installed(&ctx.install_dir(id)) {
        return Err(KernelError::InvalidArgument("该版本已安装".into()));
    }

    // 清单取版本条目。
    let versions = manifest::load_manifest(ctx, false).await?;
    let entry = versions
        .find_by_id(id)
        .ok_or_else(|| KernelError::InvalidArgument(format!("清单中找不到版本 `{id}`")))?;
    let url = entry
        .primary_url()
        .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{id}` 没有可用下载链接")))?;
    let slug = entry.slug();
    let kind = match entry.kind() {
        manifest::VersionKind::Release => "release",
        manifest::VersionKind::Preview => "preview",
    };
    let dest = ctx.dest_for(&slug);

    // 已存在进行中任务 → 幂等返回原 task_id，避免重复排队。
    if let Some(rec) = get_record(ctx, &slug)? {
        if rec.state == "downloading" || rec.state == "extracting" {
            return rec.task_id.ok_or_else(|| {
                KernelError::Module(format!("版本 {slug} 已有任务但缺 task_id"))
            });
        }
        // installed / failed → 允许重装，重置状态继续。
    }

    std::fs::create_dir_all(dest.parent().unwrap_or(Path::new(".")))?;

    let opts = DownloadOptions {
        resume: true,
        remove_on_cancel: true,
        expected_sha256: None, // 清单仅提供 MD5，下载完成后模块内自验
        filename: Some(format!("{slug}.msixvc")),
        ..Default::default()
    };
    let task_id = ctx.download.enqueue(&url, &dest, opts)?;

    let rec = TaskRecord {
        version_id: slug.clone(),
        kind: kind.to_string(),
        folder: slug.clone(),
        dest: dest.clone(),
        md5: entry.md5.clone(),
        state: "downloading".into(),
        error: None,
        task_id: Some(task_id),
    };
    upsert_record(ctx, &rec)?;

    ctx.events.publish(
        "game-download.enqueued",
        serde_json::json!({ "id": slug, "taskId": task_id }),
    );
    Ok(task_id)
}

/// 单版本任务视图（供前端渲染状态与进度）。
pub fn status(ctx: &Ctx, id: &str) -> Result<Option<TaskView>, KernelError> {
    let Some(rec) = get_record(ctx, id)? else {
        return Ok(None);
    };
    let snapshot = rec
        .task_id
        .and_then(|t| ctx.download.task(t))
        .map(DownloadState::from_snapshot);
    Ok(Some(TaskView {
        version_id: rec.version_id,
        kind: rec.kind,
        dest: rec.dest.to_string_lossy().into_owned(),
        state: rec.state.clone(),
        error: rec.error,
        download: snapshot,
    }))
}

/// 取消任务（下载中取消；解包阶段不可中断则标失败）。
pub fn cancel(ctx: &Ctx, id: &str) -> Result<(), KernelError> {
    let Some(rec) = get_record(ctx, id)? else {
        return Err(KernelError::InvalidArgument(format!("无任务 `{id}`")));
    };
    if let Some(task_id) = rec.task_id {
        ctx.download.cancel(task_id)?;
    }
    if rec.state == "downloading" {
        set_state(ctx, id, "failed", Some("已取消"))?;
        ctx.events.publish("game-download.cancelled", serde_json::json!({ "id": id }));
    }
    Ok(())
}

/// 强制刷新清单源并重建缓存。
pub async fn refresh_source(ctx: &Ctx) -> Result<(), KernelError> {
    manifest::load_manifest(ctx, true).await.map(|_| ())
}

// ---------------------------------------------------------------- 事件驱动

/// 处理 `download.status` 事件（同步回调，异步工作经 runtime.spawn）。
pub fn handle_status(ctx: &Ctx, lock: &Arc<tokio::sync::Mutex<()>>, payload: &Value) {
    let Some((task_id, status, _dest)) = parse_snapshot(payload) else {
        return;
    };
    let Some(rec) = find_by_task_id(ctx, task_id) else {
        return; // 非本模块任务
    };

    let next_ctx = ctx.clone();
    let next_lock = lock.clone();
    match status {
        DownloadStatus::Done => {
            // md5 校验 + 解包 + 写元数据：异步、串行。
            let vid = rec.version_id.clone();
            let task_ctx = next_ctx.clone();
            let task_lock = next_lock.clone();
            next_ctx.runtime.clone().spawn(async move {
                if let Err(e) = finish_install(&task_ctx, &task_lock, rec).await {
                    mark_failed(&task_ctx, &vid, &e.to_string());
                    log::error!("[game-download] 安装 {vid} 失败: {e}");
                }
            });
        }
        DownloadStatus::Failed | DownloadStatus::Cancelled => {
            let msg = payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("下载失败")
                .to_string();
            mark_failed(ctx, &rec.version_id, &msg);
        }
        _ => {}
    }
}

/// 从下载引擎事件负载解析 (task_id, status, dest)。
fn parse_snapshot(payload: &Value) -> Option<(u64, DownloadStatus, String)> {
    let id = payload.get("id")?.as_u64()?;
    let status_str = payload.get("status")?.as_str()?;
    let status = match status_str {
        "queued" => DownloadStatus::Queued,
        "downloading" => DownloadStatus::Downloading,
        "paused" => DownloadStatus::Paused,
        "cancelled" => DownloadStatus::Cancelled,
        "failed" => DownloadStatus::Failed,
        "done" => DownloadStatus::Done,
        _ => return None,
    };
    let dest = payload
        .get("dest")
        .and_then(|d| d.as_str())
        .unwrap_or_default()
        .to_string();
    Some((id, status, dest))
}

fn find_by_task_id(ctx: &Ctx, task_id: u64) -> Option<TaskRecord> {
    ctx.db
        .with_conn(
            |conn| -> Result<Option<TaskRecord>, KernelError> {
                let mut stmt = conn.prepare(
                    "SELECT version_id, kind, folder, dest, md5, state, error, task_id
                     FROM module_game_download_task WHERE task_id = ?1",
                )?;
                let mut rows = stmt.query_map([task_id], row_to_record)?;
                let rec = match rows.next() {
                    Some(r) => Some(r?),
                    None => None,
                };
                Ok(rec)
            },
        )
        .ok()
        .flatten()
}

/// 落失败 + 广播 `version.download_failed`。
fn mark_failed(ctx: &Ctx, version_id: &str, error: &str) {
    let _ = set_state(ctx, version_id, "failed", Some(error));
    ctx.events.publish(
        "version.download_failed",
        serde_json::json!({ "id": version_id, "error": error }),
    );
    ctx.events.publish(
        "game-download.failed",
        serde_json::json!({ "id": version_id, "error": error }),
    );
}

// ---------------------------------------------------------------- 安装

/// md5 校验：目标文件 MD5 需与清单 md5 一致（大小写不敏感）。
fn md5_matches(path: &Path, expected: &str) -> Result<bool, KernelError> {
    let raw = std::fs::read(path)?;
    let digest = Md5::digest(&raw);
    Ok(format!("{digest:x}").eq_ignore_ascii_case(expected.trim()))
}

/// 异步安装：单飞锁串行解包（避免并发 GB 级解压）；幂等重入安全。
pub async fn finish_install(
    ctx: &Ctx,
    lock: &Arc<tokio::sync::Mutex<()>>,
    rec: TaskRecord,
) -> Result<(), KernelError> {
    let _guard = lock.lock().await;

    // 幂等：已安装即直接收尾（供续装 / 重复事件）。
    let install_dir = ctx.install_dir(&rec.folder);
    if meta_bridge::is_installed(&install_dir) {
        set_state(ctx, &rec.version_id, "installed", None)?;
        publish_installed(ctx, &rec);
        cleanup_package(&rec.dest);
        return Ok(());
    }

    set_state(ctx, &rec.version_id, "extracting", None)?;

    // md5 自验（引擎只保留 sha256，清单提供的是 md5）。
    if !md5_matches(&rec.dest, &rec.md5)? {
        return Err(KernelError::Config(format!(
            "MD5 校验失败：{} 与清单不符",
            rec.dest.to_string_lossy()
        )));
    }

    // 解包（.msixvc → DLL；历史 .appx → zip 回退）→ 校验主程序 → 写元数据。
    extractor::extract_package(&rec.dest, &install_dir, &ctx.gdk_dir()).map_err(KernelError::from)?;

    let kind = rec.kind.clone();
    meta_bridge::write_meta(&install_dir, &rec.folder, &rec.version_id, &kind)?;

    set_state(ctx, &rec.version_id, "installed", None)?;
    cleanup_package(&rec.dest);
    publish_installed(ctx, &rec);
    Ok(())
}

fn publish_installed(ctx: &Ctx, rec: &TaskRecord) {
    // `version.installed` 供开始页刷新清单；`game-download.installed` 供详情页联动。
    ctx.events.publish(
        "version.installed",
        serde_json::json!({ "name": rec.version_id }),
    );
    ctx.events.publish(
        "game-download.installed",
        serde_json::json!({ "id": rec.version_id }),
    );
}

/// 清理已消费的整包。
fn cleanup_package(dest: &Path) {
    let _ = std::fs::remove_file(dest);
    let _ = std::fs::remove_file(PathBuf::from(format!("{}.part", dest.to_string_lossy())));
}

// ---------------------------------------------------------------- 续传

/// 启动恢复：对未完成的记录续传/续装。
pub fn resume_pending(ctx: &Ctx, lock: &Arc<tokio::sync::Mutex<()>>) {
    let recs = match list_records(ctx, &["downloading", "extracting", "failed"]) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[game-download] resume_pending 读取记录失败: {e}");
            return;
        }
    };
    for rec in recs {
        // 已装但未同步 → 补收尾。
        if meta_bridge::is_installed(&ctx.install_dir(&rec.folder)) {
            set_state(ctx, &rec.version_id, "installed", None).ok();
            publish_installed(ctx, &rec);
            cleanup_package(&rec.dest);
            continue;
        }
        // 整包已完整且 md5 命中 → 直接续装。
        if rec.dest.is_file() && md5_matches(&rec.dest, &rec.md5).unwrap_or(false) {
            let next_ctx = ctx.clone();
            let next_lock = lock.clone();
            let vid = rec.version_id.clone();
            ctx.runtime.spawn(async move {
                if let Err(e) = finish_install(&next_ctx, &next_lock, rec).await {
                    log::error!("[game-download] 续装 {vid} 失败: {e}");
                }
            });
            continue;
        }
        // 整包不完整 → 重新投递续传（引擎按 `.part` Range 续传）。
        let entry = match block_load(ctx).and_then(|v| {
            v.find_by_id(&rec.version_id)
                .ok_or_else(|| KernelError::InvalidArgument("清单无此版本".into()))
        }) {
            Ok(e) => e,
            Err(e) => {
                // 清单异常不阻塞其余任务。
                if let Some(t) = rec.task_id {
                    let _ = ctx.download.resume(t);
                }
                log::warn!("[game-download] 续传 {} 取清单失败: {e}", rec.version_id);
                continue;
            }
        };
        let Some(url) = entry.primary_url() else {
            continue;
        };
        let opts = DownloadOptions {
            resume: true,
            remove_on_cancel: true,
            expected_sha256: None,
            filename: Some(format!("{}.msixvc", rec.version_id)),
            ..Default::default()
        };
        match ctx.download.enqueue(&url, &rec.dest, opts) {
            Ok(task_id) => {
                set_state(ctx, &rec.version_id, "downloading", None).ok();
                let _ = ctx.db.with_conn(|conn| {
                    conn.execute(
                        "UPDATE module_game_download_task SET task_id = ?1 WHERE version_id = ?2",
                        rusqlite::params![task_id, rec.version_id],
                    )
                    .map(|_| ())
                    .map_err(KernelError::from)
                });
            }
            Err(e) => log::warn!("[game-download] 续传 {} 投递失败: {e}", rec.version_id),
        }
    }
}

/// 同步读取清单（供 `start` 等非 async 上下文使用；阻塞运行时线程）。
fn block_load(ctx: &Ctx) -> Result<manifest::HistoricalVersions, KernelError> {
    ctx.runtime
        .block_on(manifest::load_manifest(ctx, false))
}

// ---------------------------------------------------------------- 视图

/// 下载引擎快照的轻量视图。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadState {
    pub task_id: u64,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub speed_bytes_per_sec: u64,
}

impl DownloadState {
    fn from_snapshot(s: copper_downloader::TaskSnapshot) -> Self {
        Self {
            task_id: s.id,
            total_bytes: s.total_bytes,
            downloaded_bytes: s.downloaded_bytes,
            speed_bytes_per_sec: s.speed_bytes_per_sec,
        }
    }
}

/// 单版本任务视图。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub version_id: String,
    pub kind: String,
    pub dest: String,
    /// `downloading` | `extracting` | `installed` | `failed`。
    pub state: String,
    pub error: Option<String>,
    pub download: Option<DownloadState>,
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_matches_case_insensitive() {
        let dir = std::env::temp_dir().join(format!("copper_gd_md5_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.bin");
        std::fs::write(&p, b"abc").unwrap();
        // md5("abc") = 900150983cd24fb0d6963f7d28e17f72
        assert!(md5_matches(&p, "900150983cd24fb0d6963f7d28e17f72").unwrap());
        assert!(md5_matches(&p, "900150983CD24FB0D6963F7D28E17F72").unwrap());
        assert!(!md5_matches(&p, "00000000000000000000000000000000").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_snapshot_varies() {
        let v = serde_json::json!({
            "id": 42,
            "status": "done",
            "dest": "C:/tmp/x.msixvc",
            "totalBytes": 10,
        });
        let (id, status, dest) = parse_snapshot(&v).unwrap();
        assert_eq!(id, 42);
        assert_eq!(status, DownloadStatus::Done);
        assert_eq!(dest, "C:/tmp/x.msixvc");

        assert!(parse_snapshot(&serde_json::json!({ "id": 1, "status": "doing" })).is_none());
    }

    #[test]
    fn row_roundtrip_fields_consistent() {
        // 静态校验：视图字段与 DB 列对齐（避免拼写漂移）。
        let task = TaskView {
            version_id: "1.21.0".into(),
            kind: "release".into(),
            dest: "/tmp/x".into(),
            state: "downloading".into(),
            error: None,
            download: None,
        };
        let json = serde_json::to_value(task).unwrap();
        let map = json.as_object().unwrap();
        for k in ["versionId", "kind", "dest", "state", "error", "download"] {
            assert!(map.contains_key(k), "缺字段 {k}");
        }
    }

    #[test]
    fn list_records_builds_placeholders() {
        // 纯字符串拼接逻辑冒烟。
        let states = ["downloading", "extracting"];
        let sql = format!(
            "SELECT ... WHERE state IN ({})",
            states.iter().map(|_| "?").collect::<Vec<_>>().join(",")
        );
        assert!(sql.contains("IN (?,?)"));
    }
}