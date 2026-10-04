//! 内容管理：版本已加入资源（资源包 / 行为包 / 世界）的枚举、启用 / 禁用 / 删除。
//!
//! 内容根目录一律由 [`super::isolate`] 解析（隔离强制开启，规则只有一份）：
//! `<版本目录>/Minecraft Bedrock[/Preview]/Users/Shared/games/com.mojang`。
//!
//! 启用 / 禁用机制：加载目录（`resource_packs` 等）只装载其下条目，禁用即把条目
//! 移入同级 `*_backup` 目录，启用移回；删除则直接移除。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::KernelError;
use crate::state::KernelContext;

use super::isolate::{self, SHARED_GAME_DIR, WORLDS_DIR};
use super::meta::{resolve_version_dir, VersionMeta};

/// 内容根视图的对外转出：目录规则住在 `isolate`，但既有调用点
/// （`content_download::install_target` 等）按 `content::ContentRoots` 引用，
/// 保持这条路径不变以免规则与调用点两处都在动。
pub use super::isolate::ContentRoots;

/// 内容类型（前端过滤用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Resources,
    Behavior,
    Worlds,
    /// 模板（`development_behavior_packs`）。
    ///
    /// MCBE 不自动加载该目录（模板只在开启开发者模式时生效），但它仍是用户
    /// 通过内容下载装进来的东西，不列出来等于「装完就消失」。
    Templates,
}

impl ContentKind {
    fn as_str(self) -> &'static str {
        match self {
            ContentKind::Resources => "resources",
            ContentKind::Behavior => "behavior",
            ContentKind::Worlds => "worlds",
            ContentKind::Templates => "templates",
        }
    }
}

/// 各内容类型对应的加载目录名。
fn load_dir_name(kind: ContentKind) -> Option<&'static str> {
    match kind {
        ContentKind::Resources => Some("resource_packs"),
        ContentKind::Behavior => Some("behavior_packs"),
        ContentKind::Templates => Some("development_behavior_packs"),
        // 世界目录在玩家目录下，位置随实例变，不走固定名。
        ContentKind::Worlds => None,
    }
}

/// 内容条目。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ContentItem {
    /// 唯一 id：`<kind>:<名称>`。
    pub id: String,
    pub kind: ContentKind,
    pub name: String,
    pub enabled: bool,
    /// 条目当前绝对路径。
    pub path: String,
}

/// 解析版本内容根目录（读取侧口径）。
///
/// 目录规则全部委托给 [`isolate`]：隔离强制开启，没有「非隔离」分支，
/// 也不再需要 AppX 包族名查询（那条路径只在共享官方数据目录时才成立）。
pub fn content_roots(kernel: &KernelContext, name: &str) -> Result<ContentRoots, KernelError> {
    let dir = resolve_version_dir(&kernel.versions_root(), name)?;
    let meta = read_meta_for(&dir, name)?;
    Ok(isolate::content_roots_existing(&dir, &meta))
}

/// 解析版本内容根目录（写入侧口径：标准布局，并确保骨架存在）。
///
/// 内容删除 / 移动这类写操作必须落在标准布局上，不能因为读取侧探测到扁平
/// 布局就把内容写进另一个目录 —— 那样内容会出现在游戏读不到的地方。
pub fn content_roots_for_write(
    kernel: &KernelContext,
    name: &str,
) -> Result<ContentRoots, KernelError> {
    let dir = resolve_version_dir(&kernel.versions_root(), name)?;
    let meta = read_meta_for(&dir, name)?;
    isolate::ensure_skeleton(&dir, &meta)?;
    Ok(isolate::content_roots(&dir, &meta))
}

