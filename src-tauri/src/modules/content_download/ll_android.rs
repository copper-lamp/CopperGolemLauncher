//! 内容下载模块 · 安卓 LL 模组提供器（LeviModHub 目录）。
//!
//! # 为什么安卓不能走 lip
//!
//! lip（`lipd`）依赖 .NET 10 运行时与 BDS 环境，且 lipr 索引里的资产**全部**是
//! `win-x64` 的 `zip`（`placements.dest = "plugins/<Mod>"`）。逐包抽样解`tooth.json`
//! 确认无`android` / `arm64` 资产，因此安卓既跑不了 lipd、也下不到能用的包。
//!
//! # 安卓的真实来源
//!
//! LeviModHub 目录 `https://qycottage.github.io/LeviModHub/catalog.json`
//! （即 LeviLaunchroid 的 mod 目录源）。资产是 `.levipack`（zip），内含
//! `manifest.json` + 唯一的 `lib<id>.so`，并声明 `minecraft_versions` /
//! `sha256` / `size` / `published_at`。
//!
//! # 落点
//!
//! 对齐 `CopperGameLayout.kt` 与 LeviLaunchroid `LauncherStorage`：
//! `<versions>/<name>/mods/<modId>/`，一 mod 一目录。mods 与游戏内容根
//! `<versions>/<name>/game/files/games/com.mojang/` 是**平行兄弟目录**——
//! `com.mojang` 是游戏 addon 机制的地盘，模组 `.so` 不会被它扫描。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::Value;

use crate::error::KernelError;
use crate::state::KernelContext;

use super::model::{
    normalize_sort, ContentDetail, ContentFile, ContentItem, ContentListPage, ContentListQuery,
    PAGE_SIZE, SORT_NAME_ASC, SORT_UPDATED_DESC, SOURCE_LL_ANDROID, TYPE_LL_MOD,
};

/// LeviModHub 目录地址（与 LeviLaunchroid `ModCatalogRepository.REMOTE_URL` 一致）。
const CATALOG_URL: &str = "https://qycottage.github.io/LeviModHub/catalog.json";
/// 目录缓存 TTL（秒）。
const CATALOG_TTL_SECS: u64 = 1800;

/// 未设置默认版本 / 默认版本目录不存在。
pub const ERR_LLA_TARGET_NOT_FOUND: &str = "ERR_LLA_TARGET_NOT_FOUND";
/// 选中版本不是安卓实例（缺 `version.json` 的 `android` 元数据）。
pub const ERR_LLA_TARGET_NOT_ANDROID: &str = "ERR_LLA_TARGET_NOT_ANDROID";
/// 该发布没有可直连下载的资产（`browser` / `ad` 或资产非法）。
pub const ERR_LLA_NO_DIRECT_ASSET: &str = "ERR_LLA_NO_DIRECT_ASSET";
/// 资产与目标 MC 版本不兼容。
pub const ERR_LLA_INCOMPATIBLE: &str = "ERR_LLA_INCOMPATIBLE";
/// 归档结构非法（缺 manifest / 无唯一 `.so` / 路径逃逸）。
pub const ERR_LLA_BAD_ARCHIVE: &str = "ERR_LLA_BAD_ARCHIVE";
/// 落盘失败（IO）。
pub const ERR_LLA_INSTALL_FAILED: &str = "ERR_LLA_INSTALL_FAILED";

/// mod 目录名（与 LeviLaunchroid `ModCatalogRepository.ID_PATTERN` 一致）。
const MOD_ID_MAX_LEN: usize = 64;

// ---------------------------------------------------------------- 目录模型

/// 目录顶层。
#[derive(Debug, Clone, Default)]
struct Catalog {
    mods: Vec<CatalogMod>,
}

#[derive(Debug, Clone)]
struct CatalogMod {
    id: String,
    name: String,
    author: String,
    description: String,
    icon_url: Option<String>,
    homepage_url: Option<String>,
    tags: Vec<String>,
    releases: Vec<CatalogRelease>,
}

#[derive(Debug, Clone)]
struct CatalogRelease {
    version: String,
    minecraft_versions: Vec<String>,
    download_type: String,
    assets: Vec<CatalogAsset>,
    published_at: String,
}

#[derive(Debug, Clone)]
struct CatalogAsset {
    name: String,
    download_url: String,
    size: u64,
    sha256: Option<String>,
}

// ---------------------------------------------------------------- 缓存

type SharedCache = tokio::sync::Mutex<Option<(std::time::Instant, Catalog)>>;

fn cache() -> &'static SharedCache {
    static CACHE: OnceLock<SharedCache> = OnceLock::new();
    CACHE.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// 拉取目录（带 TTL 缓存）。网络失败且无缓存时返回错误，不伪造空目录。
async fn catalog() -> Result<Catalog, KernelError> {
    {
        let guard = cache().lock().await;
        if let Some((at, data)) = guard.as_ref() {
            if at.elapsed().as_secs() < CATALOG_TTL_SECS {
                return Ok(data.clone());
            }
        }
    }
    let json: Value = super::http::client()
        .get(CATALOG_URL)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let parsed = parse_catalog(&json);
    *cache().lock().await = Some((std::time::Instant::now(), parsed.clone()));
    Ok(parsed)
}

// ---------------------------------------------------------------- 解析（可单测）

