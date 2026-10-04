//! 内容下载模块 · 加载器（LeviLamina）目录与游戏版本匹配。
//!
//! 游戏下载模块在「版本详情页」要给出可选加载器，并在装完游戏实例后自动把选中的
//! LeviLamina 装进该实例目录。加载器版本清单来自 lipr 索引（与 LL 模组同源，见
//! [`super::lip`]），匹配规则是 **lip 语义化版本范围**。
//!
//! 匹配方向很重要：索引里声明的是「这个 LeviLamina 版本要求哪个 Minecraft/Bedrock
//! 版本」，而我们要回答的是「这个 MCBE 版本能不能装某个 LeviLamina」。因此实现是
//! `范围 ⊆ 游戏版本`（用游戏版本去满足范围），而不是反过来。
//!
//! 严格性：索引没为该版本声明平台依赖时**判定为不可用**（`compatible = false`），
//! 而不是乐观放行。徽标与下拉都据此显示——宁可少显示一个加载器，也不让用户选中一个
//! 装不上的版本，再在安装末期拿到一个来自 lipd 依赖求解器的晦涩报错。

use std::collections::HashMap;

use serde::Serialize;

use crate::error::KernelError;

use super::lip;

/// lipr 索引里 LeviLamina 的包标识。
pub const LEVILAMINA_IDENTIFIER: &str = "github.com/LiteLDev/LeviLamina";

/// lip 包引用基址（安装时下发的完整引用：`owner/repo#variant`）。
pub const LEVILAMINA_CLIENT_PACKAGE_REF: &str = "github.com/LiteLDev/LeviLamina#client";

/// LeviLamina 客户端 variant 键（游戏侧要装的就是它；`server` 是 BDS 侧）。
pub const CLIENT_VARIANT: &str = "client";

/// 平台依赖识别关键字（小写包含即认定）。
///
/// lip 的依赖键是包标识形态；MCBE / BDS 平台包一定带 `minecraft` 或 `bedrock`。
/// 用「包含」而不是写死一个标识：lip 生态里同一平台出现过多种写法
/// （`microsoft.minecraft.bedrock`、`mojang.minecraft.bedrock` 等），写死会漏。
const PLATFORM_NEEDLES: &[&str] = &["minecraft", "bedrock"];

/// 一个可选加载器版本（前端下拉项）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LoaderOption {
    /// 加载器版本号（如 `0.16.2`）。
    pub version: String,
    /// 是否与目标游戏版本匹配。
    pub compatible: bool,
    /// 该版本声明的平台依赖原文（`键 范围`）；未声明为 `None`。
    pub requirement: Option<String>,
}

/// 加载器目录：LeviLamina 客户端版本 + 各自的平台依赖范围。
#[derive(Debug, Clone, Default)]
pub struct LoaderCatalog {
    entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone)]
struct CatalogEntry {
    version: String,
    /// 原始依赖原文（键与范围），供界面展示与排障。
    requirement: Option<String>,
    /// 归一化后的范围表达式；未声明为 `None`。
    range: Option<String>,
}

impl LoaderCatalog {
    /// 从 lipr 索引拉取 LeviLamina 客户端版本清单（索引自带 TTL 缓存）。
    pub async fn load() -> Result<Self, KernelError> {
        let variants = lip::variant_versions(LEVILAMINA_IDENTIFIER).await?;
        let client = variants
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(CLIENT_VARIANT))
            .or_else(|| variants.first());
        let Some((_key, versions)) = client else {
            return Ok(Self::default());
        };

