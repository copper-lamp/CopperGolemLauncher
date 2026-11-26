//! 后台存储扫描：内存快照 + 单飞工作线程 + 清单缓存 + 阶段事件。
//!
//! 设计要点（见 docs/fix3-存储用量页重做设计.md）：
//! - 命令层永远不做重 IO：读快照是纯内存，启动扫描只是拉起线程后立刻返回；
//! - 同一时刻只允许一个扫描线程，force 会先取消旧线程再开新的一代；
//! - 只统计可清理的缓存 / 临时文件 / 日志，游戏本体与实例数据不进分类表；
//! - 小分类先扫，谁扫完谁先推事件，前端按分类渐进填充；
//! - 清理前先取消并等待扫描退出，释放字节数由清理本身累加。

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::KernelError;
use crate::registry::events::EventBus;

use super::database::DatabaseService;
use super::paths::Paths;
use super::settings::SettingsService;
use super::storage_usage::{self, CategoryDef, CategoryKind, CleanReport, Walk, WalkTotals};
use super::translation;

/// 清单文件名（放在 cache 根下）。
const MANIFEST_FILE: &str = "storage-scan-manifest.json";
/// 清单结构版本；不匹配直接当没有清单，退回冷扫。
const MANIFEST_SCHEMA: u32 = 2;
/// 进度事件最小间隔。
const PROGRESS_INTERVAL_MS: u64 = 200;

/// 扫描状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanState {
    Idle,
    Running,
    Ready,
    Failed,
}

/// 单分类状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CategoryState {
    Pending,
    Running,
    Done,
    Cached,
    Failed,
}

/// 单分类扫描结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryScan {
    pub id: String,
    /// 未知（还没扫到）为 None，前端显示占位而不是 0。
    pub bytes: Option<u64>,
    pub cleanable: bool,
    pub state: CategoryState,
    pub files: u64,
}

/// 扫描快照：前端一次读取就能画完整界面。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSnapshot {
    pub scan_id: u64,
    pub state: ScanState,
    /// 当前数据根，仅用于界面显示「统计范围」。
    pub root: String,
    pub total_bytes: Option<u64>,
    pub files: u64,
    pub categories: Vec<CategoryScan>,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    pub error: Option<String>,
}

struct ScanShared {
    snapshot: ScanSnapshot,
    running: bool,
    generation: u64,
    cancel: Arc<AtomicBool>,
}

static SHARED: OnceLock<Mutex<ScanShared>> = OnceLock::new();

