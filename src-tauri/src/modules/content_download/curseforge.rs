//! 内容下载模块 · CurseForge 提供器。
//!
//! 真实对接 CurseForge API（base `https://api.curseforge.com`）。两类来源并存：
//!
//! - 行为包 / 材质包走 MC 基岩版（`gameId=78022`）；
//! - 光影包走 Java 版（`gameId=432`，classId 6552），因为启动器的光影由 Arke
//!   （Iris 系加载器）加载，Java 版光影包数量与质量远好于基岩版。
//!
//! 两个版本表的语义不同：基岩版 `gameVersion` 是按点分段的前缀匹配（`26` 匹配不到
//! `26.60`），Java 版是真正的大版本前缀（`1.21` 覆盖 `1.21.x`），所以版本列表也分两套。
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
    normalize_sort, ContentDependency, ContentDetail, ContentFile, ContentItem, ContentListPage,
    ContentListQuery, SOURCE_CURSEFORGE, SORT_DOWNLOADS_ASC, SORT_NAME_ASC, SORT_UPDATED_DESC,
    TYPE_BEHAVIOR_PACK, TYPE_SHADER, TYPE_TEXTURE_PACK,
};

/// CurseForge API 基址与 MC 基岩版 gameId（参考 LeviLauncher）。
const BASE_URL: &str = "https://api.curseforge.com";
const GAME_ID: &str = "78022";
/// Java 版 gameId：光影包只取 Java 版资源（Arke 加载的是 Java 光影）。
const JAVA_GAME_ID: &str = "432";
/// Java 版「光影（Shaders）」类别 id。
///
/// 这个值是 CurseForge 的固定分类，不放进按名字解析的类别缓存：基岩版类别表里
/// 同名类别是另一套 id，混用会把光影查询打到错误的分类上。
const JAVA_SHADER_CLASS_ID: i64 = 6552;
/// 类别 / 分类表缓存 TTL（秒）。
const CATEGORY_CACHE_TTL_SECS: u64 = 0; // Persistent operation cache owns freshness; never re-stamp old provider values.
/// 游戏版本列表缓存 TTL（秒）。
const VERSIONS_CACHE_TTL_SECS: u64 = 0;

/// 每请求最大条数（API 上限 50）。
const PAGE_SIZE: usize = 40;

/// 内容类型 → 类别名（用于从 categories 解析 classId）。
fn class_name_for(ctype: &str) -> Option<&'static str> {
    match ctype {
        TYPE_BEHAVIOR_PACK => Some("Behavior Packs"),
        TYPE_TEXTURE_PACK => Some("Texture Packs"),
        _ => None,
    }
}

/// 光影包是否走 Java 版来源（决定 gameId / classId / 版本表）。
fn is_java_shader(ctype: Option<&str>) -> bool {
    ctype == Some(TYPE_SHADER)
}

/// Java 版光影子类别 → categoryId。
///
/// Java 光影分类只有三个固定类别（6553 Realistic / 6554 Fantasy / 6555 Vanilla），
/// 与基岩版那一整套类别名互不相干，所以这里硬编码映射，查不到就不加过滤参数。
fn java_shader_category_id(tag: &str) -> Option<i64> {
    match tag.trim().to_lowercase().as_str() {
        "realistic" => Some(6553),
        "fantasy" => Some(6554),
        "vanilla" => Some(6555),
        _ => None,
    }
}

/// Java 版版本号是否是发布版（`1.21.4`），用于剔除 `25w45a` 这类快照。
fn is_release_version(raw: &str) -> bool {
    raw.matches('.').count() >= 1
        && raw
            .split('.')
            .all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_digit()))
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