        let mut entries: Vec<CatalogEntry> = versions
            .iter()
            .map(|(version, deps)| {
                let platform = platform_requirement(deps);
                CatalogEntry {
                    version: version.clone(),
                    requirement: platform.as_ref().map(|(key, range)| format!("{key} {range}")),
                    range: platform.map(|(_, range)| range),
                }
            })
            .collect();
        // 新版本在前：下拉默认项就是最新，且与索引给出的顺序无关。
        entries.sort_by(|a, b| semver_cmp(&b.version, &a.version));
        Ok(Self { entries })
    }

    /// 该游戏版本是否至少有一个可用加载器（列表页徽标据此显示）。
    pub fn supports(&self, game_version: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.range.as_deref().is_some_and(|range| range_matches(range, game_version)))
    }

    /// 面向某游戏版本的加载器选项（新→旧，带逐项匹配结果）。
    pub fn options_for(&self, game_version: &str) -> Vec<LoaderOption> {
        self.entries
            .iter()
            .map(|entry| LoaderOption {
                version: entry.version.clone(),
                compatible: entry
                    .range
                    .as_deref()
                    .is_some_and(|range| range_matches(range, game_version)),
                requirement: entry.requirement.clone(),
            })
            .collect()
    }

    /// 目录是否为空（索引里没有 LeviLamina 条目）。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
impl LoaderCatalog {
    /// 单测构造：造一条带指定平台依赖的加载器版本。
    ///
    /// 索引拉取需要网络，而「按声明的平台依赖严格匹配」是纯逻辑，必须能离线验证。
    pub fn from_entries_for_test(version: &str, requirement: Option<&str>, range: &str) -> Self {
        Self {
            entries: vec![CatalogEntry {
                version: version.to_string(),
                requirement: requirement.map(str::to_string),
                range: Some(range.to_string()),
            }],
        }
    }
}

/// 从一个版本的依赖表里挑出平台依赖，返回 `(依赖键, 范围原文)`。
fn platform_requirement(deps: &HashMap<String, String>) -> Option<(String, String)> {
    let mut matched: Vec<(&String, &String)> = deps
        .iter()
        .filter(|(key, _)| {
            let lower = key.to_lowercase();
            PLATFORM_NEEDLES.iter().any(|needle| lower.contains(needle))
        })
        .collect();
    // 多个候选时取键名最短的：平台包标识通常比具体资产包更短，
    // 且 `HashMap` 迭代顺序不确定，不处理会让结果在两次调用间漂移。
    matched.sort_by_key(|(key, _)| key.len());
    matched
        .first()
        .map(|(key, range)| ((*key).clone(), (*range).clone()))
}

// ---------------------------------------------------------------- 版本范围匹配

/// 版本范围是否被给定版本满足。
///
/// 支持的写法（lip / semver 生态的常见子集）：
/// `*` / `x`（任意）、`1.21.130`（精确）、`=1.21.130`、`>=1.21.0`、`>1.21.0`、
/// `<=1.21.0`、`<1.21.0`、`^1.21.0`、`~1.21.0`、通配 `1.21.*` / `1.21.x`；
/// 空格或逗号分隔为「与」，`||` 分隔为「或」。
///
/// 语义细节：MCBE 版本是四段（`1.21.130.22`），而加载器声明的平台版本常是三段
/// （`1.21.130`）。精确与通配比较按**声明段数**做前缀匹配（三段 `1.21.130` 命中
/// `1.21.130.22`），比较类操作符则按数值比较、缺段补 0。
pub fn range_matches(range: &str, version: &str) -> bool {
    let Some(target) = parse_version(version) else {
        return false;
    };
    let range = range.trim();
    if range.is_empty() {
        return false;
    }
    range.split("||").any(|branch| {
        branch
            .split([',', ' '])
            .filter(|p| !p.trim().is_empty())
            .all(|part| comparator_matches(part.trim(), target))
    })
}

/// 单个比较子句（如 `>=1.21.0`）是否被满足。
fn comparator_matches(part: &str, target: Version) -> bool {
    if part.is_empty() {
        return true;
    }
    let (op, rest) = split_operator(part);
    let rest = rest.trim();
    // 任意版本：`*` / `x` / 空范围。
    if rest.is_empty() || rest == "*" || rest.eq_ignore_ascii_case("x") {
        return true;
    }
    // 通配段（`1.21.*`）：按下标逐段比较，遇到通配即只比前面数段。
    if let Some(prefix) = wildcard_prefix(rest) {
        return prefix_match(prefix, target);
    }
    let Some(bound) = parse_version(rest) else {
        // 范围写法不认识：判为不匹配（严格），而不是放行。
        return false;
    };
    match op {
        // 精确：按声明段数前缀匹配，兼容三段声明对四段游戏版本。
        "" | "=" => prefix_match(bound, target),
        ">" => target.gt(&bound),
        ">=" => target.ge(&bound),
        "<" => target.lt(&bound),
        "<=" => target.le(&bound),
        "^" => target.ge(&bound) && target.lt(&caret_upper(&bound)),
        "~" => target.ge(&bound) && target.lt(&tilde_upper(&bound)),
        _ => false,
    }
}

