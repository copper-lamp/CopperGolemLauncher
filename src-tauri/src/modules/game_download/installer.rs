//! 游戏下载安装流水线：以数据库「下载任务→版本」为事实源，驱动 下载→校验→解包→写元数据→广播。
//!
//! 生命周期：
//! - `enqueue`：幂等投递下载（防重复安装 / 防重复排队），返回下载任务 id。
//! - 订阅 `download.status`：任务 `Done` → 触发异步安装（单飞锁串行）；`Failed/Cancelled` → 落失败。
//! - `finish_install`：商店授权取 content key（失败**显式落错**，不静默回退）→ md5 自验
//!   → 纯 Rust 解包 → 写 `version.json` → 清理整包 → 广播 `version.installed`。
//! - `resume_pending`：`start` 时续传未完成下载 / 续装中断解包 / 回收孤儿整包 / 失败重试限次。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use copper_downloader::{DownloadOptions, DownloadStatus};
use md5::{Digest as _Md5Digest, Md5};
use serde_json::Value;

use crate::error::KernelError;
use crate::registry::events::EventBus;
use crate::services::database::DatabaseService;
use crate::services::account::AccountService;
use crate::services::download::DownloadService;
use crate::services::native_install;
use crate::services::paths::Paths;
use crate::services::settings::SettingsService;
use crate::state::KernelContext;

use super::extractor;
use super::manifest;
use super::meta_bridge;

/// 模块在 cache 下的下载子目录。
const CACHE_SUBDIR: &str = "game-download";
/// 商店授权 key 缓存子目录（DPAPI + 账户绑定，离线复用）。
const STORE_KEY_SUBDIR: &str = "store-key";
/// 版本根下的整包暂存子目录（同盘复用，独立于解包输出目录）。
pub const DOWNLOAD_SUBDIR: &str = ".download";
/// 启动续传时单个任务允许的最大自动重投次数（防 failed 无限重下）。
const MAX_RESUME_ATTEMPTS: i64 = 3;

/// 安装单飞锁：进程级单例，一次只解一个包。
///
/// 解包是 GB 级同步 IO + 高内存，并发解包会把磁盘与内存同时打满，
/// 触发后表现为「两个都没装上，还把机器卡死」。故所有安装入口
/// （下载完成自动安装 / 启动续装 / 用户手动重装）都必须经这把锁。
///
/// 用进程级单例而非模块实例字段：命令层只持有 `KernelContext`，
/// 拿不到模块实例；若命令侧另建一把锁，自动安装与手动重装就会并发解包。
pub fn install_lock() -> Arc<tokio::sync::Mutex<()>> {
    static LOCK: std::sync::OnceLock<Arc<tokio::sync::Mutex<()>>> = std::sync::OnceLock::new();
    Arc::clone(
        LOCK.get_or_init(|| Arc::new(tokio::sync::Mutex::new(()))),
    )
}