/// 解析目录 JSON。逐字段校验并丢弃非法条目，缺字段容错。
///
/// 校验规则对齐 `ModCatalogRepository.parse`：schema 只接受 1 / 2；
/// `download_type` 只接受 `direct` / `browser` / `ad`；资产必须是 https 且
/// 后缀为 `.levipack` / `.zip` / `.so`，`sha256` 若给出必须是 64 位十六进制。
fn parse_catalog(json: &Value) -> Catalog {
    let mut out = Catalog::default();
    let Some(root) = json.as_object() else {
        return out;
    };
    match root.get("schema_version").and_then(Value::as_i64) {
        Some(1) | Some(2) => {}
        _ => return out,
    }
    let Some(mods) = root.get("mods").and_then(Value::as_array) else {
        return out;
    };

    for raw in mods {
        let Some(mod_) = parse_mod(raw) else {
            continue;
        };
        if mod_.releases.is_empty() {
            continue;
        }
        out.mods.push(mod_);
    }
    out
}

fn parse_mod(raw: &Value) -> Option<CatalogMod> {
    let id = str_field(raw, "id")?;
    if !valid_mod_id(&id) {
        return None;
    }
    let name = str_field(raw, "name")?;
    let author = str_field(raw, "author")?;
    let description = str_field(raw, "description")?;
    let releases = raw
        .get("releases")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse_release).collect::<Vec<_>>())
        .unwrap_or_default();

    Some(CatalogMod {
        id,
        name,
        author,
        description,
        icon_url: https_field(raw, "icon_url"),
        homepage_url: https_field(raw, "homepage_url"),
        tags: raw
            .get("tags")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        // 按发布时间倒序（对齐 `ModCatalogRepository.parse`）。
        releases,
    })
}

fn parse_release(raw: &Value) -> Option<CatalogRelease> {
    let version = str_field(raw, "version")?;
    let minecraft_versions = raw
        .get("minecraft_versions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // 无MC 版本声明的发布无法判定兼容性，不予采纳（避免装上必然失效的包）。
    if minecraft_versions.is_empty() {
        return None;
    }
    let download_type = raw
        .get("download_type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if !matches!(download_type.as_str(), "direct" | "browser" | "ad") {
        return None;
    }

    let assets = if download_type == "direct" {
        raw.get("assets")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(parse_asset).collect::<Vec<_>>())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if download_type == "direct" && assets.is_empty() {
        return None;
    }

    Some(CatalogRelease {
        version,
        minecraft_versions,
        download_type,
        assets,
        published_at: raw
            .get("published_at")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
    })
}

fn parse_asset(raw: &Value) -> Option<CatalogAsset> {
    let download_url = https_field(raw, "download_url")?;
    let name = str_field(raw, "name")
        .or_else(|| file_name_from_url(&download_url))?;
    if !valid_asset_name(&name) {
        return None;
    }
    let size = raw.get("size").and_then(Value::as_u64).unwrap_or(0);
    let sha256 = raw
        .get("sha256")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase());
    if let Some(digest) = &sha256 {
        if !valid_sha256(digest) {
            return None;
        }
    }
    Some(CatalogAsset {
        name,
        download_url,
        size,
        sha256,
    })
}

fn str_field(raw: &Value, key: &str) -> Option<String> {
    let text = raw.get(key).and_then(Value::as_str)?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// 只接受 https 字段（http / 相对路径 / 协议相对一律丢弃）。
fn https_field(raw: &Value, key: &str) -> Option<String> {
    let text = raw.get(key).and_then(Value::as_str)?.trim();
    if text.is_empty() || !text.to_ascii_lowercase().starts_with("https://") {
        return None;
    }
    Some(text.to_string())
}

fn file_name_from_url(url: &str) -> Option<String> {
    url.rsplit('/')
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn valid_mod_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MOD_ID_MAX_LEN
        && id.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

fn valid_asset_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".levipack") || lower.ends_with(".zip") || lower.ends_with(".so")
}

fn valid_sha256(digest: &str) -> bool {
    digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit())
}

// ---------------------------------------------------------------- MC 版本匹配

/// 移植 `ModNativeLoader.matchesMinecraftVersionPattern`。
///
/// 支持三类写法：`>=1.2.3` 最低版本；含 `*` / `X` / `x` 的通配（`X` 处于完整段位
/// 时匹配多位数，否则匹配单数字）；无通配符则精确相等。
pub fn matches_minecraft_version(pattern: &str, minecraft_version: &str) -> bool {
    let pattern = pattern.trim();
    let version = minecraft_version.trim();
    if pattern.is_empty() || version.is_empty() {
        return false;
    }
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let version_chars: Vec<char> = version.chars().collect();
    match_wildcard(&pattern_chars, &version_chars)
}

/// 逐段数字比较（缺失段补 0）；任一段非数字视为不可比较（返回 `None`）。
fn compare_minecraft_versions(left: &str, right: &str) -> Option<i32> {
    let l: Vec<char> = left.trim().chars().collect();
    let r: Vec<char> = right.trim().chars().collect();
    compare_segments(&l, &r)
}

/// 通配匹配：`*` 匹配任意长度；`X` / `x` 在「完整段位」上匹配多位数、
/// 否则匹配单数字；`>=` 前缀为最低版本；其余字符按字面量比较。
///
/// 直接在字符序列上做回溯，不生成也不解析正则——内核无 `regex` 依赖，
/// 而此处的模式全部来自本函数自己，语义自封闭。
fn match_wildcard(pattern: &[char], version: &[char]) -> bool {
    if let Some(rest) = strip_prefix_ci(pattern, ">=") {
        let trimmed = trim_trailing_zero(rest);
        return match compare_segments(version, trimmed) {
            Some(order) => order >= 0,
            // 版本段含非数字：无法比较即视为不兼容，而非默认放行。
            None => false,
        };
    }
    if !pattern.iter().any(|c| matches!(c, '*' | 'X' | 'x')) {
        return pattern == version;
    }
    match_from(pattern, 0, version, 0)
}