/// 拆出前导操作符。
fn split_operator(part: &str) -> (&str, &str) {
    for op in [">=", "<=", "^", "~", ">", "<", "="] {
        if let Some(rest) = part.strip_prefix(op) {
            return (op, rest);
        }
    }
    ("", part)
}

/// `^1.21.0` 的上界：首个非零段的下一档（与 semver caret 一致）。
fn caret_upper(bound: &Version) -> Version {
    let parts = bound.parts;
    let upper = if parts[0] > 0 {
        [parts[0] + 1, 0, 0, 0]
    } else if parts[1] > 0 {
        [0, parts[1] + 1, 0, 0]
    } else if parts[2] > 0 {
        [0, 0, parts[2] + 1, 0]
    } else {
        [0, 0, 0, parts[3] + 1]
    };
    Version { parts: upper, len: 4 }
}

/// `~1.21.0` 的上界：次段 +1。
fn tilde_upper(bound: &Version) -> Version {
    let parts = bound.parts;
    Version {
        parts: [parts[0], parts[1] + 1, 0, 0],
        len: 4,
    }
}

/// 通配写法（`1.21.*` / `1.21.x`）的前缀部分；非通配返回 `None`。
fn wildcard_prefix(rest: &str) -> Option<Version> {
    if !rest
        .split('.')
        .any(|seg| seg == "*" || seg.eq_ignore_ascii_case("x"))
    {
        return None;
    }
    let mut parts = [0u32; 4];
    let mut len = 0;
    for (index, segment) in rest.split('.').take(4).enumerate() {
        if segment == "*" || segment.eq_ignore_ascii_case("x") {
            break;
        }
        parts[index] = segment.trim().parse().ok()?;
        len = index + 1;
    }
    Some(Version { parts, len })
}

