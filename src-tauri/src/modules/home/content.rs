//! 内容管理：版本已加入资源（资源包 / 行为包 / 世界 / 模板 / 截图）的枚举、预览与删除。
//!
//! 内容根目录一律由 [`super::isolate`] 解析（隔离强制开启，规则只有一份）：
//! `<版本目录>/Minecraft Bedrock[/Preview]/Users/Shared/games/com.mojang`。
//!
//! 启用 / 禁用是历史机制（条目在加载目录与 `*_backup` 之间移动）：隔离强制开启后
//! 它只剩迁移旧数据的意义，资源页也不再暴露「已启用 / 已停用」概念，因此列表只
//! 枚举**加载目录**中的条目；备份目录里的旧条目不出现在清单里。
//!
//! 截图不是「内容包」，但同样属于该实例的用户资产：MCBE 把截图写在数据根的
//! `Screenshots` 目录（UWP 为 `LocalState/Screenshots`，GDK 为
//! `Users/<玩家>/games/com.mojang` 附近，见 [`screenshots_dir`] 的候选顺序）。

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
    /// 游戏截图（数据根 `Screenshots` 目录里的图片文件）。
    Screenshots,
}

impl ContentKind {
    fn as_str(self) -> &'static str {
        match self {
            ContentKind::Resources => "resources",
            ContentKind::Behavior => "behavior",
            ContentKind::Worlds => "worlds",
            ContentKind::Templates => "templates",
            ContentKind::Screenshots => "screenshots",
        }
    }
}

/// 各内容类型对应的加载目录名。
fn load_dir_name(kind: ContentKind) -> Option<&'static str> {
    match kind {
        ContentKind::Resources => Some("resource_packs"),
        ContentKind::Behavior => Some("behavior_packs"),
        ContentKind::Templates => Some("development_behavior_packs"),
        // 世界目录在玩家目录下，位置随实例变，不走固定名；截图目录同理。
        ContentKind::Worlds | ContentKind::Screenshots => None,
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
    /// 条目当前绝对路径。
    pub path: String,
    /// 目录条目为 true，压缩包 / 截图为 false。
    pub is_dir: bool,
    /// 文件字节数；目录没有单一大小，为 `None`。
    pub size: Option<u64>,
    /// 最后修改时间（Unix 毫秒）；读取失败为 `None`。
    pub modified: Option<i64>,
}

/// 解析版本内容根目录（读取侧口径）。
///
/// 目录规则全部委托给 [`isolate`]：隔离强制开启，没有「非隔离」分支，
/// 也不再需要 AppX 包族名查询（那条路径只在共享官方数据目录时才成立）。
pub fn content_roots(kernel: &KernelContext, name: &str) -> Result<ContentRoots, KernelError> {
    let dir = resolve_version_dir(&kernel.versions_root(), name)?;
    let meta = read_meta_for(&dir, name)?;
    if super::compatibility::is_bound(&dir) { return bound_content_roots(&dir); }
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
    if super::compatibility::is_bound(&dir) {
        // Sensitive writes revalidate the entire source binding before resolving the same shared roots.
        let _spec=super::compatibility::resolve(&dir)?;
        return bound_content_roots(&dir);
    }
    isolate::ensure_skeleton(&dir, &meta)?;
    Ok(isolate::content_roots(&dir, &meta))
}

fn bound_content_roots(control:&Path)->Result<ContentRoots,KernelError>{let shared=super::compatibility::shared_data(control)?;super::compatibility::require_unambiguous_player(&shared.users)?;Ok(ContentRoots{com_mojang:shared.shared,users_root:shared.users,worlds:None})}

/// 读取实例元数据，缺失即报错（带实例名，便于前端定位）。
fn read_meta_for(dir: &Path, name: &str) -> Result<VersionMeta, KernelError> {
    VersionMeta::read(dir)
        .ok_or_else(|| KernelError::InvalidArgument(format!("版本 `{name}` 元数据缺失")))
}


/// 列出已加入资源（加载目录 + 截图）。
pub fn list_content(kernel: &KernelContext, name: &str) -> Result<Vec<ContentItem>, KernelError> {
    let roots = content_roots(kernel, name)?;
    let mut items = Vec::new();
    collect_dir_items(
        &roots.com_mojang.join("resource_packs"),
        ContentKind::Resources,
        &mut items,
    );
    collect_dir_items(
        &roots.com_mojang.join("behavior_packs"),
        ContentKind::Behavior,
        &mut items,
    );
    collect_dir_items(
        &roots.com_mojang.join("development_behavior_packs"),
        ContentKind::Templates,
        &mut items,
    );
    if let Some(worlds) = first_player_worlds_dir(&roots) {
        collect_dir_items(&worlds, ContentKind::Worlds, &mut items);
    }
    if let Some(dir) = screenshots_dir(&roots) {
        collect_image_items(&dir, &mut items);
    }
    Ok(items)
}