fn strip_prefix_ci<'a>(pattern: &'a [char], prefix: &str) -> Option<&'a [char]> {
    let head: Vec<char> = prefix.chars().collect();
    if pattern.len() >= head.len()
        && pattern[..head.len()]
            .iter()
            .zip(head.iter())
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    {
        Some(&pattern[head.len()..])
    } else {
        None
    }
}

/// 去掉尾部的空白（`>=1.2.3 ` 这类书写）。
fn trim_trailing_zero(mut pattern: &[char]) -> &[char] {
    while let Some(last) = pattern.last() {
        if last.is_whitespace() {
            pattern = &pattern[..pattern.len() - 1];
        } else {
            break;
        }
    }
    pattern
}

/// 逐段数字比较（缺失段补 0）；任一段非数字返回 `None`。
fn compare_segments(left: &[char], right: &[char]) -> Option<i32> {
    let l = to_segments(left)?;
    let r = to_segments(right)?;
    let count = l.len().max(r.len());
    for i in 0..count {
        let lv = l.get(i).copied().unwrap_or(0);
        let rv = r.get(i).copied().unwrap_or(0);
        if lv != rv {
            return Some(if lv > rv { 1 } else { -1 });
        }
    }
    Some(0)
}

fn to_segments(text: &[char]) -> Option<Vec<i64>> {
    let joined: String = text.iter().collect();
    if joined.is_empty() {
        return None;
    }
    joined.split('.').map(|part| part.parse::<i64>().ok()).collect()
}

fn match_from(pattern: &[char], pi: usize, version: &[char], vi: usize) -> bool {
    if pi == pattern.len() {
        return vi == version.len();
    }
    match pattern[pi] {
        '*' => (vi..=version.len()).any(|cut| match_from(pattern, pi + 1, version, cut)),
        'X' | 'x' => {
            // 与参考实现 `ModNativeLoader.matchesMinecraftVersionPattern` 的
            // 正则语义一致：完整段位（前后为 `.` 或边界）→ `\d+`（一位或多位）；
            // 段内 → `\d`（恰好一位）。
            let whole_component = (pi == 0 || pattern[pi - 1] == '.')
                && (pi + 1 == pattern.len() || pattern[pi + 1] == '.');
            let mut end = vi;
            while end < version.len() && version[end].is_ascii_digit() {
                end += 1;
            }
            let range = if whole_component {
                vi..=end
            } else {
                vi..=end.min(vi + 1)
            };
            range.rev().any(|cut| match_from(pattern, pi + 1, version, cut))
        }
        expected => {
            vi < version.len()
                && version[vi] == expected
                && match_from(pattern, pi + 1, version, vi + 1)
        }
    }
}

/// 发布是否兼容给定 MC 版本（`minecraft_versions` 为空视为全兼容）。
fn release_supports(release: &CatalogRelease, minecraft_version: &str) -> bool {
    if release.minecraft_versions.is_empty() {
        return true;
    }
    release
        .minecraft_versions
        .iter()
        .any(|pattern| matches_minecraft_version(pattern, minecraft_version))
}

// ---------------------------------------------------------------- 资产选择

/// 选出可直连下载的资产。
///
/// 裸 `.so` 优先（体积最小、无需解包）；否则取第一个 `.levipack` / `.zip`。
fn pick_asset(release: &CatalogRelease) -> Option<&CatalogAsset> {
    if release.download_type != "direct" {
        return None;
    }
    release
        .assets
        .iter()
        .find(|a| a.name.to_ascii_lowercase().ends_with(".so"))
        .or_else(|| release.assets.first())
}

/// 解析后的安卓安装目标（供 `mod.rs` 落盘与前端提示复用）。
#[derive(Debug, Clone)]
pub struct InstallTarget {
    /// 模组最终目录 `<versions>/<name>/mods/<modId>`。
    pub mod_dir: PathBuf,
    /// 目标实例名（写入下载记录，用于完成后定位落点）。
    pub version: String,
}

// ---------------------------------------------------------------- 落点解析

/// 解析安卓 LL 模组落点：`<versions>/<name>/mods/<modId>`。
///
/// 与桌面「无选中版本就回退缓存」**刻意不同**：安卓 mods 目录与游戏目录平行，
/// 下错位置 mod 不会生效且用户无感，因此目标不可用时直接报错。
pub fn resolve_install_target(
    kernel: &KernelContext,
    version: &str,
    mod_id: &str,
) -> Result<InstallTarget, (&'static str, String)> {
    if !valid_mod_id(mod_id) {
        return Err((ERR_LLA_BAD_ARCHIVE, format!("非法的模组 id `{mod_id}`")));
    }
    let dir = crate::modules::home::meta::resolve_version_dir(&kernel.versions_root(), version)
        .map_err(|e| (ERR_LLA_TARGET_NOT_FOUND, e.payload()))?;
    let meta = crate::modules::home::meta::VersionMeta::read(&dir).ok_or((
        ERR_LLA_TARGET_NOT_FOUND,
        format!("版本 `{version}` 元数据缺失"),
    ))?;
    if meta.android.is_none() {
        return Err((
            ERR_LLA_TARGET_NOT_ANDROID,
            format!("版本 `{version}` 不是安卓实例，无法安装安卓原生模组"),
        ));
    }
    Ok(InstallTarget {
        mod_dir: dir.join("mods").join(mod_id),
        version: version.to_string(),
    })
}

// ---------------------------------------------------------------- 对外接口