/// 单个项目（mod）。CurseForge API 字段均为 camelCase，统一反序列化别名。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
struct Pagination {
    #[serde(default)]
    total_count: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
struct Logo {
    #[serde(default)]
    thumbnail_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Links {
    #[serde(default)]
    website_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
    #[serde(default)]
    algo: Option<i64>,
}

/// 依赖项（relationType：3=required，2=optional）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
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

/// 排序方式 → CurseForge `sortField` / `sortOrder`。
///
/// CurseForge `sortField` 取值：2 = Name，3 = LastUpdated，6 = TotalDownloads。
fn sort_params(sort: &str) -> (u32, &'static str) {
    match sort {
        SORT_DOWNLOADS_ASC => (6, "asc"),
        SORT_NAME_ASC => (2, "asc"),
        SORT_UPDATED_DESC => (3, "desc"),
        // SORT_DOWNLOADS_DESC 及其它未知值统一为下载量降序。
        _ => (6, "desc"),
    }
}

/// `/v1/games/{gameId}/versions` 响应。
///
/// CurseForge 把这个游戏的版本按「版本类型」分组返回，形如
/// `{"data":[{"type":73249,"versions":["1.21.90","1.21.81", ...]}]}`；
/// 版本号串本身在每组的 `versions` 数组里，**不是**每项一个 `{name, slug}`。
#[derive(Debug, Deserialize)]
struct GameVersionsResponse {
    #[serde(default)]
    data: Vec<GameVersionGroup>,
}

/// 一个版本类型分组：`type` 是 CF 的版本类型 id（MCBE 目前只有 73249），
/// 这里只关心版本号串，id 不参与业务。
#[derive(Debug, Clone, Deserialize)]
struct GameVersionGroup {
    #[serde(default)]
    versions: Vec<String>,
}

/// `/v1/minecraft/version` 响应（Java 版版本表）。
#[derive(Debug, Deserialize)]
struct MinecraftVersionsResponse {
    #[serde(default)]
    data: Vec<MinecraftVersionEntry>,
}

/// Java 版版本表的一项；只用 `versionString`（其余字段是资源下载地址与类型）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MinecraftVersionEntry {
    #[serde(default)]
    version_string: String,
}

// ---------------------------------------------------------------- 类别缓存


struct CategoryCache {
    id_by_name: HashMap<String, i64>,
    type_by_id: HashMap<i64, String>,
    fetched_at: std::time::Instant,
}

static CATEGORY_CACHE: OnceLock<Mutex<Option<CategoryCache>>> = OnceLock::new();

/// 全部类别名（小写；前端用 `text-transform: capitalize` 呈现）。
///
/// CurseForge 侧没有独立的"标签"概念，列表的 categories 就是类别名，
/// 所以平铺筛选栏直接用这份类别表。
pub async fn tags(kernel: &KernelContext) -> Result<Vec<String>, KernelError> {
    ensure_categories(kernel).await?;
    let mut out: Vec<String> = category_cache()
        .lock()
        .unwrap()
        .as_ref()
        .map(|cache| cache.id_by_name.keys().cloned().collect())
        .unwrap_or_default();
    out.sort();
    Ok(out)
}

fn category_cache() -> &'static Mutex<Option<CategoryCache>> {
    CATEGORY_CACHE.get_or_init(|| Mutex::new(None))
}

/// Java 版光影的固定标签（与 `java_shader_category_id` 的键一一对应）。
pub fn shader_tags() -> Vec<String> {
    vec!["Realistic".into(), "Fantasy".into(), "Vanilla".into()]
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
    // Java 光影是固定 classId，不依赖（也不应该依赖）基岩版类别表。
    if class_id == JAVA_SHADER_CLASS_ID {
        return Ok(Some(TYPE_SHADER.to_string()));
    }
    ensure_categories(kernel).await?;
    Ok(category_cache()
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|c| c.type_by_id.get(&class_id).cloned()))
}

// ---------------------------------------------------------------- API key

/// 内置（混淆）CurseForge API key，编译内置，无需用户配置。
/// 部署运维如需更换可写设置 `content.curseforgeApiKey` 覆盖（可选）。
/// `BUILTIN_KEY_B64` 为官方 3rd Party CurseForge API key 的 base64 形式。
const BUILTIN_KEY_B64: &str = "JDJhJDEwJEdFQVlodG9xT0Q3a1c3NTJoNjE4a2VDekoxUjNuTjRtTnlrd0tRLzJEV3lpblFIdlJKcmd1";

