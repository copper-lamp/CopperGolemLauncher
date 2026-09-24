//! 元数据契约类型：`index.json` / 分片 / `_index.json` / 模块条目的 serde 模型。
//!
//! 契约依据 [cgl-libs](../../../docs/cgl-libs.md) 2.2 / 2.3.1 / 2.3.2 / 2.3.3。
//! 向后兼容规则（同文档 3.1）：
//! - **客户端必须忽略未知字段**：JSON 结构体一律不启用 `deny_unknown_fields`，新字段不会导致反序列化失败；
//! - **枚举扩展**：`kind` / `platforms` / `channel` / `status` 用带 `Unknown` 兜底的反序列化，
//!   未知枚举值解析成功、由业务层跳过该条目，而不是让整份元数据解析失败；
//! - **缺摘要即拒绝**：`sha256` 属于安全判断字段，解析后由 [`ModuleEntry::is_verifiable`] 强制校验存在性。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 客户端支持的元数据契约版本（单整数，只做大小比较，见文档 3.1）。
pub const SUPPORTED_SCHEMA_VERSION: i64 = 1;

// ---------------------------------------------------------------------------
// 顶层索引 index.json
// ---------------------------------------------------------------------------

/// 顶层索引 `index.json`（客户端唯一硬编码入口，文档 2.2）。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RegistryIndex {
    pub schema_version: i64,
    /// RFC3339 UTC 生成时间，用于诊断与防降级比较。
    #[serde(default)]
    pub generated_at: String,
    /// 生成该索引时的仓库 commit sha，用于问题溯源与防降级判定。
    #[serde(default)]
    pub repo_commit: String,
    /// 服务该索引所需的最低启动器版本（semver）。
    #[serde(default)]
    pub min_launcher: String,
    #[serde(default)]
    pub entries: Vec<RegistryEntry>,
}

impl RegistryIndex {
    /// 按 `kind` 精确匹配的条目路径（kind 大小写敏感，与仓库契约一致）。
    pub fn path_of_kind(&self, kind: IndexEntryKind) -> Option<&RegistryEntry> {
        self.entries.iter().find(|e| e.kind == kind)
    }

    /// 按仓库内相对路径查找条目（如 `modules/s-u.json`）。
    pub fn entry_by_path(&self, path: &str) -> Option<&RegistryEntry> {
        self.entries.iter().find(|e| e.path == path)
    }
}

/// 索引条目：指向仓库内某份分片 / 清单。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RegistryEntry {
    pub kind: IndexEntryKind,
    #[serde(default)]
    pub path: String,
    /// 文件内容 sha256（小写十六进制）。缺失为空串，由业务层拒绝。
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: i64,
    /// 镜像绝对地址，第一个为主镜像。
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub cache_ttl_sec: i64,
}

impl RegistryEntry {
    /// 是否具备可校验的 sha256（64 位小写十六进制）。缺摘要的条目必须被拒绝（文档 2.5 规则 1）。
    pub fn has_sha256(&self) -> bool {
        is_sha256_hex(&self.sha256)
    }

    /// 建议 TTL 秒数，非正数视为未声明（由调用方取默认值）。
    pub fn ttl_sec(&self) -> Option<u64> {
        if self.cache_ttl_sec > 0 {
            Some(self.cache_ttl_sec as u64)
        } else {
            None
        }
    }
}

/// 索引条目的种类。未知值降级为 [`IndexEntryKind::Unknown`] 并保留原始字符串，
/// 保证"仓库先发字段、客户端后支持"的灰度期不会解析失败（文档 3.1 规则 4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexEntryKind {
    ModuleIndex,
    ModuleById,
    ModuleShard,
    McbeManifest,
    McbeMeta,
    /// 未知种类（保留原值）。
    Unknown(String),
}

impl IndexEntryKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::ModuleIndex => "module_index",
            Self::ModuleById => "module_by_id",
            Self::ModuleShard => "module_shard",
            Self::McbeManifest => "mcbe_manifest",
            Self::McbeMeta => "mcbe_meta",
            Self::Unknown(raw) => raw.as_str(),
        }
    }
}

impl Default for IndexEntryKind {
    fn default() -> Self {
        Self::Unknown(String::new())
    }
}

impl<'de> Deserialize<'de> for IndexEntryKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "module_index" => Self::ModuleIndex,
            "module_by_id" => Self::ModuleById,
            "module_shard" => Self::ModuleShard,
            "mcbe_manifest" => Self::McbeManifest,
            "mcbe_meta" => Self::McbeMeta,
            _ => Self::Unknown(raw),
        })
    }
}

// ---------------------------------------------------------------------------
// 分片与分片清单
// ---------------------------------------------------------------------------

/// 模块分片（文档 2.3.1）。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ModuleShard {
    #[serde(default)]
    pub schema_version: i64,
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub range: String,
    #[serde(default)]
    pub modules: Vec<ModuleEntry>,
}

/// 分片清单 `modules/_index.json`（文档 2.3.3）。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ShardIndex {
    #[serde(default)]
    pub schema_version: i64,
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub shards: Vec<ShardRef>,
}

impl ShardIndex {
    /// 按分片相对路径查找分片引用。
    pub fn shard_by_path(&self, path: &str) -> Option<&ShardRef> {
        self.shards.iter().find(|s| s.path == path)
    }

    /// 按 `range` 查找分片引用。
    pub fn shard_by_range(&self, range: &str) -> Option<&ShardRef> {
        self.shards.iter().find(|s| s.range == range)
    }
}

/// 分片清单中的一条分片记录。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ShardRef {
    #[serde(default)]
    pub range: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub size: i64,
    /// 分片内容 sha256；缺失为空串。
    #[serde(default)]
    pub sha256: String,
}

impl ShardRef {
    pub fn has_sha256(&self) -> bool {
        is_sha256_hex(&self.sha256)
    }
}

// ---------------------------------------------------------------------------
// 模块条目
// ---------------------------------------------------------------------------