/// 读取实例元数据，缺失即报错（带实例名，便于前端定位）。
fn read_meta_for(dir: &Path, name: &str) -> Result<VersionMeta, KernelError> {
    VersionMeta::read(dir)
        .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{name}` 元数据缺失")))
}


/// 列出已加入资源。
pub fn list_content(kernel: &KernelContext, name: &str) -> Result<Vec<ContentItem>, KernelError> {
    let roots = content_roots(kernel, name)?;
    let mut items = Vec::new();
    collect_dir_items(
        &roots.com_mojang.join("resource_packs"),
        ContentKind::Resources,
        &roots.com_mojang,
        &mut items,
    );
    collect_dir_items(
        &roots.com_mojang.join("behavior_packs"),
        ContentKind::Behavior,
        &roots.com_mojang,
        &mut items,
    );
    collect_dir_items(
        &roots.com_mojang.join("development_behavior_packs"),
        ContentKind::Templates,
        &roots.com_mojang,
        &mut items,
    );
    if let Some(worlds) = first_player_worlds_dir(&roots) {
        collect_dir_items(&worlds, ContentKind::Worlds, &roots.com_mojang, &mut items);
    }
    Ok(items)
}

/// 枚举加载目录条目（目录 或 压缩包文件），并判定启用状态。
fn collect_dir_items(
    load_dir: &Path,
    kind: ContentKind,
    com_mojang: &Path,
    out: &mut Vec<ContentItem>,
) {
    if !load_dir.is_dir() {
        return;
    }
    let backup_dir = backup_dir_for(kind, com_mojang);
    let Ok(entries) = std::fs::read_dir(load_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() || is_archive(&name) {
            out.push(ContentItem {
                id: format!("{}:{name}", kind.as_str()),
                kind,
                name,
                enabled: true,
                path: path.to_string_lossy().into_owned(),
            });
        }
    }
    // 备份目录中的条目视为已禁用。
    if backup_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&backup_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if path.is_dir() || is_archive(&name) {
                    out.push(ContentItem {
                        id: format!("{}:{name}", kind.as_str()),
                        kind,
                        name,
                        enabled: false,
                        path: path.to_string_lossy().into_owned(),
                    });
                }
            }
        }
    }
}

/// 压缩包扩展名（MCBE 资源可打包为 zip / mcpack / mcaddon）。
fn is_archive(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".zip")
        || lower.ends_with(".mcpack")
        || lower.ends_with(".mcaddon")
        || lower.ends_with(".mctemplate")
}

/// 备份目录：`<com_mojang>/<加载目录名>_backup`。
fn backup_dir_for(kind: ContentKind, com_mojang: &Path) -> PathBuf {
    match load_dir_name(kind) {
        Some(name) => com_mojang.join(format!("{name}_backup")),
        None => com_mojang.join(format!("{WORLDS_DIR}_backup")),
    }
}

/// 定位世界目录：`<Users>/<首个用户>/games/com.mojang/minecraftWorlds`。
fn first_player_worlds_dir(roots: &ContentRoots) -> Option<PathBuf> {
    let users = &roots.users_root;
    if !users.is_dir() {
        return None;
    }
    let entries = std::fs::read_dir(users).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.eq_ignore_ascii_case("Shared") || name.starts_with('.') {
            continue;
        }
        let worlds = path.join(SHARED_GAME_DIR).join(WORLDS_DIR);
        if worlds.exists() {
            return Some(worlds);
        }
    }
    None
}

/// 定位世界（存档）目录，供「打开存档目录」使用。
///
/// 优先取首个玩家目录下的 `minecraftWorlds`；玩家目录尚未生成时退回首个非 Shared
/// 用户目录再退回 Shared，保证版本可隔离、多用户场景下都能得到一个可打开的目录。
/// `create` 为真时确保目录存在。
pub fn worlds_dir(
    kernel: &KernelContext,
    name: &str,
    create: bool,
) -> Result<PathBuf, KernelError> {
    let roots = content_roots_for_write(kernel, name)?;
    let dir = first_player_worlds_dir(&roots).unwrap_or_else(|| {
        first_user_root(&roots)
            .unwrap_or_else(|| roots.com_mojang.clone())
            .join(SHARED_GAME_DIR)
            .join(WORLDS_DIR)
    });
    if create {
        std::fs::create_dir_all(&dir)?;
    }
    Ok(dir)
}

/// 首个可用玩家目录（跳过 `Shared` 与点开头目录）。
fn first_user_root(roots: &ContentRoots) -> Option<PathBuf> {
    let users = &roots.users_root;
    if !users.is_dir() {
        return None;
    }
    let entries = std::fs::read_dir(users).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.eq_ignore_ascii_case("Shared") || name.starts_with('.') {
            continue;
        }
        return Some(path);
    }
    None
}

/// 按 id 定位条目（在加载目录与备份目录中查找），返回 (条目, 是否在加载目录)。
fn find_item(roots: &ContentRoots, item_id: &str) -> Result<(ContentItem, bool), KernelError> {
    let (kind_str, name) = item_id
        .split_once(':')
        .ok_or_else(|| KernelError::InvalidArgument("非法的内容条目".into()))?;
    let kind = match kind_str {
        "resources" => ContentKind::Resources,
        "behavior" => ContentKind::Behavior,
        "worlds" => ContentKind::Worlds,
        "templates" => ContentKind::Templates,
        _ => return Err(KernelError::InvalidArgument("非法的内容类型".into())),
    };
    let load_dir = load_dir_for(kind, roots);
    let backup_dir = backup_dir_for(kind, &roots.com_mojang);

    let load_path = load_dir.join(name);
    if load_path.exists() {
        return Ok((
            ContentItem {
                id: item_id.to_string(),
                kind,
                name: name.to_string(),
                enabled: true,
                path: load_path.to_string_lossy().into_owned(),
            },
            true,
        ));
    }
    let backup_path = backup_dir.join(name);
    if backup_path.exists() {
        return Ok((
            ContentItem {
                id: item_id.to_string(),
                kind,
                name: name.to_string(),
                enabled: false,
                path: backup_path.to_string_lossy().into_owned(),
            },
            false,
        ));
    }
    Err(KernelError::InvalidArgument("内容条目不存在".into()))
}

fn load_dir_for(kind: ContentKind, roots: &ContentRoots) -> PathBuf {
    match load_dir_name(kind) {
        Some(name) => roots.com_mojang.join(name),
        None => {
            first_player_worlds_dir(roots).unwrap_or_else(|| roots.com_mojang.join(WORLDS_DIR))
        }
    }
}

/// 启用 / 禁用内容条目（移动目录或文件）。
pub fn set_content_enabled(
    kernel: &KernelContext,
    name: &str,
    item_id: &str,
    enabled: bool,
) -> Result<(), KernelError> {
    let roots = content_roots_for_write(kernel, name)?;
    let (item, in_load_dir) = find_item(&roots, item_id)?;
    if item.enabled == enabled {
        return Ok(());
    }
    let load_dir = load_dir_for(item.kind, &roots);
    let backup_dir = backup_dir_for(item.kind, &roots.com_mojang);
    if enabled {
        std::fs::create_dir_all(&load_dir)?;
        std::fs::rename(&item.path, load_dir.join(&item.name))?;
    } else {
        std::fs::create_dir_all(&backup_dir)?;
        std::fs::rename(&item.path, backup_dir.join(&item.name))?;
    }
    let _ = in_load_dir;
    Ok(())
}

/// 删除内容条目。
pub fn remove_content(
    kernel: &KernelContext,
    name: &str,
    item_id: &str,
) -> Result<(), KernelError> {
    let roots = content_roots_for_write(kernel, name)?;
    let (item, _) = find_item(&roots, item_id)?;
    let path = Path::new(&item.path);
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_com_mojang(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "copper_home_content_{tag}_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let cm = base.join("games").join("com.mojang");
        fs::create_dir_all(cm.join("resource_packs")).unwrap();
        fs::create_dir_all(cm.join("behavior_packs")).unwrap();
        base
    }

    #[test]
    fn collect_and_toggle() {
        let base = tmp_com_mojang("toggle");
        let cm = base.join("games").join("com.mojang");
        // 资源包：一个目录 + 一个 zip
        fs::create_dir_all(cm.join("resource_packs").join("pack-a")).unwrap();
        fs::write(cm.join("resource_packs").join("pack-b.zip"), b"zip").unwrap();
        // 行为包：一个目录
        fs::create_dir_all(cm.join("behavior_packs").join("bp-a")).unwrap();

        let roots = ContentRoots {
            com_mojang: cm.clone(),
            users_root: base.join("Users"),
        };
        let mut items = Vec::new();
        collect_dir_items(
            &roots.com_mojang.join("resource_packs"),
            ContentKind::Resources,
            &roots.com_mojang,
            &mut items,
        );
        collect_dir_items(
            &roots.com_mojang.join("behavior_packs"),
            ContentKind::Behavior,
            &roots.com_mojang,
            &mut items,
        );
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|i| i.enabled));

        // 禁用 pack-a → 移入备份目录
        let (item, in_load) = find_item(&roots, "resources:pack-a").unwrap();
        assert!(in_load);
        let backup = backup_dir_for(item.kind, &roots.com_mojang);
        fs::create_dir_all(&backup).unwrap();
        fs::rename(&item.path, backup.join("pack-a")).unwrap();

        let mut items = Vec::new();
        collect_dir_items(
            &roots.com_mojang.join("resource_packs"),
            ContentKind::Resources,
            &roots.com_mojang,
            &mut items,
        );
        let pack_a = items.iter().find(|i| i.name == "pack-a").expect("found");
        assert!(!pack_a.enabled);
        let _ = &backup;
    }
}
