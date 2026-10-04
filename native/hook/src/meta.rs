//! `version.json` 回退解析（平台中立）。
//!
//! # 什么时候走回退
//!
//! 正常路径是启动器在 spawn 时下发四个环境变量。但游戏也可能**不是**本启动器
//! 起的：用户手工双击 `Minecraft.Windows.exe`、从资源管理器打开实例目录、
//! 或者实例目录被***接管过。这时环境变量不存在，隔离若直接失效，
//! 用户会看到「同一个实例，有的一天写进实例目录，有的一天写进 `%APPDATA%`」——
//! 比一直不隔离更难排查。因此回退读同目录 `version.json`
//! （兼容****目录，`version_config.cpp:62` 同款做法）。
//!
//! # 字段名与内核 `home::meta::VersionMeta` 保持一致（camelCase）

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::contract::VERSION_META_FILE;
use crate::paths::{plan_for, Channel, RedirectPlan};

/// 回退所需的最小元数据子集。
///
/// 只声明用得到的字段：`serde` 默认忽略未知字段，因此内核写出的其余字段
/// （`launchArgs` / `envVars` / `loader` …）不会导致解析失败。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct VersionMetaFallback {
    #[serde(rename = "type", default)]
    pub version_type: String,
    #[serde(rename = "gameVersion", default)]
    pub game_version: String,
    /// 缺失时按**开启**处理。
    ///
    /// 而是「不知道」。而隔离默认开启是产品决策（docs §1.2），所以
    /// 「不知道」必须落到「开」，否则漏字段的实例会静默共享系统目录。
    #[serde(rename = "enableIsolation", default = "default_true")]
    pub enable_isolation: bool,
}

fn default_true() -> bool {
    true
}

/// 由回退元数据构造重定向方案。
///
/// `version.json` 不存在或不可解析时返回 `None` —— 调用方据此「不装 hook」，
/// 而不是「按猜测装」。
pub fn plan_from_meta(root: &Path, meta: &VersionMetaFallback) -> Option<RedirectPlan> {
    if !meta.enable_isolation {
        return None;
    }
    Some(plan_for(
        root,
        Channel::parse(&meta.version_type),
        &meta.game_version,
    ))
}

/// 读取 `<root>/version.json` 并构造方案。
pub fn plan_from_file(root: &Path) -> Result<Option<RedirectPlan>, String> {
    let path: PathBuf = root.join(VERSION_META_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        // 文件不存在不是错误：非本启动器管理的实例就该走原样路径。
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("读取 {} 失败: {error}", path.display())),
    };
    let meta: VersionMetaFallback =
        serde_json::from_slice(&bytes).map_err(|error| format!("解析 {} 失败: {error}", path.display()))?;
    Ok(plan_from_meta(root, &meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> &'static Path {
        Path::new("/v/demo")
    }

    #[test]
    fn missing_isolation_field_defaults_to_enabled() {
        let meta: VersionMetaFallback = serde_json::from_slice(br#"{"gameVersion":"1.21.130.20"}"#).unwrap();
        assert!(meta.enable_isolation);
        let plan = plan_from_meta(root(), &meta).unwrap();
        assert_eq!(plan.root, root());
    }

    #[test]
    fn explicit_disable_stops_redirect() {
        let meta: VersionMetaFallback =
            serde_json::from_slice(br#"{"gameVersion":"1.21.130.20","enableIsolation":false}"#).unwrap();
        assert!(plan_from_meta(root(), &meta).is_none());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let json = br#"{"name":"demo","type":"preview","gameVersion":"1.21.130.20",
            "launchArgs":"-x","loader":"levilamina","enableConsole":true,
            "registered":false,"createdAt":"2026-01-01"}"#;
        let meta: VersionMetaFallback = serde_json::from_slice(json).unwrap();
        let plan = plan_from_meta(root(), &meta).unwrap();
        assert_eq!(
            plan.data,
            Path::new("/v/demo/Minecraft Bedrock Preview"),
            "预览渠道的数据目录名"
        );
    }

    #[test]
    fn flat_layout_for_1_26() {
        let meta: VersionMetaFallback = serde_json::from_slice(br#"{"type":"release","gameVersion":"1.26.45.1"}"#).unwrap();
        let plan = plan_from_meta(root(), &meta).unwrap();
        assert_eq!(plan.data, root());
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let dir = std::env::temp_dir().join(format!("copper_hook_meta_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(plan_from_file(&dir).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_plan_from_file() {
        let dir = std::env::temp_dir().join(format!("copper_hook_meta2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(VERSION_META_FILE),
            br#"{"type":"release","gameVersion":"1.21.130.20","enableIsolation":true}"#,
        )
        .unwrap();
        let plan = plan_from_file(&dir).unwrap().unwrap();
        assert_eq!(plan.root, dir);
        assert!(plan.temp.ends_with("temp"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_file_reports_error_instead_of_guessing() {
        let dir = std::env::temp_dir().join(format!("copper_hook_meta3_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(VERSION_META_FILE), b"{ not json").unwrap();
        assert!(plan_from_file(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
