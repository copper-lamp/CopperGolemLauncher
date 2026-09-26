//! 元数据客户端：`cgl-libs` 索引 / 分片的拉取、三级校验、缓存与降级。
//!
//! 这是 [cgl-libs](../../../docs/cgl-libs.md) 2.9 在内核侧的落地实现，链路为：
//!
//! ```text
//! index.json       → 拉取（索引层固定三镜像）→ sha256 校验（.sha256 摘要 + 多镜像共识）→ 缓存
//!   → modules/_index.json → 校验 sha256 → 得到分片清单
//!   → 按需拉取分片        → 校验 sha256 → 合并进本地缓存
//!   → 与 ModuleRegistry::list() 的本地模块按 id 关联（由前端完成）
//! ```
//!
//! 硬性约束（逐条对应文档条目）：
//!
//! | 约束 | 出处 |
//! |---|---|
//! | HTTP 必须走 `services::http_client::client_builder` | 2.9.3 |
//! | 索引/分片/资产三级 sha256，缺摘要即拒绝 | 2.5 规则 1 |
//! | 防降级：`generated_at` 回退且 `repo_commit` 不同 → 拒绝并保留缓存 | 2.5 规则 3 |
//! | `schema_version` 高于支持值 → 硬拒绝，不尽力解析 | 2.5 规则 4 / 3.1 |
//! | 索引层固定镜像，不走用户配置的第三方镜像 | 2.6 |
//! | 镜像连续失败 3 次 → 本次会话排到最后 | 2.6 |
//! | `trust/anchor.json` 永不因缓存清理删除 | 2.9.2 |
//! | 降级必须在状态里可见，不得"降级到看起来正常" | 2.9.6 |
//!
//! 当前**未**实现（诚实记录，见文档 3.6 / 3.5）：
//! - 安装包与图标下载（G4）：属于安装链路，需接入全局 `DownloadService`，不在本任务范围，
//!   本模块只负责元数据侧的校验与判定（[`RegistryService::verify_asset_bytes`] 已就绪，供后续接入）。
//!
//! minisign 验签已实现并对接：公钥经 `TRUSTED_PUBLIC_KEY` 内置（见 [`signature`] 模块），
//! 验签状态如实暴露（`verified` / `failed` / `signature_missing` / `not_configured`）。

pub mod anchor;
pub mod digest;
pub mod mirror;
pub mod model;
pub mod paths;
pub mod signature;
pub mod state;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;

use crate::error::KernelError;
use crate::registry::manifest::SUPPORTED_API_VERSIONS;
use crate::services::paths::Paths;
use crate::services::settings::SettingsService;

use anchor::TrustAnchor;
use digest::DigestCheck;
use mirror::{MirrorPreference, MirrorStat};
use model::{
    is_safe_relative_path, IndexEntryKind, ModuleEntry, ModuleShard, RegistryIndex,
    ShardIndex, SUPPORTED_SCHEMA_VERSION,
};
use paths::RegistryPaths;
use signature::{SignatureStatus, SignatureVerifier};
use state::{IndexFault, IndexMeta, IndexSource};

/// 索引与分片的默认 TTL（秒）。仅在索引条目未声明 `cache_ttl_sec` 时使用。
pub const DEFAULT_TTL_SEC: u64 = 3600;

/// 元数据请求超时（轻量 GET，几 KB~几百 KB，不阻塞首屏）。
pub const META_REQUEST_TIMEOUT_SEC: u64 = 15;

/// 同时保留的分片缓存上限（防磁盘无限增长，超出后按 LRU 清理）。
pub const SHARD_CACHE_QUOTA_BYTES: u64 = 64 * 1024 * 1024;

/// 元数据是否可用（供前端决定渲染策略）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryAvailability {
    /// 有可用索引（含 TTL 内缓存与 304）。
    Ready,
    /// 只能使用 last-known-good 或本地缓存副本：数据可能过期，UI 必须提示。
    Stale,
    /// 完全不可用（无缓存且全部失败）。
    Unavailable,
}

/// 给前端的元数据状态快照（`registry_status` 命令返回）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RegistryStatusView {
    pub availability: RegistryAvailability,
    /// 是否处于降级（使用了陈旧数据）。
    pub degraded: bool,
    /// 数据来源（network / cache / not_modified / last_known_good / cache_fallback）。
    pub source: IndexSource,
    /// 索引声明的生成时间（RFC3339），无数据时为 null。
    pub generated_at: Option<String>,
    /// 本地已见过的最高 `generated_at`（防降级锚点）。
    pub highest_generated_at: Option<String>,
    /// 锚点记录的 repo_commit。
    pub repo_commit: Option<String>,
    /// 索引 schema_version（无数据时为 0）。
    pub schema_version: i64,
    /// 客户端支持的 schema_version。
    pub supported_schema_version: i64,
    /// 本地副本最后确认有效的时间（Unix 秒，0 表示未知）。
    pub fetched_at: i64,
    /// 距上次确认已过秒数。
    pub age_secs: Option<i64>,
    /// 索引声明的 `min_launcher`。
    pub min_launcher: Option<String>,
    /// 当前启动器版本是否满足 `min_launcher`（false 时前端应提示"需要升级启动器"）。
    pub launcher_satisfied: bool,
    /// **签名是否已验证**。当前无内置公钥，恒为 false；不得由前端假定为 true。
    pub signature_verified: bool,
    /// 签名能力状态（not_configured / signature_missing / verified / failed）。
    pub signature_status: SignatureStatus,
    /// 签名状态文案键（前端 i18n）。
    pub signature_note_key: String,
    /// 是否正在使用 last-known-good 快照。
    pub using_last_known_good: bool,
    /// 当前生效的通道过滤（`registry.channel`）。
    pub channel_filter: String,
    /// 当前生效的资产层镜像偏好（`download.mirror`）。注意索引层固定走内置三段镜像。
    pub mirror_preference: String,
    /// 本次会话内被降权（连续失败 >= 3）的镜像地址。
    pub demoted_mirrors: Vec<String>,
    /// 已知的远端模块条目总数（来自 `_index.json` 的 `total`；未知为 0）。
    pub remote_module_count: i64,
    /// 已缓存的分片 range 列表。
    pub cached_shards: Vec<String>,
    /// 最近一次失败的原因分类（null 表示无失败）。
    pub last_fault: Option<IndexFault>,
    /// 失败原因文案键（前端 i18n）。
    pub last_fault_message_key: Option<String>,
    /// 累计被拒绝的回退次数（防降级命中次数）。
    pub rollback_rejections: u64,
}

/// 远端模块条目 + 折叠后的本地化文本（`registry_modules` 命令返回）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RemoteModuleView {
    pub id: String,
    pub i18n_namespace: String,
    /// 已按 locale 折叠的展示名（回退 `en-US`，再回退 `id`）。
    pub display_name: String,
    /// 已按 locale 折叠的简介（回退 `en-US`，再回退空串）。
    pub summary: String,
    /// 实际命中的 locale（`zh-CN` / `en-US` / 空表示均未命中）。
    pub summary_locale: String,
    pub version: String,
    pub channel: String,
    pub license: String,
    pub repo: String,
    pub author_name: String,
    pub author_contact: String,
    pub author_verified: bool,
    pub api_version: i64,
    pub min_launcher: String,
    pub max_launcher: Option<String>,
    pub published_at: String,
    pub platforms: Vec<String>,
    pub yanked: bool,
    pub yank_reason: Option<String>,
    pub permissions: Vec<String>,
    pub permissions_derived: model::PermissionsDerived,
    pub changelog: String,
    /// 当前平台是否可安装（平台 / 版本区间 / api_version / yank 的综合判定）。
    pub installable: bool,
    /// 不可安装的原因键（可安装时为 null；前端 i18n）。
    pub blocked_reason_key: Option<String>,
    /// 当前平台资产的主下载地址（无可用资产时为 null）。
    pub download_url: Option<String>,
    /// 当前平台资产的 sha256（供安装链路校验；无可用资产时为 null）。
    pub download_sha256: Option<String>,
    /// 当前平台资产的字节数。
    pub download_size: i64,
}