/// 枚举加载目录条目（目录 或 压缩包文件）。
fn collect_dir_items(load_dir: &Path, kind: ContentKind, out: &mut Vec<ContentItem>) {
    if !load_dir.is_dir() {
        return;
    }
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
            out.push(item_from_path(kind, name, path));
        }
    }
}

/// 枚举截图目录里的图片文件（不递归：游戏只往这一层写）。
fn collect_image_items(dir: &Path, out: &mut Vec<ContentItem>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !is_image(&name) {
            continue;
        }
        out.push(item_from_path(ContentKind::Screenshots, name, path));
    }
}

/// 由路径构造条目，并读一次元数据（大小 / 修改时间）。
fn item_from_path(kind: ContentKind, name: String, path: PathBuf) -> ContentItem {
    let (is_dir, size, modified) = inspect(&path);
    ContentItem {
        id: format!("{}:{name}", kind.as_str()),
        kind,
        name,
        path: path.to_string_lossy().into_owned(),
        is_dir,
        size,
        modified,
    }
}

/// 读取条目的类型、字节数与修改时间；任何一步失败都不伪造数据。
fn inspect(path: &Path) -> (bool, Option<u64>, Option<i64>) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (false, None, None);
    };
    let is_dir = meta.is_dir();
    let size = (!is_dir).then_some(meta.len());
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis() as i64);
    (is_dir, size, modified)
}

/// 截图目录候选顺序（取第一个存在的目录）：
///
/// 1. `<数据根>/Screenshots` —— UWP 把 LocalState 重定向到版本目录后截图落在这里；
/// 2. `<com.mojang>/Screenshots` —— GDK / 部分版本的共享数据根布局；
/// 3. `<Users>/<玩家>/Screenshots` 与 `<Users>/<玩家>/games/com.mojang/Screenshots`。
///
/// MCBE 的截图落点没有公开文档，而且随渠道（UWP / GDK）与版本变化。这里按
/// 上述顺序探测并只采用**实际存在**的目录：没有截图时清单里就不出现这一分类，
/// 不猜也不建空目录。
pub fn screenshots_dir(roots: &ContentRoots) -> Option<PathBuf> {
    let data_root = if roots.users_root.as_os_str().is_empty() {
        // UWP：com_mojang = <数据根>/games/com.mojang。
        roots
            .com_mojang
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
    } else {
        roots.users_root.parent().map(Path::to_path_buf)
    };
    let mut candidates = Vec::new();
    if let Some(data) = data_root {
        candidates.push(data.join("Screenshots"));
    }
    candidates.push(roots.com_mojang.join("Screenshots"));
    if !roots.users_root.as_os_str().is_empty() {
        if let Ok(entries) = std::fs::read_dir(&roots.users_root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.eq_ignore_ascii_case("Shared") || name.starts_with('.') {
                    continue;
                }
                candidates.push(path.join("Screenshots"));
                candidates.push(path.join(SHARED_GAME_DIR).join("Screenshots"));
            }
        }
    }
    candidates.into_iter().find(|dir| dir.is_dir())
}

/// 压缩包扩展名（MCBE 资源可打包为 zip / mcpack / mcaddon / addon）。
///
/// `.addon` 是非官方但已被部分项目采用的别名，与 `.mcaddon` 同为 zip 容器；
/// 这里一并识别，避免下载落位成功后在「已加入资源」里看不到。
fn is_archive(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".zip")
        || lower.ends_with(".mcpack")
        || lower.ends_with(".mcaddon")
        || lower.ends_with(".addon")
        || lower.ends_with(".mctemplate")
}

/// 备份目录：`<com_mojang>/<加载目录名>_backup`。
fn backup_dir_for(kind: ContentKind, com_mojang: &Path) -> PathBuf {
    match load_dir_name(kind) {
        Some(name) => com_mojang.join(format!("{name}_backup")),
        None => com_mojang.join(format!("{WORLDS_DIR}_backup")),
    }
}

