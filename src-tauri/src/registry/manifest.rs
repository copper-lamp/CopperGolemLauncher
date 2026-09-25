//! 附加模块包内 `module.json` 的反序列化与校验（内核侧契约）。
//!
//! 附加模块以 `.cglm`（zip 容器）分发，容器根含一份 `module.json`。本文件是
//! 该清单在内核侧的 Rust 表达，字段与模板仓库 `CopperModles/module.schema.json`
//! 逐字对齐（snake_case，不改名）：清单是跨仓库契约，改名只会制造第二套命名。
//!
//! 分工：本文件只负责「读得出来 + 语义合规」；是否装载由
//! [`crate::registry::loader`] 决策，加载后端由 [`crate::registry::dylib_backend`] 提供。
//!
//! 与 `registry/modules.rs` 的关系：模块 **id** 形态（两段式 `author.module`）
//! 复用同一套校验（[`crate::registry::modules::is_valid_module_id`]），避免两处
//! 判定漂移导致「清单合法但目录名被拒」这类不一致。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::KernelError;
use crate::registry::modules::is_valid_module_id;
use crate::services::registry::model;

/// 清单格式版本（当前内核支持值）。高于此值必须拒绝，不做尽力解析。
pub const SUPPORTED_SCHEMA_VERSION: &str = "1";

/// 当前内核支持的模块 API 版本。
pub const SUPPORTED_API_VERSION: u32 = 1;

/// `platforms` 权威枚举（与 cgl-libs / 模板 schema 逐字一致）。
pub const SUPPORTED_PLATFORMS: &[&str] = &[
    "windows-x86_64",
    "windows-aarch64",
    "android-arm64",
    "linux-x86_64",
];

/// 模块清单（`module.json` 的唯一内核侧表达）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleManifest {
    pub schema_version: String,
    /// 全局唯一模块标识（两段式 `author.module`）。
    pub id: String,
    /// i18n 命名空间（单段）。语言包与 `t()` 键一律用它，**不用 `id`**。
    pub i18n_namespace: String,
    pub display_name: String,
    pub description: String,
    /// 本地化覆盖：`locale -> { display_name, description }`。
    #[serde(default)]
    pub i18n: BTreeMap<String, LocalizedText>,
    pub author: Author,
    /// SPDX 许可标识。
    pub license: String,
    /// 模块版本（semver）。
    pub version: String,
    /// 适配平台（权威枚举）。
    pub platforms: Vec<String>,
    /// 内核兼容区间。
    pub launcher: LauncherRange,
    /// 模块 API 版本。
    pub api_version: u32,
    /// 后端产物声明。
    pub backend: BackendSpec,
    /// 前端产物声明。
    pub frontend: FrontendSpec,
    /// 权限声明（授权上界，可为空）。注意枚举取值见 [`declared_permissions`] 的说明。
    #[serde(default)]
    pub permissions: Vec<String>,
    /// 图标相对路径。
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub donation: Option<String>,
    #[serde(default)]
    pub changelog: Option<String>,
}

/// 单个 locale 的本地化文案。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalizedText {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// 作者信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Author {
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// 内核兼容区间。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LauncherRange {
    pub min: String,
    #[serde(default)]
    pub max: Option<String>,
}

/// 后端产物声明。
///
/// 清单字段名为 `crate`（Rust 关键字），故用 `#[serde(rename)]` 映射到
/// `crate_name`——`rename` 只改序列化名、不改标识符，不会与关键字冲突。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackendSpec {
    #[serde(rename = "crate")]
    pub crate_name: String,
    /// 模块类型全路径（如 `copper_module_demo::DemoModule`）。
    pub entry: String,
    /// 打包时匹配后端产物的 glob。
    pub artifact_glob: String,
}

/// 前端产物声明。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrontendSpec {
    pub dist: String,
    pub register: String,
}

impl BackendSpec {
    /// 产物文件名主干（去掉文件名中最后一个扩展名）。
    ///
    /// 用于在当前平台上定位动态库：清单的 `artifact_glob` 通常声明 Windows 的
    /// `.dll`，而同一份源码在 Linux/macOS 上产出 `.so`/`.dylib`。按**主干**而不是
    /// 完整文件名匹配，才能跨平台命中同一模块的产物。
    pub fn artifact_stem(&self) -> Option<String> {
        let base = self
            .artifact_glob
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(self.artifact_glob.as_str());
        let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
        if stem.is_empty() {
            None
        } else {
            Some(stem.to_string())
        }
    }
}

impl ModuleManifest {
    /// 解析清单字节（不做语义校验）。
    pub fn parse(raw: &[u8]) -> Result<Self, KernelError> {
        serde_json::from_slice(raw).map_err(|e| {
            KernelError::Module(format!(
                "模块清单解析失败（第 {} 行第 {} 列）: {e}",
                e.line(),
                e.column()
            ))
        })
    }