/// 按声明段数前缀匹配：`1.21.130` 命中 `1.21.130.22`。
///
/// 只能比 `bound.len` 段：把三段声明补成 `[1,21,130,0]` 后再整段比对，
/// 会让所有四段游戏版本（几乎全部正式版）都判为不匹配。
fn prefix_match(bound: Version, target: Version) -> bool {
    let len = bound.len.max(1);
    bound.parts[..len] == target.parts[..len]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Version {
    /// 四段数值；不足补 0。
    parts: [u32; 4],
    /// 实际声明的段数（1~4）。前缀比较按它决定比几段。
    len: usize,
}

impl Version {
    fn gt(&self, other: &Self) -> bool {
        self.parts > other.parts
    }
    fn ge(&self, other: &Self) -> bool {
        self.parts >= other.parts
    }
    fn lt(&self, other: &Self) -> bool {
        self.parts < other.parts
    }
    fn le(&self, other: &Self) -> bool {
        self.parts <= other.parts
    }
}

/// 解析版本串为四段数值（不足补 0，多于四段截断）；非法返回 `None`。
fn parse_version(raw: &str) -> Option<Version> {
    let cleaned = raw.trim().trim_start_matches('v');
    if cleaned.is_empty() {
        return None;
    }
    // 预发布后缀（`1.21.0-rc.1`）不参与比较。
    let core = cleaned.split(['-', '+']).next().unwrap_or(cleaned);
    let mut parts = [0u32; 4];
    let mut len = 0;
    for (index, segment) in core.split('.').take(4).enumerate() {
        let segment = segment.trim();
        if segment.is_empty() {
            return None;
        }
        parts[index] = segment.parse().ok()?;
        len = index + 1;
    }
    if len == 0 {
        return None;
    }
    Some(Version { parts, len })
}

/// 语义化版本比较（供目录排序，新版本在前）。
fn semver_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    match (parse_version(a), parse_version(b)) {
        (Some(x), Some(y)) => x.parts.cmp(&y.parts),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => a.cmp(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_requirement_matches_four_segment_game_version() {
        // 加载器声明三段、游戏版本四段：必须命中，否则所有正式版都会显示「不可用」。
        assert!(range_matches("1.21.130", "1.21.130.22"));
        assert!(range_matches("=1.21.130", "1.21.130.22"));
        assert!(!range_matches("1.21.130", "1.21.131.0"));
        assert!(range_matches("1.21.130.22", "1.21.130.22"));
        assert!(!range_matches("1.21.130.23", "1.21.130.22"));
        // 两段声明命中同一大版本的全部小版本。
        assert!(range_matches("1.21", "1.21.130.22"));
    }

    #[test]
    fn range_operators_behave() {
        assert!(range_matches(">=1.21.0", "1.21.130.22"));
        assert!(range_matches(">=1.21.0 <1.22.0", "1.21.130.22"));
        assert!(range_matches(">=1.21.0, <1.22.0", "1.21.130.22"));
        assert!(!range_matches(">=1.21.0 <1.22.0", "1.22.0.1"));
        assert!(range_matches(">1.21.100", "1.21.130.22"));
        assert!(!range_matches(">1.21.130.22", "1.21.130.22"));
        assert!(range_matches("<=1.21.130.22", "1.21.130.22"));
    }

    #[test]
    fn caret_and_tilde_follow_semver() {
        assert!(range_matches("^1.21.0", "1.21.130.22"));
        assert!(!range_matches("^1.21.0", "2.0.0"));
        assert!(range_matches("^0.16.2", "0.16.9"));
        assert!(!range_matches("^0.16.2", "0.17.0"));
        assert!(range_matches("~1.21.0", "1.21.9"));
        assert!(!range_matches("~1.21.0", "1.22.0"));
    }

    #[test]
    fn wildcards_and_disjunction() {
        assert!(range_matches("1.21.*", "1.21.130.22"));
        assert!(!range_matches("1.21.*", "1.22.0"));
        assert!(range_matches("x", "1.21.130.22"));
        assert!(range_matches("*", "1.21.130.22"));
        assert!(range_matches("1.20.0 || 1.21.130", "1.21.130.22"));
        // 不认识的写法必须判为不匹配（严格），否则会放行装上不的版本。
        assert!(!range_matches("latest", "1.21.130.22"));
        assert!(!range_matches("1.21.130", "not-a-version"));
    }

    #[test]
    fn platform_requirement_picks_bedrock_key() {
        let mut deps = HashMap::new();
        deps.insert("github.com/LiteLDev/LeviLamina".to_string(), ">=1.0.0".to_string());
        deps.insert("microsoft.minecraft.bedrock".to_string(), "1.21.130".to_string());
        let (key, range) = platform_requirement(&deps).expect("应识别出平台依赖");
        assert_eq!(key, "microsoft.minecraft.bedrock");
        assert_eq!(range, "1.21.130");

        let mut unrelated = HashMap::new();
        unrelated.insert("github.com/other/mod".to_string(), "1.0.0".to_string());
        assert!(platform_requirement(&unrelated).is_none());
    }

    #[test]
    fn catalog_requires_declared_platform_requirement() {
        let catalog = LoaderCatalog {
            entries: vec![
                CatalogEntry {
                    version: "0.16.2".into(),
                    requirement: Some("microsoft.minecraft.bedrock 1.21.130".into()),
                    range: Some("1.21.130".into()),
                },
                CatalogEntry {
                    version: "0.15.0".into(),
                    requirement: None,
                    range: None,
                },
            ],
        };
        assert!(catalog.supports("1.21.130.22"));
        assert!(!catalog.supports("1.21.131.0"));
        let options = catalog.options_for("1.21.130.22");
        assert!(options[0].compatible);
        // 未声明平台依赖的版本一律不可用（严格）。
        assert!(!options[1].compatible);
        assert!(options[1].requirement.is_none());
    }
}
