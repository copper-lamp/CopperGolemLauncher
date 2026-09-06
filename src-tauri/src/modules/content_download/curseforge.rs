//! 内容下载模块 · CurseForge 提供器。
//!
//! 真实对接 CurseForge API（base `https://api.curseforge.com`），来源为
//! MC 基岩版（`gameId=78022`），覆盖行为包 / 材质包 / 光影包三类。
//!
//! - API key 从设置 `content.curseforgeApiKey` 读取（缺失时返回友好错误）；
//! - 类型 → classId 依 `/v1/categories` 实时解析并按名字匹配（参考 LeviLauncher），
//!   解析结果缓存，命中频控友好；
//! - 下载走 `/v1/mods/{id}/files` 的 `downloadUrl`，可投递到内核下载引擎。
//!
//! 纯解析函数与网络分离，便于单测覆盖（[`parse_mods`] 等）。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;

use crate::error::KernelError;
use crate::state::KernelContext;

use super::http;
use super::model::{
    ContentDependency, ContentDetail, ContentFile, ContentItem, ContentListPage,
    ContentListQuery, SOURCE_CURSEFORGE, TYPE_BEHAVIOR_PACK, TYPE_SHADER, TYPE_TEXTURE_PACK,
};

/// CurseForge API 基址与 MC 基岩版 gameId（参考 LeviLauncher）。
const BASE_URL: &str = "https://api.curseforge.com";
const GAME_ID: &str = "78022";
/// 类别 / 分类表缓存 TTL（秒）。
const CATEGORY_CACHE_TTL_SECS: u64 = 300;

/// 每请求最大条数（API 上限 50）。
const PAGE_SIZE: usize = 40;

/// 内容类型 → 类别名（用于从 categories 解析 classId）。
fn class_name_for(ctype: &str) -> Option<&'static str> {
    match ctype {
        TYPE_BEHAVIOR_PACK => Some("Behavior Packs"),
        TYPE_TEXTURE_PACK => Some("Texture Packs"),
        TYPE_SHADER => Some("Shaders"),
        _ => None,
    }
}

// ---------------------------------------------------------------- API 响应模型

/// `/v1/mods/search` 响应。
#[derive(Debug, Deserialize)]
struct SearchResponse {
    data: Vec<ModData>,
    pagination: Option<Pagination>,
}

/// `/v1/mods/{id}` 响应。
#[derive(Debug, Deserialize)]
struct ModResponse {
    data: ModData,
}

/// 单个项目（mod）。
#[derive(Debug, Clone, Deserialize)]
struct ModData {
    #[serde(default)]
    id: i64,
    name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    download_count: i64,
    #[serde(default)]
    class_id: i64,
    #[serde(default)]
    categories: Vec<Category>,
    #[serde(default)]
    authors: Vec<Author>,
    #[serde(default)]
    logo: Option<Logo>,
    #[serde(default)]
    links: Option<Links>,
    #[serde(default)]
    latest_files_indexes: Vec<LatestFilesIndexes>,
    #[serde(default)]
    latest_files: Vec<File>,
}