    /// 解析并校验。
    pub fn parse_and_validate(raw: &[u8]) -> Result<Self, KernelError> {
        let manifest = Self::parse(raw)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// 语义校验：枚举、区间、跨字段一致性。
    ///
    /// 每条规则都给出「字段 + 实际值 + 期望」，因为模块作者拿到的是启动日志里的
    /// 一行文本，说不出「哪个字段、错在哪」就等于没报错。
    pub fn validate(&self) -> Result<(), KernelError> {
        if self.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(KernelError::Module(format!(
                "schema_version 必须为 `{SUPPORTED_SCHEMA_VERSION}`，实际为 `{}`",
                self.schema_version
            )));
        }

        // id 形态与注册表目录名共用同一套校验：两处判定必须一致，
        // 否则会出现「清单合法但 resolve_module_dir 拒绝」的矛盾。
        if !is_valid_module_id(&self.id) {
            return Err(KernelError::Module(format!(
                "id `{}` 不合规：必须是两段式 `author.module`，两段均只含小写字母、数字与连字符",
                self.id
            )));
        }

        if !is_i18n_namespace(&self.i18n_namespace) {
            return Err(KernelError::Module(format!(
                "i18n_namespace `{}` 不合规：必须是单段、以字母开头、仅含小写字母/数字/连字符、长度 3~32 且不含点号",
                self.i18n_namespace
            )));
        }

        // id 第二段必须等于命名空间：一旦漂移，前端拼键与后端注册会指向两个
        // 命名空间，表现为文案整体回退键名（历史同类 bug 的高发点）。
        if let Some((_, tail)) = self.id.rsplit_once('.') {
            if tail != self.i18n_namespace {
                return Err(KernelError::Module(format!(
                    "id 的第二段（`{tail}`）必须与 i18n_namespace（`{}`）一致",
                    self.i18n_namespace
                )));
            }
        }

        if self.display_name.trim().is_empty() {
            return Err(KernelError::Module("display_name 不能为空".into()));
        }

        // 版本必须是可解析 semver：装载器的「更高版本胜出」与区域区间判定都依赖它。
        if semver::Version::parse(&self.version).is_err() {
            return Err(KernelError::Module(format!(
                "version `{}` 不是 MAJOR.MINOR.PATCH 形式的 semver",
                self.version
            )));
        }

        if self.api_version != SUPPORTED_API_VERSION {
            return Err(KernelError::Module(format!(
                "api_version 必须为 {SUPPORTED_API_VERSION}，实际为 {}",
                self.api_version
            )));
        }

        if self.platforms.is_empty() {
            return Err(KernelError::Module("platforms 不能为空".into()));
        }
        for platform in &self.platforms {
            if !SUPPORTED_PLATFORMS.contains(&platform.as_str()) {
                return Err(KernelError::Module(format!(
                    "platforms 含未知取值 `{platform}`，合法取值：{}",
                    SUPPORTED_PLATFORMS.join(" / ")
                )));
            }
        }

        if self.backend.crate_name.trim().is_empty() {
            return Err(KernelError::Module("backend.crate 不能为空".into()));
        }
        if self.backend.entry.trim().is_empty() {
            return Err(KernelError::Module("backend.entry 不能为空".into()));
        }

        Ok(())
    }

    /// 清单是否声明支持当前运行平台。
    pub fn supports_current_platform(&self) -> bool {
        match model::Platform::current() {
            Some(current) => {
                let current = current.as_str();
                self.platforms.iter().any(|p| p == current)
            }
            // 平台无法识别时不据此拒绝（交由动态库加载阶段的扩展名匹配兜底）。
            None => true,
        }
    }

    /// 当前内核版本是否落在清单声明的兼容区间内。
    pub fn accepts_launcher(&self, launcher_version: &str) -> bool {
        model::launcher_range_covers(
            launcher_version,
            &self.launcher.min,
            self.launcher.max.as_deref(),
        )
        .unwrap_or(false)
    }
}

