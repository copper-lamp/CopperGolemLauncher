//! 预加载清单 `copper-preload.json` 的解析（平台中立）。
//!
//! # 为什么不扫 `mods/`
//!
//! 让 native 代码猜目录结构等于把不确定性写死在注入侧。清单由启动器在
//! 启动前生成，注入侧只做「解析 + 路径逃逸校验 + 按序加载」，探测的不确定性
//! 全部收敛在可测试、可观测的 Rust 侧。
//!
//! 清单版本不认识时**整体拒绝**并记日志，而不是尽力解析。

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::contract::MANIFEST_SCHEMA_VERSION;

/// 清单条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreloadEntry {
    /// 相对版本目录的路径，使用 `/` 分隔（跨平台可读，启动器侧生成时归一）。
    pub path: String,
    /// 来源标记，例如 `levilamina@1.5.5` / `fallback` / `manual`。
    ///
    /// 只用于日志与 UI 展示，不参与加载决策 —— 但**必须存在**：来源不明
    /// 的 dll 被加载是排障噩梦，宁可让启动器在生成时显式填上。
    #[serde(default)]
    pub source: String,
}

/// 预加载清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreloadManifest {
    /// schema 版本，必须等于 [`MANIFEST_SCHEMA_VERSION`]。
    pub version: u32,
    /// 生成原因：`auto-detected` / `manual` / `external-preloader` 等。
    #[serde(default)]
    pub reason: String,
    /// 按加载顺序排列的条目。
    #[serde(default)]
    pub entries: Vec<PreloadEntry>,
}

/// 清单解析失败。
#[derive(Debug, PartialEq, Eq)]
pub enum ManifestError {
    /// JSON 语法错误（附 serde 消息）。
    Syntax(String),
    /// 字段类型不符（附 serde 消息）。
    Schema(String),
    /// schema 版本不认识。
    UnsupportedVersion(u32),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Syntax(message) => write!(f, "预加载清单 JSON 非法: {message}"),
            ManifestError::Schema(message) => write!(f, "预加载清单字段非法: {message}"),
            ManifestError::UnsupportedVersion(version) => write!(
                f,
                "预加载清单版本不受支持: {version}（本 DLL 只接受 {MANIFEST_SCHEMA_VERSION}）"
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

impl PreloadManifest {
    /// 由条目构造清单（供启动器侧与测试共用同一套默认值）。
    pub fn new(reason: impl Into<String>, entries: Vec<PreloadEntry>) -> Self {
        Self {
            version: MANIFEST_SCHEMA_VERSION,
            reason: reason.into(),
            entries,
        }
    }

    /// 序列化为紧凑 JSON（启动器侧落盘用）。
    pub fn to_json(&self) -> String {
        // 结构体字段全为确定类型，序列化不会失败。
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                "{{\"version\":{MANIFEST_SCHEMA_VERSION},\"reason\":\"serialize-failed\",\"entries\":[]}}"
            )
        })
    }

    /// 解析清单字节。
    ///
    /// 版本不符是**硬失败**：按错误的 schema 猜字段可能加载到不该加载的 dll，
    /// 而这类问题的现场（游戏崩溃 / 数据写错地方）比「什么都没加载」难查得多。
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        let raw: RawManifest = serde_json::from_slice(bytes)
            .map_err(|error| classify(error, bytes))?;
        if raw.version != MANIFEST_SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedVersion(raw.version));
        }
        Ok(PreloadManifest {
            version: raw.version,
            reason: raw.reason,
            entries: raw.entries,
        })
    }
}

/// 解析期的中间形态：字段类型与最终结构一致，避免二次转换。
#[derive(Deserialize)]
struct RawManifest {
    version: u32,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    entries: Vec<PreloadEntry>,
}

fn classify(error: serde_json::Error, bytes: &[u8]) -> ManifestError {
    // serde 对「结构不符」与「语法错」给出的 `classify` 足以区分；这里额外用
    // 首字节做兜底判定：非 `{` 开头一定是语法错（多半是空文件或半截写入）。
    let looks_like_object = bytes.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{');
    if !looks_like_object {
        return ManifestError::Syntax(error.to_string());
    }
    match error.classify() {
        serde_json::error::Category::Syntax | serde_json::error::Category::Eof => {
            ManifestError::Syntax(error.to_string())
        }
        _ => ManifestError::Schema(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_shape() {
        let json = br#"{"version":1,"reason":"auto-detected","entries":[
            {"path":"mods/LeviLamina/LeviLamina.dll","source":"levilamina@1.5.5"}]}"#;
        let manifest = PreloadManifest::parse(json).unwrap();
        assert_eq!(manifest.version, MANIFEST_SCHEMA_VERSION);
        assert_eq!(manifest.reason, "auto-detected");
        assert_eq!(manifest.entries.len(), 1);
        assert_eq!(manifest.entries[0].path, "mods/LeviLamina/LeviLamina.dll");
        assert_eq!(manifest.entries[0].source, "levilamina@1.5.5");
    }

    #[test]
    fn missing_reason_and_entries_default_to_empty() {
        let manifest = PreloadManifest::parse(br#"{"version":1}"#).unwrap();
        assert!(manifest.reason.is_empty());
        assert!(manifest.entries.is_empty());
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let error = PreloadManifest::parse(br#"{"version":2,"entries":[]}"#).unwrap_err();
        assert_eq!(error, ManifestError::UnsupportedVersion(2));
    }

    #[test]
    fn rejects_garbage_and_empty_file() {
        assert!(matches!(
            PreloadManifest::parse(b"").unwrap_err(),
            ManifestError::Syntax(_)
        ));
        assert!(matches!(
            PreloadManifest::parse(b"not json").unwrap_err(),
            ManifestError::Syntax(_)
        ));
    }

    #[test]
    fn rejects_entry_without_path() {
        let error = PreloadManifest::parse(br#"{"version":1,"entries":[{"source":"x"}]}"#).unwrap_err();
        assert!(matches!(error, ManifestError::Schema(_)));
    }

    #[test]
    fn round_trips_through_json() {
        let manifest = PreloadManifest::new(
            "manual",
            vec![PreloadEntry {
                path: "mods/a/a.dll".into(),
                source: "manual".into(),
            }],
        );
        let parsed = PreloadManifest::parse(manifest.to_json().as_bytes()).unwrap();
        assert_eq!(parsed, manifest);
    }
}