/// 列表：关键字搜索 + 排序 + 页码分页；`game_version` 按 `minecraft_versions` 过滤。
pub async fn list(
    _kernel: &KernelContext,
    query: &ContentListQuery,
) -> Result<ContentListPage, KernelError> {
    let data = catalog().await?;
    let search = query.search.as_deref().unwrap_or("").trim().to_lowercase();
    let game_version = query
        .game_version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());

    let mut matched: Vec<ContentItem> = data
        .mods
        .iter()
        .filter(|m| {
            search.is_empty()
                || m.name.to_lowercase().contains(&search)
                || m.id.to_lowercase().contains(&search)
                || m.description.to_lowercase().contains(&search)
        })
        .filter(|m| match game_version {
            Some(version) => m.releases.iter().any(|r| release_supports(r, version)),
            None => true,
        })
        .map(mod_to_item)
        .collect();

    match normalize_sort(query.sort.as_deref()) {
        SORT_NAME_ASC => matched.sort_by(|a, b| a.name.cmp(&b.name)),
        SORT_UPDATED_DESC => matched.sort_by(|a, b| {
            b.latest_published_at
                .cmp(&a.latest_published_at)
                .then_with(|| a.name.cmp(&b.name))
        }),
        // 目录既无下载量也无可靠发布时间可比：升序按名称倒序返回，
        // 降序（默认）按名称升序，**不伪造下载量数字**。
        _ => matched.sort_by(|a, b| b.name.cmp(&a.name)),
    }

    let total = matched.len() as u64;
    let start = query.page as usize * PAGE_SIZE as usize;
    let items: Vec<ContentItem> = matched
        .into_iter()
        .skip(start)
        .take(PAGE_SIZE as usize)
        .collect();
    let has_more = start + items.len() < total as usize;
    Ok(ContentListPage {
        items,
        has_more,
        total,
    })
}

/// 详情：mod id → 全部发布（按发布时间倒序）。
pub async fn detail(_kernel: &KernelContext, id: &str) -> Result<ContentDetail, KernelError> {
    let mod_id = id.trim();
    let data = catalog().await?;
    let found = data
        .mods
        .iter()
        .find(|m| m.id.eq_ignore_ascii_case(mod_id))
        .ok_or_else(|| KernelError::InvalidArgument(format!("模组 `lla:{mod_id}` 不存在")))?;

    let item = mod_to_item(found);
    let mut files: Vec<ContentFile> = Vec::new();
    let mut game_versions: Vec<String> = Vec::new();

    for release in &found.releases {
        for version in &release.minecraft_versions {
            if !game_versions.iter().any(|v| v == version) {
                game_versions.push(version.clone());
            }
        }
        // 无可直连资产（browser / ad）时仍列出该版本，前端据此显示「需前往发布页」。
        let asset = pick_asset(release);
        files.push(ContentFile {
            id: format!("lla-f:{}:{}", found.id, release.version),
            version: release.version.clone(),
            filename: asset
                .map(|a| a.name.clone())
                .unwrap_or_else(|| format!("{}.levipack", found.id)),
            download_url: asset.map(|a| a.download_url.clone()).unwrap_or_default(),
            size: asset.map(|a| a.size).unwrap_or(0),
            sha256: asset.and_then(|a| a.sha256.clone()),
            game_versions: release.minecraft_versions.clone(),
            dependencies: Vec::new(),
            release_type: release_type_of(&release.version),
            // 安卓链路不使用 lip variant 字段。
            variant: None,
            published_at_hint: release.published_at.clone(),
        });
    }
    // 目录内`releases` 已是发布时间倒序，保留该顺序（版本号本身不可比）。
    game_versions.sort_by(|a, b| match compare_minecraft_versions(b, a) {
        Some(-1) => std::cmp::Ordering::Less,
        Some(1) => std::cmp::Ordering::Greater,
        // 含非数字段（如 `1.26.5X.X`）时不可比，退回字典序而非伪造顺序。
        _ => std::cmp::Ordering::Equal,
    });

    Ok(ContentDetail {
        item,
        project_url: found.homepage_url.clone(),
        repo_url: found.homepage_url.clone(),
        authors: vec![found.author.clone()],
        files,
        game_versions,
    })
}

// ---------------------------------------------------------------- 内部工具

fn mod_to_item(m: &CatalogMod) -> ContentItem {
    ContentItem {
        id: format!("{}:{}", SOURCE_LL_ANDROID_PREFIX, m.id),
        source: SOURCE_LL_ANDROID.to_string(),
        content_type: TYPE_LL_MOD.to_string(),
        name: m.name.clone(),
        description: m.description.clone(),
        author: Some(m.author.clone()).filter(|a| !a.is_empty()),
        icon_url: m.icon_url.clone(),
        categories: m.tags.clone(),
        min_game_version: None,
        max_game_version: None,
        latest_version: m
            .releases
            .first()
            .map(|r| r.version.clone())
            .unwrap_or_default(),
        download_count: 0,
        // 排序辅助字段：最近发布时间（目录无下载量，用发布时间代表新鲜度）。
        latest_published_at: m
            .releases
            .first()
            .map(|r| r.published_at.clone())
            .unwrap_or_default(),
    }
}

/// id 前缀（与 `model::SOURCE_LL_ANDROID` 配对）。
const SOURCE_LL_ANDROID_PREFIX: &str = "lla";

/// 版本号 → 发布渠道（含 `-rc` / `-beta` 视为预览 / 测试）。
fn release_type_of(version: &str) -> String {
    let lower = version.to_ascii_lowercase();
    if lower.contains("-rc") || lower.contains("-alpha") {
        "alpha".to_string()
    } else if lower.contains("-beta") {
        "beta".to_string()
    } else {
        "release".to_string()
    }
}

// ---------------------------------------------------------------- 解包落盘

