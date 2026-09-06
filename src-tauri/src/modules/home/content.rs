//! 内容管理：版本已加入资源（资源包 / 行为包 / 世界）的枚举、启用 / 禁用 / 删除。
//!
//! 内容根目录（与 LeviLauncher `GetContentRoots` 对齐）：
//! - 隔离版本：`<versions>/<name>/Minecraft Bedrock/Users/...`；
//! - 非隔离版本：通过 AppX 包定位 `<LocalAppData>/Packages/<包族名>/LocalState/games/com.mojang`。
//!
//! 启用 / 禁用机制：加载目录（`resource_packs` 等）只装载其下条目，禁用即把条目
//! 移入同级 `*_backup` 目录，启用移回；删除则直接移除。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::KernelError;
use crate::state::KernelContext;

use super::meta::{resolve_version_dir, VersionMeta};

/// 内容类型（前端过滤用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Resources,
    Behavior,
    Worlds,
}

impl ContentKind {
    fn as_str(self) -> &'static str {
        match self {
            ContentKind::Resources => "resources",
            ContentKind::Behavior => "behavior",
            ContentKind::Worlds => "worlds",
        }
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

/// 内容根目录视图。
pub struct ContentRoots {
    /// 共享内容根：`.../com.mojang`。
    pub com_mojang: PathBuf,
    /// 用户目录根：`.../Users`。
    pub users_root: PathBuf,
}

/// 共享内容目录名（`resource_packs` / `behavior_packs`）。
const SHARED_GAME_DIR: &str = "games/com.mojang";
/// 世界目录名。
const WORLDS_DIR: &str = "minecraftWorlds";

/// 解析版本内容根目录。
///
/// 非隔离版本需要 AppX 包信息，失败（未安装 / 非 Windows）时返回错误，
/// 前端据此提示内容管理不可用。
pub fn content_roots(kernel: &KernelContext, name: &str) -> Result<ContentRoots, KernelError> {
    let dir = resolve_version_dir(kernel.paths().versions_dir(), name)?;
    let meta = VersionMeta::read(&dir)
        .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{name}` 元数据缺失")))?;

    if meta.enable_isolation {
        let game_dir = game_dir_name(&meta);
        let base = dir.join(game_dir);
        return Ok(ContentRoots {
            com_mojang: base.join("Users").join("Shared").join(SHARED_GAME_DIR),
            users_root: base.join("Users"),
        });
    }

    // 非隔离：走 AppX 包族名定位 GDK 数据目录。
    let pfn = appx_package_family_name(&meta)?;
    let local = std::env::var("LOCALAPPDATA")
        .map_err(|_| KernelError::InvalidArgument("无法定位用户数据目录".into()))?;
    let base = PathBuf::from(local)
        .join("Packages")
        .join(pfn)
        .join("LocalState")
        .join("games")
        .join("com.mojang");
    if !base.exists() {
        return Err(KernelError::InvalidArgument(
            "未找到游戏数据目录，请先启动一次游戏".into(),
        ));
    }
    let users_root = base
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("Users"))
        .unwrap_or_else(|| base.clone());
    Ok(ContentRoots { com_mojang: base, users_root })
}

/// 隔离游戏目录名（正式 / 预览）。
fn game_dir_name(meta: &VersionMeta) -> &'static str {
    if meta.version_type.eq_ignore_ascii_case("preview") {
        "Minecraft Bedrock Preview"
    } else {
        "Minecraft Bedrock"
    }
}

/// 查询 AppX 包族名（PowerShell，与 LeviLauncher `Get-AppxPackage` 同款）。
#[cfg(windows)]
fn appx_package_family_name(meta: &VersionMeta) -> Result<String, KernelError> {
    let pkg = if meta.version_type.eq_ignore_ascii_case("preview") {
        "Microsoft.MinecraftWindowsBeta"
    } else {
        "Microsoft.MinecraftUWP"
    };
    let script = format!(
        "Get-AppxPackage -Name '{pkg}' | Select-Object -First 1 -ExpandProperty PackageFamilyName"
    );
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| KernelError::InvalidArgument(format!("查询游戏包失败: {e}")))?;
    if !output.status.success() {
        return Err(KernelError::InvalidArgument("未检测到已安装的 Minecraft".into()));
    }
    let pfn = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if pfn.is_empty() {
        return Err(KernelError::InvalidArgument("未检测到已安装的 Minecraft".into()));
    }
    Ok(pfn)
}

#[cfg(not(windows))]
fn appx_package_family_name(_meta: &VersionMeta) -> Result<String, KernelError> {
    Err(KernelError::InvalidArgument("内容管理暂不支持当前平台".into()))
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
    match kind {
        ContentKind::Resources => com_mojang.join("resource_packs_backup"),
        ContentKind::Behavior => com_mojang.join("behavior_packs_backup"),
        ContentKind::Worlds => com_mojang.join(format!("{WORLDS_DIR}_backup")),
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

/// 按 id 定位条目（在加载目录与备份目录中查找），返回 (条目, 是否在加载目录)。
fn find_item(roots: &ContentRoots, item_id: &str) -> Result<(ContentItem, bool), KernelError> {
    let (kind_str, name) = item_id
        .split_once(':')
        .ok_or_else(|| KernelError::InvalidArgument("非法的内容条目".into()))?;
    let kind = match kind_str {
        "resources" => ContentKind::Resources,
        "behavior" => ContentKind::Behavior,
        "worlds" => ContentKind::Worlds,
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
    match kind {
        ContentKind::Resources => roots.com_mojang.join("resource_packs"),
        ContentKind::Behavior => roots.com_mojang.join("behavior_packs"),
        ContentKind::Worlds => {
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
    let roots = content_roots(kernel, name)?;
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
    let roots = content_roots(kernel, name)?;
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
