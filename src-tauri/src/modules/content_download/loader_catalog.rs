//! 内容下载模块 · 加载器（LeviLamina）目录。
//!
//! 游戏下载模块在「版本二级页」要给出该游戏版本可用的加载器，并在装完游戏实例后自动
//! 把选中的 LeviLamina 装进去。可用清单的事实源是 LeviLauncher 使用的
//! `levilamina-client-version-db`：
//!
//! ```json
//! { "format_version": 1,
//!   "versions": { "1.26.10.04": ["26.10.14", "26.10.13", ...],
//!                 "1.21.132.01": ["1.9.9", "1.9.8", ...] } }
//! ```
//!
//! **键就是 MCBE 版本**，值是支持该版本的全部 LeviLamina 版本。这与本模块最初尝试的
//! 「从 lipr 索引读 LeviLamina 自述的平台依赖」完全不同：lipr 索引里 LeviLamina 条目
//! 并不声明 Minecraft 平台依赖，于是 1.26 及以上的版本一律判为「不可用」，而实际
//! 1.26.10.04 / 1.26.20.04 / … 都有对应加载器。改用这份专门的版本库后，可用性判定与
//! LeviLauncher 完全一致（见 `libs/LeviLauncher/internal/mcservice/levilamina.go`）。
//!
//! 匹配规则：先做**精确键匹配**（正常路径，两侧都是零填充四段版本号，如 `1.26.10.04`），
//! 失败再按「逐段数值前缀」兜底（兼容将来某一侧省掉零填充或末段的情形，如 `1.26.10.4`
//! 命中 `1.26.10.04`）。兜底是数值比较而不是字符串前缀：字符串前缀会让 `1.26.1` 命中
//! `1.26.10.04`。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::error::KernelError;

/// lip 包引用基址（安装时下发的完整引用：`owner/repo#variant`）。
pub const LEVILAMINA_CLIENT_PACKAGE_REF: &str = "github.com/LiteLDev/LeviLamina#client";

/// 版本库镜像，按下标顺序尝试；前两个已实测可达（2026-10-04）。
///
/// 顺序沿用清单源的同一套经验：jsdelivr 的两个域名国内可达；github 直连被墙、代理镜像
/// 经常 302 后不给内容，因此垫底而不是删掉——它们只是慢，不是永远不可达。
const VERSION_DB_URLS: [&str; 4] = [
    "https://fastly.jsdelivr.net/gh/LiteLDev/levilamina-client-version-db@main/v2/version-db.json",
    "https://cdn.jsdelivr.net/gh/LiteLDev/levilamina-client-version-db@main/v2/version-db.json",
    "https://github.bibk.top/LiteLDev/levilamina-client-version-db/raw/refs/heads/main/v2/version-db.json",
    "https://raw.githubusercontent.com/LiteLDev/levilamina-client-version-db/refs/heads/main/v2/version-db.json",
];

/// 目录缓存 TTL：清单命令每次都会问目录，不缓存等于每次刷新都多一次网络往返。
const CACHE_TTL: Duration = Duration::from_secs(1800);

type SharedCache = Mutex<Option<(Instant, LoaderCatalog)>>;

fn cache() -> &'static SharedCache {
    static CACHE: OnceLock<SharedCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// 一个可选加载器版本（前端下拉项）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct LoaderOption {
    /// 加载器版本号（如 `26.10.14`）。
    pub version: String,
    /// 是否与该游戏版本匹配。当前实现只返回匹配项，字段保留给未来的宽松模式。
    pub compatible: bool,
}

/// 加载器目录：MCBE 版本 → 支持的 LeviLamina 版本（降序）。
#[derive(Debug, Clone, Default)]
pub struct LoaderCatalog {
    versions: HashMap<String, Vec<String>>,
}