fn shared() -> &'static Mutex<ScanShared> {
    SHARED.get_or_init(|| {
        Mutex::new(ScanShared {
            snapshot: empty_snapshot(),
            running: false,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
        })
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

fn publish(events: &EventBus, name: &str, payload: serde_json::Value) {
    events.publish(name, payload);
}

fn category_shell(paths: &Paths, settings: &SettingsService) -> Vec<CategoryScan> {
    storage_usage::categories(paths, settings)
        .into_iter()
        .map(|def| CategoryScan {
            id: def.id.to_string(),
            bytes: None,
            cleanable: def.cleanable,
            state: CategoryState::Pending,
            files: 0,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 清单缓存：只缓存分类结果，供下次进入界面秒开
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestRoots {
    data: String,
    cache: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema: u32,
    roots: ManifestRoots,
    categories: HashMap<String, u64>,
    total_bytes: Option<u64>,
    at: u64,
}

fn manifest_path(paths: &Paths) -> PathBuf {
    paths.cache_dir().join(MANIFEST_FILE)
}

fn roots_key(paths: &Paths) -> ManifestRoots {
    ManifestRoots {
        data: paths.data_dir().to_string_lossy().into_owned(),
        cache: paths.cache_dir().to_string_lossy().into_owned(),
    }
}

fn load_manifest(paths: &Paths) -> Option<Manifest> {
    let text = std::fs::read_to_string(manifest_path(paths)).ok()?;
    let manifest: Manifest = serde_json::from_str(&text).ok()?;
    if manifest.schema != MANIFEST_SCHEMA {
        return None;
    }
    if manifest.roots != roots_key(paths) {
        return None;
    }
    Some(manifest)
}

fn store_manifest(paths: &Paths, manifest: &Manifest) {
    let path = manifest_path(paths);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let temporary = path.with_extension("json.tmp");
    let Ok(bytes) = serde_json::to_vec_pretty(manifest) else {
        return;
    };
    if std::fs::write(&temporary, bytes).is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
}

/// 用清单里的缓存值构造「秒开」快照：分类值直接落位，后台扫描只做校正。
fn seeded_snapshot(paths: &Paths, settings: &SettingsService, manifest: &Manifest) -> ScanSnapshot {
    let categories = storage_usage::categories(paths, settings)
        .into_iter()
        .map(|def| {
            let cached = manifest.categories.get(def.id).copied();
            CategoryScan {
                id: def.id.to_string(),
                bytes: cached,
                cleanable: def.cleanable,
                state: if cached.is_some() {
                    CategoryState::Cached
                } else {
                    CategoryState::Pending
                },
                files: 0,
            }
        })
        .collect();
    ScanSnapshot {
        scan_id: 0,
        state: if manifest.total_bytes.is_some() {
            ScanState::Ready
        } else {
            ScanState::Idle
        },
        root: paths.data_dir().to_string_lossy().into_owned(),
        total_bytes: manifest.total_bytes,
        files: 0,
        categories,
        started_at: None,
        finished_at: Some(manifest.at),
        error: None,
    }
}

// ---------------------------------------------------------------------------
// 对外接口
// ---------------------------------------------------------------------------

fn empty_snapshot() -> ScanSnapshot {
    ScanSnapshot {
        scan_id: 0,
        state: ScanState::Idle,
        root: String::new(),
        total_bytes: None,
        files: 0,
        categories: Vec::new(),
        started_at: None,
        finished_at: None,
        error: None,
    }
}

/// 读当前快照（零重 IO）。首次调用会尝试用清单里的缓存值秒开界面。
pub fn snapshot(paths: &Paths, settings: &SettingsService) -> ScanSnapshot {
    let mut state = shared().lock();
    if state.snapshot.scan_id == 0 && state.snapshot.categories.is_empty() {
        state.snapshot = match load_manifest(paths) {
            Some(manifest) => seeded_snapshot(paths, settings, &manifest),
            None => ScanSnapshot {
                root: paths.data_dir().to_string_lossy().into_owned(),
                categories: category_shell(paths, settings),
                ..empty_snapshot()
            },
        };
    }
    state.snapshot.clone()
}

/// 启动（或复用）一次后台扫描，立即返回启动瞬间的快照。
pub fn start(
    paths: Arc<Paths>,
    settings: Arc<SettingsService>,
    db: Arc<DatabaseService>,
    events: Arc<EventBus>,
    force: bool,
) -> ScanSnapshot {
    let mut state = shared().lock();
    if state.running {
        if !force {
            return state.snapshot.clone();
        }
        state.cancel.store(true, Ordering::SeqCst);
    }

    state.generation = state.generation.wrapping_add(1);
    let generation = state.generation;
    let cancel = Arc::new(AtomicBool::new(false));
    state.cancel = Arc::clone(&cancel);
    state.running = true;

    let previous = state.snapshot.clone();
    // 上一代已量到的值先留着（标成 Cached），后台重扫时逐类覆盖，
    // 界面不会在重扫瞬间掉回空白。
    let categories = if previous.categories.is_empty() {
        category_shell(paths.as_ref(), settings.as_ref())
    } else {
        previous
            .categories
            .into_iter()
            .map(|category| CategoryScan {
                state: if category.bytes.is_some() {
                    CategoryState::Cached
                } else {
                    CategoryState::Pending
                },
                ..category
            })
            .collect()
    };
    state.snapshot = ScanSnapshot {
        scan_id: generation,
        state: ScanState::Running,
        root: paths.data_dir().to_string_lossy().into_owned(),
        total_bytes: previous.total_bytes,
        files: 0,
        categories,
        started_at: Some(now_ms()),
        finished_at: previous.finished_at,
        error: None,
    };
    let initial = state.snapshot.clone();
    drop(state);

    let events_for_error = Arc::clone(&events);
    let spawned = std::thread::Builder::new()
        .name("storage-scan".into())
        .spawn(move || worker(generation, cancel, paths, settings, db, events));
    if let Err(error) = spawned {
        fail(generation, &events_for_error, &format!("无法启动扫描线程: {error}"));
    }
    initial
}

/// 取消当前扫描并等待它退出；返回是否已经停下。
pub fn cancel_and_wait(timeout: Duration) -> bool {
    {
        let state = shared().lock();
        if !state.running {
            return true;
        }
        state.cancel.store(true, Ordering::SeqCst);
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !shared().lock().running {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    !shared().lock().running
}

/// 清理指定分类。会先停下扫描，避免边删边算。
pub fn clean(
    paths: &Paths,
    settings: &SettingsService,
    db: &DatabaseService,
    events: &EventBus,
    requested: &[String],
) -> Result<CleanReport, KernelError> {
    if !cancel_and_wait(Duration::from_secs(2)) {
        return Err(KernelError::Conflict(
            "存储扫描仍在进行，请稍后再清理".into(),
        ));
    }
    let report = storage_usage::clean(paths, settings, db, requested)?;
    if report.freed_bytes > 0 {
        let mut state = shared().lock();
        for (id, freed) in &report.cleared {
            if let Some(category) = state.snapshot.categories.iter_mut().find(|item| &item.id == id)
            {
                category.bytes = Some(category.bytes.unwrap_or(0).saturating_sub(*freed));
                category.state = CategoryState::Done;
            }
        }
        if let Some(total) = state.snapshot.total_bytes {
            state.snapshot.total_bytes = Some(total.saturating_sub(report.freed_bytes));
        }
        drop(state);
        publish(
            events,
            "storage.cleaned",
            json!({ "freedBytes": report.freed_bytes, "cleared": report.cleared }),
        );
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// 工作线程
// ---------------------------------------------------------------------------

fn set_category(
    generation: u64,
    id: &str,
    category_state: CategoryState,
    bytes: Option<u64>,
    files: u64,
) -> Option<CategoryScan> {
    let mut state = shared().lock();
    if state.generation != generation || state.snapshot.scan_id != generation {
        return None;
    }
    let category = state
        .snapshot
        .categories
        .iter_mut()
        .find(|item| item.id == id)?;
    category.state = category_state;
    if bytes.is_some() {
        category.bytes = bytes;
    }
    category.files = files;
    Some(category.clone())
}

fn publish_category(
    generation: u64,
    events: &EventBus,
    id: &str,
    category_state: CategoryState,
    bytes: Option<u64>,
    files: u64,
) {
    if let Some(category) = set_category(generation, id, category_state, bytes, files) {
        publish(
            events,
            "storage.category-scanned",
            json!({ "scanId": generation, "category": category }),
        );
    }
}

fn bump_progress(generation: u64, files: u64) {
    let mut state = shared().lock();
    if state.generation != generation {
        return;
    }
    state.snapshot.files = files;
}

fn fail(generation: u64, events: &EventBus, message: &str) {
    {
        let mut state = shared().lock();
        if state.generation != generation {
            return;
        }
        state.running = false;
        state.snapshot.state = ScanState::Failed;
        state.snapshot.error = Some(message.to_string());
        state.snapshot.finished_at = Some(now_ms());
    }
    publish(
        events,
        "storage.scan-failed",
        json!({ "scanId": generation, "error": message }),
    );
}

fn cancelled(generation: u64, events: &EventBus) {
    {
        let mut state = shared().lock();
        if state.generation != generation {
            return;
        }
        state.running = false;
        state.snapshot.state = ScanState::Idle;
        for category in &mut state.snapshot.categories {
            if matches!(category.state, CategoryState::Running | CategoryState::Pending) {
                category.state = CategoryState::Pending;
                category.bytes = None;
            }
        }
    }
    publish(events, "storage.scan-cancelled", json!({ "scanId": generation }));
}

fn worker(
    generation: u64,
    cancel: Arc<AtomicBool>,
    paths: Arc<Paths>,
    settings: Arc<SettingsService>,
    db: Arc<DatabaseService>,
    events: Arc<EventBus>,
) {
    publish(&events, "storage.scan-started", json!({ "scanId": generation }));

    let mut manifest = load_manifest(&paths).unwrap_or_else(|| Manifest {
        schema: MANIFEST_SCHEMA,
        roots: roots_key(&paths),
        ..Manifest::default()
    });

    let defs = storage_usage::categories(&paths, &settings);
    let mut totals: HashMap<String, u64> = HashMap::new();
    let mut files_total = 0u64;

    for id in storage_usage::CATEGORY_IDS {
        if cancel.load(Ordering::Relaxed) {
            cancelled(generation, &events);
            return;
        }
        let Some(def) = defs.iter().find(|def| def.id == id) else {
            continue;
        };
        publish_category(generation, &events, def.id, CategoryState::Running, None, 0);
        let outcome = walk_category(generation, def, &db, &cancel, &events);
        totals.insert(def.id.to_string(), outcome.bytes);
        files_total = files_total.saturating_add(outcome.files);
        if cancel.load(Ordering::Relaxed) {
            cancelled(generation, &events);
            return;
        }
        publish_category(
            generation,
            &events,
            def.id,
            CategoryState::Done,
            Some(outcome.bytes),
            outcome.files,
        );
    }

    let total: u64 = totals.values().copied().sum();
    manifest.categories = totals;
    manifest.total_bytes = Some(total);
    manifest.at = now_ms();
    store_manifest(&paths, &manifest);

    let mut state = shared().lock();
    if state.generation != generation {
        return;
    }
    state.running = false;
    let snapshot = &mut state.snapshot;
    snapshot.state = ScanState::Ready;
    snapshot.total_bytes = Some(total);
    snapshot.files = files_total;
    snapshot.finished_at = Some(now_ms());
    for category in &mut snapshot.categories {
        if category.state == CategoryState::Cached {
            category.state = CategoryState::Done;
        }
    }
    let final_snapshot = snapshot.clone();
    drop(state);
    publish(
        &events,
        "storage.scan-finished",
        json!({ "scanId": generation, "snapshot": final_snapshot }),
    );
}

/// 量一个分类：目录分类逐个根走，翻译缓存查表。
fn walk_category(
    generation: u64,
    def: &CategoryDef,
    db: &DatabaseService,
    cancel: &Arc<AtomicBool>,
    events: &Arc<EventBus>,
) -> WalkTotals {
    let base = Cell::new(0u64);
    let base_files = Cell::new(0u64);
    let last = Cell::new(0u64);
    let category = def.id;
    let progress = |files: u64, bytes: u64| {
        let now = now_ms();
        if now.saturating_sub(last.get()) < PROGRESS_INTERVAL_MS {
            return;
        }
        last.set(now);
        bump_progress(generation, base_files.get() + files);
        publish(
            events,
            "storage.scan-progress",
            json!({
                "scanId": generation,
                "category": category,
                "files": base_files.get() + files,
                "bytes": base.get() + bytes,
            }),
        );
    };

    if def.kind == CategoryKind::Translation {
        return match translation::cache_usage(db) {
            Ok(usage) => {
                progress(usage.entries, usage.bytes);
                WalkTotals {
                    bytes: usage.bytes,
                    files: usage.entries,
                    skipped: 0,
                }
            }
            // 数据库被占用之类的偶发失败不该让整个扫描失败，记一次跳过。
            Err(_) => WalkTotals {
                bytes: 0,
                files: 0,
                skipped: 1,
            },
        };
    }

    let control = Walk {
        cancel: Some(cancel.as_ref()),
        on_progress: Some(&progress),
    };
    let mut totals = WalkTotals::default();
    for root in &def.roots {
        let part = storage_usage::walk(root, &[], &control);
        totals.bytes = totals.bytes.saturating_add(part.bytes);
        totals.files = totals.files.saturating_add(part.files);
        totals.skipped = totals.skipped.saturating_add(part.skipped);
        base.set(totals.bytes);
        base_files.set(totals.files);
        if cancel.load(Ordering::Relaxed) {
            return totals;
        }
    }
    totals
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_paths() -> (tempfile::TempDir, Paths, SettingsService) {
        use crate::registry::events::EventBus;
        use crate::services::database::DatabaseService;
        use std::collections::HashMap;

        let temp = tempfile::tempdir().expect("temp dir");
        let paths = Paths::with_root(temp.path().join("data"));
        let db = Arc::new(DatabaseService::open(paths.db_file()).expect("db"));
        db.migrate_scope("core", crate::services::database::CORE_MIGRATIONS)
            .expect("migrations");
        let settings = SettingsService::new(Arc::clone(&db), Arc::new(EventBus::new()), HashMap::new())
            .expect("settings");
        (temp, paths, settings)
    }

    #[test]
    fn manifest_roundtrip_requires_matching_roots() {
        let (_temp, paths, settings) = fixture_paths();
        let mut manifest = Manifest {
            schema: MANIFEST_SCHEMA,
            roots: roots_key(&paths),
            total_bytes: Some(42),
            at: 7,
            ..Manifest::default()
        };
        manifest.categories.insert("logs".into(), 42);
        store_manifest(&paths, &manifest);
        let loaded = load_manifest(&paths).expect("manifest");
        assert_eq!(loaded.total_bytes, Some(42));
        assert_eq!(loaded.categories.get("logs"), Some(&42));

        let seeded = seeded_snapshot(&paths, &settings, &loaded);
        assert_eq!(seeded.total_bytes, Some(42));
        assert!(seeded
            .categories
            .iter()
            .any(|category| category.id == "logs" && category.state == CategoryState::Cached));

        let mut broken = loaded;
        broken.schema = MANIFEST_SCHEMA + 1;
        store_manifest(&paths, &broken);
        assert!(load_manifest(&paths).is_none());
    }

    #[test]
    fn manifest_is_discarded_when_roots_change() {
        let (_temp, paths, _settings) = fixture_paths();
        let manifest = Manifest {
            schema: MANIFEST_SCHEMA,
            roots: roots_key(&paths),
            total_bytes: Some(1),
            ..Manifest::default()
        };
        store_manifest(&paths, &manifest);
        let other = Paths::with_root(paths.data_dir().join("elsewhere"));
        assert!(load_manifest(&other).is_none());
    }
}
