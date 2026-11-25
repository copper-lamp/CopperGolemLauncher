//! 存储占用核算原语：分类表、可取消的目录遍历、白名单清理。
//!
//! 这里只做「量」和「删」两件事，不带任何状态：
//! 扫描编排、快照与清单缓存都在 storage_scan 里，清理入口也由它转调本模块。
//!
//! 遍历刻意避开两次全盘：
//! - 分类各自遍历自己的根，不再为求 total 再扫一遍整盘；
//! - 根路径只做一次 symlink_metadata，递归里不再逐目录 canonicalize；
//! - 遇到重解析点（junction / symlink）直接跳过，避免跨盘重复计数与环。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::error::KernelError;

use super::paths::Paths;
use super::settings::SettingsService;

/// 分类 ID 的固定顺序：先小后大，界面上先出结果的就是先扫完的。
pub const CATEGORY_IDS: [&str; 7] = [
    "content",
    "downloads",
    "loaders",
    "index",
    "logs",
    "game",
    "other",
];

/// 允许清理的分类（缓存、临时文件、日志、索引）。
pub const CLEANABLE_IDS: [&str; 5] = ["content", "downloads", "loaders", "index", "logs"];

/// 每遍历多少条目录项检查一次取消与进度。
const CHECK_INTERVAL: u64 = 2048;

/// 一个分类的根路径定义。
#[derive(Debug, Clone)]
pub struct CategoryDef {
    pub id: &'static str,
    pub roots: Vec<PathBuf>,
    pub cleanable: bool,
}

/// 遍历控制：取消信号与进度回调（回调按目录项节奏调用，天然节流）。
pub struct Walk<'a> {
    pub cancel: Option<&'a AtomicBool>,
    pub on_progress: Option<&'a dyn Fn(u64, u64)>,
}

impl<'a> Walk<'a> {
    pub fn cancelled(&self) -> bool {
        self.cancel
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
    }
}

impl Default for Walk<'_> {
    fn default() -> Self {
        Self {
            cancel: None,
            on_progress: None,
        }
    }
}

/// 遍历结果：字节数、文件数、被跳过的条目数。
#[derive(Debug, Default, Clone, Copy)]
pub struct WalkTotals {
    pub bytes: u64,
    pub files: u64,
    pub skipped: u64,
}

/// 是否重解析点（Windows junction / symlink，Unix symlink）。
#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

/// 遍历一个根，累加普通文件字节数。
///
/// 出错不冒泡：读不了的目录计入 skipped 继续走，扫描不能因为一个坏目录整盘失败。
/// excludes 按路径全等排除（用来从仓库根里挖掉分类根，避免重复计数）。
pub fn walk(root: &Path, excludes: &[PathBuf], control: &Walk) -> WalkTotals {
    let mut totals = WalkTotals::default();
    if excludes.iter().any(|exclude| exclude == root) {
        return totals;
    }
    let meta = match std::fs::symlink_metadata(root) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return totals,
        Err(_) => {
            totals.skipped += 1;
            return totals;
        }
    };
    if is_reparse_point(&meta) {
        return totals;
    }
    if meta.is_file() {
        totals.bytes = meta.len();
        totals.files = 1;
        return totals;
    }
    if !meta.is_dir() {
        return totals;
    }

    let mut stack = vec![root.to_path_buf()];
    let mut seen: u64 = 0;
    while let Some(dir) = stack.pop() {
        if control.cancelled() {
            return totals;
        }
        let reader = match std::fs::read_dir(&dir) {
            Ok(reader) => reader,
            Err(_) => {
                totals.skipped += 1;
                continue;
            }
        };
        for entry in reader {
            seen += 1;
            if seen % CHECK_INTERVAL == 0 {
                if control.cancelled() {
                    return totals;
                }
                if let Some(callback) = control.on_progress {
                    callback(totals.files, totals.bytes);
                }
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    totals.skipped += 1;
                    continue;
                }
            };
            let path = entry.path();
            if excludes.iter().any(|exclude| *exclude == path) {
                continue;
            }
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(_) => {
                    totals.skipped += 1;
                    continue;
                }
            };
            if is_reparse_point(&meta) {
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                totals.bytes = totals.bytes.saturating_add(meta.len());
                totals.files += 1;
            }
        }
    }
    totals
}

/// 遍历一个可能是重解析点的根：先解析到真实目录再走。
///
/// 实例的数据目录在 GDK / UWP 实例里常是指向 LocalState 的 junction，
/// 直接按路径走会停在链接本身上；这里只对根做一次 canonicalize。
pub fn walk_following_root(root: &Path, excludes: &[PathBuf], control: &Walk) -> WalkTotals {
    match std::fs::canonicalize(root) {
        Ok(real) => walk(&real, excludes, control),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => WalkTotals::default(),
        Err(_) => WalkTotals {
            bytes: 0,
            files: 0,
            skipped: 1,
        },
    }
}