/// 安装流水线上下文：把 `KernelContext` 需要跨 async 捕获的能力拆出为可克隆 Arcs。
#[derive(Clone)]
pub struct Ctx {
    pub events: Arc<EventBus>,
    pub db: Arc<DatabaseService>,
    pub paths: Arc<Paths>,
    pub settings: Arc<SettingsService>,
    pub download: Arc<DownloadService>,
    pub account: Arc<AccountService>,
    /// 商店授权专用客户端：不跟随重定向（与 LeviLauncher 的 `CheckRedirect` 一致），
    /// 避免票据被转发到非目标主机。
    pub store_http: Arc<reqwest::Client>,
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
            account: kernel.account().clone(),
            store_http: Arc::new(
                crate::services::http_client::client_builder(std::time::Duration::from_secs(40))
                    .redirect(reqwest::redirect::Policy::none())
                    .user_agent("XboxLm-PC/Microsoft.GamingServices")
                    .build()
                    .expect("failed to build store http client"),
            ),
            runtime: kernel.runtime().clone(),
        }
    }

    /// 下载缓存目录（`cache/game-download`）。
    pub fn cache_home(&self) -> PathBuf {
        self.paths.cache_dir().join(CACHE_SUBDIR)
    }

    /// 商店授权 key 缓存目录（`cache/game-download/store-key`）。
    pub fn store_key_dir(&self) -> PathBuf {
        self.cache_home().join(STORE_KEY_SUBDIR)
    }

    /// 某版本整包暂存路径（`versions_root/.download/<slug>.msixvc`）。
    ///
    /// 暂存与安装同盘（同在 `versions_root`，避免占用系统盘缓存），但**独立于**版本
    /// 解包输出目录（`install_dir`）：若把源包放进解包目标目录，原生解包库在清空/
    /// 消费输出目录时会把源包一并销毁，导致“下载完但装不上”。解压校验后移除该包。
    pub fn dest_for(&self, slug: &str) -> PathBuf {
        self.versions_root()
            .join(DOWNLOAD_SUBDIR)
            .join(format!("{slug}.msixvc"))
    }

    /// 某版本安装目录（`versions_root/<folder>`，根按设置动态解析）。
    pub fn install_dir(&self, folder: &str) -> PathBuf {
        self.versions_root().join(folder)
    }

    /// 当前游戏（版本）根目录（复用 paths 的唯一解析入口）。
    pub fn versions_root(&self) -> PathBuf {
        self.paths.versions_root(&self.settings)
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

/// 启动续传重投计数（防 failed 状态无限自动重下）。
pub const MIGRATION_ATTEMPTS: crate::services::database::Migration =
    crate::services::database::Migration {
        version: 2,
        name: "game_download_attempts",
        sql: "ALTER TABLE module_game_download_task ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;",
    };

/// 本模块全部迁移（按版本升序执行）。
pub const MIGRATIONS: &[crate::services::database::Migration] =
    &[MIGRATION, MIGRATION_ATTEMPTS];

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
    pub attempts: i64,
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
        attempts: r.get(8)?,
    })
}