/// 定位世界目录：UWP 用确定落点，GDK 用 `<Users>/<首个用户>/games/com.mojang/minecraftWorlds`。
pub(crate) fn first_player_worlds_dir(roots: &ContentRoots) -> Option<PathBuf> {
    // UWP：包布局固定，世界就在 `games/com.mojang/minecraftWorlds`（没有玩家层）。
    if let Some(worlds) = roots.worlds.as_ref() {
        return worlds.is_dir().then(|| worlds.clone());
    }
    let users = &roots.users_root;
    if users.as_os_str().is_empty() || !users.is_dir() {
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
    let control=resolve_version_dir(&kernel.versions_root(),name)?;
    if super::compatibility::is_bound(&control){
        // Opening Users needs no account choice and never invents a world directory.
        return Ok(super::compatibility::shared_data(&control)?.users);
    }
    let roots = content_roots_for_write(kernel, name)?;
    let dir = first_player_worlds_dir(&roots).unwrap_or_else(|| {
        // UWP：世界目录就是 `com.mojang/minecraftWorlds`，不能再拼一层 `games`。
        if let Some(worlds) = roots.worlds.as_ref() {
            return worlds.clone();
        }
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
    if users.as_os_str().is_empty() || !users.is_dir() {
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

/// 按 id 定位条目（加载目录、截图目录与备份目录中查找）。
fn find_item(roots: &ContentRoots, item_id: &str) -> Result<ContentItem, KernelError> {
    let (kind_str, name) = item_id
        .split_once(':')
        .ok_or_else(|| KernelError::InvalidArgument("非法的内容条目".into()))?;
    let kind = match kind_str {
        "resources" => ContentKind::Resources,
        "behavior" => ContentKind::Behavior,
        "worlds" => ContentKind::Worlds,
        "templates" => ContentKind::Templates,
        "screenshots" => ContentKind::Screenshots,
        _ => return Err(KernelError::InvalidArgument("非法的内容类型".into())),
    };
    if kind == ContentKind::Screenshots {
        let dir = screenshots_dir(roots)
            .ok_or_else(|| KernelError::InvalidArgument("截图目录不存在".into()))?;
        let path = dir.join(name);
        if path.is_file() {
            return Ok(item_from_path(kind, name.to_string(), path));
        }
        return Err(KernelError::InvalidArgument("内容条目不存在".into()));
    }

    let load_dir = load_dir_for(kind, roots);
    let backup_dir = backup_dir_for(kind, &roots.com_mojang);

    let load_path = load_dir.join(name);
    if load_path.exists() {
        return Ok(item_from_path(kind, name.to_string(), load_path));
    }
    let backup_path = backup_dir.join(name);
    if backup_path.exists() {
        return Ok(item_from_path(kind, name.to_string(), backup_path));
    }
    Err(KernelError::InvalidArgument("内容条目不存在".into()))
}

fn load_dir_for(kind: ContentKind, roots: &ContentRoots) -> PathBuf {
    match load_dir_name(kind) {
        Some(name) => roots.com_mojang.join(name),
        None => match kind {
            ContentKind::Screenshots => screenshots_dir(roots)
                .unwrap_or_else(|| roots.com_mojang.join("Screenshots")),
            _ => first_player_worlds_dir(roots)
                .unwrap_or_else(|| roots.com_mojang.join(WORLDS_DIR)),
        },
    }
}

/// 启用 / 禁用内容条目（移动目录或文件）。
///
/// 资源页已不再暴露启用概念（隔离强制开启后加载目录即全部内容），此接口保留给
/// 迁移旧数据与脚本调用；截图不是内容包，直接拒绝。
pub fn set_content_enabled(
    kernel: &KernelContext,
    name: &str,
    item_id: &str,
    enabled: bool,
) -> Result<(), KernelError> {
    let roots = content_roots_for_write(kernel, name)?;
    let item = find_item(&roots, item_id)?;
    if item.kind == ContentKind::Screenshots {
        return Err(KernelError::InvalidArgument("截图不支持启用 / 停用".into()));
    }
    let load_dir = load_dir_for(item.kind, &roots);
    let in_load_dir = Path::new(&item.path) == load_dir.join(&item.name);
    if in_load_dir == enabled {
        return Ok(());
    }
    let backup_dir = backup_dir_for(item.kind, &roots.com_mojang);
    if enabled {
        std::fs::create_dir_all(&load_dir)?;
        std::fs::rename(&item.path, load_dir.join(&item.name))?;
    } else {
        std::fs::create_dir_all(&backup_dir)?;
        std::fs::rename(&item.path, backup_dir.join(&item.name))?;
    }
    Ok(())
}

/// 删除内容条目（目录 / 压缩包 / 截图文件）。
pub fn remove_content(
    kernel: &KernelContext,
    name: &str,
    item_id: &str,
) -> Result<(), KernelError> {
    let roots = content_roots_for_write(kernel, name)?;
    let item = find_item(&roots, item_id)?;
    let path = Path::new(&item.path);
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// 单张预览图的原始字节上限。超过上限不读：截图动辄数 MB，全部转 base64
/// 送进 WebView 会把渲染进程拖住，宁可让详情面板没有预览。
const PREVIEW_MAX_BYTES: u64 = 12 * 1024 * 1024;
/// 压缩包内图标的大小上限（pack_icon.png 正常只有几十 KB）。
const ARCHIVE_ICON_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// 读取内容条目的预览图（data:<mime>;base64,...）。
///
/// - 截图：直接读图片文件；
/// - 资源包 / 行为包 / 模板：目录取 pack_icon.png，压缩包从 zip 里取同名条目；
/// - 世界：取 world_icon.jpeg。
///
/// 没有图标或图标过大时返回 None，由前端回退到类型图标。
pub fn content_preview(
    kernel: &KernelContext,
    name: &str,
    item_id: &str,
) -> Result<Option<String>, KernelError> {
    let roots = content_roots(kernel, name)?;
    let item = find_item(&roots, item_id)?;
    preview_for_item(&item)
}

fn preview_for_item(item: &ContentItem) -> Result<Option<String>, KernelError> {
    let path = Path::new(&item.path);
    if item.kind == ContentKind::Screenshots {
        return read_image_data_url(path, PREVIEW_MAX_BYTES);
    }
    if item.is_dir {
        let icon_name = if item.kind == ContentKind::Worlds {
            "world_icon.jpeg"
        } else {
            "pack_icon.png"
        };
        return read_image_data_url(&path.join(icon_name), PREVIEW_MAX_BYTES);
    }
    if is_archive(&item.name) {
        return read_archive_icon(path, ARCHIVE_ICON_MAX_BYTES);
    }
    Ok(None)
}

/// 读图片文件并转成 data URL；文件不存在 / 过大 / 不是图片都返回 None。
pub(crate) fn read_image_data_url(path: &Path, max_bytes: u64) -> Result<Option<String>, KernelError> {
    let Some(mime) = image_mime(&path.to_string_lossy()) else {
        return Ok(None);
    };
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(None);
    };
    if !meta.is_file() || meta.len() > max_bytes {
        return Ok(None);
    }
    let bytes = std::fs::read(path)?;
    Ok(Some(data_url(&bytes, mime)))
}

/// 从压缩包中读取 pack_icon.png（取路径最短的一个，避免嵌套包里的同名图标）。
fn read_archive_icon(path: &Path, max_bytes: u64) -> Result<Option<String>, KernelError> {
    use std::io::Read as _;

    let file = std::fs::File::open(path)?;
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(archive) => archive,
        // 损坏 / 伪装成压缩包的文件按「没有图标」处理。
        Err(_) => return Ok(None),
    };
    let mut best: Option<(usize, usize)> = None;
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        let lower = entry.name().to_ascii_lowercase();
        if lower.ends_with("pack_icon.png") && entry.is_file() {
            let depth = lower.matches('/').count();
            if best.map(|(d, _)| depth < d).unwrap_or(true) {
                best = Some((depth, index));
            }
        }
    }
    let Some((_, index)) = best else {
        return Ok(None);
    };
    let mut entry = archive
        .by_index(index)
        .map_err(|error| KernelError::InvalidArgument(format!("读取压缩包图标失败: {error}")))?;
    if entry.size() > max_bytes {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes)?;
    Ok(Some(data_url(&bytes, "image/png")))
}

fn data_url(bytes: &[u8], mime: &str) -> String {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:{mime};base64,{b64}")
}

/// 按扩展名判断图片 MIME；不是支持的图片格式返回 None。
fn image_mime(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        Some("image/png")
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        Some("image/jpeg")
    } else if lower.ends_with(".webp") {
        Some("image/webp")
    } else {
        None
    }
}

/// 截图支持的扩展名。
fn is_image(name: &str) -> bool {
    image_mime(name).is_some()
}

/// 在系统文件管理器中定位条目（目录直接打开，文件选中）。
///
/// 路径全部来自后端枚举结果，不接受前端拼的任意路径。
pub fn open_content_folder(
    kernel: &KernelContext,
    name: &str,
    item_id: &str,
) -> Result<String, KernelError> {
    let roots = content_roots(kernel, name)?;
    let item = find_item(&roots, item_id)?;
    let path = Path::new(&item.path);
    // 文件取父目录，目录取自身。
    let target = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    };
    // 先走系统 shell（ShellExecute）：由已运行的资源管理器接管，不产生第二个进程，
    // 也会复用用户当前的文件管理器窗口。
    // 但受限进程（低完整性 / 无交互桌面）里 ShellExecute 可能直接返回
    // 「操作已被用户取消 (os error 1223)」，此时退回自己拉起 explorer.exe ——
    // 两条路都是尽力而为，全失败才把两个原因一起抛给前端，避免「点了没反应」。
    if let Err(error) = tauri_plugin_opener::open_path(&target, None::<&str>) {
        crate::platform::shell::reveal(&target).map_err(|fallback| {
            KernelError::InvalidArgument(format!(
                "打开文件夹失败: {error}；直接拉起文件管理器也失败: {fallback}"
            ))
        })?;
    }
    Ok(item.path)
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
    fn collects_entries_with_metadata() {
        let base = tmp_com_mojang("collect");
        let cm = base.join("games").join("com.mojang");
        // 资源包：一个目录 + 一个 zip
        fs::create_dir_all(cm.join("resource_packs").join("pack-a")).unwrap();
        fs::write(cm.join("resource_packs").join("pack-b.zip"), b"zip").unwrap();
        // 行为包：一个目录
        fs::create_dir_all(cm.join("behavior_packs").join("bp-a")).unwrap();

        let roots = ContentRoots {
            com_mojang: cm.clone(),
            users_root: base.join("Users"),
            worlds: None,
        };
        let mut items = Vec::new();
        collect_dir_items(
            &roots.com_mojang.join("resource_packs"),
            ContentKind::Resources,
            &mut items,
        );
        collect_dir_items(
            &roots.com_mojang.join("behavior_packs"),
            ContentKind::Behavior,
            &mut items,
        );
        assert_eq!(items.len(), 3);
        let pack_b = items
            .iter()
            .find(|item| item.name == "pack-b.zip")
            .expect("zip 条目");
        assert!(!pack_b.is_dir);
        assert_eq!(pack_b.size, Some(3));
        assert!(pack_b.modified.is_some());

        // find_item 能按 id 取回同一条目
        let found = find_item(&roots, "resources:pack-a").unwrap();
        assert!(found.is_dir);
        assert_eq!(found.id, "resources:pack-a");
    }

    #[test]
    fn lists_screenshots_and_finds_them() {
        let base = tmp_com_mojang("shots");
        let shots = base.join("Screenshots");
        fs::create_dir_all(&shots).unwrap();
        fs::write(shots.join("2026-11-08_12.00.00.png"), b"\x89PNG").unwrap();
        fs::write(shots.join("ignore.txt"), b"x").unwrap();
        let roots = ContentRoots {
            com_mojang: base.join("games").join("com.mojang"),
            users_root: PathBuf::new(),
            worlds: None,
        };
        assert_eq!(screenshots_dir(&roots).as_deref(), Some(shots.as_path()));

        let mut items = Vec::new();
        collect_image_items(&shots, &mut items);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, ContentKind::Screenshots);
        let found = find_item(&roots, "screenshots:2026-11-08_12.00.00.png").unwrap();
        assert_eq!(found.size, Some(4));
    }

    #[test]
    fn preview_reads_pack_icon_and_archive_icon() {
        let base = tmp_com_mojang("preview");
        let cm = base.join("games").join("com.mojang");
        let pack = cm.join("resource_packs").join("pack-a");
        fs::create_dir_all(&pack).unwrap();
        fs::write(pack.join("pack_icon.png"), b"\x89PNGdata").unwrap();
        let roots = ContentRoots {
            com_mojang: cm.clone(),
            users_root: base.join("Users"),
            worlds: None,
        };
        let item = find_item(&roots, "resources:pack-a").unwrap();
        let url = preview_for_item(&item).unwrap().expect("目录图标");
        assert!(url.starts_with("data:image/png;base64,"));

        // 压缩包内图标
        let archive_path = cm.join("resource_packs").join("pack-c.zip");
        let file = fs::File::create(&archive_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("pack_icon.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        use std::io::Write as _;
        zip.write_all(b"\x89PNGzip").unwrap();
        zip.finish().unwrap();
        let item = find_item(&roots, "resources:pack-c.zip").unwrap();
        let url = preview_for_item(&item).unwrap().expect("压缩包图标");
        assert!(url.starts_with("data:image/png;base64,"));
    }
}
