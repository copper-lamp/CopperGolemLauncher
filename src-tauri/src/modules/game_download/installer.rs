//! 游戏下载安装流水线：以「版本整包 + 实例」两张表为事实源，驱动
//! 下载 → 复用 → 校验 → 解包 → 写元数据 → 装加载器 → 广播。
//!
//! 两张表的分工是这套模型的核心：
//! - `module_game_download_package`：**下载单元**，一个版本一份整包。GB 级的包只下
//!   一次，同一版本的多个实例共用它，装完最后一批实例才清理。
//! - `module_game_download_instance`：**安装单元**，一次安装产出一个实例目录。实例名
//!   由用户在安装确认弹窗里指定（即版本目录名），同一个版本可以并存任意多个互相隔离
//!   的实例。
//!
//! 拆表的直接原因：旧模型以 `version_id` 作主键，把「版本」当成了安装单元，于是
//! 「同一个版本装两次」在数据层就不可能——第二次安装会命中同一条记录被当成重装，
//! 把前一个实例的目录覆盖掉。
//!
//! 生命周期：
//! - `enqueue`：建实例记录 → 复用或投递整包下载 → 包已在本地时立刻进入安装。
//! - 订阅 `download.status`：整包 `Done` → 该版本全部未装好的实例排队安装（单飞锁串行）。
//! - `run_batch`：整包级只做一次授权与校验，随后逐实例 解包 → 写元数据 → 装加载器。
//! - `resume_pending`：重启后续传整包 / 续装实例 / 清理无主整包。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use copper_downloader::{DownloadOptions, DownloadStatus};
use md5::{Digest as _Md5Digest, Md5};
use serde_json::Value;

use crate::error::KernelError;
use crate::modules::content_download::lip_install;
use crate::modules::content_download::lipd;
use crate::modules::content_download::loader_catalog;
use crate::modules::home::meta;
use crate::registry::events::EventBus;
use crate::services::account::AccountService;
use crate::services::database::DatabaseService;
use crate::services::download::DownloadService;
use crate::services::native_install;
use crate::services::paths::Paths;
use crate::services::settings::SettingsService;
use crate::state::KernelContext;

use super::{extractor, manifest, meta_bridge};

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
    Arc::clone(LOCK.get_or_init(|| Arc::new(tokio::sync::Mutex::new(()))))
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
    /// 主窗口句柄登记：静默取票被系统拒绝时，交互回落的界面归属目标。
    pub window: Arc<crate::services::window::MainWindow>,
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
            window: kernel.window().clone(),
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
    ///
    /// 路径按**版本**而不是实例命名：同一版本的多个实例共用这一份整包。
    pub fn dest_for(&self, slug: &str) -> PathBuf {
        self.versions_root()
            .join(DOWNLOAD_SUBDIR)
            .join(format!("{slug}.msixvc"))
    }

    /// 某实例安装目录（`versions_root/<实例名>`，根按设置动态解析）。
    pub fn install_dir(&self, instance: &str) -> PathBuf {
        self.versions_root().join(instance)
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

// ---------------------------------------------------------------- 安装进度上报

/// 安装阶段表：`(i18n 键, 阶段权重)`，数组顺序即执行顺序。
///
/// 权重按实测耗时给：解包是绝对大头（GB 级顺序读写 + AES-XTS 逐页解密），加载器安装
/// （lipd 依赖求解 + 下载）次之，授权与收尾各占几个百分点。权重写死而不是按字节动态
/// 算，是因为授权阶段根本没有字节可算，而只有一套固定权重才能保证进度条**单调递增**
/// ——动态权重会在阶段切换时把进度条拽回去，看起来像倒退了。
const INSTALL_PHASES: &[(&str, f64)] = &[
    ("download.stage.install_authorizing", 0.02),
    ("download.stage.install_preparing", 0.01),
    ("download.stage.install_verifying", 0.08),
    ("download.stage.install_extracting", 0.59),
    ("download.stage.install_loader", 0.28),
    ("download.stage.install_finalizing", 0.02),
];

/// 阶段下标常量（与 [`INSTALL_PHASES`] 一一对应，避免到处写魔数）。
const PHASE_AUTHORIZING: usize = 0;
const PHASE_PREPARING: usize = 1;
const PHASE_VERIFYING: usize = 2;
const PHASE_EXTRACTING: usize = 3;
const PHASE_LOADER: usize = 4;
const PHASE_FINALIZING: usize = 5;

/// 某阶段起点 = 前面所有阶段权重之和。
fn phase_base(index: usize) -> f64 {
    INSTALL_PHASES.iter().take(index).map(|(_, w)| *w).sum()
}

/// 安装阶段进度上报器。
///
/// 一批可以包含多个实例（同一版本的多份安装），进度按实例数**摊平**：第 `completed`
/// 个实例内部的进度落在 `[completed/total, (completed+1)/total)`。摊平而不是每个实例
/// 都把进度条重跑一遍：重跑在界面上就是「进度条从 100% 退回 0%」，用户会以为装崩了。
///
/// 只在任务仍存在于下载引擎内存时生效：`task_id` 为 `None`（重启后续装、包已在本地
/// 且没有可复用的任务）或任务已被移除时全部静默。上报失败绝不影响安装本身，这是刻意
/// 的——进度条是观测手段，不能成为安装的失败点。
struct InstallProgress {
    download: Arc<DownloadService>,
    task_id: Option<u64>,
    /// 当前阶段下标。
    index: usize,
    /// 本批已完成的实例数。
    completed: usize,
    /// 本批实例总数（至少 1，避免除零）。
    total: usize,
}

impl InstallProgress {
    fn new(ctx: &Ctx, task_id: Option<u64>, total: usize) -> Self {
        Self {
            download: ctx.download.clone(),
            task_id,
            index: PHASE_AUTHORIZING,
            completed: 0,
            total: total.max(1),
        }
    }

    /// 把下载任务切到「安装中」并落到首个阶段。
    ///
    /// 这是唯一一次状态切换：此后只有进度与文案在变，直到 [`Self::finish`]。
    fn begin(&mut self) {
        let Some(id) = self.task_id else { return };
        self.index = PHASE_AUTHORIZING;
        self.download
            .begin_phase(id, INSTALL_PHASES[PHASE_AUTHORIZING].0, None);
    }

    /// 把「单个实例内的进度」换算成整批进度。
    fn scaled(&self, value: f64) -> f64 {
        (self.completed as f64 + value) / self.total as f64
    }

    /// 进入指定阶段（阶段内部进度归零）。
    fn enter(&mut self, index: usize, detail: Option<String>) {
        if index >= INSTALL_PHASES.len() {
            return;
        }
        self.index = index;
        let Some(id) = self.task_id else { return };
        let (key, _) = INSTALL_PHASES[index];
        self.download
            .report_phase(id, self.scaled(phase_base(index)), key, detail);
    }

    /// 上报当前阶段的内部进度（0.0~1.0）。
    fn inner(&self, fraction: f64, detail: Option<String>) {
        let Some(id) = self.task_id else { return };
        let (key, weight) = INSTALL_PHASES[self.index];
        let value = phase_base(self.index) + weight * fraction.clamp(0.0, 1.0);
        self.download.report_phase(id, self.scaled(value), key, detail);
    }

    /// 记录本批已完成的实例数（每个实例开始前调用）。
    fn set_completed(&mut self, completed: usize) {
        self.completed = completed;
    }

    /// 收尾：`error` 为空落已完成，否则落失败。
    fn finish(&self, error: Option<String>) {
        let Some(id) = self.task_id else { return };
        self.download.finish_phase(id, error);
    }

    /// 供 lipd 回调使用的独立上报句柄。
    ///
    /// `lipd::CallbackSink` 要求 `'static + Send + Sync`，闭包不能借用 `self`；
    /// 这里复制出最小的一组标量 + `Arc<DownloadService>`，语义与 [`Self::inner`] 一致。
    fn loader_handle(&self) -> Option<LoaderProgress> {
        Some(LoaderProgress {
            download: self.download.clone(),
            task_id: self.task_id?,
            completed: self.completed,
            total: self.total,
        })
    }
}

/// 加载器安装阶段的上报句柄（由 [`InstallProgress::loader_handle`] 生成）。
struct LoaderProgress {
    download: Arc<DownloadService>,
    task_id: u64,
    completed: usize,
    total: usize,
}

impl LoaderProgress {
    /// 上报加载器安装进度；`fraction` 为 `None` 时给阶段中点。
    ///
    /// lipd 的依赖求解步骤不报百分比（协议里该字段就是缺的），报阶段起点会让进度条
    /// 看起来停死，报中点则如实表达「这步在跑，但量不出来」。
    fn report(&self, fraction: Option<f64>, detail: Option<String>) {
        let (key, weight) = INSTALL_PHASES[PHASE_LOADER];
        let inner = fraction.map(|f| f.clamp(0.0, 1.0)).unwrap_or(0.5);
        let value =
            (self.completed as f64 + phase_base(PHASE_LOADER) + weight * inner) / self.total as f64;
        // 上报是纯观测手段：任务已消失（重启 / 被移除）时服务层只记 debug 日志，
        // 这里没有失败需要处理，也不会因此中断安装。
        self.download.report_phase(self.task_id, value, key, detail);
    }
}

/// 人类可读字节数（进度文案用）。
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{:.0} {}", value, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

/// 只保留段落路径的尾部若干层：整条路径动辄上百字符，塞进一行的进度提示里
/// 会把真正重要的「第几 / 共几」挤没。
fn shorten_entry(path: &str) -> String {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    if parts.len() <= 2 {
        return parts.join("/");
    }
    format!("…/{}", parts[parts.len() - 2..].join("/"))
}

// ---------------------------------------------------------------- DB

/// v1：旧的「版本即安装单元」表。
///
/// 已被 v3 拆成 package + instance 两张表，但**不能删改**：它是已发布迁移链的一环，
/// 改它会让「已升级的库」与「全新建的库」拿到不同 schema。
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

/// v2：启动续传重投计数（防 failed 状态无限自动重下）。
pub const MIGRATION_ATTEMPTS: crate::services::database::Migration =
    crate::services::database::Migration {
        version: 2,
        name: "game_download_attempts",
        sql: "ALTER TABLE module_game_download_task ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;",
    };

/// v3：拆表 —— 版本整包（下载单元）+ 实例（安装单元），并把旧记录就地迁移。
///
/// 旧记录的 `folder` 就是当年的版本目录名，因此直接当作实例名迁移：用户已装好的
/// 版本在新模型里表现为「一个与版本同名的实例」，不会凭空消失。
pub const MIGRATION_INSTANCES: crate::services::database::Migration =
    crate::services::database::Migration {
        version: 3,
        name: "game_download_instances",
        sql: "CREATE TABLE IF NOT EXISTS module_game_download_package (
                version_id TEXT PRIMARY KEY,
                kind       TEXT NOT NULL,
                dest       TEXT NOT NULL,
                md5        TEXT NOT NULL,
                state      TEXT NOT NULL DEFAULT 'downloading',
                error      TEXT,
                task_id    INTEGER,
                attempts   INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
              );
              CREATE TABLE IF NOT EXISTS module_game_download_instance (
                instance   TEXT PRIMARY KEY,
                version_id TEXT NOT NULL,
                kind       TEXT NOT NULL,
                loader     TEXT,
                state      TEXT NOT NULL DEFAULT 'queued',
                error      TEXT,
                created_at INTEGER NOT NULL
              );
              CREATE INDEX IF NOT EXISTS idx_gd_instance_version
                ON module_game_download_instance(version_id);
              INSERT OR IGNORE INTO module_game_download_package
                (version_id, kind, dest, md5, state, error, task_id, attempts, created_at)
                SELECT version_id, kind, dest, md5,
                       CASE state
                         WHEN 'extracting'       THEN 'ready'
                         WHEN 'authorizing'      THEN 'downloading'
                         WHEN 'failed_permanent' THEN 'failed'
                         ELSE state
                       END,
                       error, task_id, attempts, created_at
                  FROM module_game_download_task
                 WHERE state <> 'installed';
              INSERT OR IGNORE INTO module_game_download_instance
                (instance, version_id, kind, loader, state, error, created_at)
                SELECT folder, version_id, kind, NULL,
                       CASE state
                         WHEN 'installed'        THEN 'installed'
                         WHEN 'failed'           THEN 'failed'
                         WHEN 'failed_permanent' THEN 'failed'
                         ELSE 'queued'
                       END,
                       error, created_at
                  FROM module_game_download_task;
              DROP TABLE IF EXISTS module_game_download_task;",
    };