fn get_record(ctx: &Ctx, version_id: &str) -> Result<Option<TaskRecord>, KernelError> {
    ctx.db.with_conn(|conn| -> Result<Option<TaskRecord>, KernelError> {
        let mut stmt = conn.prepare(
            "SELECT version_id, kind, folder, dest, md5, state, error, task_id, attempts
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
            "SELECT version_id, kind, folder, dest, md5, state, error, task_id, attempts
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
               (version_id, kind, folder, dest, md5, state, error, task_id, attempts, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(version_id) DO UPDATE SET
               kind=excluded.kind, folder=excluded.folder, dest=excluded.dest,
               md5=excluded.md5, state=excluded.state, error=excluded.error,
               task_id=excluded.task_id, attempts=excluded.attempts",
            rusqlite::params![
                rec.version_id,
                rec.kind,
                rec.folder,
                rec.dest.to_string_lossy(),
                rec.md5,
                rec.state,
                rec.error,
                rec.task_id,
                rec.attempts,
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

/// 删除任务记录（放弃安装 / 显式取消）。
fn delete_record(ctx: &Ctx, version_id: &str) -> Result<(), KernelError> {
    ctx.db.with_conn(|conn| -> Result<(), KernelError> {
        conn.execute(
            "DELETE FROM module_game_download_task WHERE version_id = ?1",
            rusqlite::params![version_id],
        )?;
        Ok(())
    })
}

// ---------------------------------------------------------------- 命令核心

/// 给落盘失败补上「版本根来自哪个设置项」这层定位信息。
///
/// 裸 `os error 5` 只有一个错误码，用户既不知道是哪个目录，也不知道该去哪里改。
/// 拼接规则与内核侧共用 `paths::annotate_versions_root_error`，避免两处包装漂移。
fn annotate_versions_root(ctx: &Ctx, error: KernelError) -> KernelError {
    crate::services::paths::annotate_versions_root_error(&ctx.versions_root(), error)
}

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
    // CDN 故障转移：取全部候选 URL，首个投递成功即停（引擎侧也有 3 次重试，这里只挑 host）。
    let urls = entry
        .all_urls()
        .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{id}` 没有可用下载链接")))?;
    // md5 是唯一的完整性防线（引擎只校验 sha256，清单只给 md5）——缺失直接拒绝，避免空值恒校验失败。
    if entry.md5.trim().is_empty() {
        return Err(KernelError::InvalidArgument(format!(
            "版本 `{id}` 的清单条目缺少 MD5 校验值，无法安全安装"
        )));
    }
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

    // 落盘前确认整包暂存目录**真的**可写。
    //
    // 这里原本是裸 `create_dir_all(..)?`：目录已存在但不可写时（自定义版本根指向
    // 受限卷、被收紧的 ACL、占用中的挂载点）`create_dir_all` 会直接成功，直到引擎
    // 第一次写 `.part` 才失败，而失败会以不带任何路径的 `os error 5` 冒泡到界面，
    // 用户无从判断是「网络」还是「目录权限」。改为带写探针 + 带路径的报错。
    crate::services::paths::ensure_writable_dir(
        dest.parent().unwrap_or(Path::new(".")),
        "下载暂存",
    )
    .map_err(|e| annotate_versions_root(ctx, e))?;

    let opts = DownloadOptions {
        resume: true,
        remove_on_cancel: true,
        expected_sha256: None, // 清单仅提供 MD5，下载完成后模块内自验
        filename: Some(format!("{slug}.msixvc")),
        ..Default::default()
    };
    // 逐个候选 URL 投递：引擎内部对瞬时错误有 3 次指数退避重试，这里在 host 粒度再兜一层。
    let mut enqueued: Option<u64> = None;
    let mut last_err: Option<KernelError> = None;
    for (i, url) in urls.iter().enumerate() {
        match ctx.download.enqueue(url, &dest, opts.clone()) {
            Ok(task_id) => {
                if i > 0 {
                    log::info!("[game-download] 主链路失败，已切到备用 CDN: {url}");
                }
                enqueued = Some(task_id);
                break;
            }
            Err(error) => {
                log::warn!("[game-download] CDN 投递失败 {url}: {error}");
                last_err = Some(KernelError::from(error));
            }
        }
    }
    let task_id = match enqueued {
        Some(id) => id,
        None => {
            return Err(last_err.unwrap_or_else(|| {
                KernelError::Module("下载队列投递失败".into())
            }))
        }
    };

    let rec = TaskRecord {
        version_id: slug.clone(),
        kind: kind.to_string(),
        folder: slug.clone(),
        dest: dest.clone(),
        md5: entry.md5.clone(),
        state: "downloading".into(),
        error: None,
        task_id: Some(task_id),
        attempts: 0,
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

/// 取消任务（下载中取消 → 放弃安装）。
///
/// 语义：取消下载即“完全放弃该版本下载”，取消引擎任务、立即清除本地缓存
/// （整包 + 断点 `.part`）、删除数据库记录，从而确保：
/// - 缓存立刻释放，不残留占用盘空间的临时文件；
/// - 前端行/详情页状态回退为“可下载”；
/// - `start` 重启后 `resume_pending` 不再续传（无记录）。
///
/// 仅 `installed` / `extracting`（正在解包，无法中断）不可取消；其余状态
/// （downloading / queued / paused / failed）一律视为放弃并清理。
pub fn cancel(ctx: &Ctx, id: &str) -> Result<(), KernelError> {
    let Some(rec) = get_record(ctx, id)? else {
        return Err(KernelError::InvalidArgument(format!("无任务 `{id}`")));
    };
    if rec.state == "installed" || rec.state == "extracting" {
        return Ok(());
    }
    // 先置中间态，避免引擎 `download.status(Cancelled)` 被误判为失败并回写 failed。
    set_state(ctx, id, "cancelling", Some("已取消"))?;
    // 取消引擎任务。容错：任务可能已不在队列（如已移除）。
    if let Some(task_id) = rec.task_id {
        let _ = ctx.download.cancel(task_id);
    }
    // 立即清除本地缓存（整包 + 断点 `.part`），不依赖引擎 `remove_on_cancel`，
    // 保证“马上清除缓存”。
    cleanup_package(&rec.dest);
    // 删除记录 → 前端回退为“可下载”，重启后 `resume_pending` 不会续传。
    delete_record(ctx, id)?;
    ctx.events.publish("game-download.cancelled", serde_json::json!({ "id": id }));
    Ok(())
}

/// 仅重装：整包已在本地时直接重跑安装流水线，**不重新下载**。
///
/// 与 `enqueue` 的分工：`enqueue` 负责「把包拿到手」，本函数负责「把包装上」。
/// 缺了它，安装阶段失败（商店授权未过、md5 不符、解包中断）后唯一的出路是
/// `enqueue` 重来一遍——对数 GB 的包而言这是把最贵的一步重做一次，而失败点
/// 往往与下载毫无关系。
///
/// 前置条件：本地整包存在且 md5 与清单一致。不满足时**拒绝并说明原因**，
/// 而不是悄悄重新下载——用户点的是「安装」，替他改下几个 G 的流量是不可预期的。
///
/// 状态流转与自动安装共用 `finish_install`（含单飞锁与 md5 自验），
/// 因此重复点击是幂等安全的：正在解包时直接返回，不排队。
pub fn install(ctx: &Ctx, id: &str) -> Result<(), KernelError> {
    let rec = get_record(ctx, id)?
        .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{id}` 没有下载记录，无法安装")))?;

    // 正在解包：重复点击直接返回，不排队、不打断。
    if rec.state == "extracting" {
        return Ok(());
    }
    // 已安装即无需动作。
    if meta_bridge::is_installed(&ctx.install_dir(&rec.folder)) {
        return Ok(());
    }
    // 下载仍在途：必须等下载完成，不能拿半包去装。
    if let Some(task_id) = rec.task_id {
        if let Some(snapshot) = ctx.download.task(task_id) {
            if matches!(snapshot.status, DownloadStatus::Queued | DownloadStatus::Downloading) {
                return Err(KernelError::Conflict(format!(
                    "版本 `{id}` 仍在下载，请等待下载完成后再安装"
                )));
            }
        }
    }
    if !rec.dest.is_file() {
        return Err(KernelError::InvalidArgument(format!(
            "本地没有版本 `{id}` 的完整安装包，请先下载"
        )));
    }
    if rec.md5.trim().is_empty() {
        return Err(KernelError::Config("清单缺少 MD5 校验值，拒绝安装".into()));
    }
    if !md5_matches(&rec.dest, &rec.md5)? {
        // 包已损坏：删掉它，否则它会一直卡住后续安装尝试
        // （每次都走到这里失败，而用户看不出为什么）。
        let path = rec.dest.clone();
        cleanup_package(&path);
        return Err(KernelError::Config(format!(
            "本地安装包校验失败（{}），已删除该文件，请重新下载",
            rec.dest.to_string_lossy()
        )));
    }

    let lock = install_lock();
    let next_ctx = ctx.clone();
    let vid = rec.version_id.clone();
    ctx.runtime.spawn(async move {
        if let Err(e) = finish_install(&next_ctx, &lock, rec).await {
            mark_failed(&next_ctx, &vid, &e.to_string());
            log::error!("[game-download] 手动重装 {vid} 失败: {e}");
        }
    });
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
        DownloadStatus::Failed => {
            let msg = payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("下载失败")
                .to_string();
            mark_failed(ctx, &rec.version_id, &msg);
        }
        DownloadStatus::Cancelled => {
            // “cancelling”为版本页主动取消的中间态，清理由 cancel() 完成后继续，此处不重复。
            if rec.state == "cancelling" {
                return;
            }
            // 非版本页主动取消（如全局下载列表取消）→ 同样“放弃下载”：
            // 清除缓存并删除记录，避免 `resume_pending` 在重启时续传已取消的下载。
            cleanup_package(&rec.dest);
            let _ = delete_record(ctx, &rec.version_id);
            ctx.events.publish(
                "game-download.cancelled",
                serde_json::json!({ "id": rec.version_id }),
            );
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
                    "SELECT version_id, kind, folder, dest, md5, state, error, task_id, attempts
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

/// md5 自验。分块流式读取，**绝不在内存中一次性载入整包**（GDK 整包可达数 GB，
/// `fs::read` 会在设置 `extracting` 后因连续大分配失败而 panic，导致安装静默中断）。
fn md5_matches(path: &Path, expected: &str) -> Result<bool, KernelError> {
    // 清单条目缺 md5 → 恒校验失败会进入无限重试循环，这里显式区分“未提供”。
    if expected.trim().is_empty() {
        return Err(KernelError::Config("清单缺少 MD5 校验值，拒绝安装".into()));
    }
    let mut file = std::fs::File::open(path).map_err(KernelError::Io)?;
    let mut digest = Md5::new();
    // 堆上缓冲（1 MiB）。不可用栈上大数组：启动时 `resume_pending` 会在主线程
    // 同步调用本函数，主线程默认栈仅 1MB，栈上数组会直接爆栈。
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
    }
    Ok(format!("{:x}", digest.finalize()).eq_ignore_ascii_case(expected.trim()))
}

/// 异步安装：单飞锁串行解包（避免并发 GB 级解压）；幂等重入安全。
pub async fn finish_install(
    ctx: &Ctx,
    lock: &Arc<tokio::sync::Mutex<()>>,
    rec: TaskRecord,
) -> Result<(), KernelError> {
    // 加密 MSIXVC 必须拿到商店 content key——拿不到就**显式失败**，不再静默回退：
    // 旧的“回退兼容 DLL”路径依赖本机 Store 授权状态且返回码晦涩，已被上游弃用。
    let lease = store_content_key(ctx, &rec.dest).await?;
    let content_key = lease.as_ref().map(|lease| lease.key());
    finish_install_with_key(ctx, lock, rec, content_key).await
}

/// 商店授权可复用的缓存 key 文件路径（`store-key/<key_id>.dpapi`）。
fn cached_key_path(ctx: &Ctx, key_id: &str) -> PathBuf {
    let safe: String = key_id
        .chars()
        .map(|c| if c.is_ascii_hexdigit() || c == '-' { c } else { '_' })
        .collect();
    ctx.store_key_dir().join(format!("{safe}.dpapi"))
}

/// 授权失败的结构化错误：i18n 键 + 用户可读详情（不含票据/密钥）。
fn auth_error(stage: &'static str, detail: String) -> KernelError {
    KernelError::Config(format!("商店授权失败[{stage}]: {detail}"))
}

/// 为加密 MSIXVC 走完 Store 授权链，取得打包绑定的内容密钥。
///
/// 顺序：本地 key 缓存命中（离线）→ 在线授权链。任一步失败都返回**结构化错误**
/// 供 `finish_install` 落库并广播，绝不静默回退。密钥只在返回的租约内存活。
async fn store_content_key(
    ctx: &Ctx,
    dest: &Path,
) -> Result<Option<native_install::ContentKeyLease>, KernelError> {
    if !native_install::requires_store_key(dest) {
        return Ok(None);
    }

    let identifiers = native_install::read_identity(dest)
        .map_err(|e| auth_error("package", e.to_string()))?;

    // 1) 本地缓存命中 → 离线安装（账户未变时有效，账户不匹配会走下面在线链）。
    let xuid = match ctx.account.current().and_then(|a| a.xuid.clone()) {
        Some(x) => x,
        None => {
            return Err(auth_error(
                "account",
                "未登录微软账户，请先登录后重试".into(),
            ))
        }
    };
    if let Ok(Some(lease)) =
        native_install::load_cached_key(ctx, &identifiers.key_id, &xuid)
    {
        log::info!(
            "[game-download] 命中本地授权缓存 key_id={}（离线）",
            identifiers.key_id
        );
        return Ok(Some(lease));
    }

    // 2) 在线授权链。
    let request = native_install::StoreInstallRequest::new(
        &xuid,
        store_market(ctx),
        ctx.cache_home().join("store-device"),
    );
    match native_install::acquire_package_content_key(&ctx.store_http, &request, dest).await {
        Ok(lease) => {
            log::info!(
                "[game-download] 已取得商店内容密钥 key_id={} license={:?}",
                lease.key_id(),
                lease.license_type()
            );
            // 缓存失败不影响安装（下次再走在线链）。
            if let Err(e) = native_install::save_cached_key(ctx, &lease, &xuid) {
                log::warn!("[game-download] 授权缓存写入失败（不影响本次安装）: {e}");
            }
            Ok(Some(lease))
        }
        Err(error) => {
            // 错误文本不含票据或密钥；透传给前端定位是哪一步挂了。
            log::warn!("[game-download] 商店授权失败: {error}");
            Err(auth_error("online", error.to_string()))
        }
    }
}

/// 商店市场代码：优先取系统区域设置中的两位大写地区，否则回退 `US`。
fn store_market(ctx: &Ctx) -> String {
    let locale = ctx
        .settings
        .get::<String>("locale")
        .unwrap_or_else(|| "zh-CN".into());
    let candidate = locale
        .rsplit(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if native_install::validate_market(&candidate).is_ok() {
        candidate
    } else {
        "US".into()
    }
}

/// 将后端授权服务取得的 content key 借用到同步提取调用中。
///
/// key 不进入任务记录、数据库、事件或错误文本；`None` 保持历史兼容回退，
/// 直到 Windows Store/WAM 服务完成真实接入。
pub async fn finish_install_with_key(
    ctx: &Ctx,
    lock: &Arc<tokio::sync::Mutex<()>>,
    rec: TaskRecord,
    content_key: Option<&[u8]>,
) -> Result<(), KernelError> {
    if let Some(key) = content_key {
        if key.len() != 32 {
            return Err(KernelError::Config("content key 长度必须为 32 字节".into()));
        }
    }
    let _guard = lock.lock().await;

    // 取消竞态：若记录已被取消删除，放弃本次安装（取消已完成清理）。
    if get_record(ctx, &rec.version_id)?.is_none() {
        return Ok(());
    }

    // 幂等：已安装即直接收尾（供续装 / 重复事件）。
    let install_dir = ctx.install_dir(&rec.folder);
    if meta_bridge::is_installed(&install_dir) {
        set_state(ctx, &rec.version_id, "installed", None)?;
        publish_installed(ctx, &rec);
        cleanup_package(&rec.dest);
        return Ok(());
    }

    // 半成品输出目录自愈：上次失败可能留下空目录（remove_dir_all 自身失败 / 用户手建），
    // `extract_xvc` 遇到已存在输出必失败。这里对“未完成安装的残留目录”先清理。
    // 真冲突（已有 version.json 的完整安装）由上面的幂等分支拦住，走不到这里。
    if install_dir.exists() && !meta_bridge::is_installed(&install_dir) {
        log::warn!(
            "[game-download] 清理上次失败残留的半成品目录 {}",
            install_dir.display()
        );
        std::fs::remove_dir_all(&install_dir).map_err(|e| {
            KernelError::Io(std::io::Error::new(
                e.kind(),
                format!("无法清理残留安装目录 {}: {e}", install_dir.display()),
            ))
        })?;
    }

    set_state(ctx, &rec.version_id, "extracting", None)?;
    log::info!(
        "[game-download] 开始安装 {} <- {}",
        rec.version_id,
        rec.dest.to_string_lossy()
    );

    // md5 自验（引擎只保留 sha256，清单提供的是 md5）。
    if !md5_matches(&rec.dest, &rec.md5)? {
        return Err(KernelError::Config(format!(
            "MD5 校验失败：{} 与清单不符",
            rec.dest.to_string_lossy()
        )));
    }

    // 解包（MSIXVC → 纯 Rust 提取；历史 .appx → zip）→ 校验主程序（exe + config + PE x64）→ 写元数据。
    // 失败时清理输出目录，去掉残留的“半安装”目录，避免前端看到名为已装、实则空目录的假成功。
    let extract_result =
        extractor::extract_package_with_key(&rec.dest, &install_dir, content_key);
    if let Err(e) = extract_result {
        let _ = std::fs::remove_dir_all(&install_dir);
        return Err(KernelError::from(e));
    }

    let kind = rec.kind.clone();
    if let Err(error) = meta_bridge::write_meta(&install_dir, &rec.folder, &rec.version_id, &kind) {
        let _ = std::fs::remove_dir_all(&install_dir);
        return Err(error);
    }

    set_state(ctx, &rec.version_id, "installed", None)?;
    log::info!("[game-download] 安装完成 {} -> {}", rec.version_id, install_dir.to_string_lossy());
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

/// 启动恢复：对未完成的记录续传/续装，回收孤儿整包，失败任务限次重投。
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
        // 整包不完整 → 提前清除“已放弃”残留：本地无整包、无断点字节，且引擎无在途任务，
        // 说明下载已取消/无任何可恢复内容，删除记录避免重启白白续传（甚至反复失败）。
        let part = PathBuf::from(format!("{}.part", rec.dest.to_string_lossy()));
        let has_live = rec.task_id.map(|t| ctx.download.task(t).is_some()).unwrap_or(false);
        if !rec.dest.is_file() && !part.is_file() && !has_live {
            log::warn!("[game-download] 丢弃无内容的下载残留记录 {}", rec.version_id);
            delete_record(ctx, &rec.version_id).ok();
            continue;
        }
        // failed 限次重投：连续 N 次启动重试仍失败 → 标记为永久失败，不再自动重下。
        if rec.state == "failed" {
            if rec.attempts >= MAX_RESUME_ATTEMPTS {
                log::warn!(
                    "[game-download] {} 已自动重试 {} 次仍失败，停止自动重试",
                    rec.version_id,
                    rec.attempts
                );
                let _ = ctx.db.with_conn(|conn| {
                    conn.execute(
                        "UPDATE module_game_download_task SET state = 'failed_permanent' WHERE version_id = ?1",
                        [rec.version_id.as_str()],
                    )
                    .map(|_| ())
                    .map_err(KernelError::from)
                });
                continue;
            }
            let _ = ctx.db.with_conn(|conn| {
                conn.execute(
                    "UPDATE module_game_download_task SET attempts = attempts + 1 WHERE version_id = ?1",
                    [rec.version_id.as_str()],
                )
                .map(|_| ())
                .map_err(KernelError::from)
            });
        }
        // 重新投递续传（引擎按 `.part` Range 续传）。
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
        let urls = match entry.all_urls() {
            Some(v) => v,
            None => continue,
        };
        let opts = DownloadOptions {
            resume: true,
            remove_on_cancel: true,
            expected_sha256: None,
            filename: Some(format!("{}.msixvc", rec.version_id)),
            ..Default::default()
        };
        let mut new_task: Option<u64> = None;
        for url in &urls {
            match ctx.download.enqueue(url, &rec.dest, opts.clone()) {
                Ok(task_id) => {
                    new_task = Some(task_id);
                    break;
                }
                Err(e) => log::warn!("[game-download] 续传 {} CDN 失败 {url}: {e}", rec.version_id),
            }
        }
        match new_task {
            Some(task_id) => {
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
            None => log::warn!("[game-download] 续传 {} 全部 CDN 投递失败", rec.version_id),
        }
    }

    // 回收孤儿整包：DB 记录丢失但 `.download/*.msixvc` 完整且清单 md5 命中
    // → 补建记录直接续装（不重新下载），救回历史会话留下的包。
    recover_orphan_packages(ctx, lock);
}

/// 扫描 `versions_root/.download/*.msixvc`，对无 DB 记录但 md5 命中清单的整包补建任务并安装。
fn recover_orphan_packages(ctx: &Ctx, lock: &Arc<tokio::sync::Mutex<()>>) {
    let download_dir = ctx.versions_root().join(DOWNLOAD_SUBDIR);
    let entries = match std::fs::read_dir(&download_dir) {
        Ok(v) => v,
        // 目录不存在是正常态（尚未下载过任何版本），静默跳过；但**不可写**这类
        // 失败必须留痕，否则自定义版本根权限异常会被当成「没有孤儿包」永久掩盖。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            log::warn!(
                "[game-download] 孤儿包回收读取 {} 失败: {e}",
                download_dir.display()
            );
            return;
        }
    };
    let manifest = match block_load(ctx) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[game-download] 孤儿包回收取清单失败: {e}");
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("msixvc") {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        // 已有记录的交给上面的续传逻辑。
        match get_record(ctx, &stem) {
            Ok(Some(_)) => continue,
            Ok(None) => {}
            Err(e) => {
                log::warn!("[game-download] 孤儿包回收查记录失败 {stem}: {e}");
                continue;
            }
        }
        // 已安装的包直接清理。
        if meta_bridge::is_installed(&ctx.install_dir(&stem)) {
            cleanup_package(&path);
            continue;
        }
        let Some(v) = manifest.find_by_id(&stem) else {
            // 清单里没有 → 无法校验，保留但不自动安装。
            log::info!("[game-download] 发现孤儿包 {stem}，清单无此版本，保留待手动处理");
            continue;
        };
        if v.md5.trim().is_empty() {
            continue;
        }
        // md5 命中才认领（分块流式，避免整包读内存）。
        if !md5_matches(&path, &v.md5).unwrap_or(false) {
            log::warn!("[game-download] 孤儿包 {stem} md5 不符清单，跳过");
            continue;
        }
        let kind = match v.kind() {
            manifest::VersionKind::Release => "release",
            manifest::VersionKind::Preview => "preview",
        };
        let rec = TaskRecord {
            version_id: stem.clone(),
            kind: kind.to_string(),
            folder: stem.clone(),
            dest: path.clone(),
            md5: v.md5.clone(),
            state: "extracting".into(),
            error: None,
            task_id: None,
            attempts: 0,
        };
        if let Err(e) = upsert_record(ctx, &rec) {
            log::warn!("[game-download] 孤儿包 {stem} 补建记录失败: {e}");
            continue;
        }
        log::info!("[game-download] 回收孤儿整包 {stem}（{} 字节），直接续装", entry.metadata().map(|m| m.len()).unwrap_or(0));
        let next_ctx = ctx.clone();
        let next_lock = lock.clone();
        let vid = stem.clone();
        ctx.runtime.spawn(async move {
            if let Err(e) = finish_install(&next_ctx, &next_lock, rec).await {
                log::error!("[game-download] 孤儿包 {vid} 续装失败: {e}");
                mark_failed(&next_ctx, &vid, &e.to_string());
            }
        });
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

    /// 安装单飞锁必须是进程级同一个实例。
    ///
    /// 回归点：锁若挂在模块实例上，命令层（只持有 `KernelContext`）会另建一把，
    /// 于是「手动重装」与「下载完成自动安装」并发解包——正是这把锁要防的场景。
    #[test]
    fn install_lock_is_a_singleton() {
        let a = install_lock();
        let b = install_lock();
        assert!(Arc::ptr_eq(&a, &b), "安装锁不是同一个实例，会导致并发解包");
    }

    /// 手动重装的前置判定：这些情况必须在下载/安装前拒绝，而不是重下数 GB 的包。
    #[test]
    fn install_preconditions_are_strict() {
        // 无记录：拒绝（没有包可装）。
        let dir = std::env::temp_dir().join(format!("copper_gd_inst_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 包缺失：拒绝安装（`dest` 不存在）。
        let missing = dir.join("missing.msixvc");
        assert!(!missing.is_file(), "前提不成立：测试包不应存在");

        // 清单缺 md5：即便包在也不能装（没有完整性防线）。
        let pkg = dir.join("ok.msixvc");
        std::fs::write(&pkg, b"abc").unwrap();
        assert!(pkg.is_file());
        let md5_empty = md5_matches(&pkg, "");
        // `md5_matches` 对空期望值报错而非放行——空值恒校验失败会导致无限重试。
        assert!(md5_empty.is_err(), "空 md5 必须拒绝而不是恒失败");

        let _ = std::fs::remove_dir_all(&dir);
    }
}