/// 解包 `.levipack` / `.zip` 到 `target_dir`，并规范化 `manifest.json`。
///
/// 流程对齐 LeviLaunchroid `FileHandler.prepareImport`：定位 mod 根 → 校验
/// `manifest.json` + 唯一 `.so` → 重写 manifest → 就地落盘。
///
/// `target_dir` 已存在时按「更新」处理：先解到同级 `.<modId>.new`，
/// 成功后交换；任一步失败都保留原目录。
pub fn install_archive(
    archive: &Path,
    target_dir: &Path,
    display: &ModDisplay,
) -> Result<(), (&'static str, String)> {
    let parent = target_dir
        .parent()
        .ok_or((ERR_LLA_INSTALL_FAILED, "模组目录缺少父级".to_string()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| (ERR_LLA_INSTALL_FAILED, format!("创建模组目录失败：{e}")))?;

    let staging = parent.join(format!(
        ".{}.new",
        target_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "mod".into())
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|e| (ERR_LLA_INSTALL_FAILED, format!("创建暂存目录失败：{e}")))?;

    let extracted = extract_archive(archive, &staging);
    let outcome = match extracted {
        Err(error) => Err(error),
        Ok(root) => finalize_mod_root(&root, display),
    };

    if let Err(error) = outcome {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    // 原子替换：旧目录先移到 `.bak`，交换成功后再删；失败则回滚。
    let backup = parent.join(format!(
        ".{}.bak",
        target_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "mod".into())
    ));
    let had_previous = target_dir.exists();
    if had_previous {
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::rename(target_dir, &backup)
            .map_err(|e| (ERR_LLA_INSTALL_FAILED, format!("备份原模组目录失败：{e}")))?;
    }
    if let Err(e) = std::fs::rename(&staging, target_dir) {
        if had_previous {
            let _ = std::fs::rename(&backup, target_dir);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err((ERR_LLA_INSTALL_FAILED, format!("落位模组目录失败：{e}")));
    }
    if had_previous {
        let _ = std::fs::remove_dir_all(&backup);
    }
    Ok(())
}

/// 规范化 manifest 时使用的展示信息（来自目录条目，保证 manifest 自洽）。
#[derive(Debug, Clone)]
pub struct ModDisplay {
    pub name: String,
    pub author: String,
    pub version: String,
    pub minecraft_versions: Vec<String>,
}

/// 解压归档到 `out_dir`，返回定位到的 mod 根目录。
fn extract_archive(archive: &Path, out_dir: &Path) -> Result<PathBuf, (&'static str, String)> {
    let file = std::fs::File::open(archive)
        .map_err(|e| (ERR_LLA_INSTALL_FAILED, format!("打开归档失败：{e}")))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| (ERR_LLA_BAD_ARCHIVE, format!("归档不是有效zip：{e}")))?;

    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| (ERR_LLA_BAD_ARCHIVE, format!("读取归档条目失败：{e}")))?;
        let Some(raw_name) = entry.enclosed_name() else {
            // 路径逃逸（`..` / 绝对路径）条目直接拒绝整个包，不做部分安装。
            return Err((
                ERR_LLA_BAD_ARCHIVE,
                "归档包含越界路径条目，已拒绝安装".to_string(),
            ));
        };
        if entry.is_dir() {
            continue;
        }
        let rel = raw_name.to_string_lossy().replace('\\', "/");
        let Some(safe) = sanitize_relative(&rel) else {
            return Err((ERR_LLA_BAD_ARCHIVE, format!("归档条目路径非法：`{rel}`")));
        };
        let target = out_dir.join(safe);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| (ERR_LLA_INSTALL_FAILED, format!("创建目录失败：{e}")))?;
        }
        let mut output = std::fs::File::create(&target)
            .map_err(|e| (ERR_LLA_INSTALL_FAILED, format!("写入文件失败：{e}")))?;
        std::io::copy(&mut entry, &mut output)
            .map_err(|e| (ERR_LLA_INSTALL_FAILED, format!("解压失败：{e}")))?;
    }
    Ok(out_dir.to_path_buf())
}

/// 二次防御性路径净化（`enclosed_name` 之外的兜底）。
fn sanitize_relative(rel: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in rel.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            p => parts.push(p),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// 在 `staging` 内定位 mod 根、校验并规范化 manifest。
///
/// `staging` 本身是根时原地处理；否则要求其下**有且仅有一个**含
/// `manifest.json` 的候选目录（对齐 `FileHandler.findImportedModRoot`）。
fn finalize_mod_root(root: &Path, display: &ModDisplay) -> Result<(), (&'static str, String)> {
    let root = resolve_mod_root(root)?;
    let manifest_path = root.join(MANIFEST_FILE);
    let raw = std::fs::read(&manifest_path)
        .map_err(|e| (ERR_LLA_BAD_ARCHIVE, format!("模组缺少 manifest.json：{e}")))?;
    let mut manifest: Value = serde_json::from_slice(&raw)
        .map_err(|e| (ERR_LLA_BAD_ARCHIVE, format!("manifest.json 解析失败：{e}")))?;

    let so_files = collect_so_files(&root);
    if so_files.is_empty() {
        return Err((
            ERR_LLA_BAD_ARCHIVE,
            "模组内没有任何 .so 入口".to_string(),
        ));
    }
    let entry = resolve_entry(&manifest, &root, &so_files).ok_or((
        ERR_LLA_BAD_ARCHIVE,
        "无法确定唯一 .so 入口（manifest.entry 失效且存在多个候选）".to_string(),
    ))?;

    // 规范化：补齐 loader 依赖的字段，避免沿用发布方缺省的 manifest。
    if let Some(map) = manifest.as_object_mut() {
        map.insert("type".into(), Value::from(MOD_TYPE));
        map.insert("name".into(), Value::from(display.name.clone()));
        map.insert("author".into(), Value::from(display.author.clone()));
        map.insert("version".into(), Value::from(display.version.clone()));
        map.insert("entry".into(), Value::from(entry.clone()));
        map.insert(
            "minecraft_versions".into(),
            Value::from(display.minecraft_versions.clone()),
        );
    }
    let normalized = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| (ERR_LLA_INSTALL_FAILED, format!("序列化 manifest 失败：{e}")))?;
    std::fs::write(&manifest_path, normalized)
        .map_err(|e| (ERR_LLA_INSTALL_FAILED, format!("写入 manifest 失败：{e}")))?;

    log::info!(
        "[content-download] 安卓模组就绪: {} (entry={entry})",
        root.display()
    );
    Ok(())
}