/// 分类根定义（顺序即扫描顺序的前 6 个，other 由剩余部分推导）。
pub fn categories(paths: &Paths, settings: &SettingsService) -> Vec<CategoryDef> {
    let cache = paths.cache_dir();
    let logs = paths.logs_dir();
    let versions = paths.versions_root(settings);
    let packages = paths.package_root(settings);
    vec![
        CategoryDef {
            id: "content",
            roots: vec![cache.join("content"), cache.join("global-import")],
            cleanable: true,
        },
        CategoryDef {
            id: "downloads",
            roots: vec![
                cache.join("game-download"),
                cache.join("system-installers"),
                packages,
            ],
            cleanable: true,
        },
        CategoryDef {
            id: "loaders",
            roots: vec![cache.join("lip")],
            cleanable: true,
        },
        CategoryDef {
            id: "index",
            roots: vec![
                cache.join("registry"),
                cache.join("gdkshared"),
                cache.join("metadata.sqlite"),
            ],
            cleanable: true,
        },
        CategoryDef {
            id: "logs",
            roots: vec![logs.to_path_buf()],
            cleanable: true,
        },
        CategoryDef {
            id: "game",
            roots: vec![versions],
            cleanable: false,
        },
        CategoryDef {
            id: "other",
            roots: Vec::new(),
            cleanable: false,
        },
    ]
}

/// 需要统计全部占用时的顶层根（去重、去掉被另一个根包含的根）。
pub fn managed_roots(paths: &Paths, settings: &SettingsService) -> Vec<PathBuf> {
    let mut roots = vec![
        paths.data_dir().clone(),
        paths.cache_dir().clone(),
        paths.versions_root(settings),
        paths.package_root(settings),
    ];
    roots.sort();
    roots.dedup();
    let mut kept: Vec<PathBuf> = Vec::new();
    for root in roots {
        if kept.iter().any(|parent| root.starts_with(parent)) {
            continue;
        }
        kept.push(root);
    }
    kept
}

/// 分类根路径的扁平集合（供 other 排除）。
pub fn category_roots(paths: &Paths, settings: &SettingsService) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = categories(paths, settings)
        .into_iter()
        .flat_map(|def| def.roots)
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

/// 清理结果：释放字节数与各分类实际删除的字节数。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanReport {
    pub freed_bytes: u64,
    pub cleared: HashMap<String, u64>,
}

/// 按分类清理白名单目录。
///
/// 只删 cleanable 分类，且每个目标都要通过 clear_allowlisted 的边界校验；
/// 释放字节数直接累加每个目标实际删掉的字节数，不再前后全盘对比。
pub fn clean(
    paths: &Paths,
    settings: &SettingsService,
    requested: &[String],
) -> Result<CleanReport, KernelError> {
    let defs = categories(paths, settings);
    let mut wanted: Vec<&CategoryDef> = Vec::new();
    for id in requested {
        let def = defs
            .iter()
            .find(|def| def.id == id)
            .ok_or_else(|| KernelError::InvalidArgument(format!("未知存储分类: {id}")))?;
        if !def.cleanable {
            return Err(KernelError::InvalidArgument(format!("分类不可清理: {id}")));
        }
        if !wanted.iter().any(|item| item.id == def.id) {
            wanted.push(def);
        }
    }

    let data_root = paths.data_dir();
    let mut report = CleanReport {
        freed_bytes: 0,
        cleared: HashMap::new(),
    };
    for def in wanted {
        let mut freed = 0u64;
        for root in &def.roots {
            freed = freed.saturating_add(clear_allowlisted(root, &data_root, root)?);
        }
        report.freed_bytes = report.freed_bytes.saturating_add(freed);
        report.cleared.insert(def.id.to_string(), freed);
    }
    Ok(report)
}

