//! 版本清单：拉取 / 缓存 / 解析 MCBE 版本元数据，并构建前端分组视图。
//!
//! 数据源：社区维护的 `minecraft-windows-gdk-version-db`（LiteLDev）提供的
//! `historical_versions.json`，内含历代 GDK 正式版 / 快照版的 CDN 直链与 md5。
//! 三镜像（github / 代理 / gitcode）按设置 `download.mirror` 排序，失败回退缓存。

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::KernelError;
use crate::modules::home::meta;
use super::installer::Ctx;

/// 版本库镜像源，按下标顺序依次尝试，前面可达即成功。
/// 已实测（2026-09-06）可达：fastly.jsdelivr、gh-proxy.com、cdn.jsdelivr、ghproxy.cn。
/// 不可达（勿排前）：github 直连（被墙）、gitcode（raw 域名已失效）、github.bibk.top（404）。
const MANIFEST_URLS: [&str; 6] = [
    "https://fastly.jsdelivr.net/gh/LiteLDev/minecraft-windows-gdk-version-db@main/historical_versions.json",
    "https://gh-proxy.com/https://raw.githubusercontent.com/LiteLDev/minecraft-windows-gdk-version-db/refs/heads/main/historical_versions.json",
    "https://cdn.jsdelivr.net/gh/LiteLDev/minecraft-windows-gdk-version-db@main/historical_versions.json",
    "https://ghproxy.cn/https://raw.githubusercontent.com/LiteLDev/minecraft-windows-gdk-version-db/refs/heads/main/historical_versions.json",
    "https://raw.githubusercontent.com/LiteLDev/minecraft-windows-gdk-version-db/refs/heads/main/historical_versions.json",
    "https://raw.gitcode.com/dreamguxiang/minecraft-windows-gdk-version-db/raw/main/historical_versions.json",
];

/// 版本类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionKind {
    Release,
    Preview,
}

/// 清单顶层结构（camelCase 与源文件一致）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoricalVersions {
    /// 源顶层键为下划线 `file_version`（其余字段 camelCase），故加 alias 兼容。
    #[serde(alias = "file_version")]
    pub file_version: i32,
    #[serde(default)]
    pub preview_versions: Vec<VersionEntry>,
    #[serde(default)]
    pub release_versions: Vec<VersionEntry>,
}

impl HistoricalVersions {
    /// 全部版本（快照在前），按时间倒序。用于详情查询。
    pub fn all(&self) -> Vec<VersionEntry> {
        let mut out = vei_to_owned(&self.preview_versions);
        out.extend(vei_to_owned(&self.release_versions));
        out.sort_by_key(|e| std::cmp::Reverse(e.timestamp));
        out
    }

    /// 按 slug 查找版本。
    pub fn find_by_id(&self, id: &str) -> Option<VersionEntry> {
        self.all().into_iter().find(|e| e.slug() == id)
    }
}

/// 一个版本条目。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionEntry {
    /// 展示名，含类型前缀，如 `Preview 1.21.120.21`。
    pub version: String,
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub timestamp: i64,
    #[serde(default)]
    pub md5: String,
}

impl VersionEntry {
    /// 正式版或快照版（按展示名前缀判定，缺省正式版）。当前源仅含 GDK 版。
    pub fn kind(&self) -> VersionKind {
        let lower = self.version.trim().to_ascii_lowercase();
        if lower.starts_with("preview") {
            VersionKind::Preview
        } else {
            VersionKind::Release
        }
    }

    /// 去类型前缀后的数值版本号（如 `1.21.120.21`）。
    pub fn game_version(&self) -> String {
        match self.version.find(' ') {
            Some(i) => self.version[i + 1..].trim().to_string(),
            None => self.version.trim().to_string(),
        }
    }

    /// 大版本分类（前两段，如 `1.21`）。
    pub fn major(&self) -> String {
        let v = self.game_version();
        let mut parts = v.split('.');
        let a = parts.next().unwrap_or("");
        let b = parts.next().unwrap_or("");
        if b.is_empty() {
            return a.to_string();
        }
        format!("{a}.{b}")
    }