/// 模块条目（文档 2.3.2 全字段表）。
///
/// 所有字段带 `serde(default)`：缺失字段降级为默认值，由业务层用
/// [`ModuleEntry::is_verifiable`] 等判定是否可安全使用，而不是在解析阶段整体失败。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ModuleEntry {
    /// 全局唯一 id（两段式 `author.module`），只用于身份识别，**不可**作 i18n 键前缀。
    #[serde(default)]
    pub id: String,
    /// 语言包命名空间（单段、无点号），框架文案键写作 `module.<i18n_namespace>.<flatKey>`。
    #[serde(default)]
    pub i18n_namespace: String,
    #[serde(default)]
    pub display_name: String,
    /// 本地化简介：locale -> 文本。
    #[serde(default)]
    pub summary: BTreeMap<String, String>,
    #[serde(default)]
    pub author: ModuleAuthor,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub channel: Channel,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub published_at: String,
    /// 声明支持的平台；未知平台值解析成功但不应参与匹配。
    #[serde(default)]
    pub platforms: Vec<Platform>,
    #[serde(default)]
    pub min_launcher: String,
    /// `null` 表示无上限。
    #[serde(default)]
    pub max_launcher: Option<String>,
    #[serde(default)]
    pub api_version: i64,
    #[serde(default)]
    pub icon: Option<ModuleIcon>,
    #[serde(default)]
    pub assets: Vec<ModuleAsset>,
    /// 权威权限形态：细粒度字符串枚举数组（可空）。
    #[serde(default)]
    pub permissions: Vec<String>,
    /// CI 派生的三布尔摘要（只读）。
    #[serde(default)]
    pub permissions_derived: PermissionsDerived,
    /// 本地化更新说明：locale -> 文本。
    #[serde(default)]
    pub changelog: BTreeMap<String, String>,
    #[serde(default)]
    pub status: ReviewStatus,
    #[serde(default)]
    pub yanked: bool,
    #[serde(default)]
    pub yank_reason: Option<String>,
}

impl ModuleEntry {
    /// 安全字段是否齐全：`id` 非空、`version` 非空且 **`assets` 内每条都有 sha256**
    /// （文档 2.5 规则 1 / 3.1 规则 3：缺摘要的条目必须被拒绝）。
    ///
    /// 注意：`assets` 为空数组的条目在本实现中视为"无可下载资产"，同样不可安装。
    pub fn is_verifiable(&self) -> bool {
        if self.id.trim().is_empty() || self.version.trim().is_empty() {
            return false;
        }
        if self.assets.is_empty() {
            return false;
        }
        self.assets.iter().all(|a| a.has_sha256())
    }

    /// 按 locale 折叠本地化文本：locale（精确 / 忽略大小写） → `en-US` → 调用方提供的兜底值。
    ///
    /// 与内核 `I18nService::catalog` 的 locale → en-US → 键名 回退链语义一致（文档 2.3.2）。
    pub fn localized<'a>(
        map: &'a BTreeMap<String, String>,
        locale: &str,
        fallback: &'a str,
    ) -> &'a str {
        lookup_locale(map, locale)
            .or_else(|| lookup_locale(map, FALLBACK_LOCALE))
            .unwrap_or(fallback)
    }

    /// 展示名：`display_name` 为空时回退 `id`（文档 2.9.4 回退链末段）。
    pub fn resolved_display_name(&self) -> String {
        let name = self.display_name.trim();
        if name.is_empty() {
            self.id.clone()
        } else {
            name.to_string()
        }
    }

    /// 按 locale 折叠的简介（回退 `en-US`，再回退空串，由前端决定是否显示 id）。
    pub fn resolved_summary(&self, locale: &str) -> String {
        Self::localized(&self.summary, locale, "").to_string()
    }

    /// 按 locale 折叠的更新说明（回退 `en-US`，再回退空串）。
    pub fn resolved_changelog(&self, locale: &str) -> String {
        Self::localized(&self.changelog, locale, "").to_string()
    }

    /// 条目是否可通过 [`crate::services::registry::model::Channel`] 过滤：
    /// `channel` 设置形如 `stable+beta` 时，`+` 分隔的集合只要命中其一即通过。
    pub fn matches_channel_filter(&self, filter: &str) -> bool {
        let wanted = filter.trim();
        if wanted.is_empty() {
            return true;
        }
        wanted.split('+').any(|c| {
            let c = c.trim();
            !c.is_empty() && self.channel.as_str().eq_ignore_ascii_case(c)
        })
    }

    /// 语言包命名空间是否合法（单段、`^[a-z][a-z0-9-]{2,31}$`）：
    /// 非法命名空间若直接注册给 i18n，会被按 `.` 切成多段而永远查不到（文档 2.3.6 第七节）。
    pub fn has_valid_i18n_namespace(&self) -> bool {
        is_valid_i18n_namespace(&self.i18n_namespace)
    }

    /// 按平台查找可下载资产（未知平台值不参与匹配）。
    pub fn asset_for(&self, platform: Platform) -> Option<&ModuleAsset> {
        self.assets.iter().find(|a| a.platform == platform)
    }

    /// 资产候选地址：主地址 + 镜像，按声明顺序。
    pub fn asset_urls(asset: &ModuleAsset) -> Vec<String> {
        let mut out = Vec::with_capacity(1 + asset.mirrors.len());
        if !asset.url.trim().is_empty() {
            out.push(asset.url.clone());
        }
        out.extend(asset.mirrors.iter().filter(|u| !u.trim().is_empty()).cloned());
        out
    }
}

/// 默认回退 locale（文档 2.3.2：缺失回退 `en-US`）。
pub const FALLBACK_LOCALE: &str = "en-US";

/// 在 locale map 中查找：先精确，再忽略大小写与 `_`/`-` 差异。
fn lookup_locale<'a>(map: &'a BTreeMap<String, String>, locale: &str) -> Option<&'a str> {
    let want = locale.trim();
    if want.is_empty() {
        return None;
    }
    if let Some(v) = map.get(want) {
        if !v.trim().is_empty() {
            return Some(v.as_str());
        }
    }
    let norm = normalize_locale(want);
    map.iter()
        .find(|(k, v)| normalize_locale(k) == norm && !v.trim().is_empty())
        .map(|(_, v)| v.as_str())
}