impl RemoteModuleView {
    /// 由条目 + locale 折叠出前端视图（纯计算）。
    pub fn from_entry(entry: &ModuleEntry, locale: &str, current_launcher: &str) -> Self {
        let summary_locale = resolved_locale(&entry.summary, locale);
        let platform_asset = model::Platform::current().and_then(|p| entry.asset_for(p));
        let blocked_reason_key = install_block_reason(entry, platform_asset, current_launcher);
        Self {
            id: entry.id.clone(),
            i18n_namespace: entry.i18n_namespace.clone(),
            display_name: entry.resolved_display_name(),
            summary: entry.resolved_summary(locale),
            summary_locale,
            version: entry.version.clone(),
            channel: entry.channel.as_str().to_string(),
            license: entry.license.clone(),
            repo: entry.repo.clone(),
            author_name: entry.author.name.clone(),
            author_contact: entry.author.contact.clone(),
            author_verified: entry.author.verified,
            api_version: entry.api_version,
            min_launcher: entry.min_launcher.clone(),
            max_launcher: entry.max_launcher.clone(),
            published_at: entry.published_at.clone(),
            platforms: entry.platforms.iter().map(|p| p.as_str().to_string()).collect(),
            yanked: entry.yanked,
            yank_reason: entry.yank_reason.clone(),
            permissions: entry.permissions.clone(),
            permissions_derived: entry.permissions_derived.clone(),
            changelog: entry.resolved_changelog(locale),
            installable: blocked_reason_key.is_none(),
            blocked_reason_key: blocked_reason_key.map(str::to_string),
            download_url: platform_asset.map(|a| a.url.clone()),
            download_sha256: platform_asset.map(|a| a.sha256.clone()),
            download_size: platform_asset.map(|a| a.size).unwrap_or(0),
        }
    }
}

/// 判定某条目在当前环境下能否安装，返回不可安装的原因键。
///
/// 对应文档 2.9.5 第 1 步：平台 / 版本区间 / api_version / yank 任一不满足即置灰。
pub fn install_block_reason(
    entry: &ModuleEntry,
    platform_asset: Option<&model::ModuleAsset>,
    current_launcher: &str,
) -> Option<&'static str> {
    if entry.yanked {
        return Some("module.registry.block.yanked");
    }
    if !entry.status.is_approved() {
        return Some("module.registry.block.not_approved");
    }
    // 注意两个"版本"不是一回事，别把它们合并：本函数判的是**模块 API 版本**
    // （`Module` trait 契约，取自 `registry::manifest`），而 `SUPPORTED_SCHEMA_VERSION`
    // 是**索引 schema 版本**。前者决定模块能否被内核装载，后者决定索引能否被解析。
    let supported_api = u32::try_from(entry.api_version)
        .map(|version| SUPPORTED_API_VERSIONS.contains(&version))
        .unwrap_or(false);
    if !supported_api {
        return Some("module.registry.block.api_version");
    }
    let Some(current) = model::Platform::current() else {
        return Some("module.registry.block.platform_unsupported");
    };
    if !entry.platforms.iter().any(|p| *p == current) {
        return Some("module.registry.block.platform");
    }
    match model::launcher_range_covers(current_launcher, &entry.min_launcher, entry.max_launcher.as_deref())
    {
        Some(true) => {}
        _ => return Some("module.registry.block.launcher"),
    }
    match platform_asset {
        None => Some("module.registry.block.asset_missing"),
        // 缺摘要的条目必须被拒绝（不可安装）。
        Some(a) if !a.has_sha256() => Some("module.registry.block.digest_missing"),
        Some(_) => None,
    }
}

/// 命中的 locale（精确 → 忽略大小写 → `en-US`），均未命中返回空串。
fn resolved_locale(map: &std::collections::BTreeMap<String, String>, locale: &str) -> String {
    for candidate in [locale, model::FALLBACK_LOCALE] {
        let want = model::normalize_locale(candidate);
        if want.is_empty() {
            continue;
        }
        if let Some((k, _)) = map
            .iter()
            .find(|(k, v)| model::normalize_locale(k) == want && !v.trim().is_empty())
        {
            return k.clone();
        }
    }
    String::new()
}

/// 分片归属判定（文档 2.3.4）：按 `id` 首字符决定所属分片 `range`。
///
/// 返回 `None` 表示 id 为空（无法归属）。
pub fn shard_range_for_id(id: &str) -> Option<&'static str> {
    let first = id.chars().next()?;
    Some(shard_range_for_char(first))
}

/// 首字符 → 分片 range 映射（文档 2.1 的七段字母范围 + 数字段 + 防御性 `_other`）。
fn shard_range_for_char(first: char) -> &'static str {
    match first {
        '0'..='9' => "0-9",
        'a'..='b' => "a-b",
        'c'..='d' => "c-d",
        'e'..='g' => "e-g",
        'h'..='l' => "h-l",
        'm'..='o' => "m-o",
        'p'..='r' => "p-r",
        's'..='u' => "s-u",
        'v'..='z' => "v-z",
        _ => "_other",
    }
}

/// 会话内状态（不持久化）。
#[derive(Debug, Default)]
struct SessionState {
    mirrors: HashMap<String, MirrorStat>,
    last_fault: Option<IndexFault>,
}

/// 元数据客户端。
pub struct RegistryService {
    paths: RegistryPaths,
    settings: Arc<SettingsService>,
    signer: SignatureVerifier,
    session: Mutex<SessionState>,
    /// 最近一次索引装载结果（内存态，供 `registry_status` 与列表接口复用）。
    last_index: Mutex<Option<LoadedIndex>>,
    current_launcher: String,
}

/// 已装载的索引（内存态）。
#[derive(Debug, Clone)]
struct LoadedIndex {
    index: Arc<RegistryIndex>,
    source: IndexSource,
    using_last_known_good: bool,
    signature_status: SignatureStatus,
    fetched_at: i64,
    fault: Option<IndexFault>,
    shard_index: Option<Arc<ShardIndex>>,
}

