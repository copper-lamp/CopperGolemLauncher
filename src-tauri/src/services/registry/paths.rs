//! 元数据缓存布局：在 `paths.cache_dir()` 下规划 `registry/` 各子目录与文件位置。
//!
//! 布局依据 [cgl-libs](../../../docs/cgl-libs.md) 2.9.2：
//!
//! ```text
//! <cache_dir>/registry/
//!   index.json                 顶层索引副本
//!   index.meta.json            etag / last_modified / fetched_at / highest_generated_at
//!   shards/<range>.json        分片副本
//!   last-known-good/           上一次校验通过的完整快照（含 index.json 与分片）
//!   trust/anchor.json          防降级锚点（highest_generated_at、repo_commit）——安全状态，永不清理
//!   assets/<id>-<version>.cglm 安装包暂存（安装成功后删除）
//!   icons/<id>.png             图标缓存
//! ```
//!
//! `trust/anchor.json` 与普通缓存分离存放且**不因缓存清理而删除**：它是防降级的安全状态，
//! 与可重建的缓存性质不同（文档 2.9.2 末段、G18）。

use std::path::{Path, PathBuf};

use crate::services::paths::Paths;

/// `registry` 缓存子目录名。
pub const REGISTRY_SUBDIR: &str = "registry";
/// 顶层索引文件名。
pub const INDEX_FILE: &str = "index.json";
/// 索引元信息文件名。
pub const INDEX_META_FILE: &str = "index.meta.json";
/// 分片子目录名。
pub const SHARDS_SUBDIR: &str = "shards";
/// 上次可用快照子目录名。
pub const LAST_KNOWN_GOOD_SUBDIR: &str = "last-known-good";
/// 信任状态子目录名（永不随缓存清理删除）。
pub const TRUST_SUBDIR: &str = "trust";
/// 防降级锚点文件名。
pub const ANCHOR_FILE: &str = "anchor.json";
/// 安装包暂存子目录名。
pub const ASSETS_SUBDIR: &str = "assets";
/// 图标缓存子目录名。
pub const ICONS_SUBDIR: &str = "icons";

/// 元数据缓存根目录（`<cache_dir>/registry`）。
#[derive(Debug, Clone)]
pub struct RegistryPaths {
    root: PathBuf,
}

impl RegistryPaths {
    /// 基于内核路径体系构造。
    pub fn new(paths: &Paths) -> Self {
        Self {
            root: paths.cache_dir().join(REGISTRY_SUBDIR),
        }
    }

    /// 基于任意缓存根目录构造（测试与独立复用时使用）。
    pub fn from_cache_dir(cache_dir: impl AsRef<Path>) -> Self {
        Self {
            root: cache_dir.as_ref().join(REGISTRY_SUBDIR),
        }
    }

    /// 缓存根。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `index.json` 本地副本。
    pub fn index_file(&self) -> PathBuf {
        self.root.join(INDEX_FILE)
    }

    /// `index.meta.json`（ETag / Last-Modified / fetched_at / highest_generated_at）。
    pub fn index_meta_file(&self) -> PathBuf {
        self.root.join(INDEX_META_FILE)
    }

    /// 分片副本目录。
    pub fn shards_dir(&self) -> PathBuf {
        self.root.join(SHARDS_SUBDIR)
    }

    /// 某具体分片的本地副本路径（`<range>.json`）。
    pub fn shard_file(&self, range: &str) -> PathBuf {
        self.shards_dir().join(format!("{}.json", sanitize_file_stem(range)))
    }

    /// 上次可用（last-known-good）快照目录。
    pub fn last_known_good_dir(&self) -> PathBuf {
        self.root.join(LAST_KNOWN_GOOD_SUBDIR)
    }

    /// last-known-good 下的索引副本。
    pub fn last_known_good_index(&self) -> PathBuf {
        self.last_known_good_dir().join(INDEX_FILE)
    }

    /// last-known-good 下的分片目录。
    pub fn last_known_good_shards_dir(&self) -> PathBuf {
        self.last_known_good_dir().join(SHARDS_SUBDIR)
    }