impl LoaderCatalog {
    /// 拉取加载器版本库（带 TTL 缓存；刷新失败时回退陈旧缓存）。
    pub async fn load() -> Result<Self, KernelError> {
        if let Some(cached) = read_cache() {
            return Ok(cached);
        }
        match fetch().await {
            Ok(catalog) => {
                write_cache(&catalog);
                Ok(catalog)
            }
            Err(error) => {
                // 拉取失败但手里有旧目录（上一次成功的结果）时继续用它：
                // 「可装加载器」只是徽标与下拉，宁可显示上一次的事实，也不要整块消失。
                if let Some(stale) = read_stale_cache() {
                    log::warn!("[content-download] 加载器版本库刷新失败，用旧目录: {error}");
                    return Ok(stale);
                }
                Err(error)
            }
        }
    }

    /// 该游戏版本是否至少有一个可用加载器（列表页徽标据此显示）。
    pub fn supports(&self, game_version: &str) -> bool {
        !self.versions_for(game_version).is_empty()
    }

    /// 面向某游戏版本的加载器选项（新→旧）。
    pub fn options_for(&self, game_version: &str) -> Vec<LoaderOption> {
        self.versions_for(game_version)
            .iter()
            .map(|version| LoaderOption {
                version: version.clone(),
                // 目录给出的就是「支持这个 MCBE 版本」的加载器，因此恒为可用。
                compatible: true,
            })
            .collect()
    }

    /// 目录是否为空（版本库里没有任何条目）。
    pub fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }

    /// 目录里是否至少有一条非空记录（单测用）。
    #[cfg(test)]
    fn supports_any(&self) -> bool {
        self.versions.values().any(|v| !v.is_empty())
    }

    /// 某 MCBE 版本的加载器版本列表：先精确键，再逐段数值前缀兜底。
    fn versions_for(&self, game_version: &str) -> &[String] {
        let key = game_version.trim();
        if let Some(exact) = self.versions.get(key) {
            return exact;
        }
        let Some(target) = parse_segments(key) else {
            return &[];
        };
        let mut best: Option<(&String, usize)> = None;
        for candidate in self.versions.keys() {
            let Some(segments) = parse_segments(candidate) else {
                continue;
            };
            let shared = segments.len().min(target.len());
            if shared == 0 || segments[..shared] != target[..shared] {
                continue;
            }
            // 多段命中优先（`1.26.10` 比 `1.26` 更明确）。
            let better = match best {
                Some((_, best_shared)) => shared > best_shared,
                None => true,
            };
            if better {
                best = Some((candidate, shared));
            }
        }
        best.and_then(|(candidate, _)| self.versions.get(candidate))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// 单测构造：直接给一份「MCBE 版本 → 加载器版本」表。
    #[cfg(test)]
    pub fn from_map_for_test(pairs: &[(&str, &[&str])]) -> Self {
        Self {
            versions: pairs
                .iter()
                .map(|(mc, list)| {
                    (
                        (*mc).to_string(),
                        list.iter().map(|v| (*v).to_string()).collect(),
                    )
                })
                .collect(),
        }
    }
}

// ---------------------------------------------------------------- 缓存

/// 命中且未过期时返回缓存。
fn read_cache() -> Option<LoaderCatalog> {
    let guard = cache().lock().ok()?;
    let (stamp, catalog) = guard.as_ref()?;
    (stamp.elapsed() < CACHE_TTL).then(|| catalog.clone())
}

/// 无论是否过期都返回缓存（刷新失败时的回退）。
fn read_stale_cache() -> Option<LoaderCatalog> {
    let guard = cache().lock().ok()?;
    guard.as_ref().map(|(_, catalog)| catalog.clone())
}

fn write_cache(catalog: &LoaderCatalog) {
    if let Ok(mut guard) = cache().lock() {
        *guard = Some((Instant::now(), catalog.clone()));
    }
}

// ---------------------------------------------------------------- 拉取与解析