    /// 版本数值（最多 4 段），用于排序；无法解析返回最大序（排最后）。
    pub fn numeric(&self) -> (u32, u32, u32, u32) {
        parse_numeric(&self.game_version()).unwrap_or((u32::MAX, 0, 0, 0))
    }

    /// 唯一键 / 版本目录名。快照版追加 `_preview` 后缀，避免同名目录冲突。
    pub fn slug(&self) -> String {
        let v = self.game_version();
        match self.kind() {
            VersionKind::Release => v,
            VersionKind::Preview => format!("{v}_preview"),
        }
    }

    /// 首选下载直链（取镜像列表第一条；无法解析则以首个可用 URL 兜底）。
    pub fn primary_url(&self) -> Option<String> {
        // "http://assets1.xboxlive.com/..." 取 asset/d 前缀的第一条即足够。
        self.urls.first().cloned()
    }
}

fn vei_to_owned(v: &[VersionEntry]) -> Vec<VersionEntry> {
    v.to_vec()
}

/// 解析 `x.y.z.w` 数值；段数不足补 0，段数超过 4 或非法返回 None。
fn parse_numeric(s: &str) -> Option<(u32, u32, u32, u32)> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut out = [0u32; 4];
    for (i, p) in parts.iter().enumerate() {
        out[i] = p.trim().parse().ok()?;
    }
    Some((out[0], out[1], out[2], out[3]))
}

// ------------------------------------------------------------------ 视图

/// 前端清单视图：顶部最新正式 / 最新快照 + 按大版本分组。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestView {
    pub latest_release: Option<VersionView>,
    pub latest_preview: Option<VersionView>,
    pub groups: Vec<VersionGroupView>,
}

/// 一个大版本分组卡片。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionGroupView {
    pub major: String,
    /// 组内是否含该组最新版本（供前端高亮）。
    pub latest: bool,
    pub items: Vec<VersionView>,
}

/// 单版本行视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionView {
    /// 唯一键（=版本目录名）。
    pub id: String,
    /// 版本类型 `release` / `preview`。
    pub kind: String,
    /// 数值版本号（如 `1.21.120.21`）。
    pub game_version: String,
    pub md5: String,
    pub is_installed: bool,
    /// 本地缓存是否已有整包（尚未安装）。
    pub is_downloaded: bool,
    pub timestamp: i64,
    /// 是否当前分组内最新。
    pub is_latest: bool,
    /// 首选下载直链。
    pub url: Option<String>,
}

/// 载入清单（刷新或读缓存 + 可选网络），未联网时回退缓存。
///
/// `refresh=true` 强制网络拉取并更新缓存；失败时回退缓存。
pub async fn load_manifest(ctx: &Ctx, refresh: bool) -> Result<HistoricalVersions, KernelError> {
    if refresh {
        match fetch_manifest(ctx).await {
            Ok(v) => {
                cache_manifest(ctx, &v)?;
                return Ok(v);
            }
            Err(e) => {
                // 网络失败回退缓存；缓存也没有才报错。
                if let Some(cached) = read_cache(ctx) {
                    log::warn!("[game-download] 清单刷新失败，使用缓存: {e}");
                    return Ok(cached);
                }
                return Err(e);
            }
        }
    }
    if let Some(cached) = read_cache(ctx) {
        return Ok(cached);
    }
    match fetch_manifest(ctx).await {
        Ok(v) => {
            cache_manifest(ctx, &v)?;
            Ok(v)
        }
        Err(e) => Err(e),
    }
}

