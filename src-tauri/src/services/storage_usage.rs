//! Storage accounting and narrowly allowlisted cache cleanup.
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::KernelError;
use super::paths::Paths;
use super::settings::SettingsService;

const CATEGORY_IDS: [&str; 7] = ["content", "downloads", "loaders", "index", "logs", "game", "other"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryUsage {
    pub id: String,
    pub bytes: u64,
    pub cleanable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageUsage {
    pub root: String,
    pub total_bytes: u64,
    pub categories: Vec<CategoryUsage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCleanResult {
    pub freed_bytes: u64,
    pub usage: StorageUsage,
}

fn normalized(path: &Path) -> Result<PathBuf, KernelError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(path.canonicalize()?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or_else(|| KernelError::InvalidArgument("存储路径无效".into()))?;
            if parent == path { return Ok(path.to_owned()); }
            let name = path.file_name().ok_or_else(|| KernelError::InvalidArgument("存储路径无效".into()))?;
            Ok(normalized(parent)?.join(name))
        }
        Err(error) => Err(error.into()),
    }
}

fn collect_excluding(path: &Path, excluded: &[PathBuf]) -> Result<u64, KernelError> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    if meta.file_type().is_symlink() { return Ok(0); }
    let canonical = path.canonicalize()?;
    if excluded.iter().any(|excluded| canonical == *excluded) { return Ok(0); }
    if meta.is_file() { return Ok(meta.len()); }
    if !meta.is_dir() { return Ok(0); }
    let mut total = 0u64;
    for entry in std::fs::read_dir(path)? {
        total = total.saturating_add(collect_excluding(&entry?.path(), excluded)?);
    }
    Ok(total)
}

fn category_paths(paths: &Paths, settings: &SettingsService) -> Vec<(&'static str, Vec<PathBuf>, bool)> {
    let cache = paths.cache_dir();
    vec![
        ("content", vec![cache.join("content"), cache.join("global-import")], true),
        ("downloads", vec![cache.join("game-download"), cache.join("system-installers"), paths.package_root(settings)], true),
        ("loaders", vec![cache.join("lip")], true),
        ("index", vec![cache.join("registry"), cache.join("gdkshared"), cache.join("metadata.sqlite")], true),
        ("logs", vec![paths.logs_dir().clone()], true),
        ("game", vec![paths.versions_root(settings)], false),
        ("other", Vec::new(), false),
    ]
}

fn unique_roots(paths: &Paths, settings: &SettingsService) -> Result<Vec<PathBuf>, KernelError> {
    let mut roots = vec![paths.data_dir().clone(), paths.cache_dir().clone(), paths.versions_root(settings), paths.package_root(settings)];
    for root in &mut roots { *root = normalized(root)?; }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

fn total_bytes(paths: &Paths, settings: &SettingsService) -> Result<u64, KernelError> {
    let roots = unique_roots(paths, settings)?;
    let mut total = 0u64;
    for (index, root) in roots.iter().enumerate() {
        let excluded = roots[..index].iter().filter(|previous| root.starts_with(previous)).cloned().collect::<Vec<_>>();
        total = total.saturating_add(collect_excluding(root, &excluded)?);
    }
    Ok(total)
}

pub fn usage(paths: &Paths, settings: &SettingsService) -> Result<StorageUsage, KernelError> {
    let roots = unique_roots(paths, settings)?;
    let mut categories = Vec::with_capacity(CATEGORY_IDS.len());
    let mut categorized = 0u64;
    for (id, category_roots, cleanable) in category_paths(paths, settings).into_iter().take(6) {
        let mut bytes = 0u64;
        for root in category_roots {
            let normalized_root = normalized(&root)?;
            let excluded = roots.iter().filter(|managed| normalized_root != **managed && managed.starts_with(&normalized_root)).cloned().collect::<Vec<_>>();
            bytes = bytes.saturating_add(collect_excluding(&root, &excluded)?);
        }
        categorized = categorized.saturating_add(bytes);
        categories.push(CategoryUsage { id: id.to_owned(), bytes, cleanable });
    }
    let total_bytes = total_bytes(paths, settings)?;
    categories.push(CategoryUsage { id: "other".into(), bytes: total_bytes.saturating_sub(categorized), cleanable: false });
    Ok(StorageUsage { root: paths.launcher_root().display().to_string(), total_bytes, categories })
}

fn tree_bytes(root: &Path) -> Result<u64, KernelError> { collect_excluding(root, &[]) }

fn remove_tree_contents(path: &Path) -> Result<(), KernelError> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() { return Err(KernelError::InvalidArgument("清理目录中发现符号链接，已停止以保护目录边界".into())); }
    if !meta.is_dir() { return Err(KernelError::InvalidArgument("清理目标不是目录".into())); }
    for entry in std::fs::read_dir(path)? {
        let child = entry?.path();
        let child_meta = std::fs::symlink_metadata(&child)?;
        if child_meta.file_type().is_symlink() { std::fs::remove_file(child)?; }
        else if child_meta.is_dir() { remove_tree_contents(&child)?; std::fs::remove_dir(child)?; }
        else if child_meta.is_file() { std::fs::remove_file(child)?; }
        else { return Err(KernelError::InvalidArgument("清理目标包含特殊文件，已停止".into())); }
    }
    Ok(())
}

