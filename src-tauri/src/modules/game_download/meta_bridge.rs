//! 版本元数据桥：复用开始页 `home::meta` 约定落 `version.json`，使安装版本可被开始页识别。

use std::path::{Path, PathBuf};

use crate::error::KernelError;
use crate::modules::home::meta;

/// 判定版本目录已安装（存在 `version.json`；与开始页扫描口径一致）。
pub fn is_installed(dir: &Path) -> bool {
    meta::VersionMeta::read(dir).is_some()
}

/// 由 slug 还原数值版本号（快照版剥离 `_preview` 后缀）。
fn game_version_of(slug: &str) -> String {
    slug
        .strip_suffix("_preview")
        .unwrap_or(slug)
        .to_string()
}

/// 写版本元数据：`version.json`（兼容 LeviLauncher / 开始页 `VersionMeta`）。
///
/// `folder` 为版本目录名（= slug），`type` 取 `release` / `preview`。
pub fn write_meta(dir: &Path, folder: &str, slug: &str, kind: &str) -> Result<(), KernelError> {
    let meta = meta::VersionMeta {
        name: folder.to_string(),
        game_version: game_version_of(slug),
        version_type: kind.to_string(),
        enable_isolation: true,
        registered: true,
        created_at: meta::now_rfc3339(),
        ..Default::default()
    };
    meta::VersionMeta::write(dir, &meta)
}

/// 安装目录（版本根下）。
pub fn install_dir(versions_root: &Path, folder: &str) -> PathBuf {
    versions_root.join(folder)
}