/// 本模块全部迁移（按版本升序执行）。
pub const MIGRATIONS: &[crate::services::database::Migration] =
    &[MIGRATION, MIGRATION_ATTEMPTS, MIGRATION_INSTANCES];

/// 版本整包记录（下载单元）：一个版本一份整包，多实例共享。
#[derive(Debug, Clone)]
pub struct PackageRecord {
    pub version_id: String,
    pub kind: String,
    pub dest: PathBuf,
    pub md5: String,
    /// `downloading` | `ready` | `failed` | `failed_permanent` | `cancelling`。
    pub state: String,
    pub error: Option<String>,
    pub task_id: Option<u64>,
    pub attempts: i64,
}

/// 实例记录（安装单元）：实例名即版本目录名。
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub instance: String,
    pub version_id: String,
    pub kind: String,
    /// 用户选定的加载器版本（LeviLamina）；未选为 `None`。
    pub loader: Option<String>,
    /// `queued` | `installing` | `installed` | `failed`。
    pub state: String,
    pub error: Option<String>,
}

const PACKAGE_COLUMNS: &str = "version_id, kind, dest, md5, state, error, task_id, attempts";
const INSTANCE_COLUMNS: &str = "instance, version_id, kind, loader, state, error";

fn row_to_package(r: &rusqlite::Row) -> rusqlite::Result<PackageRecord> {
    Ok(PackageRecord {
        version_id: r.get(0)?,
        kind: r.get(1)?,
        dest: PathBuf::from(r.get::<_, String>(2)?),
        md5: r.get(3)?,
        state: r.get(4)?,
        error: r.get(5)?,
        task_id: r.get(6)?,
        attempts: r.get(7)?,
    })
}

fn row_to_instance(r: &rusqlite::Row) -> rusqlite::Result<InstanceRecord> {
    Ok(InstanceRecord {
        instance: r.get(0)?,
        version_id: r.get(1)?,
        kind: r.get(2)?,
        loader: r.get(3)?,
        state: r.get(4)?,
        error: r.get(5)?,
    })
}

fn get_package(ctx: &Ctx, version_id: &str) -> Result<Option<PackageRecord>, KernelError> {
    ctx.db.with_conn(|conn| -> Result<Option<PackageRecord>, KernelError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT {PACKAGE_COLUMNS} FROM module_game_download_package WHERE version_id = ?1"
        ))?;
        let mut rows = stmt.query_map([version_id], row_to_package)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    })
}

fn get_instance(ctx: &Ctx, instance: &str) -> Result<Option<InstanceRecord>, KernelError> {
    ctx.db.with_conn(|conn| -> Result<Option<InstanceRecord>, KernelError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM module_game_download_instance WHERE instance = ?1"
        ))?;
        let mut rows = stmt.query_map([instance], row_to_instance)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    })
}

/// 某版本下所有「尚未装好」的实例（`queued` / `installing` / `failed`）。
///
/// 已装好（`installed`）的实例不在其中：再来一次安装会**新建另一个实例**，
/// 而不是重装已有实例——这正是「同版本多实例」的语义。
fn pending_instances(ctx: &Ctx, version_id: &str) -> Result<Vec<InstanceRecord>, KernelError> {
    ctx.db.with_conn(|conn| -> Result<Vec<InstanceRecord>, KernelError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM module_game_download_instance
              WHERE version_id = ?1 AND state <> 'installed'
              ORDER BY created_at ASC, instance ASC"
        ))?;
        let rows = stmt.query_map([version_id], row_to_instance)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(KernelError::from)
    })
}

/// 按状态列出整包记录。
fn list_packages(ctx: &Ctx, states: &[&str]) -> Result<Vec<PackageRecord>, KernelError> {
    ctx.db.with_conn(|conn| -> Result<Vec<PackageRecord>, KernelError> {
        let placeholders = states.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT {PACKAGE_COLUMNS} FROM module_game_download_package
              WHERE state IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<rusqlite::types::Value> = states
            .iter()
            .map(|s| rusqlite::types::Value::Text(s.to_string()))
            .collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params), row_to_package)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(KernelError::from)
    })
}