    /// 信任状态目录（安全状态，缓存清理时必须跳过）。
    pub fn trust_dir(&self) -> PathBuf {
        self.root.join(TRUST_SUBDIR)
    }

    /// 防降级锚点文件。
    pub fn anchor_file(&self) -> PathBuf {
        self.trust_dir().join(ANCHOR_FILE)
    }

    /// 安装包暂存目录。
    pub fn assets_dir(&self) -> PathBuf {
        self.root.join(ASSETS_SUBDIR)
    }

    /// 安装包暂存文件（`<id>-<version>.cglm`）。
    pub fn asset_file(&self, module_id: &str, version: &str) -> PathBuf {
        self.assets_dir().join(format!(
            "{}-{}.cglm",
            sanitize_file_stem(module_id),
            sanitize_file_stem(version)
        ))
    }

    /// 图标缓存目录。
    pub fn icons_dir(&self) -> PathBuf {
        self.root.join(ICONS_SUBDIR)
    }

    /// 图标缓存文件（`<id>.png`）。
    pub fn icon_file(&self, module_id: &str) -> PathBuf {
        self.icons_dir().join(format!("{}.png", sanitize_file_stem(module_id)))
    }

    /// 创建全部业务目录（幂等）。清理由 [`crate::services::registry::cleanup_cache`] 负责。
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for dir in [
            self.root.clone(),
            self.shards_dir(),
            self.last_known_good_dir(),
            self.last_known_good_shards_dir(),
            // 注意：trust 目录必须存在且永不被清理——锚点缺失等于防降级失效。
            self.trust_dir(),
            self.assets_dir(),
            self.icons_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

/// 把任意远端字符串（分片 range、模块 id、版本号）转成安全的文件名词干。
///
/// 拒绝路径分隔符与穿越写法：这些值来自远端元数据，直接拼进本地路径会成为
/// 目录穿越的入口（文档 3.2 本地提权风险）。
pub fn sanitize_file_stem(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '+' => c,
            _ => '_',
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').to_string();
    if cleaned.is_empty() {
        "_".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_layout_matches_spec() {
        let p = RegistryPaths::from_cache_dir("C:/cache");
        let root = p.root().to_string_lossy().replace('\\', "/");
        assert!(root.ends_with("/cache/registry"));

        // 统一分隔符再比较：Windows 下 PathBuf 用 `\`，直接 ends_with 会失败。
        assert!(p
            .index_file()
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("registry/index.json"));
        assert!(p
            .index_meta_file()
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("registry/index.meta.json"));
        assert!(p
            .shard_file("s-u")
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("registry/shards/s-u.json"));
        assert!(p
            .last_known_good_dir()
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("registry/last-known-good"));
        assert!(p
            .anchor_file()
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("registry/trust/anchor.json"));
        assert!(p
            .asset_file("copper-lamp.server-manager", "0.4.2")
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("registry/assets/copper-lamp.server-manager-0.4.2.cglm"));
        assert!(p
            .icon_file("copper-lamp.server-manager")
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("registry/icons/copper-lamp.server-manager.png"));
    }

    #[test]
    fn file_stem_sanitizer_blocks_path_traversal() {
        assert_eq!(sanitize_file_stem("s-u"), "s-u");
        assert_eq!(sanitize_file_stem("copper-lamp.server-manager"), "copper-lamp.server-manager");
        assert_eq!(sanitize_file_stem("0.4.2"), "0.4.2");
        assert_eq!(sanitize_file_stem("../../etc/passwd"), "_.._etc_passwd");
        assert!(!sanitize_file_stem("../../evil").contains('/'));
        assert!(!sanitize_file_stem("a\\b").contains('\\'));
        assert_eq!(sanitize_file_stem(""), "_");
        assert_eq!(sanitize_file_stem("  "), "_");
        assert_eq!(sanitize_file_stem("..."), "_");

        let p = RegistryPaths::from_cache_dir("C:/cache");
        let escaped = p.shard_file("../../../evil");
        let parent = p.shards_dir().parent().unwrap().to_path_buf();
        assert!(
            escaped.starts_with(&parent),
            "净化后的分片路径不得逃出 registry 根目录: {escaped:?}"
        );
    }
}