impl RegistryService {
    /// 构造服务。`settings` 用于读取 `download.mirror` 与 `registry.channel`。
    pub fn new(settings: Arc<SettingsService>, paths: &Paths) -> Self {
        Self {
            paths: RegistryPaths::new(paths),
            settings,
            signer: SignatureVerifier::new(),
            session: Mutex::new(SessionState::default()),
            last_index: Mutex::new(None),
            current_launcher: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// 缓存路径（供命令层展示与测试）。
    pub fn cache_paths(&self) -> &RegistryPaths {
        &self.paths
    }

    /// 当前配置的通道过滤值（`registry.channel`，默认 `stable`）。
    pub fn channel_filter(&self) -> String {
        self.settings
            .get_or("registry.channel", "stable".to_string())
    }

    /// 当前配置的资产层镜像偏好（`download.mirror`，默认 `auto`）。
    pub fn mirror_preference(&self) -> MirrorPreference {
        MirrorPreference::parse(&self.settings.get_or("download.mirror", "auto".to_string()))
    }

    /// 构建元数据 HTTP 客户端。
    ///
    /// **必须**走 [`crate::services::http_client::client_builder`] 以继承系统代理与环境变量代理
    /// （文档 2.9.3：国内网络直连 raw.githubusercontent 常失败，需代理感知）。
    fn build_client(&self) -> Result<reqwest::Client, KernelError> {
        Ok(crate::services::http_client::client_builder(Duration::from_secs(
            META_REQUEST_TIMEOUT_SEC,
        ))
        .user_agent(concat!("copper-golem/", env!("CARGO_PKG_VERSION")))
        .build()?)
    }

    /// 当前时间（Unix 秒）。
    fn now_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// 当前时间（RFC3339 UTC，用于锚点与日志）。
    fn now_rfc3339() -> String {
        format_rfc3339(Self::now_secs())
    }

    /// 读取防降级锚点（文件损坏时留证据）。
    pub fn anchor(&self) -> TrustAnchor {
        anchor::read_anchor_with_backup(&self.paths.anchor_file()).0
    }

    /// 加载索引（TTL 命中 0 网络请求；`force` 绕过 TTL 但仍受防降级约束）。
    ///
    /// 降级链（文档 2.9.6）：网络/校验失败 → last-known-good → 本地缓存副本 → 不可用。
    pub async fn load_index(&self, force: bool) -> Result<IndexSource, KernelError> {
        self.paths.ensure_dirs().map_err(KernelError::Io)?;

        let meta_path = self.paths.index_meta_file();
        let index_path = self.paths.index_file();
        let mut meta = IndexMeta::load(&meta_path);
        let now = Self::now_secs();

        let cached_bytes = std::fs::read(&index_path).ok();
        let cached_index = cached_bytes
            .as_deref()
            .and_then(|b| parse_index_checked(b).ok());

        // 1) TTL 内直接使用本地缓存：0 网络请求（文档 2.9.3）。
        if !force {
            if let (Some(index), Some(bytes)) = (cached_index.as_ref(), cached_bytes.as_ref()) {
                let ttl = self.effective_ttl(index);
                if meta.is_fresh(now, ttl) {
                    self.record_loaded(
                        // `index` 是 `&RegistryIndex`，显式解引用克隆出拥有所有权的索引。
                        Arc::new((*index).clone()),
                        IndexSource::Cache,
                        false,
                        meta.schema_version,
                        // 缓存路径本次未做验签：公钥未配置时如实为 NotConfigured，
                        // 已配置时也只能是"未验签"，绝不冒充 Verified。
                        self.signer.status(),
                        meta.fetched_at,
                        None,
                    );
                    self.load_shard_index().await;
                    return Ok(IndexSource::Cache);
                }
                // 条件请求前置校验：本地副本自身若已损坏，不得作为 304 的比对基准。
                let cache_intact =
                    digest::verify_bytes(bytes, &meta.content_sha256) == DigestCheck::Match;
                if !cache_intact {
                    log::warn!("registry 本地索引副本与 index.meta.json 记录不一致，将强制全量拉取");
                    meta.etag.clear();
                    meta.last_modified.clear();
                }
            }
        }

        // 2) 网络拉取（索引层固定三镜像 + 条件请求）。
        // 先把结果绑定到局部变量再 match：避免 scrutinee 的临时借用与分支内
        // 对 `meta` 的写入（304 分支刷新 fetched_at）发生借用冲突。
        let fetched = self.fetch_index_from_mirrors(&meta).await;
        match fetched {
            Ok(FetchIndex::Fresh { bytes, index, etag, last_modified, digest }) => {
                // 2a) 防降级校验（文档 2.5 规则 3）。
                let mut anchor = self.anchor();
                let verdict = anchor.compare(&index.generated_at, &index.repo_commit);
                match verdict {
                    model::RollbackVerdict::Rollback => {
                        anchor.note_rollback(&Self::now_rfc3339());
                        let _ = anchor.save(&self.paths.anchor_file());
                        self.set_fault(Some(IndexFault::RollbackDetected));
                        log::warn!(
                            "检测到元数据回退（incoming generated_at={} commit={}），已拒绝加载并保留本地缓存",
                            index.generated_at,
                            index.repo_commit
                        );
                        return self.fallback(IndexFault::RollbackDetected).await;
                    }
                    _ => {}
                }

                // 2b) `min_launcher` 门槛：数据仍可展示，但状态必须如实告知"需要升级启动器"。
                if !model::index_min_launcher_satisfied(&self.current_launcher, &index.min_launcher) {
                    log::warn!(
                        "registry 索引要求启动器 >= {}，当前 {}",
                        index.min_launcher,
                        self.current_launcher
                    );
                }

                // 2c) minisign 验签。公钥未配置时如实返回 NotConfigured，绝不冒充"已验证"；
                //     验签失败则拒绝该数据并使用 last-known-good（文档 2.9.6）。
                let sig_text = self.fetch_signature_file(&index.repo_commit).await;
                let outcome = match sig_text {
                    Some(text) => self.signer.verify_index(&bytes, &text),
                    None => self.signer.verify_index(&bytes, ""),
                };
                if outcome.should_reject() {
                    log::error!("registry 索引 minisign 验签失败，拒绝该数据");
                    self.set_fault(Some(IndexFault::SignatureFailed));
                    return self.fallback(IndexFault::SignatureFailed).await;
                }
                let signature_status = outcome.status();

                // 2d) 写入缓存 + 更新锚点。
                std::fs::write(&index_path, &bytes).map_err(KernelError::Io)?;
                let new_meta = IndexMeta {
                    etag,
                    last_modified,
                    fetched_at: now,
                    highest_generated_at: Some(index.generated_at.clone()),
                    schema_version: index.schema_version,
                    repo_commit: index.repo_commit.clone(),
                    content_sha256: digest,
                };
                new_meta.save(&meta_path).map_err(KernelError::Io)?;
                anchor.observe(&index.generated_at, &index.repo_commit, &Self::now_rfc3339());
                let _ = anchor.save(&self.paths.anchor_file());
                // last-known-good：只有完整通过校验的快照才写入（离线兜底）。
                self.write_last_known_good(&bytes);

                self.record_loaded(
                    Arc::new(index.clone()),
                    IndexSource::Network,
                    false,
                    index.schema_version,
                    signature_status,
                    now,
                    None,
                );
                self.load_shard_index().await;
                Ok(IndexSource::Network)
            }
            Ok(FetchIndex::NotModified) => {
                // 304：本地副本仍有效，只刷新 fetched_at。
                meta.fetched_at = now;
                meta.save(&meta_path).map_err(KernelError::Io)?;
                let index = cached_index.ok_or_else(|| {
                    KernelError::Config("服务端返回 304 但本地无可用索引副本".into())
                })?;
                self.record_loaded(
                    Arc::new(index.clone()),
                    IndexSource::NotModified,
                    false,
                    index.schema_version,
                    // 304 表示沿用本地副本，本次未重新验签。
                    self.signer.status(),
                    now,
                    None,
                );
                self.load_shard_index().await;
                Ok(IndexSource::NotModified)
            }
            Err(fault) => self.fallback(fault).await,
        }
    }

    /// 网络/校验失败后的降级：last-known-good → 本地缓存副本 → 不可用。
    async fn fallback(&self, fault: IndexFault) -> Result<IndexSource, KernelError> {
        self.set_fault(Some(fault));

        // 优先 last-known-good（上一次完整通过校验的快照）。
        if let Some(bytes) = std::fs::read(self.paths.last_known_good_index()).ok() {
            if let Ok(index) = parse_index_checked(&bytes) {
                log::warn!("registry 使用 last-known-good 快照（故障: {fault:?}）");
                let meta = IndexMeta::load(&self.paths.index_meta_file());
                self.record_loaded(
                    Arc::new(index),
                    IndexSource::LastKnownGood,
                    true,
                    meta.schema_version,
                    self.signer.status(),
                    meta.fetched_at,
                    Some(fault),
                );
                self.load_shard_index_from_last_known_good().await;
                return Ok(IndexSource::LastKnownGood);
            }
        }

        // 其次用本地缓存副本（可能是过期但仍可解析的索引）。
        if let Some(bytes) = std::fs::read(self.paths.index_file()).ok() {
            if let Ok(index) = parse_index_checked(&bytes) {
                log::warn!("registry 使用本地缓存副本（故障: {fault:?}）");
                let meta = IndexMeta::load(&self.paths.index_meta_file());
                self.record_loaded(
                    Arc::new(index),
                    IndexSource::CacheFallback,
                    meta.fetched_at > 0,
                    meta.schema_version,
                    self.signer.status(),
                    meta.fetched_at,
                    Some(fault),
                );
                self.load_shard_index().await;
                return Ok(IndexSource::CacheFallback);
            }
        }

        // 完全不可用：清空内存态，前端据此展示"网络不可用，仅显示已安装模块"。
        *self.last_index.lock() = None;
        Err(KernelError::Config(
            "元数据不可用：全部镜像失败且本地无可用缓存".into(),
        ))
    }

    /// 拉取 `index.json`：索引层固定三镜像 + 多镜像 `index.json.sha256` 共识 + 条件请求。
    async fn fetch_index_from_mirrors(&self, meta: &IndexMeta) -> Result<FetchIndex, IndexFault> {
        let client = self.build_client().map_err(|e| {
            log::error!("registry HTTP 客户端构建失败: {e}");
            IndexFault::NetworkUnreachable
        })?;

        // 仓库最新 commit 未知时，jsdelivr 退化为 @main（仍有三条入口）。
        let candidates = mirror::index_mirrors("main");

        // 3a) 多镜像锚点摘要求共识（文档 2.5 规则 2：`.sha256` 提供轻量比对）。
        let mut anchor_digests = Vec::new();
        for path in &candidates {
            let sha_url = format!("{path}.sha256");
            if let Ok(resp) = client.get(&sha_url).send().await {
                if resp.status().is_success() {
                    if let Ok(text) = resp.text().await {
                        if let Some(d) = digest::parse_sha256_file(&text) {
                            anchor_digests.push((path.clone(), d));
                        }
                    }
                }
            }
        }
        if anchor_digests.is_empty() {
            // 所有镜像都取不到摘要文件：按"缺摘要"拒绝（文档 2.5 规则 1）。
            log::warn!("registry 全部镜像均未提供 index.json.sha256，按缺摘要拒绝");
            return Err(IndexFault::DigestMissing);
        }
        let digest_list: Vec<String> = anchor_digests.iter().map(|(_, d)| d.clone()).collect();
        let (consensus, hits) = digest::digest_consensus(&digest_list)
            .ok_or(IndexFault::DigestMissing)?;
        if hits * 2 <= digest_list.len() && digest_list.len() > 1 {
            // 多镜像摘要不一致且无多数派：疑似单点污染，拒绝。
            log::warn!("registry index.json.sha256 多镜像不一致且无多数派，拒绝本次数据");
            return Err(IndexFault::AnchorDisagreement);
        }

        // 3b) 按会话自愈顺序逐镜像拉取索引本体。
        let demoted = |url: &str| self.is_demoted(url);
        let order = mirror::order_candidates(&candidates, demoted);
        let mut last_fault = IndexFault::NetworkUnreachable;

        for idx in order {
            let Some(base) = candidates.get(idx) else {
                continue;
            };
            let mut req = client.get(base);
            if meta.has_conditional_headers() {
                if !meta.etag.trim().is_empty() {
                    req = req.header(reqwest::header::IF_NONE_MATCH, meta.etag.trim());
                }
                if !meta.last_modified.trim().is_empty() {
                    req = req.header(
                        reqwest::header::IF_MODIFIED_SINCE,
                        meta.last_modified.trim(),
                    );
                }
            }

            match req.send().await {
                Ok(resp) if resp.status() == reqwest::StatusCode::NOT_MODIFIED => {
                    self.record_mirror_result(base, true);
                    return Ok(FetchIndex::NotModified);
                }
                Ok(resp) if resp.status().is_success() => {
                    let etag = header_string(resp.headers(), reqwest::header::ETAG);
                    let last_modified =
                        header_string(resp.headers(), reqwest::header::LAST_MODIFIED);
                    let bytes = match resp.bytes().await {
                        Ok(b) => b.to_vec(),
                        Err(e) => {
                            log::warn!("registry 镜像 {base} 读取响应体失败: {e}");
                            self.record_mirror_result(base, false);
                            last_fault = IndexFault::NetworkUnreachable;
                            continue;
                        }
                    };

                    // 3c) 摘要校验：以多镜像共识摘要为准；该镜像未提供摘要时退回共识值。
                    let expected = anchor_digests
                        .iter()
                        .find(|(p, _)| p == base)
                        .map(|(_, d)| d.to_string())
                        .unwrap_or_else(|| consensus.clone());
                    // 先做判定再 move `bytes`：避免 match scrutinee 的借用与分支内移动冲突。
                    let check = digest::verify_bytes(&bytes, &expected);
                    match check {
                        DigestCheck::Match => {
                            self.record_mirror_result(base, true);
                            let index = match parse_index_checked(&bytes) {
                                Ok(i) => i,
                                Err(fault) => {
                                    // `schema_version` 过高 / JSON 非法：硬拒绝，不尝试其它镜像之外的兜底。
                                    last_fault = fault;
                                    continue;
                                }
                            };
                            return Ok(FetchIndex::Fresh {
                                bytes,
                                index,
                                etag,
                                last_modified,
                                digest: expected,
                            });
                        }
                        DigestCheck::Missing => {
                            log::warn!("registry 镜像 {base} 摘要缺失，拒绝该响应");
                            self.record_mirror_result(base, false);
                            last_fault = IndexFault::DigestMissing;
                        }
                        DigestCheck::Mismatch => {
                            log::warn!("registry 镜像 {base} 的 index.json 摘要不匹配，静默重试下一镜像");
                            self.record_mirror_result(base, false);
                            last_fault = IndexFault::DigestMismatch;
                        }
                    }
                }
                Ok(resp) => {
                    log::warn!("registry 镜像 {base} 返回 HTTP {}", resp.status());
                    self.record_mirror_result(base, false);
                    last_fault = IndexFault::NetworkUnreachable;
                }
                Err(e) => {
                    log::warn!("registry 镜像 {base} 请求失败: {e}");
                    self.record_mirror_result(base, false);
                    last_fault = IndexFault::NetworkUnreachable;
                }
            }
        }
        Err(last_fault)
    }

    /// 拉取 `index.json.minisig`（尽力而为；缺失时返回 `None`，由验签器如实归类）。
    async fn fetch_signature_file(&self, commit: &str) -> Option<String> {
        let client = self.build_client().ok()?;
        for url in mirror::repo_file_mirrors("index.json.minisig", commit) {
            if let Ok(resp) = client.get(&url).send().await {
                if resp.status().is_success() {
                    if let Ok(text) = resp.text().await {
                        if !text.trim().is_empty() {
                            return Some(text);
                        }
                    }
                }
            }
        }
        None
    }

    /// 是否处于本会话降权状态。
    fn is_demoted(&self, url: &str) -> bool {
        self.session
            .lock()
            .mirrors
            .get(url)
            .map(|s| s.is_demoted())
            .unwrap_or(false)
    }

    /// 记录镜像使用结果（会话内，不持久化）。
    fn record_mirror_result(&self, url: &str, success: bool) {
        let mut session = self.session.lock();
        let stat = session.mirrors.entry(url.to_string()).or_default();
        mirror::record_result(stat, success);
    }

    /// 设置最近一次故障（`None` 表示本次成功、清除故障）。
    fn set_fault(&self, fault: Option<IndexFault>) {
        self.session.lock().last_fault = fault;
    }

    /// 索引条目的生效 TTL：取索引内 `cache_ttl_sec` 的最小值（文档 2.9.2）。
    fn effective_ttl(&self, index: &RegistryIndex) -> u64 {
        index
            .entries
            .iter()
            .filter_map(|e| e.ttl_sec())
            .min()
            .unwrap_or(DEFAULT_TTL_SEC)
    }

    /// 写入/更新 last-known-good 快照。
    fn write_last_known_good(&self, index_bytes: &[u8]) {
        let dir = self.paths.last_known_good_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        if std::fs::write(self.paths.last_known_good_index(), index_bytes).is_err() {
            log::warn!("registry last-known-good 索引写入失败（不影响本次使用）");
        }
    }

    /// 记录一次装载结果。
    #[allow(clippy::too_many_arguments)]
    fn record_loaded(
        &self,
        index: Arc<RegistryIndex>,
        source: IndexSource,
        using_last_known_good: bool,
        _schema_version: i64,
        signature_status: SignatureStatus,
        fetched_at: i64,
        fault: Option<IndexFault>,
    ) {
        if fault.is_none() {
            self.set_fault(None);
        } else {
            self.set_fault(fault);
        }
        let shard_index = self.last_index.lock().as_ref().and_then(|l| l.shard_index.clone());
        *self.last_index.lock() = Some(LoadedIndex {
            index,
            source,
            using_last_known_good,
            signature_status,
            fetched_at,
            fault,
            shard_index,
        });
    }

    /// 拉取并缓存分片清单 `modules/_index.json`（校验其 sha256）。
    async fn load_shard_index(&self) {
        let Some(index) = self.current_index() else {
            return;
        };
        let Some(entry) = index.path_of_kind(IndexEntryKind::ModuleIndex) else {
            log::warn!("registry 索引中没有 module_index 条目，无法获取分片清单");
            self.set_fault(Some(IndexFault::DigestMissing));
            return;
        };
        if !entry.has_sha256() {
            // 缺摘要的条目必须被拒绝（文档 2.5 规则 1）。
            log::warn!("registry modules/_index.json 条目缺少 sha256，拒绝使用");
            self.set_fault(Some(IndexFault::DigestMissing));
            return;
        }
        let cache_file = self.paths.root().join(&entry.path);
        let bytes = match self.fetch_entry_bytes(entry, &cache_file).await {
            Ok(b) => b,
            Err(fault) => {
                log::warn!("registry 分片清单拉取失败: {fault:?}");
                self.set_fault(Some(fault));
                return;
            }
        };
        match serde_json::from_slice::<ShardIndex>(&bytes) {
            Ok(shard_index) => {
                if !super::registry::schema_supported(shard_index.schema_version) {
                    log::warn!(
                        "registry 分片清单 schema_version={} 高于支持值 {}，拒绝",
                        shard_index.schema_version,
                        SUPPORTED_SCHEMA_VERSION
                    );
                    self.set_fault(Some(IndexFault::SchemaUnsupported));
                    return;
                }
                if let Some(loaded) = self.last_index.lock().as_mut() {
                    loaded.shard_index = Some(Arc::new(shard_index));
                }
            }
            Err(e) => {
                log::warn!("registry 分片清单解析失败: {e}");
                self.set_fault(Some(IndexFault::DigestMissing));
            }
        }
    }

    /// 从 last-known-good 快照加载分片清单（离线兜底）。
    async fn load_shard_index_from_last_known_good(&self) {
        let path = self.paths.last_known_good_dir().join("modules/_index.json");
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(shard_index) = serde_json::from_slice::<ShardIndex>(&bytes) {
                if let Some(loaded) = self.last_index.lock().as_mut() {
                    loaded.shard_index = Some(Arc::new(shard_index));
                }
            }
        }
    }

    /// 按索引条目拉取一份文件：校验 sha256 与 size（三级校验中的第二级）。
    ///
    /// 成功时写入本地缓存路径并返回字节；失败返回故障分类。
    async fn fetch_entry_bytes(
        &self,
        entry: &model::RegistryEntry,
        cache_file: &std::path::Path,
    ) -> Result<Vec<u8>, IndexFault> {
        // 路径安全：条目的相对路径会被拼进缓存目录。
        if !is_safe_relative_path(&entry.path) {
            log::warn!("registry 条目的 path 非法（疑似穿越）: {}", entry.path);
            return Err(IndexFault::DigestMissing);
        }

        // 缓存命中：内容未变（sha256 一致）则零下载（文档 3.3 增量更新）。
        if let Ok(cached) = std::fs::read(cache_file) {
            if digest::verify_bytes(&cached, &entry.sha256).is_ok()
                && digest::size_matches(cached.len(), entry.size)
            {
                return Ok(cached);
            }
            // 缓存损坏：删除后重新拉取。
            let _ = std::fs::remove_file(cache_file);
        }

        let client = self.build_client().map_err(|_| IndexFault::NetworkUnreachable)?;
        let demoted = |url: &str| self.is_demoted(url);
        let order = mirror::order_candidates(&entry.urls, demoted);
        let mut last_fault = IndexFault::NetworkUnreachable;

        for idx in order {
            let Some(url) = entry.urls.get(idx) else {
                continue;
            };
            match client.get(url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let bytes = match resp.bytes().await {
                        Ok(b) => b.to_vec(),
                        Err(_) => {
                            self.record_mirror_result(url, false);
                            last_fault = IndexFault::NetworkUnreachable;
                            continue;
                        }
                    };
                    if !digest::size_matches(bytes.len(), entry.size) {
                        log::warn!(
                            "registry {} 字节数与索引声明不符（{} vs {}），拒绝",
                            entry.path,
                            bytes.len(),
                            entry.size
                        );
                        self.record_mirror_result(url, false);
                        last_fault = IndexFault::DigestMismatch;
                        continue;
                    }
                    // 先做判定再 move `bytes`：避免 match scrutinee 的借用与分支内返回冲突。
                    let check = digest::verify_bytes(&bytes, &entry.sha256);
                    match check {
                        DigestCheck::Match => {
                            self.record_mirror_result(url, true);
                            if let Some(parent) = cache_file.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            if let Err(e) = std::fs::write(cache_file, &bytes) {
                                log::warn!("registry 缓存写入失败 {}: {e}", cache_file.display());
                            }
                            return Ok(bytes);
                        }
                        DigestCheck::Missing => {
                            log::warn!("registry {} 缺少 sha256 摘要，拒绝该响应", entry.path);
                            self.record_mirror_result(url, false);
                            last_fault = IndexFault::DigestMissing;
                        }
                        DigestCheck::Mismatch => {
                            log::warn!("registry {} 摘要不匹配，静默重试下一镜像", entry.path);
                            self.record_mirror_result(url, false);
                            last_fault = IndexFault::DigestMismatch;
                        }
                    }
                }
                Ok(_) | Err(_) => {
                    self.record_mirror_result(url, false);
                    last_fault = IndexFault::NetworkUnreachable;
                }
            }
        }
        Err(last_fault)
    }