/// locale 归一化：小写并把 `_` 统一为 `-`（`zh_CN` 与 `zh-CN` 视为同一语言）。
pub fn normalize_locale(locale: &str) -> String {
    locale.trim().to_ascii_lowercase().replace('_', "-")
}

/// 模块作者信息。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ModuleAuthor {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub contact: String,
    /// 官方身份验证标记，由 cgl-libs 维护者置位。
    #[serde(default)]
    pub verified: bool,
}

/// 模块图标元信息。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ModuleIcon {
    /// 仓库内相对路径，必须在 `assets/icons/` 下。
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: i64,
}

impl ModuleIcon {
    pub fn has_sha256(&self) -> bool {
        is_sha256_hex(&self.sha256)
    }

    /// 图标路径是否落在 `assets/icons/` 白名单内（含路径穿越防护）。
    ///
    /// 图标路径来自远端元数据，若允许 `../` 或绝对路径会把文件写到缓存根之外。
    pub fn is_safe_repo_path(&self) -> bool {
        is_safe_relative_path(&self.path) && self.path.starts_with("assets/icons/")
    }
}

/// 单个平台的安装包资产。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ModuleAsset {
    #[serde(default)]
    pub platform: Platform,
    #[serde(default)]
    pub url: String,
    /// 备用镜像，可为空数组。
    #[serde(default)]
    pub mirrors: Vec<String>,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub sha256: String,
}

impl ModuleAsset {
    pub fn has_sha256(&self) -> bool {
        is_sha256_hex(&self.sha256)
    }
}

/// 权限派生摘要（CI 生成，只读）。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PermissionsDerived {
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub spawn_process: bool,
    #[serde(default)]
    pub write_outside_module_dir: bool,
}

// ---------------------------------------------------------------------------
// 枚举（全部带 Unknown 兜底）
// ---------------------------------------------------------------------------

/// 发布通道：`stable` / `beta` / `dev`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Stable,
    Beta,
    Dev,
    /// 未知通道（保留原值）：未知通道的条目一律不出现在默认列表里。
    Unknown(String),
}

impl Channel {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Dev => "dev",
            Self::Unknown(raw) => raw.as_str(),
        }
    }
}

impl Default for Channel {
    fn default() -> Self {
        // 缺省按最保守的 dev 处理：未声明通道的条目不会进 stable 列表。
        Self::Dev
    }
}

impl<'de> Deserialize<'de> for Channel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "stable" => Self::Stable,
            "beta" => Self::Beta,
            "dev" => Self::Dev,
            _ => Self::Unknown(raw),
        })
    }
}

/// 审核状态（只有 `approved` 会进 `index.json`，此处仍做容错）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Approved,
    Pending,
    Rejected,
    Unknown(String),
}

impl ReviewStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Approved => "approved",
            Self::Pending => "pending",
            Self::Rejected => "rejected",
            Self::Unknown(raw) => raw.as_str(),
        }
    }

    pub fn is_approved(&self) -> bool {
        matches!(self, Self::Approved)
    }
}

impl Default for ReviewStatus {
    fn default() -> Self {
        Self::Pending
    }
}

impl<'de> Deserialize<'de> for ReviewStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "approved" => Self::Approved,
            "pending" => Self::Pending,
            "rejected" => Self::Rejected,
            _ => Self::Unknown(raw),
        })
    }
}

/// 平台权威枚举（文档 2.3.6）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    WindowsX86_64,
    WindowsAarch64,
    AndroidArm64,
    LinuxX86_64,
    /// 未知平台（保留原值）：客户端跳过该资产而非报错（文档 3.1 规则 4）。
    Unknown(String),
}

impl Platform {
    pub fn as_str(&self) -> &str {
        match self {
            Self::WindowsX86_64 => "windows-x86_64",
            Self::WindowsAarch64 => "windows-aarch64",
            Self::AndroidArm64 => "android-arm64",
            Self::LinuxX86_64 => "linux-x86_64",
            Self::Unknown(raw) => raw.as_str(),
        }
    }

    /// 当前构建目标的平台；不匹配时返回 `None`（未知平台一律不匹配）。
    pub fn current() -> Option<Self> {
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            Some(Self::WindowsX86_64)
        } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
            Some(Self::WindowsAarch64)
        } else if cfg!(all(target_os = "android", target_arch = "aarch64")) {
            Some(Self::AndroidArm64)
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Some(Self::LinuxX86_64)
        } else {
            None
        }
    }
}

impl Default for Platform {
    fn default() -> Self {
        Self::Unknown(String::new())
    }
}

impl<'de> Deserialize<'de> for Platform {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "windows-x86_64" => Self::WindowsX86_64,
            "windows-aarch64" => Self::WindowsAarch64,
            "android-arm64" => Self::AndroidArm64,
            "linux-x86_64" => Self::LinuxX86_64,
            _ => Self::Unknown(raw),
        })
    }
}

// ---------------------------------------------------------------------------
// 纯函数校验工具
// ---------------------------------------------------------------------------

