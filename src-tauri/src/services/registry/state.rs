//! 元数据运行态：`index.meta.json`（条件请求凭据与 TTL 判定）与索引装载来源。
//!
//! 依据 [cgl-libs](../../../docs/cgl-libs.md) 2.9.2 / 2.9.3：
//! `index.meta.json` 保存 ETag / Last-Modified / `fetched_at` / `highest_generated_at`，
//! 是"TTL 内 0 网络请求、超 TTL 带条件请求、304 只刷新 fetched_at"这条链路的本地状态。

use serde::{Deserialize, Serialize};

/// `index.meta.json` 的持久化结构。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct IndexMeta {
    /// 最近一次成功网络响应带回来的 ETag（缺失为空串）。
    #[serde(default)]
    pub etag: String,
    /// 最近一次成功网络响应带回来的 Last-Modified（缺失为空串）。
    #[serde(default)]
    pub last_modified: String,
    /// 最近一次"确认有效"的时间（UTC 秒）。304 只刷新该字段，不写 index.json。
    #[serde(default)]
    pub fetched_at: i64,
    /// 本地已见过的最高 `generated_at`（与 [`super::anchor::TrustAnchor`] 保持一致）。
    #[serde(default)]
    pub highest_generated_at: Option<String>,
    /// 本次落盘对应的索引 schema_version（诊断用）。
    #[serde(default)]
    pub schema_version: i64,
    /// 本次落盘对应的 repo_commit（诊断用）。
    #[serde(default)]
    pub repo_commit: String,
    /// 本地副本自身的 sha256（用于"缓存被外部改写"检测）。
    #[serde(default)]
    pub content_sha256: String,
}

impl IndexMeta {
    /// 读取；文件缺失或损坏时返回默认值（表现为"无缓存"，会触发一次网络请求）。
    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Self>(&b).ok())
            .unwrap_or_default()
    }

    /// 原子写入。
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        std::fs::rename(&tmp, path)
    }

    /// 距离 `now` 已经过多少秒（`fetched_at` 未设置时返回 `None`）。
    pub fn age_secs(&self, now: i64) -> Option<i64> {
        if self.fetched_at <= 0 {
            None
        } else {
            Some((now - self.fetched_at).max(0))
        }
    }

    /// TTL 是否仍然有效（`fetched_at + ttl` 尚未过期）。
    ///
    /// `fetched_at` 未设置、或 `ttl_sec` 为 0 时返回 `false` → 需要发起网络请求。
    pub fn is_fresh(&self, now: i64, ttl_sec: u64) -> bool {
        if ttl_sec == 0 {
            return false;
        }
        match self.age_secs(now) {
            Some(age) => age < ttl_sec as i64,
            None => false,
        }
    }

    /// 是否具备发起条件请求所需的凭据。
    pub fn has_conditional_headers(&self) -> bool {
        !self.etag.trim().is_empty() || !self.last_modified.trim().is_empty()
    }
}

/// 本次索引数据的来源，决定前端应展示的降级提示强度（文档 2.9.6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexSource {
    /// 本次从网络拉取并校验通过。
    Network,
    /// 命中本地缓存且在 TTL 内（0 网络请求）。
    Cache,
    /// 服务端 304，本地副本仍有效，只刷新了 fetched_at。
    NotModified,
    /// 网络全部失败（或校验失败），使用上一份可用快照——**数据可能过期**。
    LastKnownGood,
    /// 索引层曾校验失败但按降级策略回退到本地缓存副本（hash 不匹配等）。
    CacheFallback,
}

impl IndexSource {
    /// 是否属于"陈旧/降级"来源：前端必须给出可见标识，不允许"降级到看起来正常"。
    pub fn is_degraded(self) -> bool {
        matches!(self, Self::LastKnownGood | Self::CacheFallback)
    }
}

/// 索引层校验失败的分类（用于状态上报与日志，不用于向用户暴露内部细节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexFault {
    /// 全部镜像网络不可达。
    NetworkUnreachable,
    /// 摘要不匹配（疑似镜像污染）。
    DigestMismatch,
    /// 缺少 sha256 摘要（缺摘要的条目必须被拒绝）。
    DigestMissing,
    /// `index.json.sha256` 在多镜像间不一致（疑似单点污染）。
    AnchorDisagreement,
    /// minisign 验签失败（当前无公钥，不会产生该值）。
    SignatureFailed,
    /// `schema_version` 高于客户端支持值（硬拒绝）。
    SchemaUnsupported,
    /// 检测到元数据回退（防降级拦截）。
    RollbackDetected,
    /// `min_launcher` 高于当前启动器版本。
    LauncherTooOld,
}