    /// 拉取全部已声明的模块分片（校验 sha256），返回条目总数。
    ///
    /// 单个分片校验失败只丢弃该分片，其余照常展示（文档 2.9.6）。
    pub async fn load_all_shards(&self) -> Result<Vec<ModuleEntry>, KernelError> {
        let shard_index = self
            .last_index
            .lock()
            .as_ref()
            .and_then(|l| l.shard_index.clone());

        let Some(shard_index) = shard_index else {
            // 尚未装载分片清单：先尝试装载。
            self.load_shard_index().await;
            // 注意：`last_index` 的读锁守卫必须先在独立语句里释放，不能直接跨 `.await`
            // 持有——`parking_lot` 的守卫不是 `Send`，会让本 future 无法满足 Tauri 命令
            // 所要求的 `Send` 约束。
            let reloaded = self
                .last_index
                .lock()
                .as_ref()
                .and_then(|l| l.shard_index.clone());
            return match reloaded {
                Some(si) => self.collect_shards(si).await,
                None => Ok(Vec::new()),
            };
        };
        self.collect_shards(shard_index).await
    }

    /// 逐个拉取分片并合并（shard 级失败只跳过该分片）。
    async fn collect_shards(&self, shard_index: Arc<ShardIndex>) -> Result<Vec<ModuleEntry>, KernelError> {
        let index = match self.current_index() {
            Some(i) => i,
            None => return Ok(Vec::new()),
        };
        let mut out: Vec<ModuleEntry> = Vec::new();
        let mut failed_shards = 0usize;

        for shard in &shard_index.shards {
            if !shard.has_sha256() {
                log::warn!("registry 分片 {} 缺少 sha256，跳过（缺摘要必须拒绝）", shard.range);
                failed_shards += 1;
                continue;
            }
            let entry = index.entry_by_path(&shard.path).cloned().unwrap_or_else(|| {
                // 索引中没有显式列出该分片：按标准 URL 模板构造候选地址（索引层固定镜像）。
                model::RegistryEntry {
                    kind: IndexEntryKind::ModuleShard,
                    path: shard.path.clone(),
                    sha256: shard.sha256.clone(),
                    size: shard.size,
                    urls: mirror::repo_file_mirrors(&shard.path, &index.repo_commit),
                    cache_ttl_sec: 0,
                }
            });
            let cache_file = self.paths.shard_file(&shard.range);
            match self.fetch_entry_bytes(&entry, &cache_file).await {
                Ok(bytes) => match serde_json::from_slice::<ModuleShard>(&bytes) {
                    Ok(parsed) => {
                        if !super::registry::schema_supported(parsed.schema_version) {
                            log::warn!(
                                "registry 分片 {} schema_version={} 不受支持，跳过",
                                shard.range,
                                parsed.schema_version
                            );
                            failed_shards += 1;
                            continue;
                        }
                        out.extend(parsed.modules);
                    }
                    Err(e) => {
                        log::warn!("registry 分片 {} 解析失败: {e}", shard.range);
                        failed_shards += 1;
                    }
                },
                Err(fault) => {
                    log::warn!("registry 分片 {} 拉取失败: {fault:?}", shard.range);
                    failed_shards += 1;
                }
            }
        }
        if failed_shards > 0 {
            log::warn!(
                "registry 有 {failed_shards} 个分片不可用，相关模块信息将标记为不可用"
            );
        }
        Ok(out)
    }

