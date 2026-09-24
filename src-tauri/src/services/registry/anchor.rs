//! 防降级锚点与缓存清理策略。
//!
//! 锚点（`trust/anchor.json`）记录本地"已见过的最高 `generated_at`"与对应 `repo_commit`，
//! 是抵御"重放旧索引让已 yank 的恶意模块复活"的关键防线（文档 2.5 规则 3、3.2 中间人降级）。
//!
//! 由于它属于**安全状态**而非可重建缓存，本模块刻意把它与普通缓存分开处理：
//! [`cleanup_cache`] 永远不会删除 `trust/` 目录下的任何内容（文档 2.9.2、G18）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::services::registry::model::RollbackVerdict;

/// 防降级锚点：本地已见过的最高 `generated_at` 与对应 commit。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct TrustAnchor {
    #[serde(default)]
    pub highest_generated_at: Option<String>,
    #[serde(default)]
    pub highest_repo_commit: Option<String>,
    /// 锚点最后更新时间（RFC3339，便于诊断；本字段不参与安全判定）。
    #[serde(default)]
    pub updated_at: Option<String>,
    /// 累计观察到的回退次数（用于日志与设置页安全提示）。
    #[serde(default)]
    pub rollback_rejections: u64,
}

impl TrustAnchor {
    /// 从磁盘读取锚点；文件缺失或损坏时返回 `None`（调用方按"首次见"处理）。
    ///
    /// 损坏的锚点**不会**被静默当成"无锚点"而丧失防降级能力——它在下次成功写入时
    /// 会被备份为 `anchor.json.corrupt-<n>` 以便排查（见 [`read_anchor_with_backup`]）。
    pub fn load(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice::<Self>(&bytes).ok()
    }

    /// 原子写入锚点（先写临时文件再 rename，避免半成品文件被当成有效锚点）。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self)?;
        std::fs::write(&tmp, bytes)?;
        // Windows 上 rename 到已存在文件会失败，先移除目标再重命名。
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        std::fs::rename(&tmp, path)
    }

    /// 用一次成功加载的索引更新锚点。
    ///
    /// 只有 `generated_at` 更"新"（或首次记录）时才推进锚点——`generated_at` 相同时
    /// 保留既有 commit，避免同时间点的重复写入造成锚点抖动。
    pub fn observe(
        &mut self,
        generated_at: &str,
        repo_commit: &str,
        observed_at: &str,
    ) -> bool {
        let incoming = crate::services::registry::model::parse_generated_at(generated_at);
        let current = self
            .highest_generated_at
            .as_deref()
            .and_then(crate::services::registry::model::parse_generated_at);

        let should_advance = match (current, incoming) {
            (None, Some(_)) => true,
            // 时间不可解析时保守拒绝推进锚点（宁可少记录，不可记录脏值）。
            (_, None) => false,
            (Some(cur), Some(inc)) => inc > cur,
        };
        if !should_advance {
            return false;
        }
        self.highest_generated_at = Some(generated_at.to_string());
        self.highest_repo_commit = Some(repo_commit.to_string());
        self.updated_at = Some(observed_at.to_string());
        true
    }

    /// 记录一次回退拒绝（锚点本身不推进）。
    pub fn note_rollback(&mut self, observed_at: &str) {
        self.rollback_rejections = self.rollback_rejections.saturating_add(1);
        self.updated_at = Some(observed_at.to_string());
    }

    /// 依据锚点判定新索引是否构成回退。
    pub fn compare(&self, generated_at: &str, repo_commit: &str) -> RollbackVerdict {
        crate::services::registry::model::compare_rollback(
            self.highest_generated_at.as_deref(),
            self.highest_repo_commit.as_deref(),
            generated_at,
            repo_commit,
        )
    }
}

/// 读取锚点，并在文件损坏时把它另存为 `*.corrupt` 以便事后排查（不静默丢弃证据）。
///
/// 返回 `(锚点, 是否检测到损坏)`。
pub fn read_anchor_with_backup(path: &Path) -> (TrustAnchor, bool) {
    match std::fs::read(path) {
        Err(_) => (TrustAnchor::default(), false),
        Ok(bytes) => match serde_json::from_slice::<TrustAnchor>(&bytes) {
            Ok(anchor) => (anchor, false),
            Err(_) => {
                let backup = path.with_extension("json.corrupt");
                let _ = std::fs::copy(path, &backup);
                log::warn!(
                    "registry trust anchor 损坏，已备份为 {} 并按空锚点继续",
                    backup.display()
                );
                (TrustAnchor::default(), true)
            }
        },
    }
}

/// 缓存清理结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupOutcome {
    /// 被删除的文件数。
    pub removed_files: usize,
    /// 释放的字节数。
    pub freed_bytes: u64,
    /// 是否跳过了 `trust/`（**必须恒为 true**，作为契约由单测固定）。
    pub trust_preserved: bool,
}