/// 清空一个白名单目标的内容，返回删除的字节数。
///
/// 这是一个破坏性入口，因此边界校验写死在这里：目标必须是调用方声明的那个路径、
/// 不能等于数据根、也不能是数据根的祖先。
fn clear_allowlisted(target: &Path, data_root: &Path, expected: &Path) -> Result<u64, KernelError> {
    if target != expected {
        return Err(KernelError::InvalidArgument(format!(
            "清理目标与白名单不一致: {}",
            target.display()
        )));
    }
    if target == data_root {
        return Err(KernelError::InvalidArgument("拒绝清理数据根目录".into()));
    }
    if data_root.starts_with(target) {
        return Err(KernelError::InvalidArgument(
            "拒绝清理包含数据根的目录".into(),
        ));
    }

    let meta = match std::fs::symlink_metadata(target) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    if is_reparse_point(&meta) {
        return Err(KernelError::InvalidArgument(format!(
            "拒绝清理重解析点: {}",
            target.display()
        )));
    }
    if meta.is_file() {
        let bytes = meta.len();
        std::fs::remove_file(target)?;
        return Ok(bytes);
    }
    if !meta.is_dir() {
        return Err(KernelError::InvalidArgument(format!(
            "拒绝清理特殊文件: {}",
            target.display()
        )));
    }

    let bytes = walk(target, &[], &Walk::default()).bytes;
    remove_tree_contents(target)?;
    Ok(bytes)
}

/// 递归删除目录内容，但保留目录本身；遇到 symlink / 特殊文件直接拒绝，避免跟出盘外。
fn remove_tree_contents(dir: &Path) -> Result<(), KernelError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(KernelError::InvalidArgument(format!(
                "拒绝删除符号链接: {}",
                path.display()
            )));
        }
        if file_type.is_dir() {
            remove_tree_contents(&path)?;
            std::fs::remove_dir(&path)?;
        } else if file_type.is_file() {
            std::fs::remove_file(&path)?;
        } else {
            return Err(KernelError::InvalidArgument(format!(
                "拒绝删除特殊文件: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::events::EventBus;
    use crate::services::database::DatabaseService;
    use crate::services::paths::Paths;
    use crate::services::settings::SettingsService;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn fixture() -> (tempfile::TempDir, Paths, SettingsService) {
        let temp = tempfile::tempdir().expect("temp dir");
        let paths = Paths::with_root(temp.path().join("data"));
        let db = DatabaseService::open(paths.db_file()).expect("db");
        db.migrate_scope("core", crate::services::database::CORE_MIGRATIONS)
            .expect("migrations");
        let settings = SettingsService::new(Arc::new(db), Arc::new(EventBus::new()), HashMap::new())
            .expect("settings");
        (temp, paths, settings)
    }

    fn write_bytes(path: &Path, len: usize) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(path, vec![0u8; len]).expect("write");
    }

    #[test]
    fn categories_keep_stable_order() {
        let (_temp, paths, settings) = fixture();
        let defs = categories(&paths, &settings);
        let ids: Vec<&str> = defs.iter().map(|def| def.id).collect();
        assert_eq!(ids, CATEGORY_IDS.to_vec());
    }

    #[test]
    fn clean_accumulates_freed_bytes_per_category() {
        let (_temp, paths, settings) = fixture();
        let cache = paths.cache_dir();
        write_bytes(&cache.join("content").join("a.bin"), 4096);
        write_bytes(&cache.join("lip").join("b.bin"), 2048);

        let report =
            clean(&paths, &settings, &["content".into(), "loaders".into()]).expect("clean");
        assert_eq!(report.freed_bytes, 6144);
        assert_eq!(report.cleared.get("content").copied(), Some(4096));
        assert_eq!(report.cleared.get("loaders").copied(), Some(2048));
        assert!(!cache.join("content").join("a.bin").exists());

        let again = clean(&paths, &settings, &["content".into()]).expect("idempotent");
        assert_eq!(again.freed_bytes, 0);
    }

    #[test]
    fn clean_refuses_uncleanable_and_escaping_targets() {
        let (_temp, paths, settings) = fixture();
        let game = clean(&paths, &settings, &["game".into()]);
        assert!(matches!(game, Err(KernelError::InvalidArgument(_))));
        let unknown = clean(&paths, &settings, &["nope".into()]);
        assert!(matches!(unknown, Err(KernelError::InvalidArgument(_))));

        let data_root = paths.data_dir();
        let inside = data_root.join("logs");
        std::fs::create_dir_all(&inside).expect("logs");
        let escaped = clear_allowlisted(&inside, &data_root, &data_root);
        assert!(matches!(escaped, Err(KernelError::InvalidArgument(_))));
    }

    #[test]
    fn walk_counts_files_and_skips_missing() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        write_bytes(&root.join("a").join("1.bin"), 100);
        write_bytes(&root.join("b").join("2.bin"), 200);
        let totals = walk(root, &[], &Walk::default());
        assert_eq!(totals.bytes, 300);
        assert_eq!(totals.files, 2);
        let missing = walk(&root.join("missing"), &[], &Walk::default());
        assert_eq!(missing.bytes, 0);
    }
}