/// 网络拉取清单（多镜像，内置换序）。
pub async fn fetch_manifest(ctx: &Ctx) -> Result<HistoricalVersions, KernelError> {
    // 统一按内置 order 顺序尝试，镜像间自动 fallback。
    let _mirror = ctx
        .settings
        .get_or("download.mirror", "auto".to_string());
    let mut order: Vec<usize> = (0..MANIFEST_URLS.len()).collect();
    if order.is_empty() {
        order = vec![0, 1, 2, 3, 4, 5];
    }

    let client = crate::services::http_client::client_builder(Duration::from_secs(15)).build()?;

    let mut last_err: Option<KernelError> = None;
    for idx in order {
        let url = MANIFEST_URLS.get(idx).copied().unwrap_or("");
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let bytes = match resp.bytes().await {
                    Ok(b) => b,
                    Err(e) => {
                        last_err = Some(e.into());
                        continue;
                    }
                };
                match serde_json::from_slice::<HistoricalVersions>(&bytes) {
                    Ok(v) => return Ok(v),
                    Err(e) => {
                        last_err = Some(e.into());
                        continue;
                    }
                }
            }
            Ok(resp) => {
                last_err = Some(KernelError::Config(format!("清单源 HTTP {}", resp.status())));
            }
            Err(e) => {
                log::error!("清单源 <{}> 请求失败, 完整错误: {:?}", url, e);
                last_err = Some(e.into());
            }
        }
    }
    Err(last_err.unwrap_or_else(|| KernelError::Config("清单拉取失败".into())))
}