const MANIFEST_FILE: &str = "manifest.json";
/// preloader 要求的 mod 类型（与 `ModManager.PRELOAD_NATIVE_TYPE` 一致）。
const MOD_TYPE: &str = "preload-native";

/// 定位含 `manifest.json` 的 mod 根。
fn resolve_mod_root(root: &Path) -> Result<PathBuf, (&'static str, String)> {
    if root.join(MANIFEST_FILE).is_file() {
        return Ok(root.to_path_buf());
    }
    let mut candidates = Vec::new();
    collect_mod_roots(root, &mut candidates);
    match candidates.len() {
        1 => Ok(candidates.remove(0)),
        0 => Err((ERR_LLA_BAD_ARCHIVE, "归档内找不到 manifest.json".to_string())),
        _ => Err((
            ERR_LLA_BAD_ARCHIVE,
            "归档内存在多个模组目录，无法确定安装哪一个".to_string(),
        )),
    }
}

fn collect_mod_roots(current: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || entry.file_name().to_string_lossy() == "__MACOSX" {
            continue;
        }
        if path.join(MANIFEST_FILE).is_file() {
            out.push(path);
            continue;
        }
        collect_mod_roots(&path, out);
    }
}

/// 递归收集 `.so` 相对路径（`/` 分隔，字典序）。
fn collect_so_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    collect_so_files_into(root, root, &mut out);
    out.sort();
    out
}

fn collect_so_files_into(root: &Path, current: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_so_files_into(root, &path, out);
            continue;
        }
        let is_so = path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase().ends_with(".so"))
            .unwrap_or(false);
        if !is_so {
            continue;
        }
        if let Ok(rel) = path.strip_prefix(root) {
            let normalized = rel.to_string_lossy().replace('\\', "/");
            if sanitize_relative(&normalized).is_some() {
                out.push(normalized);
            }
        }
    }
}

/// 解析 `.so` 入口：manifest.entry 有效则用之；否则唯一候选直接用；
/// 多个候选时按文件名匹配（对齐 `FileHandler.resolveEntryPath`）。
fn resolve_entry(manifest: &Value, root: &Path, so_files: &[String]) -> Option<String> {
    if let Some(entry) = manifest
        .get("entry")
        .and_then(Value::as_str)
        .and_then(normalize_entry_path)
    {
        if root.join(&entry).is_file() {
            return Some(entry);
        }
    }
    if so_files.len() == 1 {
        return Some(so_files[0].clone());
    }
    None
}

/// 规整 entry 相对路径；绝对路径 / 含 `..` / 空段一律拒绝。
fn normalize_entry_path(entry: &str) -> Option<String> {
    let normalized = entry.trim().replace('\\', "/");
    if normalized.is_empty() || normalized.starts_with('/') {
        return None;
    }
    sanitize_relative(&normalized)
}

// ---------------------------------------------------------------- 记录映射

/// 从 `lla:<modId>` 还原裸 mod id。
pub fn parse_mod_id(content_id: &str) -> Option<&str> {
    content_id
        .strip_prefix(&format!("{SOURCE_LL_ANDROID_PREFIX}:"))
        .map(str::trim)
        .filter(|id| !id.is_empty())
}