/// i18n 命名空间：单段、以字母开头、仅含小写字母/数字/连字符、长度 3~32。
fn is_i18n_namespace(ns: &str) -> bool {
    if ns.len() < 3 || ns.len() > 32 {
        return false;
    }
    if ns.contains('.') {
        return false;
    }
    let mut chars = ns.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    ns.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模板仓库的真实清单（与 CopperModles/module.json 同形）。
    const SAMPLE: &str = r#"{
      "schema_version": "1",
      "id": "copper-lamp.demo-tools",
      "i18n_namespace": "demo-tools",
      "display_name": "示例工具",
      "description": "示例模块",
      "author": { "name": "copper-lamp", "url": "https://github.com/copper-lamp" },
      "license": "MIT",
      "version": "0.1.0",
      "platforms": ["windows-x86_64", "android-arm64", "linux-x86_64"],
      "launcher": { "min": "0.1.0", "max": null },
      "api_version": 1,
      "backend": {
        "crate": "copper-module-demo",
        "entry": "copper_module_demo::DemoModule",
        "artifact_glob": "target/release/copper_module_demo.dll"
      },
      "frontend": { "dist": "frontend/dist", "register": "register.js" },
      "permissions": [],
      "icon": "assets/icon.svg",
      "category": "utility"
    }"#;

    #[test]
    fn sample_manifest_parses_and_validates() {
        let m = ModuleManifest::parse_and_validate(SAMPLE.as_bytes()).expect("模板清单必须合法");
        assert_eq!(m.id, "copper-lamp.demo-tools");
        assert_eq!(m.i18n_namespace, "demo-tools");
        assert_eq!(m.backend.crate_name, "copper-module-demo");
        // `crate` 关键字映射到 crate_name，且能回写同名。
        let round = serde_json::to_string(&m.backend).unwrap();
        assert!(round.contains("\"crate\":\"copper-module-demo\""), "got: {round}");
    }

    #[test]
    fn artifact_stem_is_platform_independent() {
        let spec = BackendSpec {
            crate_name: "c".into(),
            entry: "e".into(),
            artifact_glob: "target/release/copper_module_demo.dll".into(),
        };
        assert_eq!(spec.artifact_stem().as_deref(), Some("copper_module_demo"));

        // 反斜杠路径（Windows 风格）同样按主干解析。
        let win = BackendSpec {
            artifact_glob: "target\\release\\foo.so".into(),
            ..spec
        };
        assert_eq!(win.artifact_stem().as_deref(), Some("foo"));
    }

    #[test]
    fn validation_rejects_bad_fields() {
        let base: ModuleManifest = ModuleManifest::parse(SAMPLE.as_bytes()).unwrap();

        let mut m = base.clone();
        m.schema_version = "2".into();
        assert!(m.validate().is_err());

        let mut m = base.clone();
        m.id = "single".into();
        assert!(m.validate().is_err(), "单段 id 必须拒绝");

        let mut m = base.clone();
        m.i18n_namespace = "renamed".into();
        assert!(m.validate().is_err(), "命名空间与 id 第二段不一致必须拒绝");

        let mut m = base.clone();
        m.version = "1.0".into();
        assert!(m.validate().is_err(), "非三段 semver 必须拒绝");

        let mut m = base.clone();
        m.api_version = 2;
        assert!(m.validate().is_err());

        let mut m = base.clone();
        m.platforms = vec!["windows-x64".into()];
        assert!(m.validate().is_err(), "未知平台必须拒绝");

        let mut m = base.clone();
        m.platforms = vec![];
        assert!(m.validate().is_err());

        let mut m = base.clone();
        m.backend.entry = "  ".into();
        assert!(m.validate().is_err());

        // 合法基准不得误判。
        assert!(base.validate().is_ok());
    }

    #[test]
    fn i18n_namespace_rules_are_explicit() {
        assert!(is_i18n_namespace("demo-tools"));
        assert!(is_i18n_namespace("abc"));
        assert!(!is_i18n_namespace("ab")); // 过短
        assert!(!is_i18n_namespace("Demo")); // 含大写
        assert!(!is_i18n_namespace("1demo")); // 数字开头
        assert!(!is_i18n_namespace("a.b")); // 含点号
        assert!(!is_i18n_namespace(&"a".repeat(33))); // 过长
    }

    #[test]
    fn platform_support_matches_current_target() {
        let base: ModuleManifest = ModuleManifest::parse(SAMPLE.as_bytes()).unwrap();
        // 样例声明了 windows-x86_64；当前平台是否命中取决于构建目标，
        // 这里只断言判定函数与 model::Platform::current() 口径一致。
        match model::Platform::current() {
            Some(p) => assert_eq!(base.supports_current_platform(), base.platforms.iter().any(|x| x == p.as_str())),
            None => assert!(base.supports_current_platform()),
        }
    }

    #[test]
    fn launcher_range_is_enforced() {
        let base: ModuleManifest = ModuleManifest::parse(SAMPLE.as_bytes()).unwrap();
        assert!(base.accepts_launcher("0.1.0"));
        assert!(base.accepts_launcher("9.9.9"));
        assert!(!base.accepts_launcher("0.0.1"));

        let mut bounded = base.clone();
        bounded.launcher.max = Some("0.5.0".into());
        assert!(bounded.accepts_launcher("0.4.0"));
        assert!(!bounded.accepts_launcher("0.6.0"));
    }
}