impl IndexFault {
    /// 供前端映射文案的键（前端 i18n）。
    pub fn message_key(self) -> &'static str {
        match self {
            Self::NetworkUnreachable => "module.registry.fault.network",
            Self::DigestMismatch => "module.registry.fault.digest_mismatch",
            Self::DigestMissing => "module.registry.fault.digest_missing",
            Self::AnchorDisagreement => "module.registry.fault.anchor_disagreement",
            Self::SignatureFailed => "module.registry.fault.signature_failed",
            Self::SchemaUnsupported => "module.registry.fault.schema_unsupported",
            Self::RollbackDetected => "module.registry.fault.rollback_detected",
            Self::LauncherTooOld => "module.registry.fault.launcher_too_old",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_decides_zero_network_requests() {
        let mut meta = IndexMeta::default();
        // 无 fetched_at：不新鲜，需要网络请求。
        assert!(!meta.is_fresh(1_000_000, 3600));
        assert_eq!(meta.age_secs(1_000_000), None);
        assert!(!meta.has_conditional_headers());

        meta.fetched_at = 1_000_000;
        meta.etag = "\"abc\"".into();
        meta.last_modified = "Sun, 14 Sep 2026 03:12:40 GMT".into();
        assert!(meta.has_conditional_headers());

        // TTL 内：0 网络请求。
        assert!(meta.is_fresh(1_000_000, 3600));
        assert!(meta.is_fresh(1_003_599, 3600));
        // 到期与超时：需要条件请求。
        assert!(!meta.is_fresh(1_003_600, 3600));
        assert!(!meta.is_fresh(1_010_000, 3600));
        // ttl 为 0 视为不缓存。
        assert!(!meta.is_fresh(1_000_000, 0));
        // 时钟回拨不产生负 age。
        assert_eq!(meta.age_secs(999_000), Some(0));
    }

    #[test]
    fn index_meta_roundtrip_and_corruption_tolerance() {
        let dir = std::env::temp_dir().join(format!("cgl-meta-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.meta.json");

        let meta = IndexMeta {
            etag: "\"v1\"".into(),
            last_modified: "Sun, 14 Sep 2026 03:12:40 GMT".into(),
            fetched_at: 1_789_000_000,
            highest_generated_at: Some("2026-09-14T03:12:40Z".into()),
            schema_version: 1,
            repo_commit: "abc".into(),
            content_sha256: "d".repeat(64),
        };
        meta.save(&path).unwrap();
        let loaded = IndexMeta::load(&path);
        assert_eq!(loaded.etag, "\"v1\"");
        assert_eq!(loaded.fetched_at, 1_789_000_000);
        assert_eq!(loaded.repo_commit, "abc");
        assert!(loaded.has_conditional_headers());

        // 未知字段容忍。
        std::fs::write(&path, br#"{"etag":"\"v2\"","future_field":123}"#).unwrap();
        let tolerant = IndexMeta::load(&path);
        assert_eq!(tolerant.etag, "\"v2\"");
        assert_eq!(tolerant.fetched_at, 0);

        // 损坏文件退化为默认值（等价于无缓存），不 panic。
        std::fs::write(&path, b"{ broken").unwrap();
        let broken = IndexMeta::load(&path);
        assert_eq!(broken.etag, "");
        assert!(!broken.is_fresh(1, 3600));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn degraded_sources_are_flagged() {
        assert!(!IndexSource::Network.is_degraded());
        assert!(!IndexSource::Cache.is_degraded());
        assert!(!IndexSource::NotModified.is_degraded());
        assert!(IndexSource::LastKnownGood.is_degraded());
        assert!(IndexSource::CacheFallback.is_degraded());

        // 每种故障都有可映射到前端的键。
        for fault in [
            IndexFault::NetworkUnreachable,
            IndexFault::DigestMismatch,
            IndexFault::DigestMissing,
            IndexFault::AnchorDisagreement,
            IndexFault::SignatureFailed,
            IndexFault::SchemaUnsupported,
            IndexFault::RollbackDetected,
            IndexFault::LauncherTooOld,
        ] {
            assert!(fault.message_key().starts_with("module.registry.fault."));
        }
    }
}