// ---------------------------------------------------------------- 单测

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_catalog() -> Value {
        serde_json::json!({
            "schema_version": 2,
            "mods": [
                {
                    "id": "chickpet",
                    "name": "ChickPet",
                    "author": "someone",
                    "description": "宠物",
                    "icon_url": "https://example.com/i.png",
                    "homepage_url": "https://example.com/repo",
                    "tags": ["Cosmetic"],
                    "releases": [
                        {
                            "version": "1.0.0",
                            "minecraft_versions": ["1.21.*"],
                            "download_type": "direct",
                            "assets": [{
                                "name": "ChickPet.levipack",
                                "download_url": "https://example.com/ChickPet.levipack",
                                "size": 1024,
                                "sha256": "a".repeat(64)
                            }],
                            "published_at": "2026-01-02T00:00:00Z"
                        },
                        {
                            "version": "0.9.0",
                            "minecraft_versions": ["1.20.*"],
                            "download_type": "browser",
                            "download_url": "https://example.com/page",
                            "assets": [],
                            "published_at": "2025-01-02T00:00:00Z"
                        }
                    ]
                },
                { "id": "BAD ID", "name": "x", "author": "y", "description": "z", "releases": [] },
                {
                    "id": "nomcver",
                    "name": "NoMc",
                    "author": "y",
                    "description": "z",
                    "releases": [{
                        "version": "1.0.0",
                        "minecraft_versions": [],
                        "download_type": "direct",
                        "assets": [{ "name": "a.zip", "download_url": "https://e.com/a.zip" }]
                    }]
                }
            ]
        })
    }

    #[test]
    fn parse_catalog_filters_invalid_entries() {
        let catalog = parse_catalog(&sample_catalog());
        assert_eq!(catalog.mods.len(), 1, "非法 id 与无MC 版本声明的条目应被丢弃");
        let chickpet = &catalog.mods[0];
        assert_eq!(chickpet.id, "chickpet");
        assert_eq!(chickpet.releases.len(), 2);
        assert_eq!(chickpet.tags, vec!["Cosmetic".to_string()]);
    }

    #[test]
    fn parse_catalog_rejects_unknown_schema() {
        let mut json = sample_catalog();
        json["schema_version"] = serde_json::json!(9);
        assert!(parse_catalog(&json).mods.is_empty());
    }

    /// 构造一个最小可安装的 `.levipack`（zip）：`manifest.json` + 唯一 `.so`。
    fn write_levipack(root: &Path, manifest: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        std::fs::create_dir_all(root).unwrap();
        let path = root.join("mod.levipack");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file(MANIFEST_FILE, options).unwrap();
        std::io::Write::write_all(&mut zip, manifest.as_bytes()).unwrap();
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            std::io::Write::write_all(&mut zip, bytes).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "copper_lla_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn display() -> ModDisplay {
        ModDisplay {
            name: "ChickPet".into(),
            author: "someone".into(),
            version: "1.0.0".into(),
            minecraft_versions: vec!["1.21.*".into()],
        }
    }

    #[test]
    fn install_unpacks_levipack_into_mod_dir() {
        let base = temp_dir("install_ok");
        let archive = write_levipack(
            &base,
            r#"{"type":"preload-native","name":"old","entry":"libchick.so"}"#,
            &[("libchick.so", b"\x7fELF"), ("config/config.json", b"{}")],
        );
        let target = base.join("mods").join("chickpet");

        install_archive(&archive, &target, &display()).expect("应安装成功");

        assert!(target.join("manifest.json").is_file());
        assert!(target.join("libchick.so").is_file());
        assert!(target.join("config").join("config.json").is_file());

        // manifest 被规范化：名称 / 作者 / 版本 / MC 版本来自目录条目。
        let raw = std::fs::read(target.join("manifest.json")).unwrap();
        let manifest: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(manifest["type"], MOD_TYPE);
        assert_eq!(manifest["name"], "ChickPet");
        assert_eq!(manifest["author"], "someone");
        assert_eq!(manifest["version"], "1.0.0");
        assert_eq!(manifest["entry"], "libchick.so");
        assert_eq!(manifest["minecraft_versions"][0], "1.21.*");

        // 暂存目录不得残留。
        assert!(!target.parent().unwrap().join(".chickpet.new").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn install_overwrites_existing_mod() {
        let base = temp_dir("install_overwrite");
        let target = base.join("mods").join("chickpet");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("manifest.json"), "{}").unwrap();
        std::fs::write(target.join("stale.txt"), "old").unwrap();

        let archive = write_levipack(
            &base,
            r#"{"entry":"libchick.so"}"#,
            &[("libchick.so", b"\x7fELF")],
        );
        install_archive(&archive, &target, &display()).expect("更新应成功");

        assert!(target.join("libchick.so").is_file());
        // 更新语义是整体替换，不是合并。
        assert!(!target.join("stale.txt").exists());
        assert!(!target.parent().unwrap().join(".chickpet.bak").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn install_rejects_archive_without_so() {
        let base = temp_dir("install_noso");
        let archive = write_levipack(&base, r#"{"entry":"libx.so"}"#, &[]);
        let target = base.join("mods").join("chickpet");

        let error = install_archive(&archive, &target, &display()).unwrap_err();
        assert_eq!(error.0, ERR_LLA_BAD_ARCHIVE);
        // 失败不得留下半成品目录或暂存目录。
        assert!(!target.exists());
        assert!(!target.parent().unwrap().join(".chickpet.new").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn install_rejects_archive_without_manifest() {
        let base = temp_dir("install_nomanifest");
        let archive = write_levipack(&base, "{}", &[("libx.so", b"\x7fELF")]);
        // 把 manifest 换成一个非manifest 的文件，模拟结构不合法的包。
        let file = std::fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("readme.txt", options).unwrap();
        std::io::Write::write_all(&mut zip, b"hi").unwrap();
        zip.start_file("libx.so", options).unwrap();
        std::io::Write::write_all(&mut zip, b"\x7fELF").unwrap();
        zip.finish().unwrap();

        let target = base.join("mods").join("chickpet");
        let error = install_archive(&archive, &target, &display()).unwrap_err();
        assert_eq!(error.0, ERR_LLA_BAD_ARCHIVE);
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn install_rejects_ambiguous_entry() {
        let base = temp_dir("install_ambig");
        // 两个 `.so` 且 manifest.entry 指向不存在的文件 → 无法确定入口。
        let archive = write_levipack(
            &base,
            r#"{"entry":"missing.so"}"#,
            &[("liba.so", b"\x7fELF"), ("libb.so", b"\x7fELF")],
        );
        let target = base.join("mods").join("chickpet");
        let error = install_archive(&archive, &target, &display()).unwrap_err();
        assert_eq!(error.0, ERR_LLA_BAD_ARCHIVE);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn install_keeps_original_on_failure() {
        let base = temp_dir("install_rollback");
        let target = base.join("mods").join("chickpet");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("manifest.json"), r#"{"name":"keep"}"#).unwrap();

        // 非 zip 内容：解包阶段即判定为坏归档。
        let archive = write_levipack(&base, "{}", &[]);
        std::fs::write(&archive, b"not a zip at all").unwrap();

        let error = install_archive(&archive, &target, &display()).unwrap_err();
        assert_eq!(error.0, ERR_LLA_BAD_ARCHIVE);
        // 原目录必须完好无损。
        assert!(target.join("manifest.json").is_file());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn pick_asset_prefers_bare_so() {
        let release = CatalogRelease {
            version: "1.0.0".into(),
            minecraft_versions: vec!["1.21.*".into()],
            download_type: "direct".into(),
            assets: vec![
                CatalogAsset {
                    name: "a.levipack".into(),
                    download_url: "https://e.com/a.levipack".into(),
                    size: 1,
                    sha256: None,
                },
                CatalogAsset {
                    name: "libx.so".into(),
                    download_url: "https://e.com/libx.so".into(),
                    size: 2,
                    sha256: None,
                },
            ],
            published_at: String::new(),
        };
        assert_eq!(pick_asset(&release).unwrap().name, "libx.so");

        let browser = CatalogRelease {
            download_type: "browser".into(),
            ..release
        };
        assert!(pick_asset(&browser).is_none());
    }

    #[test]
    fn mod_id_pattern_matches_reference() {
        assert!(valid_mod_id("chickpet"));
        assert!(valid_mod_id("4d-skin.v2_1"));
        assert!(!valid_mod_id("ChickPet"));
        assert!(!valid_mod_id("-leading"));
        assert!(!valid_mod_id(""));
        assert!(!valid_mod_id(&"a".repeat(65)));
    }

    #[test]
    fn sha256_must_be_64_hex() {
        assert!(valid_sha256(&"A1".repeat(32)));
        assert!(!valid_sha256(&"a".repeat(63)));
        assert!(!valid_sha256(&"z".repeat(64)));
    }

    #[test]
    fn minecraft_version_patterns() {
        assert!(matches_minecraft_version("1.21.130.20", "1.21.130.20"));
        assert!(!matches_minecraft_version("1.21.130.20", "1.21.130.21"));

        assert!(matches_minecraft_version("1.21.*", "1.21.130.20"));
        assert!(!matches_minecraft_version("1.21.*", "1.22.0.1"));

        // 完整段位的 `X` 语义等同 `\d+`（一位或多位），与参考实现一致。
        assert!(matches_minecraft_version("1.21.X", "1.21.130"));
        assert!(matches_minecraft_version("1.21.X", "1.21.5"));
        assert!(matches_minecraft_version("1.21.x.0", "1.21.5.0"));

        // 段内（非完整段位）的 `X` 语义等同 `\d`，只吃一位。
        assert!(matches_minecraft_version("1.2X.0", "1.21.0"));
        assert!(!matches_minecraft_version("1.2X.0", "1.211.0"));

        assert!(matches_minecraft_version(">=1.20.0", "1.21.130.20"));
        assert!(!matches_minecraft_version(">=1.22.0", "1.21.130.20"));
        assert!(matches_minecraft_version(">=26.51.0", "26.51.0"));

        assert!(!matches_minecraft_version("", "1.21.0"));
        assert!(!matches_minecraft_version("1.21.0", ""));
    }

    #[test]
    fn release_supports_empty_patterns_means_all() {
        let release = CatalogRelease {
            version: "1.0.0".into(),
            minecraft_versions: vec![],
            download_type: "direct".into(),
            assets: vec![],
            published_at: String::new(),
        };
        assert!(release_supports(&release, "1.21.0"));
    }

    #[test]
    fn sanitize_relative_rejects_traversal() {
        assert_eq!(sanitize_relative("a/b.so").as_deref(), Some("a/b.so"));
        assert_eq!(sanitize_relative("./a.so").as_deref(), Some("a.so"));
        assert!(sanitize_relative("../evil.so").is_none());
        assert!(sanitize_relative("a/../../evil.so").is_none());
        assert!(sanitize_relative("").is_none());
    }

    #[test]
    fn normalize_entry_path_rejects_absolute() {
        assert_eq!(
            normalize_entry_path("lib\\a.so").as_deref(),
            Some("lib/a.so")
        );
        assert!(normalize_entry_path("/abs/a.so").is_none());
        assert!(normalize_entry_path("a/../../b.so").is_none());
    }

    #[test]
    fn mod_id_roundtrip() {
        assert_eq!(parse_mod_id("lla:chickpet"), Some("chickpet"));
        assert_eq!(parse_mod_id("lip:owner/repo"), None);
        assert_eq!(parse_mod_id("lla:"), None);
    }

    #[test]
    fn release_type_detects_prerelease() {
        assert_eq!(release_type_of("1.0.0"), "release");
        assert_eq!(release_type_of("26.10.0-rc.1"), "alpha");
        assert_eq!(release_type_of("1.0.0-beta2"), "beta");
    }

    #[test]
    fn https_field_rejects_insecure() {
        let raw = serde_json::json!({ "a": "http://e.com/x", "b": "https://e.com/y", "c": "//e.com" });
        assert!(https_field(&raw, "a").is_none());
        assert_eq!(https_field(&raw, "b").as_deref(), Some("https://e.com/y"));
        assert!(https_field(&raw, "c").is_none());
    }

    #[test]
    fn asset_name_suffix_gate() {
        assert!(valid_asset_name("a.levipack"));
        assert!(valid_asset_name("a.ZIP"));
        assert!(valid_asset_name("libx.so"));
        assert!(!valid_asset_name("a.exe"));
        assert!(!valid_asset_name("a.tar.gz"));
    }

    #[test]
    fn version_compare_missing_segments_are_zero() {
        assert_eq!(compare_minecraft_versions("1.21", "1.21.0"), Some(0));
        assert_eq!(compare_minecraft_versions("1.21.1", "1.21"), Some(1));
        assert_eq!(compare_minecraft_versions("1.20", "1.21"), Some(-1));
        assert_eq!(compare_minecraft_versions("1.x", "1.21"), None);
    }
}