/// 分页信息。
#[derive(Debug, Deserialize)]
struct Pagination {
    #[serde(default)]
    total_count: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct Category {
    #[serde(default)]
    id: i64,
    name: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    class_id: i64,
    #[serde(default)]
    is_class: bool,
    #[serde(default)]
    game_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct Author {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Logo {
    #[serde(default)]
    thumbnail_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Links {
    #[serde(default)]
    website_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct LatestFilesIndexes {
    #[serde(default)]
    game_version: Option<String>,
    #[serde(default)]
    file_id: i64,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    release_type: i64,
}

/// 可下载文件。
#[derive(Debug, Clone, Deserialize)]
struct File {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    file_name: String,
    #[serde(default)]
    download_url: Option<String>,
    #[serde(default)]
    release_type: i64,
    #[serde(default)]
    file_length: Option<i64>,
    #[serde(default)]
    hashes: Vec<FileHash>,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    dependencies: Vec<Dependency>,
    #[serde(default)]
    mod_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct FileHash {
    #[serde(default)]
    value: Option<String>,
}

/// 依赖项（relationType：3=required，2=optional）。
#[derive(Debug, Clone, Deserialize)]
struct Dependency {
    #[serde(default)]
    mod_id: i64,
    #[serde(default)]
    relation_type: i64,
}

/// `/v1/mods/{id}/files` 响应。
#[derive(Debug, Deserialize)]
struct GetFilesResponse {
    data: Vec<File>,
}

/// `/v1/mods/{id}/description` 响应。
#[derive(Debug, Deserialize)]
struct ModDescriptionResponse {
    data: String,
}

/// `/v1/categories` 响应。
#[derive(Debug, Deserialize)]
struct CategoriesResponse {
    data: Vec<Category>,
}

// ---------------------------------------------------------------- 类别缓存

struct CategoryCache {
    id_by_name: HashMap<String, i64>,
    type_by_id: HashMap<i64, String>,
    fetched_at: std::time::Instant,
}

static CATEGORY_CACHE: OnceLock<Mutex<Option<CategoryCache>>> = OnceLock::new();

fn category_cache() -> &'static Mutex<Option<CategoryCache>> {
    CATEGORY_CACHE.get_or_init(|| Mutex::new(None))
}

/// 类别名 → 内容类型（`class_name_for` 的反向）。
fn content_type_by_class_name(name: &str) -> Option<&'static str> {
    match name.to_lowercase().as_str() {
        "behavior packs" => Some(TYPE_BEHAVIOR_PACK),
        "texture packs" => Some(TYPE_TEXTURE_PACK),
        "shaders" => Some(TYPE_SHADER),
        _ => None,
    }
}

/// 拉取 / 复用类别缓存。
async fn ensure_categories(kernel: &KernelContext) -> Result<(), KernelError> {
    if let Some(c) = category_cache().lock().unwrap().as_ref() {
        if c.fetched_at.elapsed().as_secs() < CATEGORY_CACHE_TTL_SECS {
            return Ok(());
        }
    }
    let api_key = api_key(kernel)?;
    let url = format!("{BASE_URL}/v1/categories?gameId={GAME_ID}");
    let resp: CategoriesResponse = http::client()
        .get(&url)
        .header("x-api-key", api_key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let mut by_name = HashMap::new();
    let mut by_id = HashMap::new();
    for c in resp.data {
        if let Some(ty) = content_type_by_class_name(&c.name) {
            by_id.insert(c.id, ty.to_string());
        }
        by_name.insert(c.name.to_lowercase(), c.id);
    }
    *category_cache().lock().unwrap() = Some(CategoryCache {
        id_by_name: by_name,
        type_by_id: by_id,
        fetched_at: std::time::Instant::now(),
    });
    Ok(())
}

/// 返回类别名 → id。
async fn class_id_by_name(kernel: &KernelContext, name: &str) -> Result<Option<i64>, KernelError> {
    ensure_categories(kernel).await?;
    Ok(category_cache()
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|c| c.id_by_name.get(&name.to_lowercase()).copied()))
}

/// 返回类别 id → 内容类型。
async fn content_type_for_class(
    kernel: &KernelContext,
    class_id: i64,
) -> Result<Option<String>, KernelError> {
    ensure_categories(kernel).await?;
    Ok(category_cache()
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|c| c.type_by_id.get(&class_id).cloned()))
}

// ---------------------------------------------------------------- API key

/// 读取 CurseForge API key；未配置返回友好错误（部署配置项，见设计文档备注）。
fn api_key(kernel: &KernelContext) -> Result<String, KernelError> {
    let key = kernel.settings().get_or("content.curseforgeApiKey", String::new());
    let key = key.trim();
    if key.is_empty() {
        return Err(KernelError::InvalidArgument(
            "未配置 CurseForge API key（设置 content.curseforgeApiKey）。".into(),
        ));
    }
    Ok(key.to_string())
}

// ---------------------------------------------------------------- 对外接口

/// 列表：搜索 / 过滤 / 分页。
pub async fn list(
    kernel: &KernelContext,
    query: &ContentListQuery,
) -> Result<ContentListPage, KernelError> {
    let api_key = api_key(kernel)?;
    let page = query.page;

    // 类型 → classId（可选）。未指定类型则不过滤 class。
    let class_id = match query.content_type.as_deref() {
        Some(ct) => match class_name_for(ct) {
            Some(cls) => class_id_by_name(kernel, cls).await?,
            None => Some(0),
        },
        None => Some(0),
    };

    let mut url = format!(
        "{BASE_URL}/v1/mods/search?gameId={GAME_ID}&pageSize={PAGE_SIZE}&index={}",
        page * PAGE_SIZE as u32
    );
    if class_id.is_some_and(|v| v > 0) {
        url.push_str(&format!("&classId={}", class_id.unwrap_or(0)));
    }
    if let Some(s) = query.search.as_deref() {
        if !s.is_empty() {
            url.push_str(&format!("&searchFilter={}", urlencoded(s)));
        }
    }

    let resp: SearchResponse = http::client()
        .get(&url)
        .header("x-api-key", api_key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let total = resp.pagination.as_ref().map(|p| p.total_count.max(0) as u64).unwrap_or(0);
    // 已按 classId 过滤，返回内容类型即查询类型；未指定类型时用 `unknown` 兜底。
    let content_type = match query.content_type.as_deref() {
        Some(ct) if class_name_for(ct).is_some() => ct,
        _ => "unknown",
    };
    let items: Vec<ContentItem> = resp
        .data
        .iter()
        .map(|m| mod_to_item(m, content_type))
        .collect();
    let has_more = resp.data.len() as u64 == PAGE_SIZE as u64
        && ((page + 1) * PAGE_SIZE as u32) < total as u32;

    Ok(ContentListPage {
        items,
        has_more,
        total,
    })
}

/// 详情：项目信息 + 全部文件 + 游戏版本聚合。
pub async fn detail(kernel: &KernelContext, mod_id: i64) -> Result<ContentDetail, KernelError> {
    let api_key = api_key(kernel)?;

    let mod_url = format!("{BASE_URL}/v1/mods/{mod_id}");
    let mod_resp: ModResponse = http::client()
        .get(&mod_url)
        .header("x-api-key", &api_key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let files_resp: GetFilesResponse = http::client()
        .get(format!("{BASE_URL}/v1/mods/{mod_id}/files?pageSize=50"))
        .header("x-api-key", &api_key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let item = {
        let ctype = if mod_resp.data.class_id > 0 {
            content_type_for_class(kernel, mod_resp.data.class_id)
                .await?
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            "unknown".to_string()
        };
        mod_to_item(&mod_resp.data, &ctype)
    };

    let files: Vec<ContentFile> = files_resp.data.iter().map(file_to_model).collect();
    let game_versions = aggregate_game_versions(&files);

    Ok(ContentDetail {
        item,
        project_url: mod_resp.data.links.as_ref().and_then(|l| l.website_url.clone()),
        repo_url: None,
        authors: mod_resp
            .data
            .authors
            .iter()
            .map(|a| a.name.clone())
            .collect(),
        files,
        game_versions,
    })
}

/// 拉取项目 readme（HTML）。前端按需调用；无内容返回 None。
pub async fn description(kernel: &KernelContext, mod_id: i64) -> Result<Option<String>, KernelError> {
    let api_key = api_key(kernel)?;
    let resp: ModDescriptionResponse = http::client()
        .get(format!("{BASE_URL}/v1/mods/{mod_id}/description"))
        .header("x-api-key", api_key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let html = resp.data;
    Ok((!html.trim().is_empty()).then_some(html))
}

// ---------------------------------------------------------------- 归一化

fn release_type_name(rt: i64) -> &'static str {
    match rt {
        2 => "beta",
        3 => "alpha",
        _ => "release",
    }
}

fn dep_kind(relation_type: i64) -> Option<&'static str> {
    match relation_type {
        3 => Some("required"),
        2 => Some("optional"),
        _ => None,
    }
}

/// 从项目取最新版本串（优先 displayName，其次文件名去扩展名）。
fn latest_version_of(m: &ModData) -> String {
    if let Some(f) = m.latest_files.first() {
        if let Some(dn) = &f.display_name {
            if !dn.trim().is_empty() {
                return dn.clone();
            }
        }
        if !f.file_name.trim().is_empty() {
            return strip_extension(&f.file_name);
        }
    }
    if let Some(i) = m.latest_files_indexes.first() {
        if let Some(fn_) = &i.filename {
            if !fn_.trim().is_empty() {
                return strip_extension(fn_);
            }
        }
    }
    "latest".into()
}

fn strip_extension(name: &str) -> String {
    let name = name.trim();
    match name.rsplit_once('.') {
        Some((base, ext)) if !base.is_empty() && !ext.is_empty() => base.to_string(),
        _ => name.to_string(),
    }
}

/// 把 CF 项目归一化为列表模型。`content_type` 为查询命中的类型。
fn mod_to_item(m: &ModData, content_type: &str) -> ContentItem {
    let categories: Vec<String> = m
        .categories
        .iter()
        .filter(|c| c.class_id == m.class_id && !c.is_class)
        .map(|c| c.name.to_lowercase())
        .collect();

    let (min_game_version, max_game_version) = version_range(m);

    ContentItem {
        id: format!("cf:{}", m.id),
        source: SOURCE_CURSEFORGE.to_string(),
        content_type: content_type.to_string(),
        name: m.name.clone(),
        description: m.summary.clone(),
        author: m.authors.first().map(|a| a.name.clone()),
        icon_url: m.logo.as_ref().and_then(|l| l.thumbnail_url.clone()),
        categories,
        min_game_version,
        max_game_version,
        latest_version: latest_version_of(m),
        download_count: m.download_count.max(0) as u64,
    }
}

fn version_range(m: &ModData) -> (Option<String>, Option<String>) {
    let mut versions: Vec<(u32, u32)> = Vec::new();
    for i in &m.latest_files_indexes {
        if let Some(gv) = &i.game_version {
            if let Some(num) = parse_game_version(gv) {
                versions.push(num);
            }
        }
    }
    if versions.is_empty() {
        // 退路：最新文件的 gameVersions。
        if let Some(f) = m.latest_files.first() {
            for gv in &f.game_versions {
                if let Some(num) = parse_game_version(gv) {
                    versions.push(num);
                }
            }
        }
    }
    versions.sort_unstable();
    match (versions.first(), versions.last()) {
        (Some(min), Some(max)) if min != max => {
            (Some(fmt_version(*min)), Some(fmt_version(*max)))
        }
        (Some(only), _) => (Some(fmt_version(*only)), None),
        _ => (None, None),
    }
}

/// 解析形如 `1.21` / `26.40` 的游戏版本为 (major, minor)。
fn parse_game_version(v: &str) -> Option<(u32, u32)> {
    let clean = v.trim().split('-').next().unwrap_or(v.trim());
    let mut parts = clean.split('.');
    let major: u32 = parts.next()?.trim().parse().ok()?;
    let minor: u32 = parts.next().unwrap_or("0").trim().parse().ok()?;
    Some((major, minor))
}

fn fmt_version((major, minor): (u32, u32)) -> String {
    format!("{major}.{minor}")
}

fn file_to_model(f: &File) -> ContentFile {
    let mut game_versions: Vec<String> = f.game_versions.clone();
    game_versions.sort();
    game_versions.dedup();

    let sha256 = f
        .hashes
        .iter()
        .find(|h| h.value.is_some())
        .and_then(|h| h.value.clone());

    ContentFile {
        id: format!("cf-f:{}", f.id),
        version: strip_extension(&f.file_name),
        filename: f.file_name.clone(),
        download_url: f.download_url.clone().unwrap_or_default(),
        size: f.file_length.unwrap_or(0).max(0) as u64,
        sha256,
        game_versions,
        dependencies: f
            .dependencies
            .iter()
            .filter_map(|d| {
                dep_kind(d.relation_type).map(|kind| ContentDependency {
                    ref_id: format!("cf:{}", d.mod_id),
                    name: String::new(),
                    kind: kind.to_string(),
                })
            })
            .collect(),
        release_type: release_type_name(f.release_type).to_string(),
    }
}

/// 聚合全部文件的 MCBE 游戏版本（去重、降序）。
fn aggregate_game_versions(files: &[ContentFile]) -> Vec<String> {
    let mut set: Vec<(u32, u32)> = Vec::new();
    for f in files {
        for gv in &f.game_versions {
            if let Some(num) = parse_game_version(gv) {
                if !set.contains(&num) {
                    set.push(num);
                }
            }
        }
    }
    set.sort_unstable_by(|a, b| b.cmp(a));
    set.into_iter().map(fmt_version).collect()
}

fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_game_version_basic() {
        assert_eq!(parse_game_version("1.21"), Some((1, 21)));
        assert_eq!(parse_game_version("26.40"), Some((26, 40)));
        assert_eq!(parse_game_version("1.20.30"), Some((1, 20)));
        assert_eq!(parse_game_version("x.y"), None);
        assert_eq!(parse_game_version("abc"), None);
    }

    #[test]
    fn strip_extension_trims_file_suffix() {
        assert_eq!(strip_extension("pack_v2.mcpack"), "pack_v2");
        assert_eq!(strip_extension("mod.zip"), "mod");
    }

    #[test]
    fn release_type_and_dependency_mapping() {
        assert_eq!(release_type_name(1), "release");
        assert_eq!(release_type_name(2), "beta");
        assert_eq!(release_type_name(3), "alpha");
        assert_eq!(dep_kind(3), Some("required"));
        assert_eq!(dep_kind(2), Some("optional"));
        assert_eq!(dep_kind(5), None);
    }

    #[test]
    fn mod_to_item_maps_core_fields() {
        let m = ModData {
            id: 123,
            name: "测试包".into(),
            summary: "desc".into(),
            download_count: 99,
            class_id: 0,
            categories: vec![Category {
                id: 1,
                name: "GUI".into(),
                slug: "gui".into(),
                class_id: 0,
                is_class: false,
                game_id: 78022,
            }],
            authors: vec![Author { name: "作者".into() }],
            logo: Some(Logo {
                thumbnail_url: Some("http://x/thumb.png".into()),
            }),
            links: None,
            latest_files_indexes: vec![],
            latest_files: vec![File {
                id: 1,
                display_name: Some("v2".into()),
                file_name: "a.mcpack".into(),
                download_url: Some("http://d".into()),
                release_type: 1,
                file_length: Some(1024),
                hashes: vec![],
                game_versions: vec!["1.21".into(), "1.20".into()],
                dependencies: vec![],
                mod_id: 123,
            }],
        };
        let item = mod_to_item(&m, TYPE_BEHAVIOR_PACK);
        assert_eq!(item.id, "cf:123");
        assert_eq!(item.author.as_deref(), Some("作者"));
        assert_eq!(item.latest_version, "v2");
        assert_eq!(item.icon_url.as_deref(), Some("http://x/thumb.png"));
        assert!(item.categories.contains(&"gui".to_string()));
    }

    #[test]
    fn version_range_from_latest_files_indexes() {
        let m = ModData {
            latest_files_indexes: vec![
                LatestFilesIndexes {
                    game_version: Some("1.20".into()),
                    file_id: 1,
                    filename: Some("a.mcpack".into()),
                    release_type: 1,
                },
                LatestFilesIndexes {
                    game_version: Some("1.21".into()),
                    file_id: 2,
                    filename: Some("b.mcpack".into()),
                    release_type: 1,
                },
                LatestFilesIndexes {
                    game_version: Some("notanumber".into()),
                    file_id: 3,
                    filename: None,
                    release_type: 1,
                },
            ],
            ..empty_mod()
        };
        let (min, max) = version_range(&m);
        assert_eq!(min.as_deref(), Some("1.20"));
        assert_eq!(max.as_deref(), Some("1.21"));
    }

    #[test]
    fn file_to_model_maps_dependencies_and_hash() {
        let f = File {
            id: 55,
            display_name: Some("v5".into()),
            file_name: "b.mcpack".into(),
            download_url: Some("http://d".into()),
            release_type: 3,
            file_length: Some(2048),
            hashes: vec![FileHash {
                value: Some("deadbeef".into()),
            }],
            game_versions: vec!["1.21".into(), "1.21".into()],
            dependencies: vec![
                Dependency {
                    mod_id: 77,
                    relation_type: 3,
                },
                Dependency {
                    mod_id: 88,
                    relation_type: 1,
                },
            ],
            mod_id: i64::MIN,
        };
        let model = file_to_model(&f);
        assert_eq!(model.id, "cf-f:55");
        assert_eq!(model.sha256.as_deref(), Some("deadbeef"));
        assert_eq!(model.release_type, "alpha");
        assert_eq!(model.game_versions.len(), 1);
        assert_eq!(model.game_versions[0], "1.21");
        assert_eq!(model.dependencies.len(), 1);
        assert_eq!(model.dependencies[0].ref_id, "cf:77");
    }

    fn empty_mod() -> ModData {
        ModData {
            id: 0,
            name: String::new(),
            summary: String::new(),
            download_count: 0,
            class_id: 0,
            categories: vec![],
            authors: vec![],
            logo: None,
            links: None,
            latest_files_indexes: vec![],
            latest_files: vec![],
        }
    }
}