/// 多镜像拉取并解析版本库；全部镜像失败才返回错误。
async fn fetch() -> Result<LoaderCatalog, KernelError> {
    let client = crate::services::http_client::client_builder(Duration::from_secs(15)).build()?;
    let mut last_err: Option<KernelError> = None;
    for url in VERSION_DB_URLS {
        match client.get(url).send().await {
            Ok(response) if response.status().is_success() => match response.json::<Value>().await {
                Ok(json) => return Ok(parse(&json)),
                Err(error) => {
                    log::warn!("[content-download] 加载器版本库解析失败 {url}: {error}");
                    last_err = Some(error.into());
                }
            },
            Ok(response) => {
                log::warn!(
                    "[content-download] 加载器版本库 HTTP {} ({url})",
                    response.status()
                );
                last_err = Some(KernelError::Config(format!(
                    "加载器版本库 HTTP {}",
                    response.status()
                )));
            }
            Err(error) => {
                log::warn!("[content-download] 加载器版本库请求失败 {url}: {error}");
                last_err = Some(error.into());
            }
        }
    }
    Err(last_err.unwrap_or_else(|| KernelError::Config("加载器版本库拉取失败".into())))
}

/// 解析版本库 JSON。缺字段容错，返回空目录而不是报错（空目录 = 无可用加载器）。
fn parse(json: &Value) -> LoaderCatalog {
    let mut versions: HashMap<String, Vec<String>> = HashMap::new();
    let Some(map) = json.get("versions").and_then(Value::as_object) else {
        return LoaderCatalog::default();
    };
    for (mc_version, list) in map {
        let Some(list) = list.as_array() else {
            continue;
        };
        let mut loader_versions: Vec<String> = list
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .collect();
        if loader_versions.is_empty() {
            continue;
        }
        // 去重 + 新→旧：上游列表未必有序，而下拉的第一项就是用户最容易直接确认的那个。
        loader_versions.sort_by(|a, b| compare_loader_versions(b, a));
        loader_versions.dedup();
        versions.insert(mc_version.trim().to_string(), loader_versions);
    }
    LoaderCatalog { versions }
}

/// 解析版本号为数值段；非法返回 `None`。
fn parse_segments(raw: &str) -> Option<Vec<u32>> {
    let cleaned = raw.trim().trim_start_matches('v');
    if cleaned.is_empty() {
        return None;
    }
    let core = cleaned.split(['-', '+']).next().unwrap_or(cleaned);
    let segments: Option<Vec<u32>> = core
        .split('.')
        .map(|segment| segment.trim().parse::<u32>().ok())
        .collect();
    let segments = segments?;
    (!segments.is_empty()).then_some(segments)
}

/// 加载器版本比较：先比数值段，再比预发布标记（正式版 > 预发布，`rc.2` > `rc.1`）。
fn compare_loader_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let (a_core, a_pre) = split_prerelease(a);
    let (b_core, b_pre) = split_prerelease(b);
    match (parse_segments(a_core), parse_segments(b_core)) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| match (a_pre, b_pre) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(x), Some(y)) => x.cmp(y),
        }),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => a.cmp(b),
    }
}