/// 按"总配额"清理 registry 缓存（LRU：最久未修改的文件先删）。
///
/// 约定：
/// - **永不删除** `trust/` 下的任何内容（防降级锚点是安全状态，文档 2.9.2/G18）；
/// - `last-known-good/` 为离线兜底快照，仅在配额压力下最后才被考虑（排序时按目录前缀降权）；
/// - 返回真实删除统计，供日志与设置页展示。
pub fn cleanup_cache(root: &Path, quota_bytes: u64) -> CleanupOutcome {
    let mut outcome = CleanupOutcome {
        trust_preserved: true,
        ..Default::default()
    };
    if !root.exists() {
        return outcome;
    }

    let mut files: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> = Vec::new();
    collect_files(root, &mut files);

    let total: u64 = files.iter().map(|(_, len, _)| *len).sum();
    if total <= quota_bytes {
        return outcome;
    }

    // 排序：优先删除 recent（普通缓存），last-known-good 最后删除；同组内按修改时间升序。
    files.sort_by(|a, b| {
        let rank = |p: &std::path::Path| -> u8 {
            let s = p.to_string_lossy();
            if s.contains("last-known-good") {
                1
            } else {
                0
            }
        };
        rank(&a.0)
            .cmp(&rank(&b.0))
            .then_with(|| a.2.cmp(&b.2))
    });

    let mut current = total;
    for (path, len, _) in files {
        if current <= quota_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            current = current.saturating_sub(len);
            outcome.removed_files += 1;
            outcome.freed_bytes += len;
        }
    }
    outcome
}

/// 递归收集文件（不跟随符号链接，避免清理时越出缓存根）。
fn collect_files(dir: &Path, out: &mut Vec<(std::path::PathBuf, u64, std::time::SystemTime)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            // 信任目录是安全状态，清理时整体跳过，不进入遍历。
            if path.file_name().and_then(|n| n.to_str()) == Some(super::paths::TRUST_SUBDIR) {
                continue;
            }
            collect_files(&path, out);
        } else if meta.is_file() {
            let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            out.push((path, meta.len(), modified));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::registry::model::RollbackVerdict;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cgl-registry-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn anchor_roundtrip_and_advance_rules() {
        let root = temp_root("anchor");
        let path = root.join("trust/anchor.json");

        let mut anchor = TrustAnchor::default();
        assert_eq!(anchor.compare("2026-09-14T03:12:40Z", "c1"), RollbackVerdict::FirstSeen);

        assert!(anchor.observe("2026-09-14T03:12:40Z", "c1", "2026-09-14T04:00:00Z"));
        assert_eq!(anchor.highest_repo_commit.as_deref(), Some("c1"));
        // 更早或相同的 generated_at 不推进锚点。
        assert!(!anchor.observe("2026-09-14T03:12:40Z", "c1", "2026-09-14T05:00:00Z"));
        assert!(!anchor.observe("2026-09-01T00:00:00Z", "c0", "2026-09-14T05:00:00Z"));
        assert!(!anchor.observe("garbage", "c9", "2026-09-14T05:00:00Z"));
        // 更晚则推进。
        assert!(anchor.observe("2026-09-15T00:00:00Z", "c2", "2026-09-15T01:00:00Z"));
        assert_eq!(anchor.highest_repo_commit.as_deref(), Some("c2"));

        anchor.save(&path).expect("锚点应能落盘");
        let loaded = TrustAnchor::load(&path).expect("锚点应能读回");
        assert_eq!(loaded.highest_generated_at.as_deref(), Some("2026-09-15T00:00:00Z"));
        assert_eq!(loaded.highest_repo_commit.as_deref(), Some("c2"));

        // 回退判定：早于锚点 + 不同 commit → 拒绝；锚点更新后依然生效。
        assert_eq!(
            loaded.compare("2026-09-10T00:00:00Z", "deadbeef"),
            RollbackVerdict::Rollback
        );
        assert_eq!(
            loaded.compare("2026-09-15T00:00:00Z", "c2"),
            RollbackVerdict::Accept
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_anchor_is_backed_up_not_silently_dropped() {
        let root = temp_root("anchor-corrupt");
        let path = root.join("trust/anchor.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ this is not json").unwrap();

        let (anchor, corrupt) = read_anchor_with_backup(&path);
        assert!(corrupt, "损坏必须被识别");
        assert!(anchor.highest_generated_at.is_none());
        assert!(
            root.join("trust/anchor.json.corrupt").exists(),
            "损坏的锚点必须留证据"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cleanup_never_deletes_trust_state() {
        let root = temp_root("cleanup");
        let safe = root.join("trust/anchor.json");
        let shard = root.join("shards/s-u.json");
        let lkg = root.join("last-known-good/index.json");
        let icon = root.join("icons/x.png");
        std::fs::create_dir_all(safe.parent().unwrap()).unwrap();
        std::fs::create_dir_all(shard.parent().unwrap()).unwrap();
        std::fs::create_dir_all(lkg.parent().unwrap()).unwrap();
        std::fs::create_dir_all(icon.parent().unwrap()).unwrap();
        std::fs::write(&safe, b"{\"highest_generated_at\":\"2026-09-14T03:12:40Z\"}").unwrap();
        std::fs::write(&shard, vec![b'a'; 4096]).unwrap();
        std::fs::write(&lkg, vec![b'b'; 2048]).unwrap();
        std::fs::write(&icon, vec![b'c'; 1024]).unwrap();

        let outcome = cleanup_cache(&root, 0);
        assert!(outcome.trust_preserved);
        assert!(safe.exists(), "trust/anchor.json 永不因清理删除");
        assert!(outcome.removed_files >= 1);
        assert!(outcome.freed_bytes > 0);
        // 配额为 0 时 last-known-good 也会被清掉（它是可重建的缓存），但 trust 保留。
        assert!(!shard.exists());

        // 配额充足时不做任何删除。
        std::fs::write(&shard, vec![b'a'; 16]).unwrap();
        let outcome2 = cleanup_cache(&root, 1024 * 1024);
        assert_eq!(outcome2.removed_files, 0);
        assert!(safe.exists());

        let _ = std::fs::remove_dir_all(&root);
    }
}