    /// 当前内存中的索引。
    fn current_index(&self) -> Option<Arc<RegistryIndex>> {
        self.last_index.lock().as_ref().map(|l| l.index.clone())
    }

    /// 已缓存的远端模块条目（已按通道过滤 + 去重 + 忽略非法条目）。
    ///
    /// 过滤规则（文档 2.3.2 / 3.1）：
    /// - `status` 非 `approved` 的条目跳过；
    /// - `channel` 不在配置集合内跳过（未知通道一律不展示）；
    /// - `id` 或 `i18n_namespace` 非法跳过（非法命名空间会导致语言包互相覆盖）；
    /// - 缺资产摘要的条目**保留展示但标记不可安装**（由 [`install_block_reason`] 负责）。
    pub async fn list_modules(&self) -> Result<(Vec<ModuleEntry>, i64), KernelError> {
        let entries = self.load_all_shards().await?;
        let filter = self.channel_filter();
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut out: Vec<ModuleEntry> = Vec::new();

        for entry in entries {
            if !entry.status.is_approved() {
                continue;
            }
            if !entry.matches_channel_filter(&filter) {
                continue;
            }
            if !model::is_valid_module_id(&entry.id) {
                log::warn!("registry 跳过 id 非法的条目: {}", entry.id);
                continue;
            }
            if !entry.has_valid_i18n_namespace() {
                log::warn!(
                    "registry 跳过 i18n_namespace 非法的条目 {}（namespace={}）",
                    entry.id,
                    entry.i18n_namespace
                );
                continue;
            }
            // 先复制出 key 并结束对 `seen` 的借用：否则 match 的 scrutinee 临时值
            // 会把借用延伸到整个 match，导致后续 `out[i] = entry` 的移动被拒绝。
            let key = entry.id.clone();
            let existing = seen.get(&key).copied();
            match existing {
                Some(i) => {
                    // 同一 id 出现在多个分片：保留版本更高的一条，并记录冲突。
                    log::warn!("registry 条目 id 重复: {}（保留更高版本）", entry.id);
                    if compare_version(&entry.version, &out[i].version).is_gt() {
                        out[i] = entry;
                    }
                }
                None => {
                    seen.insert(key, out.len());
                    out.push(entry);
                }
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        let total = out.len() as i64;
        Ok((out, total))
    }

    /// 折叠为前端视图（locale → en-US → id 的回退链在此处落地）。
    pub async fn list_module_views(&self, locale: &str) -> Result<Vec<RemoteModuleView>, KernelError> {
        let (entries, _) = self.list_modules().await?;
        Ok(entries
            .iter()
            .map(|e| RemoteModuleView::from_entry(e, locale, &self.current_launcher))
            .collect())
    }

    /// 状态快照（`registry_status` 命令的数据来源）。
    pub fn status(&self) -> RegistryStatusView {
        let session = self.session.lock();
        let demoted = session
            .mirrors
            .iter()
            .filter(|(_, s)| s.is_demoted())
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>();
        let last_fault = session.last_fault;
        drop(session);

        let anchor = self.anchor();
        let guard = self.last_index.lock();
        let loaded = guard.as_ref();

        let (index, source, using_lkg, signature_status, fetched_at, fault, remote_count, shards) =
            match loaded {
                Some(l) => (
                    Some(l.index.clone()),
                    l.source,
                    l.using_last_known_good,
                    l.signature_status,
                    l.fetched_at,
                    l.fault,
                    l.shard_index.as_ref().map(|s| s.total).unwrap_or(0),
                    l.shard_index
                        .as_ref()
                        .map(|s| s.shards.iter().map(|x| x.range.clone()).collect::<Vec<_>>())
                        .unwrap_or_default(),
                ),
                None => (
                    None,
                    IndexSource::CacheFallback,
                    false,
                    self.signer.status(),
                    0,
                    last_fault,
                    0,
                    Vec::new(),
                ),
            };

        let availability = match (&index, source) {
            (None, _) => RegistryAvailability::Unavailable,
            (Some(_), s) if s.is_degraded() => RegistryAvailability::Stale,
            (Some(_), _) => RegistryAvailability::Ready,
        };
        let fault = fault.or(last_fault);
        let meta = IndexMeta::load(&self.paths.index_meta_file());

        RegistryStatusView {
            availability,
            degraded: source.is_degraded(),
            source,
            generated_at: index.as_ref().map(|i| i.generated_at.clone()),
            highest_generated_at: anchor.highest_generated_at.clone(),
            repo_commit: anchor
                .highest_repo_commit
                .clone()
                .or_else(|| index.as_ref().map(|i| i.repo_commit.clone())),
            schema_version: index.as_ref().map(|i| i.schema_version).unwrap_or(0),
            supported_schema_version: SUPPORTED_SCHEMA_VERSION,
            fetched_at,
            age_secs: meta.age_secs(Self::now_secs()),
            min_launcher: index.as_ref().map(|i| i.min_launcher.clone()),
            launcher_satisfied: index
                .as_ref()
                .map(|i| model::index_min_launcher_satisfied(&self.current_launcher, &i.min_launcher))
                .unwrap_or(true),
            // 关键诚实性约束：公钥未配置时绝不报告已验签。
            signature_verified: signature_status.is_verified(),
            signature_status,
            signature_note_key: signature_status.message_key().to_string(),
            using_last_known_good: using_lkg,
            channel_filter: self.channel_filter(),
            mirror_preference: format!("{:?}", self.mirror_preference()).to_ascii_lowercase(),
            demoted_mirrors: demoted,
            remote_module_count: remote_count,
            cached_shards: shards,
            last_fault: fault,
            last_fault_message_key: fault.map(|f| f.message_key().to_string()),
            rollback_rejections: anchor.rollback_rejections,
        }
    }

    /// 供安装链路复用的资产校验：下载完成后校验安装包 sha256（文档 2.5 规则 1）。
    ///
    /// 本方法只做判定，不发起下载——安装包下载属于全局 `DownloadService` 的职责
    /// （文档 2.9.3：MB 级资产必须进下载队列以获得断点续传与进度）。
    pub fn verify_asset_bytes(bytes: &[u8], expected_sha256: &str, expected_size: i64) -> Result<(), String> {
        if !digest::size_matches(bytes.len(), expected_size) {
            return Err(format!(
                "安装包字节数与元数据不符（{} vs {}）",
                bytes.len(),
                expected_size
            ));
        }
        match digest::verify_bytes(bytes, expected_sha256) {
            DigestCheck::Match => Ok(()),
            DigestCheck::Missing => Err("元数据未提供安装包 sha256，拒绝安装".into()),
            DigestCheck::Mismatch => Err("安装包 sha256 校验失败，已丢弃".into()),
        }
    }

    /// 清理缓存至配额以内（永不删除 `trust/` 下的防降级锚点）。
    pub fn cleanup_cache(&self, quota_bytes: u64) -> anchor::CleanupOutcome {
        anchor::cleanup_cache(self.paths.root(), quota_bytes)
    }
}

/// 索引拉取结果。
enum FetchIndex {
    Fresh {
        bytes: Vec<u8>,
        index: RegistryIndex,
        etag: String,
        last_modified: String,
        digest: String,
    },
    NotModified,
}

/// 读取响应头为字符串。
fn header_string(headers: &reqwest::header::HeaderMap, name: reqwest::header::HeaderName) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// 解析索引并做 schema 硬校验。
///
/// `schema_version` 高于客户端支持值 → **拒绝**，不做尽力解析（文档 2.5 规则 4 / 3.1）。
pub fn parse_index_checked(bytes: &[u8]) -> Result<RegistryIndex, IndexFault> {
    let index: RegistryIndex =
        serde_json::from_slice(bytes).map_err(|_| IndexFault::NetworkUnreachable)?;
    if !schema_supported(index.schema_version) {
        log::warn!(
            "registry 索引 schema_version={} 高于客户端支持值 {}，硬拒绝",
            index.schema_version,
            SUPPORTED_SCHEMA_VERSION
        );
        return Err(IndexFault::SchemaUnsupported);
    }
    Ok(index)
}

/// schema_version 是否受支持（相同或更低均可；更高必须拒绝，文档 3.1）。
pub fn schema_supported(schema_version: i64) -> bool {
    schema_version <= SUPPORTED_SCHEMA_VERSION
}

/// 比较两个 semver 版本串（不可解析者视为最小，保证不会因脏数据选中"更高版本"）。
fn compare_version(a: &str, b: &str) -> std::cmp::Ordering {
    match (model::parse_semver(a), model::parse_semver(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// Unix 秒 → RFC3339 UTC 字符串（仅用于本地锚点与日志，不依赖时间库）。
pub fn format_rfc3339(epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86400);
    let rem = epoch_secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Howard Hinnant 的 `civil_from_days`：1970-01-01 起的天数 → 公历日期。
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_assignment_follows_id_first_char() {
        assert_eq!(shard_range_for_id("copper-lamp.server-manager"), Some("c-d"));
        assert_eq!(shard_range_for_id("apple.mod"), Some("a-b"));
        assert_eq!(shard_range_for_id("gap.mod"), Some("e-g"));
        assert_eq!(shard_range_for_id("hello.mod"), Some("h-l"));
        assert_eq!(shard_range_for_id("mine.mod"), Some("m-o"));
        assert_eq!(shard_range_for_id("pack.mod"), Some("p-r"));
        assert_eq!(shard_range_for_id("server.mod"), Some("s-u"));
        assert_eq!(shard_range_for_id("vendor.mod"), Some("v-z"));
        assert_eq!(shard_range_for_id("0day.mod"), Some("0-9"));
        assert_eq!(shard_range_for_id("9lives.mod"), Some("0-9"));
        assert_eq!(shard_range_for_id("中文.mod"), Some("_other"));
        assert_eq!(shard_range_for_id(""), None);

        // 每个字母都必须有归属，不能出现空格子（否则客户端会漏查）。
        for c in 'a'..='z' {
            let id = format!("{c}mod.example");
            assert!(
                shard_range_for_id(&id).is_some(),
                "字母 {c} 必须有分片归属"
            );
        }
    }

    #[test]
    fn install_block_reason_covers_all_gates() {
        let mut entry: ModuleEntry = serde_json::from_str(
            r#"{
              "id": "example.mod", "i18n_namespace": "example-mod", "display_name": "E",
              "summary": { "en-US": "x" },
              "author": { "name": "a", "contact": "c", "verified": true },
              "repo": "https://example.com", "license": "MIT", "channel": "stable",
              "version": "1.0.0", "published_at": "2026-09-14T00:00:00Z",
              "platforms": ["windows-x86_64", "android-arm64", "linux-x86_64", "windows-aarch64"],
              "min_launcher": "0.1.0", "max_launcher": null, "api_version": 1,
              "assets": [{ "platform": "windows-x86_64", "url": "https://e.com/a", "mirrors": [], "size": 1,
                           "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }],
              "permissions": [], "changelog": { "en-US": "y" },
              "status": "approved", "yanked": false, "yank_reason": null
            }"#,
        )
        .expect("条目应能解析");

        // 平台匹配依赖编译目标；用条目声明的平台集合覆盖当前平台。
        let current = model::Platform::current().expect("测试平台应被识别");
        // `Platform` 非 `Copy`，`asset_for` 取所有权，故闭包里要用克隆。
        let asset = entry.asset_for(current.clone()).cloned().unwrap_or_else(|| model::ModuleAsset {
            platform: current.clone(),
            url: "https://e.com/a".into(),
            mirrors: vec![],
            size: 1,
            sha256: "a".repeat(64),
        });
        assert_eq!(install_block_reason(&entry, Some(&asset), "0.1.0"), None);

        // yank 优先于一切。
        entry.yanked = true;
        assert_eq!(
            install_block_reason(&entry, Some(&asset), "0.1.0"),
            Some("module.registry.block.yanked")
        );
        entry.yanked = false;

        // 启动器版本不足。
        assert_eq!(
            install_block_reason(&entry, Some(&asset), "0.0.1"),
            Some("module.registry.block.launcher")
        );
        // 启动器版本过高（上限约束）。
        entry.max_launcher = Some("0.0.5".into());
        assert_eq!(
            install_block_reason(&entry, Some(&asset), "0.1.0"),
            Some("module.registry.block.launcher")
        );
        entry.max_launcher = None;

        // api_version 不受支持：1 与 2 都受支持，故用 3 触发拒绝。
        entry.api_version = 3;
        assert_eq!(
            install_block_reason(&entry, Some(&asset), "0.1.0"),
            Some("module.registry.block.api_version")
        );

        // api_version 2（受监管运行时）必须可安装。
        entry.api_version = 2;
        assert_eq!(install_block_reason(&entry, Some(&asset), "0.1.0"), None);
        entry.api_version = 1;

        // 平台不支持。
        entry.platforms = vec![model::Platform::Unknown("linux-arm64".into())];
        assert_eq!(
            install_block_reason(&entry, Some(&asset), "0.1.0"),
            Some("module.registry.block.platform")
        );
        entry.platforms = vec![current.clone()];

        // 无可用资产 / 缺摘要。
        assert_eq!(
            install_block_reason(&entry, None, "0.1.0"),
            Some("module.registry.block.asset_missing")
        );
        let unsigned = model::ModuleAsset {
            sha256: String::new(),
            ..asset.clone()
        };
        assert_eq!(
            install_block_reason(&entry, Some(&unsigned), "0.1.0"),
            Some("module.registry.block.digest_missing")
        );

        // 未审核条目不可安装。
        entry.status = model::ReviewStatus::Pending;
        assert_eq!(
            install_block_reason(&entry, Some(&asset), "0.1.0"),
            Some("module.registry.block.not_approved")
        );
    }

    #[test]
    fn remote_view_strips_yanked_and_localizes() {
        let entry: ModuleEntry = serde_json::from_str(
            r#"{
              "id": "copper-lamp.server-manager", "i18n_namespace": "server-manager",
              "display_name": "服务端管理器",
              "summary": { "zh-CN": "中文简介", "en-US": "English summary" },
              "author": { "name": "copper-lamp", "contact": "https://github.com/copper-lamp", "verified": true },
              "repo": "https://github.com/copper-lamp/x", "license": "MIT", "channel": "stable",
              "version": "0.4.2", "published_at": "2026-09-11T09:20:00Z",
              "platforms": ["windows-x86_64", "android-arm64", "linux-x86_64", "windows-aarch64"],
              "min_launcher": "0.1.0", "max_launcher": null, "api_version": 1,
              "icon": { "path": "assets/icons/copper-lamp.server-manager.png", "sha256": "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0", "size": 4211 },
              "assets": [{ "platform": "windows-x86_64", "url": "https://e.com/a.cglm", "mirrors": [], "size": 3187456,
                           "sha256": "d4e6f80a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6" }],
              "permissions": ["network"], "permissions_derived": { "network": true, "spawn_process": false, "write_outside_module_dir": false },
              "changelog": { "zh-CN": "中文更新", "en-US": "English changelog" },
              "status": "approved", "yanked": false, "yank_reason": null
            }"#,
        )
        .expect("条目应能解析");

        let zh = RemoteModuleView::from_entry(&entry, "zh-CN", "0.1.0");
        assert_eq!(zh.display_name, "服务端管理器");
        assert_eq!(zh.summary, "中文简介");
        assert_eq!(zh.summary_locale, "zh-CN");
        assert_eq!(zh.changelog, "中文更新");
        assert_eq!(zh.download_sha256.as_deref(), Some("d4e6f80a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6"));
        assert_eq!(zh.download_size, 3187456);

        // 未知 locale → 回退 en-US。
        let ja = RemoteModuleView::from_entry(&entry, "ja-JP", "0.1.0");
        assert_eq!(ja.summary, "English summary");
        assert_eq!(ja.summary_locale, "en-US");
        assert_eq!(ja.changelog, "English changelog");
        // 展示名不参与 locale 折叠（display_name 是默认语言名）。
        assert_eq!(ja.display_name, "服务端管理器");

        // 无 display_name → 回退 id。
        let mut no_name = entry.clone();
        no_name.display_name = String::new();
        let view = RemoteModuleView::from_entry(&no_name, "zh-CN", "0.1.0");
        assert_eq!(view.display_name, "copper-lamp.server-manager");
    }

    #[test]
    fn json_helpers_and_rfc3339_formatting() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        // 1_755_000_000 = 2025-08-12T12:00:00Z（用 Date.parse 对照核实过）。
        assert_eq!(format_rfc3339(1_755_000_000), "2025-08-12T12:00:00Z");
        // 与解析器互为逆运算（同一个整秒时间点）。
        let parsed = model::parse_generated_at(&format_rfc3339(1_755_000_000)).unwrap();
        assert_eq!(parsed.epoch_secs, 1_755_000_000);
        // 闰年日期。
        assert_eq!(format_rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn schema_support_boundary_is_hard() {
        assert!(schema_supported(0));
        assert!(schema_supported(1));
        assert!(!schema_supported(2));
        assert!(!schema_supported(99));

        let bytes = br#"{"schema_version": 2, "generated_at": "2026-09-14T03:12:40Z",
            "repo_commit": "c", "min_launcher": "0.1.0", "entries": []}"#;
        // 判定失败原因而非整体相等：`RegistryIndex` 不实现 `PartialEq`，
        // 且这里真正要断言的是"因 schema 版本过高而被拒绝"（文档 2.5 规则 4）。
        assert_eq!(
            parse_index_checked(bytes).unwrap_err(),
            IndexFault::SchemaUnsupported
        );

        let ok = br#"{"schema_version": 1, "generated_at": "2026-09-14T03:12:40Z",
            "repo_commit": "c", "min_launcher": "0.1.0", "entries": []}"#;
        let parsed = parse_index_checked(ok).expect("受支持版本应能解析");
        assert_eq!(parsed.schema_version, 1);

        // 完全不是 JSON → 拒绝。
        assert!(parse_index_checked(b"not json").is_err());
    }

    #[test]
    fn asset_verification_is_fail_closed() {
        let bytes = b"abc";
        let good = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(RegistryService::verify_asset_bytes(bytes, good, 3).is_ok());
        assert!(RegistryService::verify_asset_bytes(bytes, good, 4).is_err());
        assert!(RegistryService::verify_asset_bytes(b"abcd", good, 3).is_err());
        assert!(RegistryService::verify_asset_bytes(bytes, "", 3).is_err());
        assert!(RegistryService::verify_asset_bytes(bytes, &"f".repeat(64), 3).is_err());
        // size 未声明（0）时不参与判定。
        assert!(RegistryService::verify_asset_bytes(bytes, good, 0).is_ok());
    }
}