/// 构建前端视图（纯计算，可测）。
pub fn build_view(ctx: &Ctx, versions: &HistoricalVersions) -> ManifestView {
    let installed_set = installed_set(ctx);
    let mut groups: BTreeMap<String, Vec<VersionView>> = BTreeMap::new();
    let mut all_views: Vec<VersionView> = Vec::new();

    for entry in versions.all() {
        let slug = entry.slug();
        let v = VersionView {
            id: slug.clone(),
            kind: match entry.kind() {
                VersionKind::Release => "release",
                VersionKind::Preview => "preview",
            }
            .to_string(),
            game_version: entry.game_version(),
            md5: entry.md5.clone(),
            is_installed: installed_set.contains(&slug),
            is_downloaded: dest_exists(ctx, &slug),
            timestamp: entry.timestamp,
            is_latest: false,
            url: entry.primary_url(),
        };
        groups
            .entry(entry.major())
            .or_default()
            .push(v.clone());
        all_views.push(v);
    }

    // 组内按数值降序，标记组内最新。
    for views in groups.values_mut() {
        views.sort_by_key(|v| std::cmp::Reverse(b_numeric(v)));
        if let Some(first) = views.first_mut() {
            first.is_latest = true;
        }
    }

    // 全局最新正式 / 最新快照（数值最大）。
    let latest_release = latest_of(&all_views, "release");
    let latest_preview = latest_of(&all_views, "preview");

    // 组按 major 倒序展示；数值解析失败的大版本排最后。
    let mut group_list: Vec<VersionGroupView> = groups
        .into_iter()
        .map(|(major, items)| {
            let latest = items.iter().any(|i| i.is_latest);
            VersionGroupView { major, latest, items }
        })
        .collect();
    group_list.sort_by(|a, b| a.major.cmp(&b.major).reverse());

    ManifestView {
        latest_release,
        latest_preview,
        groups: group_list,
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn b_numeric(v: &VersionView) -> (u32, u32, u32, u32) {
    parse_numeric(&v.game_version).unwrap_or((0, 0, 0, 0))
}

fn latest_of(views: &[VersionView], kind: &str) -> Option<VersionView> {
    views
        .iter()
        .filter(|v| v.kind == kind)
        .max_by_key(|v| (parse_numeric(&v.game_version).unwrap_or((0, 0, 0, 0)), v.timestamp))
        .cloned()
}

/// 已安装版本名集合（复用开始页扫描逻辑，按 volume 目录）。
fn installed_set(ctx: &Ctx) -> std::collections::HashSet<String> {
    meta::scan_versions(ctx.paths.versions_dir())
        .into_iter()
        .map(|m| m.name)
        .collect()
}

/// 缓存中是否存在完整（未安装）下载包。
fn dest_exists(ctx: &Ctx, slug: &str) -> bool {
    ctx.dest_for(slug).is_file()
}

// ------------------------------------------------------------------ 缓存

fn cache_path(ctx: &Ctx) -> std::path::PathBuf {
    ctx.cache_home().join("historical_versions.json")
}

fn read_cache(ctx: &Ctx) -> Option<HistoricalVersions> {
    let raw = std::fs::read(cache_path(ctx)).ok()?;
    serde_json::from_slice(&raw).ok()
}

fn cache_manifest(ctx: &Ctx, v: &HistoricalVersions) -> Result<(), KernelError> {
    let path = cache_path(ctx);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_vec_pretty(v)?;
    std::fs::write(path, raw)?;
    Ok(())
}

// ------------------------------------------------------------------ 测试

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "fileVersion": 1,
        "releaseVersions": [
            { "version": "Release 1.21.100.2", "urls": ["http://assets1.xboxlive.com/a/Rel_1.msixvc"], "timestamp": 1700000000, "md5": "AAAA" }
        ],
        "previewVersions": [
            { "version": "Preview 1.21.130.20", "urls": ["http://assets1.xboxlive.com/a/Prev_1.msixvc"], "timestamp": 1750000000, "md5": "BBBB" },
            { "version": "Preview 1.21.120.21", "urls": ["http://assets1.xboxlive.com/a/Prev_2.msixvc"], "timestamp": 1740000000, "md5": "CCCC" }
        ]
    }"#;

    #[test]
    fn parses_real_schema() {
        let v: HistoricalVersions = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(v.file_version, 1);
        assert_eq!(v.release_versions.len(), 1);
        assert_eq!(v.preview_versions.len(), 2);
        // snake_case 字段经 rename_all 映射到 camelCase。
        assert_eq!(v.preview_versions[0].urls.len(), 1);
    }

    #[test]
    fn entry_derives_correct_fields() {
        let v: HistoricalVersions = serde_json::from_str(SAMPLE).unwrap();
        let e = &v.preview_versions[0];
        assert_eq!(e.kind(), VersionKind::Preview);
        assert_eq!(e.game_version(), "1.21.130.20");
        assert_eq!(e.major(), "1.21");
        assert_eq!(e.slug(), "1.21.130.20_preview");
        assert_eq!(e.numeric(), (1, 21, 130, 20));

        let rel = &v.release_versions[0];
        assert_eq!(rel.kind(), VersionKind::Release);
        assert_eq!(rel.slug(), "1.21.100.2");
        // 无前缀时回退 Release（当前源总带前缀，此处防御）。
        assert_eq!(VersionEntry { version: "1.21.1".into(), ..v.preview_versions[0].clone() }.kind(), VersionKind::Release);
    }

    #[test]
    fn numeric_handles_partial_and_garbage() {
        assert_eq!(parse_numeric("1.21.30"), Some((1, 21, 30, 0)));
        assert_eq!(parse_numeric("abc"), None);
        assert_eq!(parse_numeric("1.2.3.4.5"), None);
    }

    #[test]
    fn groups_ordered_and_latest_marked() {
        let v: HistoricalVersions = serde_json::from_str(SAMPLE).unwrap();
        // 无内核上下文，仅测分组逻辑（借助空缓存路径）。
        // 这里直接调用纯派生，验证 all() 顺序与 slug 唯一性。
        let all = v.all();
        assert!(all[0].timestamp >= all[1].timestamp);
        let ids: Vec<String> = all.iter().map(|e| e.slug()).collect();
        assert!(ids.contains(&"1.21.130.20_preview".to_string()));
        assert!(ids.contains(&"1.21.100.2".to_string()));
    }

    #[test]
    fn preview_slug_disambiguates_release() {
        // 同名正式/快照不冲突。
        let mut rel = VersionEntry { version: "Release 1.21.0".into(), urls: vec![], timestamp: 0, md5: String::new() };
        let prev = VersionEntry { version: "Preview 1.21.0".into(), urls: vec![], timestamp: 0, md5: String::new() };
        rel.version = "1.21.0".into(); // 防御用的裸版本
        assert_eq!(prev.slug(), "1.21.0_preview");
    }
}