/// 是否为合法的 sha256 十六进制串（64 位、大小写均可）。
pub fn is_sha256_hex(value: &str) -> bool {
    let v = value.trim();
    v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 是否为合法的 i18n 命名空间：`^[a-z][a-z0-9-]{2,31}$`（单段、总长 3~32、无点号）。
pub fn is_valid_i18n_namespace(ns: &str) -> bool {
    let bytes = ns.as_bytes();
    if bytes.len() < 3 || bytes.len() > 32 {
        return false;
    }
    if !bytes[0].is_ascii_lowercase() {
        return false;
    }
    // 末位不得为连字符（避免 `a-` 这类歧义前缀）。
    if bytes[bytes.len() - 1] == b'-' {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// 是否为安全的仓库内相对路径：非空、无 `\`、无 `..` 段、非绝对路径、无 Windows 盘符。
///
/// 元数据里的路径会被拼接进本地缓存目录，必须拒绝穿越写法（文档 3.2 本地提权风险）。
pub fn is_safe_relative_path(path: &str) -> bool {
    let p = path.trim();
    if p.is_empty() || p.contains('\\') {
        return false;
    }
    if p.starts_with('/') || p.starts_with("//") {
        return false;
    }
    // Windows 盘符（`C:` 形式）与 UNC。
    let bytes = p.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return false;
    }
    !p.split('/').any(|seg| seg == ".." || seg == ".")
}

/// 模块 id 是否符合两段式规范 `^[a-z0-9]+(-[a-z0-9]+)*(\.[a-z0-9]+(-[a-z0-9]+)*)+$`。
///
/// 仅作判定，不用于拒绝整份元数据：不合规的条目由业务层跳过并计入问题列表。
pub fn is_valid_module_id(id: &str) -> bool {
    let mut segments = id.split('.');
    let Some(first) = segments.next() else {
        return false;
    };
    if !is_valid_id_segment(first) {
        return false;
    }
    let mut rest = 0usize;
    for seg in segments {
        if !is_valid_id_segment(seg) {
            return false;
        }
        rest += 1;
    }
    rest >= 1
}

/// 单段 id：`[a-z0-9]+(-[a-z0-9]+)*`。
fn is_valid_id_segment(seg: &str) -> bool {
    if seg.is_empty() {
        return false;
    }
    seg.split('-').all(|part| {
        !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// 解析 semver（容忍常见前缀 `v`）；非法返回 `None`。
///
/// `max_launcher` 为 `null` 时无上限，不需要解析。
pub fn parse_semver(raw: &str) -> Option<semver::Version> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    semver::Version::parse(s)
        .or_else(|_| semver::Version::parse(s.trim_start_matches(['v', 'V'])))
        .ok()
}

/// 版本区间是否合法：两端都可解析且 `min <= max`（文档 2.8.2 第 8 条）。
///
/// `max` 为 `None` 表示无上限。
pub fn is_valid_launcher_range(min: &str, max: Option<&str>) -> bool {
    let Some(min_v) = parse_semver(min) else {
        return false;
    };
    match max {
        None => true,
        Some(m) => match parse_semver(m) {
            Some(max_v) => min_v <= max_v,
            None => false,
        },
    }
}

/// 当前启动器版本是否满足 [min, max] 区间。区间本身非法时判定为不满足（保守拒绝）。
pub fn launcher_range_covers(
    current: &str,
    min: &str,
    max: Option<&str>,
) -> Option<bool> {
    let current = parse_semver(current)?;
    if !is_valid_launcher_range(min, max) {
        return Some(false);
    }
    let min_v = parse_semver(min)?;
    if current < min_v {
        return Some(false);
    }
    match max.and_then(parse_semver) {
        Some(max_v) if current > max_v => Some(false),
        _ => Some(true),
    }
}

/// `generated_at` 解析为可比较的 `NaiveDateTime`（RFC3339，容忍小数秒与时区偏移）。
pub fn parse_generated_at(raw: &str) -> Option<chrono_lite::DateTime> {
    chrono_lite::DateTime::parse_rfc3339(raw)
}

/// 防降级判定结果（文档 2.5 规则 3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackVerdict {
    /// 首次见到该数据，接受并记录锚点。
    FirstSeen,
    /// 不早于本地锚点，或回退但 commit 相同（同一次 CI 重建），接受。
    Accept,
    /// `generated_at` 早于锚点且 `repo_commit` 不同 → 拒绝加载，保留本地缓存。
    Rollback,
}

/// 判定拉取到的索引是否构成回退。
///
/// 规则（文档 2.5 规则 3）：拉到的 `generated_at` **早于**本地已记录的最高值，
/// 且 `repo_commit` 不同 → 拒绝；`generated_at` 无法解析时保守拒绝（不升级锚点）。
pub fn compare_rollback(
    highest_generated_at: Option<&str>,
    highest_repo_commit: Option<&str>,
    incoming_generated_at: &str,
    incoming_repo_commit: &str,
) -> RollbackVerdict {
    let Some(anchor_raw) = highest_generated_at else {
        return RollbackVerdict::FirstSeen;
    };
    let Some(anchor) = parse_generated_at(anchor_raw) else {
        // 锚点脏数据：不做回退判定，交由调用方按首次见处理。
        return RollbackVerdict::FirstSeen;
    };
    let Some(incoming) = parse_generated_at(incoming_generated_at) else {
        // 新数据时间不可解析：无法证明其不比锚点旧，按回退拒绝。
        return RollbackVerdict::Rollback;
    };
    if incoming >= anchor {
        return RollbackVerdict::Accept;
    }
    let same_commit = highest_repo_commit
        .map(|c| !c.is_empty() && c == incoming_repo_commit.trim())
        .unwrap_or(false);
    if same_commit {
        RollbackVerdict::Accept
    } else {
        RollbackVerdict::Rollback
    }
}

/// 客户端版本是否满足索引声明的 `min_launcher`（文档 2.5 规则 5）。
///
/// 索引未声明或不可解析时返回 `true`：不因旧格式索引阻塞列表展示，
/// 但语义上"索引要求升级启动器"必须由本函数如实返回 `false` 供前端提示。
pub fn index_min_launcher_satisfied(current: &str, min_launcher: &str) -> bool {
    match parse_semver(min_launcher) {
        Some(required) => match parse_semver(current) {
            Some(cur) => cur >= required,
            None => false,
        },
        None => true,
    }
}

/// 极简 RFC3339 解析（避免为单点需求引入时间库；只服务于"时间先后比较"）。
///
/// 支持 `YYYY-MM-DDTHH:MM:SS[.fff][Z|±HH:MM]`，返回以 UTC 秒为基准的整数时间戳。
/// 仅在无法解析时返回 `None`，不会把畸形数据当成"新"或"旧"。
pub mod chrono_lite {
    use serde::Serialize;

    /// 解析后的时间点：以 UTC 秒为基准，保留来源时区偏移用于同秒比较。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
    pub struct DateTime {
        /// UTC 秒级时间戳。
        pub epoch_secs: i64,
        /// 亚秒部分（纳秒），用于同一秒内排序。
        pub nanos: u32,
    }

    impl DateTime {
        /// 解析 RFC3339 时间串。
        pub fn parse_rfc3339(raw: &str) -> Option<Self> {
            let s = raw.trim();
            let bytes = s.as_bytes();
            if bytes.len() < 20 {
                return None;
            }
            // 必须包含日期与时间的分隔符 'T'（或小写 't'）。
            if bytes[10] != b'T' && bytes[10] != b't' {
                return None;
            }
            let year: i64 = s.get(0..4)?.parse().ok()?;
            let month: u32 = s.get(5..7)?.parse().ok()?;
            let day: u32 = s.get(8..10)?.parse().ok()?;
            let hour: i64 = s.get(11..13)?.parse().ok()?;
            let minute: i64 = s.get(14..16)?.parse().ok()?;
            let second: i64 = s.get(17..19)?.parse().ok()?;
            if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
                return None;
            }
            if hour > 23 || minute > 59 || second > 60 {
                return None;
            }

            let mut rest = &s[19..];
            let mut nanos = 0u32;
            if let Some(stripped) = rest.strip_prefix('.') {
                let digits: String = stripped.chars().take_while(|c| c.is_ascii_digit()).collect();
                if digits.is_empty() {
                    return None;
                }
                let mut frac = digits.clone();
                frac.truncate(9);
                while frac.len() < 9 {
                    frac.push('0');
                }
                nanos = frac.parse().ok()?;
                rest = &stripped[digits.len()..];
            }

            let offset_secs: i64 = match rest {
                "" | "Z" | "z" => 0,
                _ => {
                    let sign = match rest.as_bytes().first()? {
                        b'+' => 1,
                        b'-' => -1,
                        _ => return None,
                    };
                    let body = &rest[1..];
                    let (h, m) = match body.split_once(':') {
                        Some((h, m)) => (h, m),
                        None => return None,
                    };
                    let hh: i64 = h.parse().ok()?;
                    let mm: i64 = m.parse().ok()?;
                    if hh > 23 || mm > 59 {
                        return None;
                    }
                    sign * (hh * 3600 + mm * 60)
                }
            };

            let days = days_from_civil(year, month as i64, day as i64);
            let epoch_secs =
                days * 86400 + hour * 3600 + minute * 60 + second - offset_secs;
            Some(Self { epoch_secs, nanos })
        }
    }

    /// Howard Hinnant 的 `days_from_civil`：公历日期 → 1970-01-01 起的天数。
    fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX_JSON: &str = r#"{
      "schema_version": 1,
      "generated_at": "2026-09-14T03:12:40Z",
      "repo_commit": "7f3c1ab9e4d2b6c8a0f5e1d3c7b9a2f4e6d8c0b1",
      "min_launcher": "0.1.0",
      "entries": [
        {
          "kind": "module_index",
          "path": "modules/_index.json",
          "sha256": "9c1f0b7a3d5e82461bc0f9a7d2e34c58b6109fd7a2c4e83b5d9016fa7c2be405",
          "size": 1462,
          "cache_ttl_sec": 3600,
          "urls": [
            "https://cdn.jsdelivr.net/gh/copper-lamp/cgl-libs@main/modules/_index.json",
            "https://gh-proxy.com/https://raw.githubusercontent.com/copper-lamp/cgl-libs/refs/heads/main/modules/_index.json",
            "https://raw.githubusercontent.com/copper-lamp/cgl-libs/refs/heads/main/modules/_index.json"
          ]
        },
        {
          "kind": "module_shard",
          "path": "modules/s-u.json",
          "sha256": "5a2c8e014f6b93d7c05e28a1f4b6d8e0c2a4f6b8d0e2c4a6f8b0d2e4c6a8f0b2",
          "size": 53421,
          "cache_ttl_sec": 3600,
          "urls": ["https://raw.githubusercontent.com/copper-lamp/cgl-libs/refs/heads/main/modules/s-u.json"]
        }
      ]
    }"#;

    const SHARD_JSON: &str = r#"{
      "schema_version": 1,
      "generated_at": "2026-09-14T03:12:40Z",
      "range": "s-u",
      "modules": [
        {
          "id": "copper-lamp.server-manager",
          "i18n_namespace": "server-manager",
          "display_name": "服务端管理器",
          "summary": {
            "zh-CN": "在启动器内创建与管理 MCBE 服务端实例。",
            "en-US": "Create and manage MCBE server instances inside the launcher."
          },
          "author": { "name": "copper-lamp", "contact": "https://github.com/copper-lamp", "verified": true },
          "repo": "https://github.com/copper-lamp/copper-server-manager",
          "license": "MIT",
          "channel": "stable",
          "version": "0.4.2",
          "published_at": "2026-09-11T09:20:00Z",
          "platforms": ["windows-x86_64"],
          "min_launcher": "0.1.0",
          "max_launcher": null,
          "api_version": 1,
          "icon": {
            "path": "assets/icons/copper-lamp.server-manager.png",
            "sha256": "1f8b3c5d7e90a2b4c6d8e0f2a4b6c8d0e2f4a6b8c0d2e4f6a8b0c2d4e6f80a1b",
            "size": 4211
          },
          "assets": [
            {
              "platform": "windows-x86_64",
              "url": "https://github.com/copper-lamp/copper-server-manager/releases/download/v0.4.2/a.cglm",
              "mirrors": ["https://gh-proxy.com/https://github.com/copper-lamp/copper-server-manager/releases/download/v0.4.2/a.cglm"],
              "size": 3187456,
              "sha256": "d4e6f80a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6"
            }
          ],
          "permissions": ["network", "process:spawn", "download:enqueue"],
          "permissions_derived": { "network": true, "spawn_process": true, "write_outside_module_dir": false },
          "changelog": { "zh-CN": "新增批量启停。", "en-US": "Add batch start/stop." },
          "status": "approved",
          "yanked": false,
          "yank_reason": null
        }
      ]
    }"#;

    #[test]
    fn index_json_real_structure_parses() {
        let idx: RegistryIndex = serde_json::from_str(INDEX_JSON).expect("索引应能解析");
        assert_eq!(idx.schema_version, 1);
        assert_eq!(idx.generated_at, "2026-09-14T03:12:40Z");
        assert_eq!(idx.repo_commit, "7f3c1ab9e4d2b6c8a0f5e1d3c7b9a2f4e6d8c0b1");
        assert_eq!(idx.min_launcher, "0.1.0");
        assert_eq!(idx.entries.len(), 2);

        let shard = idx.path_of_kind(IndexEntryKind::ModuleShard).expect("应有分片条目");
        assert_eq!(shard.path, "modules/s-u.json");
        assert_eq!(shard.size, 53421);
        assert_eq!(shard.ttl_sec(), Some(3600));
        assert!(shard.has_sha256());
        assert_eq!(shard.urls.len(), 1);
        assert!(shard.urls[0].starts_with("https://"));

        let module_index = idx
            .entry_by_path("modules/_index.json")
            .expect("按路径应能定位条目");
        assert_eq!(module_index.kind, IndexEntryKind::ModuleIndex);
    }

    #[test]
    fn unknown_fields_and_enums_are_tolerated() {
        // 未知字段（顶层 / 条目 / 分片 / 模块条目）不得导致反序列化失败。
        let raw = r#"{
          "schema_version": 1,
          "generated_at": "2026-09-14T03:12:40Z",
          "repo_commit": "abc",
          "min_launcher": "0.1.0",
          "future_top_level": { "a": 1 },
          "entries": [
            {
              "kind": "future_kind_v2",
              "path": "modules/x-y.json",
              "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
              "size": 10,
              "cache_ttl_sec": 60,
              "urls": ["https://example.com/x-y.json"],
              "future_entry_field": [1, 2, 3]
            }
          ]
        }"#;
        let idx: RegistryIndex = serde_json::from_str(raw).expect("未知字段必须被忽略");
        assert_eq!(idx.entries.len(), 1);
        assert_eq!(
            idx.entries[0].kind,
            IndexEntryKind::Unknown("future_kind_v2".to_string())
        );
        assert_eq!(idx.entries[0].kind.as_str(), "future_kind_v2");
        assert!(idx.path_of_kind(IndexEntryKind::ModuleShard).is_none());

        // 未知 platform / channel / status：解析成功但保持未知语义。
        let raw_entry = r#"{
          "id": "example.mod",
          "i18n_namespace": "example-mod",
          "display_name": "Example",
          "summary": { "en-US": "x" },
          "author": { "name": "a", "contact": "b", "verified": true },
          "repo": "https://example.com",
          "license": "MIT",
          "channel": "canary",
          "version": "1.0.0",
          "published_at": "2026-09-14T00:00:00Z",
          "platforms": ["windows-x86_64", "linux-arm64"],
          "min_launcher": "0.1.0",
          "max_launcher": null,
          "api_version": 1,
          "assets": [
            { "platform": "linux-arm64", "url": "https://example.com/a", "mirrors": [], "size": 1,
              "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" }
          ],
          "permissions": [],
          "changelog": { "en-US": "y" },
          "status": "quarantined",
          "yanked": false,
          "yank_reason": null,
          "brand_new_field": 42
        }"#;
        let entry: ModuleEntry = serde_json::from_str(raw_entry).expect("未知枚举值应能解析");
        assert_eq!(entry.channel, Channel::Unknown("canary".to_string()));
        assert_eq!(
            entry.platforms[1],
            Platform::Unknown("linux-arm64".to_string())
        );
        assert_eq!(entry.status, ReviewStatus::Unknown("quarantined".to_string()));
        assert_eq!(
            entry.assets[0].platform,
            Platform::Unknown("linux-arm64".to_string())
        );
        // 未知平台不应匹配当前平台查询。
        assert!(entry.asset_for(Platform::WindowsX86_64).is_none());
        assert_eq!(entry.assets[0].platform.as_str(), "linux-arm64");
    }

    #[test]
    fn schema_version_above_supported_is_rejected() {
        let raw = INDEX_JSON.replace("\"schema_version\": 1", "\"schema_version\": 2");
        let idx: RegistryIndex = serde_json::from_str(&raw).expect("解析本身应成功");
        assert!(idx.schema_version > SUPPORTED_SCHEMA_VERSION);
        // 解析成功但业务层必须硬拒绝（不做尽力解析，文档 2.5 规则 4）。
        assert!(!super::super::schema_supported(idx.schema_version));
        // 同版本与更低版本允许（低版本为旧格式，按旧规则解析）。
        assert!(super::super::schema_supported(1));
        assert!(super::super::schema_supported(0));
    }

    #[test]
    fn sha256_shape_check_rejects_missing_and_short_digest() {
        assert!(is_sha256_hex(
            "d4e6f80a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6"
        ));
        assert!(is_sha256_hex("D4E6F80A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4E5F60718293A4B5C6"));
        assert!(!is_sha256_hex(""));
        assert!(!is_sha256_hex("d4e6f8"));
        assert!(!is_sha256_hex(&"z".repeat(64)));
    }

    #[test]
    fn entry_without_asset_digest_is_not_verifiable() {
        let mut entry: ModuleEntry = serde_json::from_str(SHARD_JSON)
            .map(|s: ModuleShard| s.modules[0].clone())
            .expect("条目应能解析");
        assert!(entry.is_verifiable());
        // 剥掉资产摘要 → 必须判定为不可校验（缺摘要即拒绝）。
        entry.assets[0].sha256 = String::new();
        assert!(!entry.is_verifiable());
        // 完全没有资产 → 同样不可安装。
        entry.assets.clear();
        assert!(!entry.is_verifiable());
        // id 缺失 → 拒绝。
        entry.assets.push(ModuleAsset {
            platform: Platform::WindowsX86_64,
            sha256: "c".repeat(64),
            ..Default::default()
        });
        entry.id = String::new();
        assert!(!entry.is_verifiable());
    }

    #[test]
    fn locale_fallback_chain_prefers_locale_then_en_us() {
        let mut map = BTreeMap::new();
        map.insert("zh-CN".to_string(), "中文".to_string());
        map.insert("en-US".to_string(), "English".to_string());

        assert_eq!(ModuleEntry::localized(&map, "zh-CN", "id"), "中文");
        assert_eq!(ModuleEntry::localized(&map, "zh_CN", "id"), "中文");
        assert_eq!(ModuleEntry::localized(&map, "zh-cn", "id"), "中文");
        // 缺失 locale → 回退 en-US。
        assert_eq!(ModuleEntry::localized(&map, "ja-JP", "id"), "English");
        assert_eq!(ModuleEntry::localized(&map, "", "id"), "English");

        // 只有 zh-CN 时，非中文 locale 回退到调用方兜底（最终回退 id）。
        let mut only_zh = BTreeMap::new();
        only_zh.insert("zh-CN".to_string(), "中文".to_string());
        assert_eq!(ModuleEntry::localized(&only_zh, "ja-JP", "example.mod"), "example.mod");

        // 空串值视为缺失，不参与回退链。
        let mut blank = BTreeMap::new();
        blank.insert("zh-CN".to_string(), "   ".to_string());
        blank.insert("en-US".to_string(), "English".to_string());
        assert_eq!(ModuleEntry::localized(&blank, "zh-CN", "id"), "English");
    }

    #[test]
    fn channel_filter_selects_configured_channels_only() {
        let mut entry: ModuleEntry = serde_json::from_str(SHARD_JSON)
            .map(|s: ModuleShard| s.modules[0].clone())
            .expect("条目应能解析");
        assert_eq!(entry.channel, Channel::Stable);

        assert!(entry.matches_channel_filter("stable"));
        assert!(entry.matches_channel_filter("stable+beta"));
        assert!(!entry.matches_channel_filter("beta"));
        assert!(!entry.matches_channel_filter("dev"));
        // 空过滤器视为不过滤。
        assert!(entry.matches_channel_filter(""));

        entry.channel = Channel::Beta;
        assert!(entry.matches_channel_filter("beta"));
        assert!(!entry.matches_channel_filter("stable"));

        // 未知通道不会被默认 stable 过滤命中。
        entry.channel = Channel::Unknown("canary".to_string());
        assert!(!entry.matches_channel_filter("stable"));
        assert!(entry.matches_channel_filter("canary"));
    }

    #[test]
    fn rollback_detection_rejects_older_generated_at_with_other_commit() {
        let anchor_at = "2026-09-14T03:12:40Z";
        let anchor_commit = "7f3c1ab9e4d2b6c8a0f5e1d3c7b9a2f4e6d8c0b1";

        // 首次：接受并记录锚点。
        assert_eq!(
            compare_rollback(None, None, anchor_at, anchor_commit),
            RollbackVerdict::FirstSeen
        );
        // 时间前进：接受。
        assert_eq!(
            compare_rollback(
                Some(anchor_at),
                Some(anchor_commit),
                "2026-09-14T04:00:00Z",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            RollbackVerdict::Accept
        );
        // 同一时刻、不同 commit（幂等重建）：接受。
        assert_eq!(
            compare_rollback(
                Some(anchor_at),
                Some(anchor_commit),
                anchor_at,
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            ),
            RollbackVerdict::Accept
        );
        // 时间回退 + commit 不同：拒绝（重放旧索引的攻击特征）。
        assert_eq!(
            compare_rollback(
                Some(anchor_at),
                Some(anchor_commit),
                "2026-09-01T00:00:00Z",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            ),
            RollbackVerdict::Rollback
        );
        // 时间回退但 commit 相同：同一次生成的等价副本，接受。
        assert_eq!(
            compare_rollback(
                Some(anchor_at),
                Some(anchor_commit),
                "2026-09-01T00:00:00Z",
                anchor_commit
            ),
            RollbackVerdict::Accept
        );
        // 新数据时间不可解析：保守拒绝。
        assert_eq!(
            compare_rollback(
                Some(anchor_at),
                Some(anchor_commit),
                "not-a-timestamp",
                "cccccccccccccccccccccccccccccccccccccccc"
            ),
            RollbackVerdict::Rollback
        );
    }

    #[test]
    fn generated_at_parsing_handles_offsets_and_fractional_seconds() {
        let utc = parse_generated_at("2026-09-14T03:12:40Z").expect("应可解析 UTC");
        let offset = parse_generated_at("2026-09-14T11:12:40+08:00").expect("应可解析带偏移时间");
        assert_eq!(utc.epoch_secs, offset.epoch_secs);

        let frac = parse_generated_at("2026-09-14T03:12:40.500Z").expect("应可解析小数秒");
        assert_eq!(frac.epoch_secs, utc.epoch_secs);
        assert!(frac > utc);

        assert!(parse_generated_at("2026-09-14 03:12:40").is_none());
        assert!(parse_generated_at("").is_none());
        assert!(parse_generated_at("2026-13-45T99:99:99Z").is_none());
    }

    #[test]
    fn semver_range_and_launcher_gate_are_enforced() {
        assert!(is_valid_launcher_range("0.1.0", None));
        assert!(is_valid_launcher_range("0.1.0", Some("0.4.0")));
        assert!(is_valid_launcher_range("0.1.0", Some("0.1.0")));
        assert!(!is_valid_launcher_range("0.4.0", Some("0.1.0")), "语义倒挂必须非法");
        assert!(!is_valid_launcher_range("not-semver", None));
        assert!(is_valid_launcher_range("v0.1.0", Some("v0.2.0")), "容忍 v 前缀");

        assert_eq!(launcher_range_covers("0.2.0", "0.1.0", Some("0.3.0")), Some(true));
        assert_eq!(launcher_range_covers("0.0.9", "0.1.0", None), Some(false));
        assert_eq!(launcher_range_covers("0.4.0", "0.1.0", Some("0.3.0")), Some(false));
        assert_eq!(launcher_range_covers("0.2.0", "0.4.0", Some("0.1.0")), Some(false));

        assert!(index_min_launcher_satisfied("0.1.0", "0.1.0"));
        assert!(!index_min_launcher_satisfied("0.1.0", "0.2.0"));
        assert!(index_min_launcher_satisfied("0.1.0", ""), "未声明视为满足");
    }

    #[test]
    fn module_id_and_i18n_namespace_rules_match_contract() {
        assert!(is_valid_module_id("copper-lamp.server-manager"));
        assert!(is_valid_module_id("a.b"));
        assert!(is_valid_module_id("thirdparty.world-editor"));
        assert!(!is_valid_module_id("server-manager"), "必须两段式");
        assert!(!is_valid_module_id("Copper-Lamp.Server"), "必须小写");
        assert!(!is_valid_module_id("copper..lamp"));
        assert!(!is_valid_module_id(".lamp"));
        assert!(!is_valid_module_id("copper-lamp."));

        assert!(is_valid_i18n_namespace("server-manager"));
        assert!(is_valid_i18n_namespace("abc"));
        assert!(!is_valid_i18n_namespace("ab"), "至少 3 字符");
        assert!(!is_valid_i18n_namespace("copper-lamp.server-manager"), "不得含点号");
        assert!(!is_valid_i18n_namespace("Server"));
        assert!(!is_valid_i18n_namespace("server-"), "末位不得为连字符");
        assert!(!is_valid_i18n_namespace(&"a".repeat(33)), "最长 32");
    }

    #[test]
    fn repo_relative_path_guard_blocks_traversal() {
        assert!(is_safe_relative_path("modules/s-u.json"));
        assert!(is_safe_relative_path("assets/icons/a.png"));
        assert!(!is_safe_relative_path("../secrets.json"));
        assert!(!is_safe_relative_path("modules/../../x.json"));
        assert!(!is_safe_relative_path("/etc/passwd"));
        assert!(!is_safe_relative_path("C:/windows/system32"));
        assert!(!is_safe_relative_path("modules\\s-u.json"));
        assert!(!is_safe_relative_path(""));

        let icon = ModuleIcon {
            path: "assets/icons/x.png".into(),
            sha256: "a".repeat(64),
            size: 10,
        };
        assert!(icon.is_safe_repo_path());
        let evil = ModuleIcon {
            path: "../evil.png".into(),
            ..icon.clone()
        };
        assert!(!evil.is_safe_repo_path());
        let outside = ModuleIcon {
            path: "modules/x.png".into(),
            ..icon
        };
        assert!(!outside.is_safe_repo_path(), "图标必须位于 assets/icons/ 下");
    }

    #[test]
    fn entry_localized_display_and_asset_lookup() {
        let shard: ModuleShard = serde_json::from_str(SHARD_JSON).expect("分片应能解析");
        assert_eq!(shard.schema_version, 1);
        assert_eq!(shard.range, "s-u");
        assert_eq!(shard.modules.len(), 1);

        let entry = &shard.modules[0];
        assert_eq!(entry.id, "copper-lamp.server-manager");
        assert_eq!(entry.i18n_namespace, "server-manager");
        assert!(entry.has_valid_i18n_namespace());
        assert_eq!(entry.resolved_display_name(), "服务端管理器");
        assert_eq!(
            entry.resolved_summary("zh-CN"),
            "在启动器内创建与管理 MCBE 服务端实例。"
        );
        assert_eq!(
            entry.resolved_summary("ja-JP"),
            "Create and manage MCBE server instances inside the launcher."
        );
        assert_eq!(entry.resolved_changelog("ja-JP"), "Add batch start/stop.");
        assert_eq!(entry.author.name, "copper-lamp");
        assert!(entry.author.verified);
        assert!(entry.status.is_approved());
        assert!(!entry.yanked);
        assert!(entry.yank_reason.is_none());
        assert!(entry.permissions_derived.network);
        assert!(entry.permissions_derived.spawn_process);
        assert!(!entry.permissions_derived.write_outside_module_dir);
        assert_eq!(entry.icon.as_ref().map(|i| i.size), Some(4211));

        let asset = entry
            .asset_for(Platform::WindowsX86_64)
            .expect("应能找到当前平台资产");
        assert_eq!(asset.size, 3187456);
        assert!(asset.has_sha256());
        let urls = ModuleEntry::asset_urls(asset);
        assert_eq!(urls.len(), 2, "主地址 + 镜像");
        assert!(urls[0].contains("releases/download"));
        assert!(urls[1].starts_with("https://gh-proxy.com/"));

        // 空 display_name 回退 id。
        let mut fallback = entry.clone();
        fallback.display_name = "  ".into();
        assert_eq!(fallback.resolved_display_name(), "copper-lamp.server-manager");
    }

    #[test]
    fn shard_index_parses_and_locates_shards() {
        let raw = r#"{
          "schema_version": 1,
          "generated_at": "2026-09-14T03:12:40Z",
          "total": 2,
          "shards": [
            { "range": "a-b", "path": "modules/a-b.json", "count": 1, "size": 1200,
              "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" },
            { "range": "s-u", "path": "modules/s-u.json", "count": 1, "size": 53421,
              "sha256": "5a2c8e014f6b93d7c05e28a1f4b6d8e0c2a4f6b8d0e2c4a6f8b0d2e4c6a8f0b2" }
          ],
          "unknown_future_field": true
        }"#;
        let idx: ShardIndex = serde_json::from_str(raw).expect("_index.json 应能解析");
        assert_eq!(idx.total, 2);
        assert_eq!(idx.shards.len(), 2);
        let shard = idx.shard_by_range("s-u").expect("应按 range 定位");
        assert_eq!(shard.path, "modules/s-u.json");
        assert_eq!(shard.count, 1);
        assert!(shard.has_sha256());
        assert!(idx.shard_by_path("modules/a-b.json").is_some());
        assert!(idx.shard_by_path("modules/v-z.json").is_none());

        // 缺 sha256 的分片引用必须被判为不可校验。
        let broken: ShardIndex = serde_json::from_str(
            r#"{"schema_version":1,"generated_at":"x","total":1,
                "shards":[{"range":"a-b","path":"modules/a-b.json","count":1,"size":1}]}"#,
        )
        .expect("缺字段应能解析");
        assert!(!broken.shards[0].has_sha256());
    }
}
