//! 内容下载模块 · 统一内容模型。
//!
//! 内容下载把两类数据源（CurseForge：行为包 / 材质包 / 光影包；
//! LIP：LL 模组）归一为同一份模型供前端消费，字段 `camelCase` 序列化。
//!
//! 约定：
//! - `id` 为跨源唯一键：CurseForge 用 `cf:<modId>`，LIP 用 `lip:<identifier>`。
//! - `content_type` 取值 `behavior_pack` / `texture_pack` / `shader` / `ll_mod`。
//! - `source` 取值 `curseforge` / `lip`（供"按来源过滤"）。

use serde::{Deserialize, Serialize};

/// 内容类型。
pub const TYPE_BEHAVIOR_PACK: &str = "behavior_pack";
pub const TYPE_TEXTURE_PACK: &str = "texture_pack";
pub const TYPE_SHADER: &str = "shader";
pub const TYPE_LL_MOD: &str = "ll_mod";

/// 内容来源。
pub const SOURCE_CURSEFORGE: &str = "curseforge";
pub const SOURCE_LIP: &str = "lip";

/// 列表卡片内容项。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentItem {
    /// 跨源唯一 id（`cf:<modId>` / `lip:<identifier>`）。
    pub id: String,
    /// 来源：`curseforge` / `lip`。
    pub source: String,
    /// 内容类型。
    pub content_type: String,
    pub name: String,
    pub description: String,
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    /// 分类 / 标签（用于过滤）。
    pub categories: Vec<String>,
    /// 支持的 MCBE 游戏版本范围（最低～最高，粗略聚合）。
    pub min_game_version: Option<String>,
    pub max_game_version: Option<String>,
    /// 最新版本号。
    pub latest_version: String,
    /// 下载量（近似热度，用于排序）。
    pub download_count: u64,
}

/// 详情页内的一个可下载文件 / 版本。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentFile {
    pub id: String,
    /// 版本号（如 `1.0.0`；CurseForge 用 `latestFilesIndexes.gameVersion` 相近值）。
    pub version: String,
    pub filename: String,
    pub download_url: String,
    /// 文件大小（字节）。
    pub size: u64,
    /// 期望 sha256（可选，命中时投递给下载引擎做完整性校验）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// 适配的 MCBE 游戏版本列表。
    pub game_versions: Vec<String>,
    /// 前置 / 可选依赖。
    pub dependencies: Vec<ContentDependency>,
    /// 发布类型：release / beta / alpha。
    pub release_type: String,
}

/// 依赖项（前置模组跳转 / 可选依赖）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentDependency {
    /// 依赖项的跨源 id（可经详情命令跳转）。
    pub ref_id: String,
    /// 展示名（未知为空）。
    pub name: String,
    /// `required` / `optional`。
    pub kind: String,
}

/// 内容详情。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentDetail {
    /// 供前端立即派生的列表信息。
    pub item: ContentItem,
    /// 项目主页（来源站 / GitHub），用于"快速链接"跳转。
    pub project_url: Option<String>,
    /// GitHub 仓库 URL（存在时可拉取 readme；中文优先 `readme-zh-cn`）。
    pub repo_url: Option<String>,
    /// 全部作者名。
    pub authors: Vec<String>,
    /// 全部可下载文件 / 版本。
    pub files: Vec<ContentFile>,
    /// 涉及的全部 MCBE 游戏版本（已去重、降序），供版本分类。
    pub game_versions: Vec<String>,
}

/// 列表查询参数（命令层入参，camelCase）。
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContentListQuery {
    /// 按来源过滤（`curseforge` / `lip`）。
    pub source: Option<String>,
    /// 按内容类型过滤（`behavior_pack` 等）。
    pub content_type: Option<String>,
    /// 搜索关键字。
    pub search: Option<String>,
    /// 页码（从 0 起），每页默认 40。
    pub page: u32,
}

/// 每页条数（与来源分页对齐，不可用则客户端截断）。
pub const PAGE_SIZE: u32 = 40;

/// 列表返回。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentListPage {
    pub items: Vec<ContentItem>,
    /// 是否还有下一页。
    pub has_more: bool,
    pub total: u64,
}