/// 拆出预发布后缀：`1.8.0-rc.1` → (`1.8.0`, `Some("rc.1")`)。
fn split_prerelease(raw: &str) -> (&str, Option<&str>) {
    let trimmed = raw.trim();
    match trimmed.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (trimmed, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实版本库的节选（2026-10-04 拉取）：1.26 系与 1.21 系各留一条。
    const SAMPLE: &str = r#"{
      "format_version": 1,
      "versions": {
        "1.21.124.02": ["1.8.0-rc.1", "1.8.0-rc.2"],
        "1.21.132.01": ["1.9.7", "1.9.8", "1.9.9", "1.9.10"],
        "1.26.10.04": ["26.10.3", "26.10.14", "26.10.0"],
        "1.26.51.01": ["26.51.6", "26.51.0"]
      }
    }"#;

    fn catalog() -> LoaderCatalog {
        parse(&serde_json::from_str::<Value>(SAMPLE).unwrap())
    }

    /// 1.26 及以上的版本必须能匹配到加载器（此前用 lipr 索引平台依赖判定时全部落空）。
    #[test]
    fn matches_newer_game_versions() {
        let catalog = catalog();
        assert!(catalog.supports("1.26.10.04"));
        assert!(catalog.supports("1.26.51.01"));
        assert!(catalog.supports("1.21.132.01"));
        // 版本库里没有的构建：不能凭空说可以装。
        assert!(!catalog.supports("1.26.52.03"));
        assert!(!catalog.supports("1.21.130.22"));
    }

    /// 选项按加载器版本新→旧排列，第一项就是最新（用户最容易直接确认的那个）。
    #[test]
    fn options_are_newest_first() {
        let catalog = catalog();
        let options = catalog.options_for("1.26.10.04");
        let versions: Vec<&str> = options.iter().map(|o| o.version.as_str()).collect();
        assert_eq!(versions, ["26.10.14", "26.10.3", "26.10.0"]);
        assert!(options.iter().all(|o| o.compatible));
        // 数值段比较而非字典序：1.9.10 必须排在 1.9.9 前面。
        let older = catalog.options_for("1.21.132.01");
        assert_eq!(older[0].version, "1.9.10");
    }

    /// 预发布版排在同数值的正式版之后，且 rc.2 在 rc.1 之前。
    #[test]
    fn prerelease_ordering() {
        let catalog = catalog();
        let versions: Vec<String> = catalog
            .options_for("1.21.124.02")
            .into_iter()
            .map(|o| o.version)
            .collect();
        assert_eq!(versions, ["1.8.0-rc.2", "1.8.0-rc.1"]);
    }

    /// 键的零填充差异走数值前缀兜底，而不是字符串前缀（`1.26.1` 不得命中 `1.26.10.04`）。
    #[test]
    fn segment_fallback_is_numeric() {
        let catalog = LoaderCatalog::from_map_for_test(&[("1.26.10.04", &["26.10.14"])]);
        assert!(catalog.supports("1.26.10.4"));
        assert!(catalog.supports("1.26.10.04"));
        assert!(!catalog.supports("1.26.1"));
        // 反过来（三段查询对四段键）是**预期**命中：上游某天改格式时不至于全灭。
        assert!(catalog.supports("1.26.10"));
        // 三段键对四段游戏版本：同前缀即命中（上游某天改格式时不至于全灭）。
        let short = LoaderCatalog::from_map_for_test(&[("1.26.10", &["26.10.14"])]);
        assert!(short.supports("1.26.10.04"));
        assert!(!short.supports("1.26.11.01"));
    }

    /// 多段命中优先：`1.26.10` 比 `1.26` 更明确。
    #[test]
    fn most_specific_key_wins() {
        let catalog =
            LoaderCatalog::from_map_for_test(&[("1.26", &["generic"]), ("1.26.10", &["specific"])]);
        let versions: Vec<String> = catalog
            .options_for("1.26.10.04")
            .into_iter()
            .map(|o| o.version)
            .collect();
        assert_eq!(versions, ["specific"]);
    }

    /// 缺字段 / 脏数据不能变成「可用」。
    #[test]
    fn malformed_input_yields_empty_catalog() {
        assert!(parse(&serde_json::json!({})).is_empty());
        assert!(parse(&serde_json::json!({ "versions": [] })).is_empty());
        assert!(!parse(&serde_json::json!({ "versions": { "1.26.10.04": [] } })).supports_any());
        // 非字符串项被丢弃，剩下的仍然可用。
        let mixed = parse(&serde_json::json!({
            "versions": { "1.26.10.04": [1, null, "26.10.14", " "] }
        }));
        assert_eq!(mixed.options_for("1.26.10.04").len(), 1);
    }
}