/// 读取 CurseForge API key：优先用户覆盖，未覆盖则回退内置混淆 key。
fn api_key(kernel: &KernelContext) -> Result<String, KernelError> {
    let custom = kernel.settings().get_or("content.curseforgeApiKey", String::new());
    let custom = custom.trim();
    if !custom.is_empty() {
        return Ok(custom.to_string());
    }
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(BUILTIN_KEY_B64)
        .map_err(|_| KernelError::Config("内置内容下载 key 缺失，请联系开发者配置".into()))?;
    String::from_utf8(raw).map_err(|_| KernelError::Config("内置内容下载 key 损坏".into()))
}

// ---------------------------------------------------------------- 对外接口

/// 列表：搜索 / 过滤 / 分页。
pub async fn list(
    kernel: &KernelContext,
    query: &ContentListQuery,
) -> Result<ContentListPage, KernelError> {
    let api_key = api_key(kernel)?;
    let page = query.page;
    let java_shader = is_java_shader(query.content_type.as_deref());

    // 类型 → classId（可选）。未指定类型则不过滤 class。
    // 光影固定走 Java 版：gameId 换成 432，classId 固定 6552。
    let class_id = if java_shader {
        Some(JAVA_SHADER_CLASS_ID)
    } else {
        match query.content_type.as_deref() {
            Some(ct) => match class_name_for(ct) {
                Some(cls) => class_id_by_name(kernel, cls).await?,
                None => Some(0),
            },
            None => Some(0),
        }
    };
    let game_id = if java_shader { JAVA_GAME_ID } else { GAME_ID };

    let mut url = format!(
        "{BASE_URL}/v1/mods/search?gameId={game_id}&pageSize={PAGE_SIZE}&index={}",
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
    if let Some(tag) = query.tag.as_deref() {
        let tag = tag.trim();
        if !tag.is_empty() {
            // CurseForge 没有"标签"概念，能做的是按**类别**过滤：
            // 基岩版的类别名能反查回 categoryId，Java 光影只有三个固定子类别。
            // 查不到同名类别时不加参数 —— 宁可不过滤，也不伪造一个必然为空的结果。
            let category_id = if java_shader {
                java_shader_category_id(tag)
            } else {
                class_id_by_name(kernel, tag).await?
            };
            if let Some(id) = category_id {
                url.push_str(&format!("&categoryId={id}"));
            }
        }
    }
    if let Some(gv) = query.game_version.as_deref() {
        let gv = gv.trim();
        if !gv.is_empty() {
            url.push_str(&format!("&gameVersion={}", urlencoded(gv)));
        }
    }
    let (sort_field, sort_order) = sort_params(normalize_sort(query.sort.as_deref()));
    url.push_str(&format!("&sortField={sort_field}&sortOrder={sort_order}"));

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
        Some(TYPE_SHADER) => TYPE_SHADER,
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
        total_pages: total.div_ceil(PAGE_SIZE as u64),
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

/// 游戏版本列表缓存（用于前端「版本过滤」下拉）。TTL 1 小时。
struct VersionsCache {
    versions: Vec<String>,
    fetched_at: std::time::Instant,
}

static VERSIONS_CACHE: OnceLock<Mutex<Option<VersionsCache>>> = OnceLock::new();

fn versions_cache() -> &'static Mutex<Option<VersionsCache>> {
    VERSIONS_CACHE.get_or_init(|| Mutex::new(None))
}

/// 拉取 / 复用 MCBE 游戏版本列表（降序，去重）。
///
/// 该列表即 CurseForge 搜索 `gameVersion` 参数的可选值，供前端版本过滤下拉使用。
pub async fn game_versions(kernel: &KernelContext) -> Result<Vec<String>, KernelError> {
    if let Some(c) = versions_cache().lock().unwrap().as_ref() {
        if c.fetched_at.elapsed().as_secs() < VERSIONS_CACHE_TTL_SECS {
            return Ok(c.versions.clone());
        }
    }
    let api_key = api_key(kernel)?;
    let url = format!("{BASE_URL}/v1/games/{GAME_ID}/versions");
    let resp: GameVersionsResponse = http::client()
        .get(&url)
        .header("x-api-key", api_key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let mut versions: Vec<String> = Vec::new();
    for group in resp.data {
        for raw in group.versions {
            let raw = raw.trim();
            if !raw.is_empty() && !versions.iter().any(|v| v == raw) {
                versions.push(raw.to_string());
            }
        }
    }
    // 版本号降序（新版本在前）：逐段数值比较，"1.21.90" 必须排在 "1.21.9" 之后，
    // 只比 (major, minor) 会让同一 minor 下的补丁版本互相乱序。
    versions.sort_by(|a, b| compare_game_versions(b, a).then_with(|| a.cmp(b)));

    *versions_cache().lock().unwrap() = Some(VersionsCache {
        versions: versions.clone(),
        fetched_at: std::time::Instant::now(),
    });
    Ok(versions)
}

/// Java 版游戏版本列表缓存（光影包用）。
static JAVA_VERSIONS_CACHE: OnceLock<Mutex<Option<VersionsCache>>> = OnceLock::new();

fn java_versions_cache() -> &'static Mutex<Option<VersionsCache>> {
    JAVA_VERSIONS_CACHE.get_or_init(|| Mutex::new(None))
}

/// 拉取 / 复用 Java 版游戏版本列表（发布版，降序）。
///
/// 这里不能用 `/v1/games/432/versions`：那个接口把 ModLoader、插件加载器版本
/// 和游戏版本混在一个分组里。`/v1/minecraft/version` 才是纯净的 Java 版本表，
/// 但也包含 `25w45a` 这类快照，按「纯数字点分」过滤掉。
pub async fn java_game_versions(kernel: &KernelContext) -> Result<Vec<String>, KernelError> {
    if let Some(c) = java_versions_cache().lock().unwrap().as_ref() {
        if c.fetched_at.elapsed().as_secs() < VERSIONS_CACHE_TTL_SECS {
            return Ok(c.versions.clone());
        }
    }
    let api_key = api_key(kernel)?;
    let url = format!("{BASE_URL}/v1/minecraft/version");
    let resp: MinecraftVersionsResponse = http::client()
        .get(&url)
        .header("x-api-key", api_key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let mut versions: Vec<String> = Vec::new();
    for entry in resp.data {
        let raw = entry.version_string.trim();
        if !is_release_version(raw) {
            continue;
        }
        if !versions.iter().any(|v| v == raw) {
            versions.push(raw.to_string());
        }
    }
    versions.sort_by(|a, b| compare_game_versions(b, a).then_with(|| a.cmp(b)));

    *java_versions_cache().lock().unwrap() = Some(VersionsCache {
        versions: versions.clone(),
        fetched_at: std::time::Instant::now(),
    });
    Ok(versions)
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
        latest_published_at: String::new(),
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

/// 逐段数值比较两个游戏版本号，用于「版本过滤」下拉的排序。
///
/// 不能只用 `parse_game_version` 的 (major, minor)：那会把 `1.21.9` 与 `1.21.90`
/// 视为同版本，排序结果随输入顺序漂移。这里按 `.`/`-`/`+` 切段逐段比数值，
/// 段数不同时短者补 0（`1.21` == `1.21.0`），非数字段退化为字符串比较。
fn compare_game_versions(a: &str, b: &str) -> std::cmp::Ordering {
    fn segments(v: &str) -> Vec<&str> {
        v.trim().split(['.', '-', '+']).collect()
    }
    let (sa, sb) = (segments(a), segments(b));
    for i in 0..sa.len().max(sb.len()) {
        let x = sa.get(i).copied().unwrap_or("0");
        let y = sb.get(i).copied().unwrap_or("0");
        let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(nx), Ok(ny)) => nx.cmp(&ny),
            _ => x.cmp(y),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
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

fn normalize_sha256_digest(value: &str) -> Option<String> {
    let value = value.trim();
    (value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

fn file_to_model(f: &File) -> ContentFile {
    let mut game_versions: Vec<String> = f.game_versions.clone();
    game_versions.sort();
    game_versions.dedup();

    let sha256 = f
        .hashes
        .iter()
        .filter(|h| h.algo == Some(3) || h.algo.is_none())
        .filter_map(|h| h.value.as_deref())
        .find_map(normalize_sha256_digest);

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
        variant: None,
        published_at_hint: String::new(),
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
    fn game_versions_response_flattens_groups() {
        // 真实响应形状：版本号串在每组的 versions 数组里，而不是每项一个 {name, slug}。
        let raw = r#"{"data":[{"type":73249,"versions":["1.21.90","1.21.81","1.21.9"]},
            {"type":73250,"versions":["1.20"]}]}"#;
        let resp: GameVersionsResponse = serde_json::from_str(raw).unwrap();
        let versions: Vec<String> = resp
            .data
            .into_iter()
            .flat_map(|g| g.versions)
            .collect();
        assert_eq!(versions, vec!["1.21.90", "1.21.81", "1.21.9", "1.20"]);
    }

    #[test]
    fn compare_game_versions_orders_patch_segments() {
        use std::cmp::Ordering;
        assert_eq!(compare_game_versions("1.21.90", "1.21.9"), Ordering::Greater);
        assert_eq!(compare_game_versions("1.21", "1.21.0"), Ordering::Equal);
        assert_eq!(compare_game_versions("1.21.100", "1.21.81"), Ordering::Greater);
        assert_eq!(compare_game_versions("26.60", "1.21.90"), Ordering::Greater);
        assert_eq!(compare_game_versions("1.9", "1.10"), Ordering::Less);
    }

    #[test]
    fn java_shader_tags_map_to_fixed_categories() {
        assert_eq!(java_shader_category_id("Realistic"), Some(6553));
        assert_eq!(java_shader_category_id(" fantasy "), Some(6554));
        assert_eq!(java_shader_category_id("VANILLA"), Some(6555));
        // 基岩版类别名不能落到 Java 光影上，否则会拼出一个必然为空的 categoryId。
        assert_eq!(java_shader_category_id("Adventure"), None);
    }

    #[test]
    fn release_version_filter_drops_snapshots() {
        assert!(is_release_version("1.21.4"));
        assert!(is_release_version("26.0"));
        assert!(!is_release_version("25w45a"));
        assert!(!is_release_version("1.21.4-pre1"));
        assert!(!is_release_version("1."));
        assert!(!is_release_version(""));
    }

    #[test]
    fn minecraft_versions_response_reads_version_string() {
        let raw = r#"{"data":[{"versionString":"1.21.4","type":"release"},
            {"versionString":"25w45a","type":"snapshot"}]}"#;
        let resp: MinecraftVersionsResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[0].version_string, "1.21.4");
        assert!(is_release_version(&resp.data[0].version_string));
        assert!(!is_release_version(&resp.data[1].version_string));
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
    fn file_to_model_skips_non_sha256_hashes() {
        let model = file_to_model(&File {
            hashes: vec![FileHash {
                value: Some("md5-value".into()),
                algo: Some(2),
            }],
            ..empty_file()
        });
        assert_eq!(model.sha256, None);
    }

    #[test]
    fn file_to_model_accepts_only_explicit_sha256_shape() {
        let digest = "A".repeat(64);
        let model = file_to_model(&File {
            hashes: vec![FileHash {
                value: Some(format!("  {digest}  ")),
                algo: None,
            }],
            ..empty_file()
        });
        assert_eq!(model.sha256.as_deref(), Some(digest.to_ascii_lowercase().as_str()));

        let model = file_to_model(&File {
            hashes: vec![FileHash {
                value: Some("a".repeat(64)),
                algo: Some(2),
            }],
            ..empty_file()
        });
        assert_eq!(model.sha256, None);
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
            hashes: vec![
                FileHash {
                    value: Some("md5".into()),
                    algo: Some(2),
                },
                FileHash {
                    value: Some("deadbeef".into()),
                    algo: Some(3),
                },
            ],
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
        assert_eq!(model.sha256, None);
        assert_eq!(model.release_type, "alpha");
        assert_eq!(model.game_versions.len(), 1);
        assert_eq!(model.game_versions[0], "1.21");
        assert_eq!(model.dependencies.len(), 1);
        assert_eq!(model.dependencies[0].ref_id, "cf:77");
    }

    fn empty_file() -> File {
        File {
            id: 0,
            display_name: None,
            file_name: "file.mcpack".into(),
            download_url: None,
            release_type: 1,
            file_length: None,
            hashes: vec![],
            game_versions: vec![],
            dependencies: vec![],
            mod_id: 0,
        }
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