fn clear_allowlisted(path: &Path, data_root: &Path, expected: &Path) -> Result<u64, KernelError> {
    let target = normalized(path)?;
    let data = normalized(data_root)?;
    let expected = normalized(expected)?;
    if target != expected || target == data || data.starts_with(&target) {
        return Err(KernelError::InvalidArgument("拒绝清理白名单外或位于数据根之外的路径".into()));
    }
    if !path.exists() { return Ok(0); }
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() { return Err(KernelError::InvalidArgument("清理目标是符号链接，已拒绝".into())); }
    if meta.is_file() {
        let size = meta.len();
        std::fs::remove_file(path)?;
        return Ok(size);
    }
    if !meta.is_dir() { return Err(KernelError::InvalidArgument("清理目标不是普通文件或目录".into())); }
    let size = tree_bytes(path)?;
    remove_tree_contents(path)?;
    Ok(size)
}

pub fn clean(paths: &Paths, settings: &SettingsService, requested: &[String]) -> Result<StorageCleanResult, KernelError> {
    let before = usage(paths, settings)?;
    let definitions = category_paths(paths, settings);
    let allowed = ["content", "downloads", "loaders", "index", "logs"];
    let data_root = paths.data_dir();
    for id in requested {
        if !allowed.contains(&id.as_str()) { continue; }
        let Some((_, roots, _)) = definitions.iter().find(|(category, _, _)| *category == id) else { continue };
        for root in roots {
            // Targets are derived solely from the fixed category map, never from caller-supplied paths.
            let _ = clear_allowlisted(root, data_root, root)?;
        }
    }
    let current = usage(paths, settings)?;
    let measured_freed = before.total_bytes.saturating_sub(current.total_bytes);
    Ok(StorageCleanResult { freed_bytes: measured_freed, usage: current })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::registry::events::EventBus;
    use crate::services::database::{DatabaseService, CORE_MIGRATIONS};

    fn fixture() -> (PathBuf, Paths, SettingsService) {
        let root = std::env::temp_dir().join(format!("storage-usage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("data")).unwrap();
        let paths = Paths::with_root(root.clone());
        let db = Arc::new(DatabaseService::open(paths.db_file()).unwrap());
        db.migrate_scope("core", CORE_MIGRATIONS).unwrap();
        let settings = SettingsService::new(db, Arc::new(EventBus::new()), std::collections::HashMap::new()).unwrap();
        (root, paths, settings)
    }

    #[test]
    fn usage_categories_follow_frontend_contract_and_game_is_protected() {
        let (root, paths, settings) = fixture();
        let result = usage(&paths, &settings).unwrap();
        assert_eq!(result.root, paths.launcher_root().display().to_string());
        assert_eq!(result.categories.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(), CATEGORY_IDS);
        assert!(!result.categories.iter().find(|item| item.id == "game").unwrap().cleanable);
        assert!(!result.categories.iter().find(|item| item.id == "other").unwrap().cleanable);
        drop(settings);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_classifies_paths_and_batch_clean_returns_freed_and_latest_usage() {
        let (root, paths, settings) = fixture();
        let cache = paths.cache_dir();
        std::fs::create_dir_all(cache.join("content")).unwrap();
        std::fs::write(cache.join("content/item"), [0u8; 7]).unwrap();
        std::fs::create_dir_all(cache.join("game-download")).unwrap();
        std::fs::write(cache.join("game-download/item"), [0u8; 5]).unwrap();
        std::fs::create_dir_all(paths.package_root(&settings)).unwrap();
        std::fs::write(paths.package_root(&settings).join("pkg"), [0u8; 3]).unwrap();
        std::fs::create_dir_all(paths.versions_dir()).unwrap();
        std::fs::write(paths.versions_dir().join("game.bin"), [0u8; 11]).unwrap();
        let result = usage(&paths, &settings).unwrap();
        assert_eq!(result.categories[0].bytes, 7);
        assert_eq!(result.categories[1].bytes, 8);
        assert_eq!(result.categories[5].bytes, 11);
        let cleaned = clean(&paths, &settings, &["content".into(), "game".into(), "other".into(), "unknown".into()]).unwrap();
        assert_eq!(cleaned.freed_bytes, 7);
        assert_eq!(cleaned.usage.categories[0].bytes, 0);
        assert_eq!(cleaned.usage.categories[5].bytes, 11);
        assert!(paths.versions_dir().join("game.bin").is_file());
        drop(settings);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clean_refuses_allowlist_targets_that_escape_data_root() {
        let (root, paths, settings) = fixture();
        let outside = std::env::temp_dir().join(format!("storage-outside-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&outside).unwrap();
        // Simulate a managed cache path that resolves outside the data root.
        let expected = paths.cache_dir().join("content");
        let result = clear_allowlisted(&outside, paths.data_dir(), &expected);
        assert!(result.is_err());
        drop(settings);
        std::fs::remove_dir_all(outside).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