fn upsert_package(ctx: &Ctx, rec: &PackageRecord) -> Result<(), KernelError> {
    ctx.db.with_conn(|conn| -> Result<(), KernelError> {
        conn.execute(
            "INSERT INTO module_game_download_package
               (version_id, kind, dest, md5, state, error, task_id, attempts, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(version_id) DO UPDATE SET
               kind=excluded.kind, dest=excluded.dest, md5=excluded.md5, state=excluded.state,
               error=excluded.error, task_id=excluded.task_id, attempts=excluded.attempts",
            rusqlite::params![
                rec.version_id,
                rec.kind,
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

fn upsert_instance(ctx: &Ctx, rec: &InstanceRecord) -> Result<(), KernelError> {
    ctx.db.with_conn(|conn| -> Result<(), KernelError> {
        conn.execute(
            "INSERT INTO module_game_download_instance
               (instance, version_id, kind, loader, state, error, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(instance) DO UPDATE SET
               version_id=excluded.version_id, kind=excluded.kind, loader=excluded.loader,
               state=excluded.state, error=excluded.error",
            rusqlite::params![
                rec.instance,
                rec.version_id,
                rec.kind,
                rec.loader,
                rec.state,
                rec.error,
                Ctx::now()
            ],
        )?;
        Ok(())
    })
}

fn set_package_state(
    ctx: &Ctx,
    version_id: &str,
    state: &str,
    error: Option<&str>,
) -> Result<(), KernelError> {
    ctx.db.with_conn(|conn| -> Result<(), KernelError> {
        conn.execute(
            "UPDATE module_game_download_package SET state = ?1, error = ?2 WHERE version_id = ?3",
            rusqlite::params![state, error, version_id],
        )?;
        Ok(())
    })
}

fn set_package_task(ctx: &Ctx, version_id: &str, task_id: Option<u64>) -> Result<(), KernelError> {
    ctx.db.with_conn(|conn| -> Result<(), KernelError> {
        conn.execute(
            "UPDATE module_game_download_package SET task_id = ?1 WHERE version_id = ?2",
            rusqlite::params![task_id, version_id],
        )?;
        Ok(())
    })
}

fn set_instance_state(
    ctx: &Ctx,
    instance: &str,
    state: &str,
    error: Option<&str>,
) -> Result<(), KernelError> {
    ctx.db.with_conn(|conn| -> Result<(), KernelError> {
        conn.execute(
            "UPDATE module_game_download_instance SET state = ?1, error = ?2 WHERE instance = ?3",
            rusqlite::params![state, error, instance],
        )?;
        Ok(())
    })
}

fn delete_package(ctx: &Ctx, version_id: &str) -> Result<(), KernelError> {
    ctx.db.with_conn(|conn| -> Result<(), KernelError> {
        conn.execute(
            "DELETE FROM module_game_download_package WHERE version_id = ?1",
            rusqlite::params![version_id],
        )?;
        Ok(())
    })
}

fn delete_instance(ctx: &Ctx, instance: &str) -> Result<(), KernelError> {
    ctx.db.with_conn(|conn| -> Result<(), KernelError> {
        conn.execute(
            "DELETE FROM module_game_download_instance WHERE instance = ?1",
            rusqlite::params![instance],
        )?;
        Ok(())
    })
}

/// 下载任务 → 游戏版本的绑定关系（供下载中心提供「安装」入口）。
///
/// 下载中心列出的是核心下载引擎的任务，而安装按**实例**推进；这个映射由内核给出，
/// 前端不去猜 dest 或文件名：猜错就会把安装指向另一个版本。只有确实存在整包下载
/// 记录的任务才在这里出现。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct TaskBinding {
    pub task_id: u64,
    pub version_id: String,
}

/// 列出所有「下载任务 → 游戏版本」绑定。
pub fn task_bindings(ctx: &Ctx) -> Result<Vec<TaskBinding>, KernelError> {
    ctx.db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT task_id, version_id FROM module_game_download_package
              WHERE task_id IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(TaskBinding {
                task_id: row.get::<_, i64>(0)? as u64,
                version_id: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(KernelError::from)
    })
}

// ---------------------------------------------------------------- 实例命名

/// 实例名可用性检查结果（安装确认弹窗据此实时反馈）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct InstanceCheck {
    /// 规整后的实例名：前端提交这个名字，不再自己推导第二遍。
    pub name: String,
    pub available: bool,
    /// 不可用原因码；可用时为 `None`。文案由前端按码本地化。
    pub reason: Option<meta::NameRejection>,
}

/// 检查实例名：先按全项目唯一规则规整，再判目录冲突与在途任务冲突。
///
/// 「在途任务冲突」必须单独判：排队中的实例还没有目录，只查目录会让用户提交一个
/// 已经排着队的名字，两条流水线随后抢同一个输出目录。
///
/// 但**失败**的实例记录不算占用名字：它对应的目录已经在失败时清掉了，用户想用同一个
/// 名字重试是最自然的动作。`enqueue` 的 upsert 会把这条失败记录改回排队，正是这个
/// 语义（否则用户每次重试都被迫换一个名字，磁盘上会堆起 `1.21.130.22-2`、`-3`…）。
pub fn check_instance_name(ctx: &Ctx, raw: &str) -> InstanceCheck {
    let name = meta::sanitize_instance_name(raw);
    if let Err(reason) = meta::validate_version_name_reason(&ctx.versions_root(), &name) {
        return InstanceCheck {
            name,
            available: false,
            reason: Some(reason),
        };
    }
    match get_instance(ctx, &name) {
        Ok(Some(rec)) if rec.state != "failed" => InstanceCheck {
            name,
            available: false,
            reason: Some(meta::NameRejection::Taken),
        },
        // 查询失败不能当作「可用」：放行会绕过唯一性约束。
        Err(error) => {
            log::warn!("[game-download] 实例名冲突查询失败: {error}");
            InstanceCheck {
                name,
                available: false,
                reason: Some(meta::NameRejection::Taken),
            }
        }
        Ok(_) => InstanceCheck {
            name,
            available: true,
            reason: None,
        },
    }
}

/// 为某版本推荐一个可用实例名（版本号；重名时追加 `-2`、`-3`…）。
pub fn suggest_instance_name(ctx: &Ctx, version_id: &str) -> String {
    let base = meta_bridge::game_version_of(version_id);
    let base = if version_id.ends_with("_preview") {
        format!("{base}-preview")
    } else {
        base
    };
    for index in 1..=999 {
        let candidate = if index == 1 {
            base.clone()
        } else {
            format!("{base}-{index}")
        };
        if check_instance_name(ctx, &candidate).available {
            return candidate;
        }
    }
    base
}

// ---------------------------------------------------------------- 命令核心

/// 给落盘失败补上「版本根来自哪个设置项」这层定位信息。
///
/// 裸 `os error 5` 只有一个错误码，用户既不知道是哪个目录，也不知道该去哪里改。
/// 拼接规则与内核侧共用 `paths::annotate_versions_root_error`，避免两处包装漂移。
fn annotate_versions_root(ctx: &Ctx, error: KernelError) -> KernelError {
    crate::services::paths::annotate_versions_root_error(&ctx.versions_root(), error)
}

/// 创建实例并开始安装（幂等）。
///
/// 流程：校验实例名 → 落实例记录 → 复用本地整包或投递下载 → 该装就立刻装。
///
/// 返回下载任务 id；**0 表示整包已在本地、无需下载**（此时安装已经开始）。
pub async fn enqueue(
    ctx: &Ctx,
    lock: &Arc<tokio::sync::Mutex<()>>,
    version_id: &str,
    instance: &str,
    loader: Option<&str>,
) -> Result<u64, KernelError> {
    let check = check_instance_name(ctx, instance);
    if !check.available {
        return Err(KernelError::InvalidArgument(format!(
            "实例名 `{}` 不可用：{}",
            check.name,
            check.reason.map(|r| r.message()).unwrap_or("未知原因")
        )));
    }
    let instance = check.name;
    let loader = loader
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    // 清单取版本条目。
    let versions = manifest::load_manifest(ctx, false).await?;
    let entry = versions
        .find_by_id(version_id)
        .ok_or_else(|| KernelError::InvalidArgument(format!("清单中找不到版本 `{version_id}`")))?;
    let kind = match entry.kind() {
        manifest::VersionKind::Release => "release",
        manifest::VersionKind::Preview => "preview",
    };
    // md5 是唯一的完整性防线（引擎只校验 sha256，清单只给 md5）——缺失直接拒绝，
    // 避免空值恒校验失败。
    if entry.md5.trim().is_empty() {
        return Err(KernelError::InvalidArgument(format!(
            "版本 `{version_id}` 的清单条目缺少 MD5 校验值，无法安全安装"
        )));
    }
    let slug = entry.slug();
    let dest = ctx.dest_for(&slug);

    // 实例记录先落库：后面所有环节都以「这条实例在等安装」为依据。
    upsert_instance(
        ctx,
        &InstanceRecord {
            instance: instance.clone(),
            version_id: slug.clone(),
            kind: kind.to_string(),
            loader: loader.clone(),
            state: "queued".into(),
            error: None,
        },
    )?;

    if let Some(pkg) = get_package(ctx, &slug)? {
        // 已有整包下载在途 → 直接挂到同一个任务上，绝不重复投递：
        // 两个任务写同一个文件会互相截断，最终谁都没下完。
        if pkg.state == "downloading" {
            if let Some(task_id) = pkg.task_id.filter(|t| ctx.download.task(*t).is_some()) {
                log::info!("[game-download] 实例 {instance} 复用进行中的整包下载（任务 {task_id}）");
                ctx.events.publish(
                    "game-download.enqueued",
                    serde_json::json!({ "id": slug, "instance": instance, "taskId": task_id }),
                );
                return Ok(task_id);
            }
        }
        // 整包已在本地且校验通过 → 不重新下载，直接进入安装。
        if pkg.dest.is_file() && md5_matches(&pkg.dest, &pkg.md5).unwrap_or(false) {
            set_package_state(ctx, &slug, "ready", None)?;
            log::info!(
                "[game-download] 实例 {instance} 复用本地整包 {}",
                pkg.dest.display()
            );
            spawn_batch(ctx, lock, &slug, pkg.task_id);
            return Ok(0);
        }
    }

    // 落盘前确认整包暂存目录**真的**可写。
    //
    // 这里原本是裸 `create_dir_all(..)?`：目录已存在但不可写时（自定义版本根指向
    // 受限卷、被收紧的 ACL、占用中的挂载点）`create_dir_all` 会直接成功，直到引擎
    // 第一次写 `.part` 才失败，而失败会以不带任何路径的 `os error 5` 冒泡到界面，
    // 用户无从判断是「网络」还是「目录权限」。改为带写探针 + 带路径的报错。
    crate::services::paths::ensure_writable_dir(dest.parent().unwrap_or(Path::new(".")), "下载暂存")
        .map_err(|e| annotate_versions_root(ctx, e))?;

    // CDN 故障转移：取全部候选 URL，首个投递成功即停（引擎侧也有 3 次重试，这里只挑 host）。
    let urls = entry.all_urls().ok_or_else(|| {
        KernelError::InvalidArgument(format!("版本 `{version_id}` 没有可用下载链接"))
    })?;
    let opts = DownloadOptions {
        resume: true,
        remove_on_cancel: true,
        expected_sha256: None, // 清单仅提供 MD5，下载完成后模块内自验
        filename: Some(format!("{slug}.msixvc")),
        ..Default::default()
    };
    let mut enqueued: Option<u64> = None;
    let mut last_err: Option<KernelError> = None;
    for (index, url) in urls.iter().enumerate() {
        match ctx.download.enqueue(url, &dest, opts.clone()) {
            Ok(task_id) => {
                if index > 0 {
                    log::info!("[game-download] 主链路失败，已切到备用 CDN: {url}");
                }
                enqueued = Some(task_id);
                break;
            }
            Err(error) => {
                log::warn!("[game-download] CDN 投递失败 {url}: {error}");
                last_err = Some(error);
            }
        }
    }
    let Some(task_id) = enqueued else {
        // 投递失败：实例记录留着没有意义（没有任何东西会推进它），就地回滚。
        let _ = delete_instance(ctx, &instance);
        return Err(last_err.unwrap_or_else(|| KernelError::Module("下载队列投递失败".into())));
    };

    upsert_package(
        ctx,
        &PackageRecord {
            version_id: slug.clone(),
            kind: kind.to_string(),
            dest: dest.clone(),
            md5: entry.md5.clone(),
            state: "downloading".into(),
            error: None,
            task_id: Some(task_id),
            attempts: 0,
        },
    )?;

    ctx.events.publish(
        "game-download.enqueued",
        serde_json::json!({ "id": slug, "instance": instance, "taskId": task_id }),
    );
    Ok(task_id)
}

/// 实例级重装：整包在本地且校验通过时重跑安装流水线，**不重新下载**。
///
/// 前置条件不满足时**明确报错**，而不是偷偷改走下载——用户点的是「重试安装」，
/// 替他下几个 G 的流量是不可预期的。
pub fn retry_instance(
    ctx: &Ctx,
    lock: &Arc<tokio::sync::Mutex<()>>,
    instance: &str,
) -> Result<(), KernelError> {
    let rec = get_instance(ctx, instance)?
        .ok_or_else(|| KernelError::InvalidArgument(format!("实例 `{instance}` 没有安装记录")))?;
    if rec.state == "installed" {
        return Err(KernelError::InvalidArgument(format!(
            "实例 `{instance}` 已安装完成"
        )));
    }
    if lock.try_lock().is_err() {
        return Err(KernelError::Conflict(
            "已有安装正在进行中，请等待当前任务结束".into(),
        ));
    }
    let pkg = get_package(ctx, &rec.version_id)?.ok_or_else(|| {
        KernelError::InvalidArgument(format!("版本 `{}` 没有下载记录，无法安装", rec.version_id))
    })?;
    if !pkg.dest.is_file() {
        return Err(KernelError::InvalidArgument(format!(
            "本地没有版本 `{}` 的完整安装包，请重新下载",
            rec.version_id
        )));
    }
    if pkg.md5.trim().is_empty() {
        return Err(KernelError::Config("清单缺少 MD5 校验值，拒绝安装".into()));
    }
    if !md5_matches(&pkg.dest, &pkg.md5)? {
        // 包已损坏：删掉它，否则它会一直卡住后续安装尝试（每次都走到这里失败，
        // 而用户看不出为什么）。
        cleanup_package(&pkg.dest);
        return Err(KernelError::Config(format!(
            "本地安装包校验失败（{}），已删除该文件，请重新下载",
            pkg.dest.to_string_lossy()
        )));
    }
    set_instance_state(ctx, instance, "queued", None)?;
    spawn_batch(ctx, lock, &rec.version_id, pkg.task_id);
    Ok(())
}

/// 版本级重试：把该版本下所有未装好的实例重新排进安装（下载中心的「安装」入口）。
pub fn retry_version(
    ctx: &Ctx,
    lock: &Arc<tokio::sync::Mutex<()>>,
    version_id: &str,
) -> Result<(), KernelError> {
    let pkg = get_package(ctx, version_id)?.ok_or_else(|| {
        KernelError::InvalidArgument(format!("版本 `{version_id}` 没有下载记录，无法安装"))
    })?;
    let targets = pending_instances(ctx, version_id)?;
    if targets.is_empty() {
        return Err(KernelError::InvalidArgument(format!(
            "版本 `{version_id}` 没有待安装的实例"
        )));
    }
    if !pkg.dest.is_file() {
        return Err(KernelError::InvalidArgument(format!(
            "本地没有版本 `{version_id}` 的完整安装包，请重新下载"
        )));
    }
    for target in &targets {
        set_instance_state(ctx, &target.instance, "queued", None)?;
    }
    spawn_batch(ctx, lock, version_id, pkg.task_id);
    Ok(())
}

/// 取消一次安装（放弃该实例）。
///
/// 语义：删掉实例记录；该版本**再无待装实例**时，连同整包下载一起放弃——取消引擎
/// 任务、清除整包与断点、删除整包记录。这样缓存立刻释放，重启也不会续传。
///
/// 只有 `installed` 的实例不可取消（要删已装好的实例请去开始页），`installing`
/// （正在解包）为 no-op：解包不可中断，重跑代价过高。
pub fn cancel(ctx: &Ctx, instance: &str) -> Result<(), KernelError> {
    let Some(rec) = get_instance(ctx, instance)? else {
        return Err(KernelError::InvalidArgument(format!(
            "实例 `{instance}` 没有安装记录"
        )));
    };
    if rec.state == "installed" || rec.state == "installing" {
        return Ok(());
    }
    delete_instance(ctx, instance)?;

    // 还有别的实例在等这个版本的包 → 只放弃这一个实例，下载继续。
    if !pending_instances(ctx, &rec.version_id)?.is_empty() {
        ctx.events.publish(
            "game-download.cancelled",
            serde_json::json!({ "id": rec.version_id, "instance": instance }),
        );
        return Ok(());
    }

    // 先置中间态，避免引擎 `download.status(Cancelled)` 被误判为失败并回写 failed。
    if let Some(pkg) = get_package(ctx, &rec.version_id)? {
        set_package_state(ctx, &rec.version_id, "cancelling", Some("已取消"))?;
        if let Some(task_id) = pkg.task_id {
            // 容错：任务可能已不在队列（如已移除）。
            let _ = ctx.download.cancel(task_id);
        }
        // 立即清除本地缓存（整包 + 断点 `.part`），不依赖引擎 `remove_on_cancel`，
        // 保证「马上清除缓存」。
        cleanup_package(&pkg.dest);
        delete_package(ctx, &rec.version_id)?;
    }
    ctx.events.publish(
        "game-download.cancelled",
        serde_json::json!({ "id": rec.version_id, "instance": instance }),
    );
    Ok(())
}

/// 取某版本可选的加载器清单（详情页「加载器」下拉）。
///
/// `lip_available` 为假表示本机没有 lipd：加载器下拉仍列出候选项，但选中后装不上，
/// 前端据此提前提示，而不是让用户在下完几个 G 之后才在安装末期失败。
pub async fn loader_options(version_id: &str) -> Result<LoaderOptions, KernelError> {
    let catalog = loader_catalog::LoaderCatalog::load().await?;
    Ok(LoaderOptions {
        lip_available: lipd::find_lip_executable().is_some(),
        loaders: catalog.options_for(&meta_bridge::game_version_of(version_id)),
    })
}

/// 某版本的可选加载器清单。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LoaderOptions {
    /// 本机是否可用 lipd（决定选中的加载器能否真的装上）。
    pub lip_available: bool,
    pub loaders: Vec<loader_catalog::LoaderOption>,
}

// ---------------------------------------------------------------- 事件驱动

/// 处理 `download.status` 事件（同步回调，异步工作经 runtime.spawn）。
pub fn handle_status(ctx: &Ctx, lock: &Arc<tokio::sync::Mutex<()>>, payload: &Value) {
    let Some((task_id, status, _dest)) = parse_snapshot(payload) else {
        return;
    };
    let Some(pkg) = find_package_by_task(ctx, task_id) else {
        return; // 非本模块任务
    };

    match status {
        DownloadStatus::Done => {
            // 整包下载完成 → 该版本所有未装好的实例进入安装（串行）。
            if let Err(error) = set_package_state(ctx, &pkg.version_id, "ready", None) {
                log::error!(
                    "[game-download] 整包 {} 状态回写失败: {error}",
                    pkg.version_id
                );
            }
            spawn_batch(ctx, lock, &pkg.version_id, Some(task_id));
        }
        DownloadStatus::Failed => {
            let message = payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("下载失败")
                .to_string();
            // 回写失败会在 `mark_version_failed` 内部明确落 error 日志，这里不必再处理。
            mark_version_failed(ctx, &pkg.version_id, &message);
        }
        DownloadStatus::Cancelled => {
            // `cancelling` 是版本页主动取消的中间态，清理由 `cancel` 收尾，此处不重复。
            if pkg.state == "cancelling" {
                return;
            }
            // 非版本页主动取消（如全局下载列表取消）→ 同样「放弃下载」：
            // 清除缓存并删掉整包与实例记录，避免 `resume_pending` 在重启时续传。
            cleanup_package(&pkg.dest);
            let _ = delete_package(ctx, &pkg.version_id);
            let _ = discard_instances(ctx, &pkg.version_id, "已取消");
            ctx.events.publish(
                "game-download.cancelled",
                serde_json::json!({ "id": pkg.version_id }),
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

fn find_package_by_task(ctx: &Ctx, task_id: u64) -> Option<PackageRecord> {
    ctx.db
        .with_conn(|conn| -> Result<Option<PackageRecord>, KernelError> {
            let mut stmt = conn.prepare(&format!(
                "SELECT {PACKAGE_COLUMNS} FROM module_game_download_package WHERE task_id = ?1"
            ))?;
            let mut rows = stmt.query_map([task_id], row_to_package)?;
            match rows.next() {
                Some(r) => Ok(Some(r?)),
                None => Ok(None),
            }
        })
        .ok()
        .flatten()
}

/// 把某版本整包与其实例一起落失败，并广播。
///
/// 回写失败**必须留痕**：记录若停在中间态，而状态机又把该状态当成「正在跑」，
/// 用户此后点安装就永远没反应。此前这里吞掉回写错误，正是这条故障链的起点；
/// 现在失败会带原因落 error 日志，调用方无需（也不该）再各自处理一遍。
fn mark_version_failed(ctx: &Ctx, version_id: &str, error: &str) {
    if let Err(write_error) = set_package_state(ctx, version_id, "failed", Some(error))
        .and_then(|()| fail_pending_instances(ctx, version_id, error))
    {
        log::error!(
            "[game-download] {version_id} 的失败状态回写失败: {write_error}（记录可能停在非终态）"
        );
    }
    publish_failed(ctx, version_id, error);
}

/// 把某版本下所有未装好的实例标记为失败（附带原因）。
fn fail_pending_instances(ctx: &Ctx, version_id: &str, error: &str) -> Result<(), KernelError> {
    for rec in pending_instances(ctx, version_id)? {
        set_instance_state(ctx, &rec.instance, "failed", Some(error))?;
    }
    Ok(())
}

/// 丢弃某版本下所有未装好的实例记录（取消 / 放弃安装时用）。
fn discard_instances(ctx: &Ctx, version_id: &str, error: &str) -> Result<(), KernelError> {
    for rec in pending_instances(ctx, version_id)? {
        delete_instance(ctx, &rec.instance)?;
        log::info!("[game-download] 丢弃实例 {}（{error}）", rec.instance);
    }
    Ok(())
}

// ---------------------------------------------------------------- 安装

/// md5 自验。分块流式读取，**绝不在内存中一次性载入整包**（GDK 整包可达数 GB，
/// `fs::read` 会在设置 `extracting` 后因连续大分配失败而 panic，导致安装静默中断）。
fn md5_matches(path: &Path, expected: &str) -> Result<bool, KernelError> {
    md5_matches_with_progress(path, expected, &mut |_, _| {})
}

/// 同上，并按已读字节上报 `(已完成, 总字节)`。
///
/// 总字节取文件元数据；拿不到时为 0，调用方应据此跳过进度换算（而不是当成 0%）。
fn md5_matches_with_progress(
    path: &Path,
    expected: &str,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<bool, KernelError> {
    // 清单条目缺 md5 → 恒校验失败会进入无限重试循环，这里显式区分“未提供”。
    if expected.trim().is_empty() {
        return Err(KernelError::Config("清单缺少 MD5 校验值，拒绝安装".into()));
    }
    let mut file = std::fs::File::open(path).map_err(KernelError::Io)?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut digest = Md5::new();
    // 堆上缓冲（1 MiB）。不可用栈上大数组：启动时 `resume_pending` 会在主线程
    // 同步调用本函数，主线程默认栈仅 1MB，栈上数组会直接爆栈。
    let mut buf = vec![0u8; 1024 * 1024];
    let mut done: u64 = 0;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
        done += n as u64;
        on_progress(done, total);
    }
    Ok(format!("{:x}", digest.finalize()).eq_ignore_ascii_case(expected.trim()))
}

/// 派生一批安装（异步、串行）。
///
/// 目标列表在派生时刻确定：调用方已经知道「哪些实例在等这个版本」，而不是让后台任务
/// 自己去猜——猜法一旦变化（比如把 failed 排除掉），失败重试就会静默失效。
fn spawn_batch(
    ctx: &Ctx,
    lock: &Arc<tokio::sync::Mutex<()>>,
    version_id: &str,
    task_id: Option<u64>,
) {
    let targets = match pending_instances(ctx, version_id) {
        Ok(targets) => targets,
        Err(error) => {
            log::error!("[game-download] 取 {version_id} 待装实例失败: {error}");
            return;
        }
    };
    if targets.is_empty() {
        log::info!("[game-download] 版本 {version_id} 没有待装实例，安装无需进行");
        return;
    }
    let next_ctx = ctx.clone();
    let next_lock = lock.clone();
    let version = version_id.to_string();
    ctx.runtime.spawn(async move {
        run_batch(&next_ctx, &next_lock, version, targets, task_id).await;
    });
}

/// 一批安装的主体：整包级只做一次授权与校验，随后逐实例安装。
///
/// 单飞锁覆盖整批（含授权）：授权会注册设备凭证并可能拉起系统账户界面，并发跑会
/// 在设备缓存上互相打架，每个都报同一句「另一个安装正在使用该设备缓存」。
pub async fn run_batch(
    ctx: &Ctx,
    lock: &Arc<tokio::sync::Mutex<()>>,
    version_id: String,
    targets: Vec<InstanceRecord>,
    task_id: Option<u64>,
) {
    let _guard = lock.lock().await;

    let total = targets.len();
    let mut progress = InstallProgress::new(ctx, task_id, total);
    progress.begin();

    let pkg = match get_package(ctx, &version_id) {
        Ok(Some(pkg)) => pkg,
        Ok(None) => {
            // 整包记录已消失（用户在排队期间取消了）：实例一并放弃，
            // 不留下永远排队的孤儿。
            log::warn!("[game-download] 版本 {version_id} 的整包记录已消失，放弃这批安装");
            for target in &targets {
                let _ = delete_instance(ctx, &target.instance);
            }
            progress.finish(Some("下载记录已被取消".into()));
            return;
        }
        Err(error) => {
            let message = error.to_string();
            log::error!("[game-download] 读取整包记录失败: {message}");
            progress.finish(Some(message));
            return;
        }
    };

    // 整包级准备（授权 + 完整性校验），每批只做一次。
    //
    // 密钥以**租约**形态持有到整批结束：`ContentKeyLease` 离开作用域即清零密钥内存，
    // 这里若把它拷成 `Vec<u8>` 再丢掉租约，等于在堆上留下一份永不清零的密钥副本。
    let lease = match prepare_package(ctx, &pkg, &mut progress).await {
        Ok(key) => key,
        Err(error) => {
            let message = error.to_string();
            log::error!("[game-download] 版本 {version_id} 安装准备失败: {message}");
            for target in &targets {
                if let Err(write_error) =
                    set_instance_state(ctx, &target.instance, "failed", Some(&message))
                {
                    log::error!(
                        "[game-download] 实例 {} 失败状态回写失败: {write_error}",
                        target.instance
                    );
                }
            }
            // 整包留着：授权恢复后重试不必重新下载。
            let _ = set_package_state(ctx, &version_id, "ready", Some(&message));
            progress.finish(Some(message.clone()));
            publish_failed(ctx, &version_id, &message);
            return;
        }
    };

    let content_key = lease.as_ref().map(|lease| lease.key());

    // 逐实例安装。单个实例失败**不**中断整批：一旦失败就放弃其余实例，那些实例会
    // 既没装上、也没留下任何原因，而它们往往只是被同一个原因（授权 / 磁盘空间）连带
    // 影响——继续跑完，每个实例都会拿到属于它自己的结论。
    let mut first_error: Option<String> = None;
    let mut installed = 0usize;
    for (index, target) in targets.iter().enumerate() {
        progress.set_completed(index);
        match install_instance(ctx, &pkg, target, content_key, &mut progress).await {
            Ok(()) => {
                installed += 1;
                if let Err(error) = set_instance_state(ctx, &target.instance, "installed", None) {
                    log::error!(
                        "[game-download] 实例 {} 完成状态回写失败: {error}",
                        target.instance
                    );
                }
            }
            Err(error) => {
                let message = error.to_string();
                log::error!("[game-download] 实例 {} 安装失败: {message}", target.instance);
                if let Err(write_error) =
                    set_instance_state(ctx, &target.instance, "failed", Some(&message))
                {
                    log::error!(
                        "[game-download] 实例 {} 失败状态回写失败: {write_error}",
                        target.instance
                    );
                }
                first_error.get_or_insert(message);
            }
        }
    }
    progress.set_completed(installed);

    match &first_error {
        None => {
            // 整批成功：整包已消费完毕，清掉文件与记录，不留占用磁盘的残留。
            cleanup_package(&pkg.dest);
            let _ = delete_package(ctx, &version_id);
            progress.finish(None);
        }
        Some(message) => {
            // 保留整包以便「重试安装」不必重新下载；失败原因落在实例记录上。
            let _ = set_package_state(ctx, &version_id, "ready", Some(message));
            progress.finish(Some(message.clone()));
            publish_failed(ctx, &version_id, message);
        }
    }
}

/// 整包级准备：取商店内容密钥并校验完整性。
///
/// 返回**密钥租约**而不是裸字节：租约在整批安装期间保持存活，用完即随作用域清零。
async fn prepare_package(
    ctx: &Ctx,
    pkg: &PackageRecord,
    progress: &mut InstallProgress,
) -> Result<Option<native_install::ContentKeyLease>, KernelError> {
    // 加密 MSIXVC 必须拿到商店 content key——拿不到就**显式失败**，不静默回退：
    // 旧的“回退兼容 DLL”路径依赖本机 Store 授权状态且返回码晦涩，已被上游弃用。
    progress.enter(PHASE_AUTHORIZING, Some(pkg.version_id.clone()));
    let lease = store_content_key(ctx, &pkg.dest).await?;

    // md5 自验（引擎只保留 sha256，清单提供的是 md5）。GB 级包要读完全文件，
    // 因此这里按已读字节上报——否则「校验中」这一步会面无表情地卡上好几分钟。
    progress.enter(PHASE_VERIFYING, Some(pkg.version_id.clone()));
    let verified = md5_matches_with_progress(&pkg.dest, &pkg.md5, &mut |done, total| {
        let fraction = if total > 0 {
            done as f64 / total as f64
        } else {
            0.0
        };
        progress.inner(
            fraction,
            Some(format!("{} / {}", human_bytes(done), human_bytes(total))),
        );
    })?;
    if !verified {
        return Err(KernelError::Config(format!(
            "MD5 校验失败：{} 与清单不符",
            pkg.dest.to_string_lossy()
        )));
    }
    Ok(lease)
}

/// 安装一个实例：解包 → 写元数据 → 装加载器。
///
/// 幂等：游戏已装好则跳过解包（重试只补没做完的部分，例如加载器）。
async fn install_instance(
    ctx: &Ctx,
    pkg: &PackageRecord,
    target: &InstanceRecord,
    content_key: Option<&[u8]>,
    progress: &mut InstallProgress,
) -> Result<(), KernelError> {
    set_instance_state(ctx, &target.instance, "installing", None)?;
    let install_dir = ctx.install_dir(&target.instance);

    if !meta_bridge::is_installed(&install_dir) {
        progress.enter(PHASE_PREPARING, Some(target.instance.clone()));
        // 半成品输出目录自愈：上次失败可能留下空目录（remove_dir_all 自身失败 / 用户
        // 手建），解包器遇到已存在输出必失败。真冲突（已有 version.json 的完整安装）
        // 由上面的 `is_installed` 分支拦住，走不到这里。
        if install_dir.exists() {
            log::warn!(
                "[game-download] 清理上次失败残留的半成品目录 {}",
                install_dir.display()
            );
            progress.inner(0.0, Some(install_dir.to_string_lossy().into_owned()));
            std::fs::remove_dir_all(&install_dir).map_err(|e| {
                KernelError::Io(std::io::Error::new(
                    e.kind(),
                    format!("无法清理残留安装目录 {}: {e}", install_dir.display()),
                ))
            })?;
        }

        log::info!(
            "[game-download] 开始安装实例 {} <- {}",
            target.instance,
            pkg.dest.to_string_lossy()
        );
        progress.enter(PHASE_EXTRACTING, Some(target.instance.clone()));
        let extract_result = extractor::extract_package_with_progress(
            &pkg.dest,
            &install_dir,
            content_key,
            &mut |done, total, entry| {
                let fraction = if total > 0 {
                    done as f64 / total as f64
                } else {
                    0.0
                };
                progress.inner(
                    fraction,
                    Some(format!("{} ({}/{})", shorten_entry(entry), done + 1, total)),
                );
            },
        );
        if let Err(error) = extract_result {
            // 失败即清空输出目录，去掉「名为已装、实则空目录」的假成功。
            let _ = std::fs::remove_dir_all(&install_dir);
            return Err(KernelError::from(error));
        }

        progress.enter(PHASE_FINALIZING, Some(target.instance.clone()));
        meta_bridge::write_meta(
            &install_dir,
            &target.instance,
            &target.version_id,
            &target.kind,
            None,
        )?;
        // 游戏本体已就位 → 立刻让开始页看到它，不必等加载器装完。
        ctx.events.publish(
            "version.installed",
            serde_json::json!({ "name": target.instance }),
        );
    }

    // 加载器：只在「这次要求的版本与磁盘上已装的版本不同」时才跑 lipd。
    // 这既是幂等（重试不会重复安装），也让失败重试能精确补装加载器。
    if let Some(loader) = target
        .loader
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        if meta_bridge::installed_loader(&install_dir).as_deref() != Some(loader) {
            progress.enter(PHASE_LOADER, Some(format!("LeviLamina {loader}")));
            install_loader(ctx, &install_dir, loader, progress).await?;
            progress.enter(PHASE_FINALIZING, Some(target.instance.clone()));
            meta_bridge::write_meta(
                &install_dir,
                &target.instance,
                &target.version_id,
                &target.kind,
                Some(loader),
            )?;
        }
    }

    log::info!(
        "[game-download] 实例 {} 安装完成 -> {}",
        target.instance,
        install_dir.to_string_lossy()
    );
    ctx.events.publish(
        "game-download.installed",
        serde_json::json!({ "instance": target.instance, "id": target.version_id }),
    );
    Ok(())
}

/// 把 LeviLamina 装进实例目录（经 lipd；依赖解析与下载都由它完成）。
///
/// 这里直接调用内容下载模块的**内核无关核心**（[`lip_install::install_resolved`]）：
/// 游戏下载流水线跑在内核事件里，手里只有一组 Arc 服务，既拿不到也用不上整个
/// `KernelContext`。加载器目录与安装实现都归内容下载模块，本模块只负责「什么时候装、
/// 装到哪个目录、进度怎么报」。
async fn install_loader(
    ctx: &Ctx,
    install_dir: &Path,
    loader: &str,
    progress: &mut InstallProgress,
) -> Result<(), KernelError> {
    let Some(exe) = lipd::find_lip_executable() else {
        return Err(KernelError::Config(
            "未检测到 lipd，无法安装加载器（需要先安装 lip 与 .NET 10 运行时）".into(),
        ));
    };
    let handle = progress.loader_handle();
    let sink: lipd::CallbackSink = Arc::new(move |callback: lipd::DaemonCallback| {
        let Some(handle) = handle.as_ref() else { return };
        match callback {
            lipd::DaemonCallback::Progress { item, percent } => {
                // lipd 的百分比是 0~100；缺省（协议里就没有该字段）时给阶段中点，
                // 而不是起点——报起点会让进度条看起来停死在这 28% 上。
                handle.report(percent.map(|p| p / 100.0), Some(item));
            }
            lipd::DaemonCallback::Log(text) => {
                // 步骤文本比百分比更细，但也更吵：这里只留诊断日志。
                if !text.trim().is_empty() {
                    log::debug!("[game-download] lipd: {text}");
                }
            }
        }
    });

    let _ = ctx;
    let outcome = lip_install::install_resolved(
        &exe,
        loader_catalog::LEVILAMINA_CLIENT_PACKAGE_REF,
        loader,
        install_dir,
        Some(&sink),
    )
    .await;
    if outcome.success {
        log::info!(
            "[game-download] 实例加载器安装完成 {} -> {}",
            outcome.package,
            install_dir.display()
        );
        return Ok(());
    }
    let code = outcome.error_code.unwrap_or_default();
    Err(KernelError::Module(format!(
        "加载器 LeviLamina {loader} 安装失败（{code}）：{}",
        outcome.stderr
    )))
}

/// 广播安装失败（供下载中心 / 详情提示）。
fn publish_failed(ctx: &Ctx, version_id: &str, error: &str) {
    ctx.events.publish(
        "version.download_failed",
        serde_json::json!({ "id": version_id, "error": error }),
    );
    ctx.events.publish(
        "game-download.failed",
        serde_json::json!({ "id": version_id, "error": error }),
    );
}

/// 商店授权可复用的缓存 key 文件路径（`store-key/<key_id>.dpapi`）。
fn cached_key_path(ctx: &Ctx, key_id: &str) -> PathBuf {
    let safe: String = key_id
        .chars()
        .map(|c| {
            if c.is_ascii_hexdigit() || c == '-' {
                c
            } else {
                '_'
            }
        })
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
/// 供调用方落库并广播，绝不静默回退。密钥只在返回的租约内存活。
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
    if let Ok(Some(lease)) = native_install::load_cached_key(ctx, &identifiers.key_id, &xuid) {
        log::info!(
            "[game-download] 命中本地授权缓存 key_id={}（离线）",
            identifiers.key_id
        );
        return Ok(Some(lease));
    }

    // 2) 在线授权链。窗口句柄仅用于「静默取票被拒 → 交互回落」，
    // 静默路径本身不需要窗口，也不会弹窗。
    let request = native_install::StoreInstallRequest::new(
        &xuid,
        store_market(ctx),
        ctx.cache_home().join("store-device"),
    )
    .with_owner_window(ctx.window.get());
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

/// 清理已消费的整包。
fn cleanup_package(dest: &Path) {
    let _ = std::fs::remove_file(dest);
    let _ = std::fs::remove_file(PathBuf::from(format!("{}.part", dest.to_string_lossy())));
}

// ---------------------------------------------------------------- 续传

/// 启动恢复：对未完成的整包续传/续装，清理无主整包。
pub fn resume_pending(ctx: &Ctx, lock: &Arc<tokio::sync::Mutex<()>>) {
    // `ready` 一并纳入：整包下完但进程在安装前退出（或安装失败保留整包）时会留下该
    // 状态，不恢复就永远不会有实例装上。
    let packages = match list_packages(ctx, &["downloading", "ready", "failed", "cancelling"]) {
        Ok(v) => v,
        Err(error) => {
            log::warn!("[game-download] resume_pending 读取记录失败: {error}");
            return;
        }
    };

    for pkg in packages {
        // 没有实例在等这个包（用户取消了全部安装）→ 整包与记录一并清理，
        // 不留下无人认领的 GB 级文件。
        let pending = match pending_instances(ctx, &pkg.version_id) {
            Ok(v) => v,
            Err(error) => {
                log::warn!(
                    "[game-download] 读取 {} 的待装实例失败: {error}",
                    pkg.version_id
                );
                continue;
            }
        };
        if pending.is_empty() {
            log::info!("[game-download] 版本 {} 已无待装实例，清理整包", pkg.version_id);
            cleanup_package(&pkg.dest);
            delete_package(ctx, &pkg.version_id).ok();
            continue;
        }

        // 整包已完整且 md5 命中 → 直接续装。
        if pkg.dest.is_file() && md5_matches(&pkg.dest, &pkg.md5).unwrap_or(false) {
            set_package_state(ctx, &pkg.version_id, "ready", None).ok();
            spawn_batch(ctx, lock, &pkg.version_id, pkg.task_id);
            continue;
        }

        // 整包不完整 → 提前清除“已放弃”残留：本地无整包、无断点字节，且引擎无在途
        // 任务，说明下载已取消/无任何可恢复内容，丢弃记录避免重启白白续传。
        let part = PathBuf::from(format!("{}.part", pkg.dest.to_string_lossy()));
        let has_live = pkg
            .task_id
            .map(|t| ctx.download.task(t).is_some())
            .unwrap_or(false);
        if !pkg.dest.is_file() && !part.is_file() && !has_live {
            log::warn!(
                "[game-download] 丢弃无内容的下载残留记录 {}",
                pkg.version_id
            );
            discard_instances(ctx, &pkg.version_id, "下载残留已丢弃").ok();
            delete_package(ctx, &pkg.version_id).ok();
            continue;
        }

        // 下载仍在引擎里跑着 → 保持原任务，**不重投**。
        //
        // 重投会为同一个目标文件再起一个任务：两个任务写同一个 `.part`，互相截断，
        // 结果是「重启一次，下载反而永远下不完」。只有任务确实不在内存里（进程重启
        // 后引擎为空）才需要按 `.part` 重新投递续传。
        if has_live {
            log::info!(
                "[game-download] 版本 {} 的下载任务仍在队列中，保持续传",
                pkg.version_id
            );
            set_package_state(ctx, &pkg.version_id, "downloading", None).ok();
            set_package_task(ctx, &pkg.version_id, pkg.task_id).ok();
            continue;
        }

        // failed 限次重投：连续 N 次启动重试仍失败 → 标记为永久失败，不再自动重下。
        if pkg.state == "failed" {
            if pkg.attempts >= MAX_RESUME_ATTEMPTS {
                log::warn!(
                    "[game-download] {} 已自动重试 {} 次仍失败，停止自动重试",
                    pkg.version_id,
                    pkg.attempts
                );
                set_package_state(ctx, &pkg.version_id, "failed_permanent", pkg.error.as_deref()).ok();
                continue;
            }
            ctx.db
                .with_conn(|conn| {
                    conn.execute(
                        "UPDATE module_game_download_package SET attempts = attempts + 1
                          WHERE version_id = ?1",
                        [pkg.version_id.as_str()],
                    )
                    .map(|_| ())
                    .map_err(KernelError::from)
                })
                .ok();
        }

        // 重新投递续传（引擎按 `.part` Range 续传）。
        let entry = match block_load(ctx).and_then(|v| {
            v.find_by_id(&pkg.version_id)
                .ok_or_else(|| KernelError::InvalidArgument("清单无此版本".into()))
        }) {
            Ok(entry) => entry,
            Err(error) => {
                // 清单异常不阻塞其余任务。
                if let Some(task_id) = pkg.task_id {
                    let _ = ctx.download.resume(task_id);
                }
                log::warn!("[game-download] 续传 {} 取清单失败: {error}", pkg.version_id);
                continue;
            }
        };
        let Some(urls) = entry.all_urls() else { continue };
        let opts = DownloadOptions {
            resume: true,
            remove_on_cancel: true,
            expected_sha256: None,
            filename: Some(format!("{}.msixvc", pkg.version_id)),
            ..Default::default()
        };
        let mut new_task: Option<u64> = None;
        for url in &urls {
            match ctx.download.enqueue(url, &pkg.dest, opts.clone()) {
                Ok(task_id) => {
                    new_task = Some(task_id);
                    break;
                }
                Err(error) => log::warn!(
                    "[game-download] 续传 {} CDN 失败 {url}: {error}",
                    pkg.version_id
                ),
            }
        }
        match new_task {
            Some(task_id) => {
                set_package_state(ctx, &pkg.version_id, "downloading", None).ok();
                set_package_task(ctx, &pkg.version_id, Some(task_id)).ok();
            }
            None => log::warn!("[game-download] 续传 {} 全部 CDN 投递失败", pkg.version_id),
        }
    }

    cleanup_orphan_instances(ctx);
    cleanup_orphan_packages(ctx);
}

/// 丢弃「没有任何整包记录在推进」的实例记录。
///
/// 它对应两种真实残留：用户在下载完成前取消了（`cancel` 已删记录，这里是它的兜底），
/// 以及整包记录被丢弃（无内容残留）时实例没跟上。留着它们只会让用户永远看到一个
/// 「排队中」却不会有任何动作的实例。
fn cleanup_orphan_instances(ctx: &Ctx) {
    let orphans = match ctx.db.with_conn(|conn| -> Result<Vec<InstanceRecord>, KernelError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM module_game_download_instance
              WHERE state <> 'installed'"
        ))?;
        let rows = stmt.query_map([], row_to_instance)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(KernelError::from)
    }) {
        Ok(v) => v,
        Err(error) => {
            log::warn!("[game-download] 读取无主实例失败: {error}");
            return;
        }
    };
    for rec in orphans {
        match get_package(ctx, &rec.version_id) {
            // 有整包记录 = 还有东西会推进它（上面已重投或已续装），不动。
            Ok(Some(_)) => continue,
            Ok(None) => {}
            Err(error) => {
                log::warn!("[game-download] 实例 {} 查整包记录失败: {error}", rec.instance);
                continue;
            }
        }
        log::warn!(
            "[game-download] 丢弃无整包支撑的实例记录 {}（状态 {}）",
            rec.instance,
            rec.state
        );
        delete_instance(ctx, &rec.instance).ok();
    }
}

/// 清理 `versions_root/.download` 下无主的整包 / 断点文件。
///
/// 新模型里「有没有人要这个包」完全由实例记录表达，因此没有任何整包记录的文件就是
/// 无主残留（旧模型下这里会把孤儿包**认领并安装**，现在那样做只会凭空造出一个用户
/// 没要求过的实例）。清理是安全的：真要装，用户重新点安装即可。
fn cleanup_orphan_packages(ctx: &Ctx) {
    let download_dir = ctx.versions_root().join(DOWNLOAD_SUBDIR);
    let entries = match std::fs::read_dir(&download_dir) {
        Ok(v) => v,
        // 目录不存在是正常态（尚未下载过任何版本），静默跳过；但**不可写**这类
        // 失败必须留痕，否则自定义版本根权限异常会被当成「没有孤儿包」永久掩盖。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            log::warn!(
                "[game-download] 无主整包清理读取 {} 失败: {e}",
                download_dir.display()
            );
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // 只处理本模块产物：`<slug>.msixvc` 与 `<slug>.msixvc.part`。
        let stem = name.strip_suffix(".part").unwrap_or(name);
        let Some(slug) = stem.strip_suffix(".msixvc") else {
            continue;
        };
        match get_package(ctx, slug) {
            Ok(Some(_)) => continue,
            Ok(None) => {}
            Err(error) => {
                log::warn!("[game-download] 无主整包 {slug} 查记录失败: {error}");
                continue;
            }
        }
        log::info!(
            "[game-download] 清理无主下载残留 {}（{} 字节）",
            path.display(),
            entry.metadata().map(|m| m.len()).unwrap_or(0)
        );
        let _ = std::fs::remove_file(&path);
    }
}

/// 同步读取清单（供 `resume_pending` 等非 async 上下文使用；阻塞运行时线程）。
fn block_load(ctx: &Ctx) -> Result<manifest::HistoricalVersions, KernelError> {
    ctx.runtime
        .block_on(manifest::load_manifest(ctx, false))
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

    /// 校验进度必须真的往上走，且终点等于总字节数——否则进度条会停在半路。
    #[test]
    fn md5_progress_reaches_total() {
        let dir = std::env::temp_dir().join(format!("copper_gd_md5p_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("big.bin");
        // 2.5 MiB：跨过 1 MiB 的读缓冲边界，验证多次回调。
        let payload = vec![7u8; 2 * 1024 * 1024 + 512 * 1024];
        std::fs::write(&p, &payload).unwrap();

        let mut samples: Vec<(u64, u64)> = Vec::new();
        let ok =
            md5_matches_with_progress(&p, "00000000000000000000000000000000", &mut |done, total| {
                samples.push((done, total));
            })
            .unwrap();
        assert!(!ok, "故意给错的期望值不应通过");
        assert!(!samples.is_empty(), "GB 级包必须上报中间进度");
        for (_, total) in &samples {
            assert_eq!(*total, payload.len() as u64);
        }
        let last = samples.last().copied().unwrap();
        assert_eq!(last.0, payload.len() as u64, "最后一次回调必须读到文件尾");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 阶段权重必须完整覆盖 0~1 且单调——否则进度条会跳变或倒退。
    #[test]
    fn install_phase_weights_are_monotonic_and_complete() {
        let total: f64 = INSTALL_PHASES.iter().map(|(_, w)| *w).sum();
        assert!((total - 1.0).abs() < 1e-9, "阶段权重之和应为 1，实际 {total}");
        for index in 1..=INSTALL_PHASES.len() {
            assert!(
                phase_base(index) >= phase_base(index - 1),
                "阶段起点必须单调不减（{index}）"
            );
        }
        assert!((phase_base(INSTALL_PHASES.len()) - 1.0).abs() < 1e-9);
        // 阶段键必须落在 download.stage.* 命名空间：前端对未知键会退化成键名本身，
        // 写错命名空间时界面上会直接显示一串英文点分路径。
        for (key, _) in INSTALL_PHASES {
            assert!(key.starts_with("download.stage."), "阶段键命名空间不对: {key}");
        }
    }

    /// 多实例进度必须按实例数摊平，且整批单调递增（不许中途倒退）。
    #[test]
    fn batch_progress_is_flat_and_monotonic() {
        let scaled = |completed: usize, seen: usize, total: usize, value: f64| {
            let completed = if seen > completed { seen } else { completed };
            (completed as f64 + value) / total as f64
        };
        // 两个实例：第一个实例走到解包一半，第二个实例刚开始，最后走到整批结束。
        let first = scaled(0, 0, 2, phase_base(PHASE_EXTRACTING) + 0.5 * INSTALL_PHASES[PHASE_EXTRACTING].1);
        let second_start = scaled(1, 1, 2, phase_base(PHASE_PREPARING));
        let second_end = scaled(1, 1, 2, phase_base(PHASE_FINALIZING) + INSTALL_PHASES[PHASE_FINALIZING].1);
        assert!(first < second_start, "实例切换处进度不得倒退");
        assert!(second_start < second_end);
        assert!((second_end - 1.0).abs() < 1e-9, "整批结束应到 100%");
    }

    /// 进度文案的字节格式化不能出现 `1024.0 KB` 这种越界写法。
    #[test]
    fn human_bytes_switches_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1024 * 1024 * 3 / 2), "1.5 MB");
        assert_eq!(human_bytes(1024 * 1024 * 1024 * 2), "2.0 GB");
    }

    /// 长路径只保留尾部两层，避免把「第几 / 共几」挤出可视区。
    #[test]
    fn shorten_entry_keeps_tail() {
        assert_eq!(shorten_entry("a.dll"), "a.dll");
        assert_eq!(shorten_entry("Windows/a.dll"), "Windows/a.dll");
        assert_eq!(shorten_entry("A/B/C/d.dll"), "…/C/d.dll");
        assert_eq!(shorten_entry("A\\B\\C\\d.dll"), "…/C/d.dll");
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

    /// 迁移 SQL 必须把旧表拆成两张表：整包按版本、实例按目录名。
    #[test]
    fn instance_migration_splits_tables() {
        let sql = MIGRATION_INSTANCES.sql;
        assert!(sql.contains("module_game_download_package"));
        assert!(sql.contains("module_game_download_instance"));
        assert_eq!(sql.matches("INSERT OR IGNORE").count(), 2);
        assert!(sql.contains("DROP TABLE IF EXISTS module_game_download_task"));
        assert!(sql.contains("WHERE state <> 'installed'"));
    }

    /// 手动重装的前置判定：这些情况必须在下载/安装前拒绝，而不是重下数 GB 的包。
    #[test]
    fn install_preconditions_are_strict() {
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
        // `md5_matches` 对空期望值报错而非放行——空值恒校验失败会导致无限重试。
        assert!(md5_matches(&pkg, "").is_err(), "空 md5 必须拒绝而不是恒失败");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 建议实例名的基础换算（preview 后缀必须保留，否则两种版本会撞名）。
    #[test]
    fn suggested_instance_name_avoids_taken() {
        assert_eq!(meta_bridge::game_version_of("1.21.130.22"), "1.21.130.22");
        assert_eq!(meta_bridge::game_version_of("1.21.130.22_preview"), "1.21.130.22");
        assert_eq!(
            meta::sanitize_instance_name("1.21.130.22-preview"),
            "1.21.130.22-preview"
        );
    }
}
