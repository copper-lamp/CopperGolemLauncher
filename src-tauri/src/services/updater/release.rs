//! Release 解析：版本发现、通道过滤、版本挑选、平台产物选择。
//!
//! 全部为**纯函数**，不触网、不依赖运行时状态，因此可被单元测试完整覆盖。
//!
//! 存在的理由（旧实现不可用的根因）：更新来源是「一个 Release 挂几十个资产」的
//! 集合，而客户端只该取其中**属于自己平台与架构的那一个**。此前用「后缀白名单 +
//! 匹配不到就取第一个带地址的资产」处理，而 GitHub 每个 Release 都自带
//! `Source code (zip)` / `Source code (tar.gz)`，那条兜底分支必然把源码包当成更新包
//! 投进下载队列。这里取消兜底：识别不出就是识别不出，明确报错。

use crate::services::registry::model::Platform;
use serde_json::Value;

/// 一个 Release 的元信息（解析自 GitHub Releases 列表项）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// 原始 tag（可能带 `v` 前缀）。
    pub tag: String,
    /// 规范化版本号（去前缀，纯 semver）。
    pub version: String,
    /// 发行说明（`body`）。
    pub notes: String,
    /// 发布时间（ISO-8601 字符串，未知时为空）。
    pub published_at: String,
    /// 是否为预发布。
    pub prerelease: bool,
    /// 发行页地址。
    pub html_url: String,
}

/// 下载所需的资产最小集。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    /// 资产原始文件名（落盘用，不可改）。
    pub name: String,
    /// 字节数（GitHub 未给时为 0）。
    pub size: u64,
    /// 直链（`browser_download_url`）。
    pub url: String,
    /// GitHub 资产摘要，形如 `sha256:<hex>`（缺失时为 `None`）。
    pub digest: Option<String>,
    /// 独立 `.sha256` 文本资产的直链（无内联摘要时的兜底）。
    pub digest_url: Option<String>,
}

/// 产物选择失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetError {
    /// Release 里一个资产都没有（产物还没挂上去）。
    NoAssets,
    /// 有资产，但没有当前平台/架构的可执行产物。
    NoMatchForPlatform { platform: String },
}

impl AssetError {
    /// 面向用户的中文原因（内核统一在此归一，前端原样展示）。
    pub fn message(&self) -> String {
        match self {
            Self::NoAssets => "该版本尚未挂载任何发布产物".to_string(),
            Self::NoMatchForPlatform { platform } => {
                format!("该版本没有适配 {platform} 的更新包")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 版本发现
// ---------------------------------------------------------------------------

/// 归一 tag：去 `v` / `V` 前缀与首尾空白；非法则返回 `None`。
///
/// 只接受纯 semver（含预发布与构建号），`release-2024.01` 这类**不接受**——
/// 与不可比较的 tag 比大小只会得到随意结果，不如当作没有版本。
pub fn parse_tag(tag: &str) -> Option<semver::Version> {
    let trimmed = tag.trim().trim_start_matches(['v', 'V']);
    semver::Version::parse(trimmed).ok()
}

/// 把 Releases 响应解析成候选列表。
///
/// - 丢弃 `draft`；
/// - 丢弃 tag 无法解析成 semver 的条目；
/// - 解析 `prerelease`、`body`、`published_at`、`html_url`。
pub fn collect_releases(payload: &Value) -> Vec<ReleaseInfo> {
    let Some(items) = payload.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            if item.get("draft").and_then(Value::as_bool).unwrap_or(false) {
                return None;
            }
            let tag = item.get("tag_name").and_then(Value::as_str)?.trim().to_string();
            let version = parse_tag(&tag)?;
            Some(ReleaseInfo {
                version: version.to_string(),
                tag,
                notes: item
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                published_at: item
                    .get("published_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                prerelease: item
                    .get("prerelease")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                html_url: item
                    .get("html_url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect()
}

/// 通道是否接受预发布。
pub fn channel_allows_prerelease(channel: &str) -> bool {
    matches!(channel.trim().to_ascii_lowercase().as_str(), "beta" | "dev")
}

/// 从候选里挑出「比当前版本新」的最高版本。
///
/// **按 semver 取最大，不按数组顺序**。GitHub 的 `/releases/latest` 与列表默认顺序
/// 都以发布时间为准，tag 被改期或回填时会把旧版本排到前面——那是旧实现选错版本的
/// 第二个来源。预发布仅在 `channel_allows_prerelease` 为真时参与。
///
/// 传空 `releases` 返回 `None`（对应「无发布」）。
pub fn pick_newer<'a>(
    releases: &'a [ReleaseInfo],
    current_version: &str,
    allow_prerelease: bool,
) -> Option<&'a ReleaseInfo> {
    let current = parse_tag(current_version);
    releases
        .iter()
        .filter(|r| allow_prerelease || !r.prerelease)
        .filter_map(|r| parse_tag(&r.version).map(|v| (r, v)))
        .filter(|(_, v)| match &current {
            Some(c) => v > c,
            // 当前版本不可解析（例如开发期脏版本）时不做降级判断：
            // 报「已是最新」比报一个错版本安全。
            None => false,
        })
        .max_by(|a, b| a.1.cmp(&b.1))
        .map(|(r, _)| r)
}

// ---------------------------------------------------------------------------
// 资产名解析
// ---------------------------------------------------------------------------

/// 资产名归一：小写，并把 `_ . 空格 ( ) +` 统一成 `-`。
///
/// 不直接做子串匹配，是因为 `"darwin".contains("win")` 这类误判会把 macOS 产物
/// 当成 Windows 产物挑走。切成 token 后按集合判定，杜绝这类错误。
fn tokenize(name: &str) -> Vec<String> {
    name.to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// 从 token 序列判定操作系统；无法判定返回 `None`（视为通用产物）。
fn platform_of(tokens: &[String]) -> Option<&'static str> {
    for token in tokens {
        match token.as_str() {
            "windows" | "win" | "win32" | "win64" | "winnt" | "nt" => return Some("windows"),
            "linux" | "linuxgnu" => return Some("linux"),
            "macos" | "osx" | "darwin" | "mac" => return Some("macos"),
            "android" => return Some("android"),
            _ => {}
        }
    }
    None
}

/// 从 token 序列判定 CPU 架构；无法判定返回 `None`（视为通用产物）。
fn arch_of(tokens: &[String]) -> Option<&'static str> {
    for (index, token) in tokens.iter().enumerate() {
        match token.as_str() {
            "amd64" | "x64" | "win64" => return Some("x86_64"),
            // `x86_64` 归一后被切成 `x86` + `64`，两段相邻才算 64 位。
            "x86" | "x32" => {
                if tokens.get(index + 1).map(String::as_str) == Some("64") {
                    return Some("x86_64");
                }
                return Some("x86");
            }
            "aarch64" | "arm64" | "armv8" | "arm64v8" => return Some("aarch64"),
            "armv7" | "armv7l" => return Some("armv7"),
            "i386" | "i686" | "win32" => return Some("x86"),
            _ => {}
        }
    }
    None
}

/// 归档/安装器扩展名（归一后取最后一段）。
fn extension_of(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    for compound in [".tar.gz", ".tar.xz", ".tar.bz2"] {
        if lower.ends_with(compound) {
            return compound.trim_start_matches('.').to_string();
        }
    }
    match lower.rsplit_once('.') {
        Some((_, ext)) if !ext.contains('/') => ext.to_string(),
        _ => String::new(),
    }
}

/// 与平台无关的黑名单：命中即淘汰，无论当前平台是什么。
///
/// GitHub 自动生成的源码包是头号坑（名字里既没有平台也没有架构，且排在资产列表
/// 最前面）；签名与摘要文件同理——它们不是可执行产物。
fn is_blacklisted(name: &str) -> bool {
    // 源码包两种命名（`Source code (zip)` 与 GitLab 式 `-sources.zip`）都覆盖。
    if name.to_ascii_lowercase().contains("source") {
        return true;
    }
    const BLOCKED_EXT: &[&str] = &[
        "sha256", "sha256sum", "sha512", "sig", "asc", "pem", "blockmap", "json", "yml", "yaml",
        "txt", "md", "sbom", "spdx",
    ];
    let ext = extension_of(name);
    BLOCKED_EXT.contains(&ext.as_str())
}

/// 该平台下可执行产物的扩展名 → 优先级。数值越大越优先。
///
/// 未列出的扩展名不进入候选集：宁可报「无匹配产物」，也不要投一个装不上的包。
fn kind_priority(os: &str, ext: &str) -> Option<u32> {
    let rank = match (os, ext) {
        // Windows：便携 zip（含裸 exe，可原地自替换）优先于 NSIS 安装器。
        ("windows", "zip") => 100,
        ("windows", "exe") => 90,
        ("windows", "msi") => 10,
        ("linux", "appimage") => 100,
        ("linux", "tar" | "gz" | "tar.gz" | "tar.xz" | "tar.bz2") => 80,
        ("macos", "dmg") => 100,
        ("macos", "tar" | "gz" | "tar.gz" | "tar.xz" | "tar.bz2") => 80,
        ("android", "apk") => 100,
        _ => return None,
    };
    Some(rank)
}

/// 当前平台的 (os, arch) 规范化二元组。
///
/// 不能直接拿 `Platform::as_str()`（`windows-x86_64`）去和 token 判定结果
/// （`windows`）比：两套词汇混用会让所有候选都被判成「平台不符」。
/// 显式派生一次，两侧共用同一套词汇。
fn platform_os_arch(platform: Option<&Platform>) -> (Option<&'static str>, &'static str) {
    let os = match platform {
        Some(Platform::WindowsX86_64) | Some(Platform::WindowsAarch64) => Some("windows"),
        Some(Platform::LinuxX86_64) => Some("linux"),
        Some(Platform::AndroidArm64) => Some("android"),
        Some(Platform::Unknown(_)) | None => None,
    };
    let arch = match platform {
        Some(Platform::WindowsX86_64) | Some(Platform::LinuxX86_64) => "x86_64",
        // Android 的 arm64 与其它平台的 aarch64 同源，归一到同一标记。
        Some(Platform::WindowsAarch64) | Some(Platform::AndroidArm64) => "aarch64",
        _ => "unknown",
    };
    (os, arch)
}

/// 把响应里的资产数组解析成结构体（顺带解析内联摘要与 `.sha256` 兄弟资产）。
fn parse_assets(assets: Option<&Vec<Value>>) -> Vec<ReleaseAsset> {
    let Some(items) = assets else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| {
            let name = item.get("name").and_then(Value::as_str)?.to_string();
            let url = item
                .get("browser_download_url")
                .and_then(Value::as_str)?
                .to_string();
            let digest = item
                .get("digest")
                .and_then(Value::as_str)
                .and_then(|raw| raw.strip_prefix("sha256:"))
                .map(|hex| hex.trim().to_ascii_lowercase())
                .filter(|hex| !hex.is_empty());
            let digest_url = items
                .iter()
                .find(|other| {
                    other.get("name").and_then(Value::as_str)
                        == Some(format!("{name}.sha256").as_str())
                })
                .and_then(|other| other.get("browser_download_url").and_then(Value::as_str))
                .map(str::to_string);
            Some(ReleaseAsset {
                name,
                size: item.get("size").and_then(Value::as_u64).unwrap_or(0),
                url,
                digest,
                digest_url,
            })
        })
        .collect()
}

/// 为当前平台挑出唯一可用的更新产物。
///
/// 匹配分三档，取第一档非空的结果：
/// 1. 平台标记**与**架构标记都命中；
/// 2. 平台标记命中、架构未标注（发布方只出一个架构时属正常）；
/// 3. 平台与架构都未标注（通用产物名）。
///
/// 三档全空即报错，不做「随便挑一个」的兜底。
pub fn select_asset(
    assets: Option<&Vec<Value>>,
    platform: Option<&Platform>,
) -> Result<ReleaseAsset, AssetError> {
    let parsed = parse_assets(assets);
    if parsed.is_empty() {
        return Err(AssetError::NoAssets);
    }
    let (current_os, current_architecture) = platform_os_arch(platform);
    let platform_key = platform.map(Platform::as_str).unwrap_or("unknown");

    // (资产, 平台标记, 架构标记, 优先级)
    let mut tier1: Vec<(ReleaseAsset, u32)> = Vec::new();
    let mut tier2: Vec<(ReleaseAsset, u32)> = Vec::new();
    let mut tier3: Vec<(ReleaseAsset, u32)> = Vec::new();

    for asset in parsed {
        let tokens = tokenize(&asset.name);
        if is_blacklisted(&asset.name) {
            continue;
        }
        let found_platform = platform_of(&tokens);
        let found_arch = arch_of(&tokens);
        // 标注了但对不上：直接淘汰（这正是「不能投别的平台的包」的关键一步）。
        if found_platform.is_some_and(|p| Some(p) != current_os) {
            continue;
        }
        if found_arch.is_some_and(|a| a != current_architecture) {
            continue;
        }
        // 资产没写平台标记时按当前平台解释，否则通用名（如 `app.zip`）
        // 会被判为「不是安装产物」。当前平台未知时无解，跳过。
        let Some(effective_os) = found_platform.or(current_os) else {
            continue;
        };
        let Some(rank) = kind_priority(effective_os, &extension_of(&asset.name)) else {
            continue;
        };
        match (found_platform, found_arch) {
            (Some(_), Some(_)) => tier1.push((asset, rank)),
            (Some(_), None) => tier2.push((asset, rank)),
            _ => tier3.push((asset, rank)),
        }
    }

    let mut pool = if !tier1.is_empty() {
        tier1
    } else if !tier2.is_empty() {
        tier2
    } else {
        tier3
    };
    if pool.is_empty() {
        return Err(AssetError::NoMatchForPlatform {
            platform: platform_key.to_string(),
        });
    }

    // 同优先级取文件名字典序最小者：发布脚本产物名唯一，这里只保证结果确定。
    pool.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.cmp(&b.0.name)));
    Ok(pool.remove(0).0)
}



#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn asset(name: &str) -> Value {
        json!({
            "name": name,
            "size": 100,
            "browser_download_url": format!("https://example.test/{name}"),
        })
    }

    fn select(names: &[&str], platform: Option<&Platform>) -> Result<String, AssetError> {
        let assets: Vec<Value> = names.iter().map(|n| asset(n)).collect();
        select_asset(Some(&assets), platform).map(|a| a.name)
    }

    #[test]
    fn tag_accepts_v_prefix_and_rejects_non_semver() {
        assert_eq!(parse_tag("v1.2.3").unwrap().to_string(), "1.2.3");
        assert_eq!(parse_tag(" 1.2.3 ").unwrap().to_string(), "1.2.3");
        assert_eq!(parse_tag("v1.2.3-beta.1").unwrap().to_string(), "1.2.3-beta.1");
        assert!(parse_tag("release-2024.01").is_none());
        assert!(parse_tag("latest").is_none());
    }

    #[test]
    fn collect_skips_drafts_and_unparsable_tags() {
        let payload = json!([
            {"tag_name": "v0.2.0", "draft": false, "prerelease": false, "body": "a"},
            {"tag_name": "v0.3.0", "draft": true,  "prerelease": false, "body": "hidden"},
            {"tag_name": "nightly",  "draft": false, "prerelease": true},
        ]);
        let releases = collect_releases(&payload);
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].version, "0.2.0");
        assert_eq!(releases[0].notes, "a");
    }

    #[test]
    fn newest_is_semver_max_not_array_order() {
        let releases = collect_releases(&json!([
            {"tag_name": "v0.10.0", "prerelease": false},
            {"tag_name": "v0.9.0",  "prerelease": false},
            {"tag_name": "v0.2.0",  "prerelease": false},
        ]));
        let picked = pick_newer(&releases, "0.1.0", false).expect("should pick");
        // 列表顺序把 0.10.0 排在首位，但真正最大的是它；换成 0.9.0 在前即可看出
        // 这里比的是版本号而不是位置（见下方 reversed 用例）。
        assert_eq!(picked.version, "0.10.0");

        let reversed = collect_releases(&json!([
            {"tag_name": "v0.2.0",  "prerelease": false},
            {"tag_name": "v0.10.0", "prerelease": false},
        ]));
        assert_eq!(
            pick_newer(&reversed, "0.1.0", false).map(|r| r.version.clone()),
            Some("0.10.0".to_string())
        );
    }

    #[test]
    fn older_or_equal_versions_are_ignored() {
        let releases = collect_releases(&json!([
            {"tag_name": "v0.1.0", "prerelease": false},
            {"tag_name": "v0.0.9", "prerelease": false},
        ]));
        assert!(pick_newer(&releases, "0.1.0", false).is_none());
    }

    #[test]
    fn prerelease_excluded_on_stable_channel() {
        let releases = collect_releases(&json!([
            {"tag_name": "v0.3.0-beta.1", "prerelease": true},
        ]));
        assert!(pick_newer(&releases, "0.2.0", false).is_none());
        assert_eq!(
            pick_newer(&releases, "0.2.0", true).map(|r| r.version.clone()),
            Some("0.3.0-beta.1".to_string())
        );
        assert!(channel_allows_prerelease("beta"));
        assert!(!channel_allows_prerelease("stable"));
    }

    #[test]
    fn github_source_code_archives_are_never_selected() {
        // 旧实现的兜底分支正是在这个场景下把源码包投给了用户。
        let picked = select(
            &[
                "Source code (zip)",
                "Source code (tar.gz)",
                "CopperGolemLauncher-0.2.0-windows-x86_64.zip",
            ],
            Some(&Platform::WindowsX86_64),
        )
        .expect("should pick");
        assert_eq!(picked, "CopperGolemLauncher-0.2.0-windows-x86_64.zip");
    }

    #[test]
    fn windows_prefers_portable_zip_over_nsis() {
        let picked = select(
            &[
                "CopperGolemLauncher-0.2.0-windows-x86_64.exe",
                "CopperGolemLauncher-0.2.0-windows-x86_64.zip",
            ],
            Some(&Platform::WindowsX86_64),
        )
        .expect("should pick");
        assert_eq!(picked, "CopperGolemLauncher-0.2.0-windows-x86_64.zip");
    }

    #[test]
    fn other_platform_assets_are_never_selected() {
        // 同一个 Release 挂全部平台产物，Windows 客户端只能拿到 Windows 的。
        let names = [
            "CopperGolemLauncher-0.2.0-linux-x86_64.AppImage",
            "CopperGolemLauncher-0.2.0-macos-aarch64.dmg",
            "CopperGolemLauncher-0.2.0-windows-x86_64.zip",
            "CopperGolemLauncher-0.2.0-android-arm64.apk",
        ];
        assert_eq!(
            select(&names, Some(&Platform::WindowsX86_64)).ok(),
            Some("CopperGolemLauncher-0.2.0-windows-x86_64.zip".to_string())
        );
        assert_eq!(
            select(&names, Some(&Platform::LinuxX86_64)).ok(),
            Some("CopperGolemLauncher-0.2.0-linux-x86_64.AppImage".to_string())
        );
        assert_eq!(
            select(&names, Some(&Platform::AndroidArm64)).ok(),
            Some("CopperGolemLauncher-0.2.0-android-arm64.apk".to_string())
        );
    }

    #[test]
    fn darwin_token_is_not_mistaken_for_windows() {
        // "darwin" 含子串 "win"；按 token 集合判定才不会中招。
        let picked = select(
            &["CopperGolemLauncher-0.2.0-darwin-x86_64.dmg"],
            Some(&Platform::WindowsX86_64),
        );
        assert!(picked.is_err(), "macOS 产物不得落到 Windows 客户端");
    }

    #[test]
    fn mismatched_arch_is_rejected() {
        let names = [
            "CopperGolemLauncher-0.2.0-windows-x86_64.zip",
            "CopperGolemLauncher-0.2.0-windows-aarch64.zip",
        ];
        assert_eq!(
            select(&names, Some(&Platform::WindowsX86_64)).ok(),
            Some("CopperGolemLauncher-0.2.0-windows-x86_64.zip".to_string())
        );
    }

    #[test]
    fn platform_match_without_arch_tag_still_resolves() {
        let picked = select(
            &["CopperGolemLauncher-0.2.0-windows.zip"],
            Some(&Platform::WindowsX86_64),
        )
        .expect("should pick");
        assert_eq!(picked, "CopperGolemLauncher-0.2.0-windows.zip");
    }

    #[test]
    fn digest_fields_are_parsed_including_sibling_sha256_asset() {
        let assets = vec![
            json!({
                "name": "CopperGolemLauncher-0.2.0-windows-x86_64.zip",
                "size": 42,
                "browser_download_url": "https://example.test/p.zip",
                "digest": "sha256:ABCDEF",
            }),
            json!({
                "name": "CopperGolemLauncher-0.2.0-windows-x86_64.zip.sha256",
                "browser_download_url": "https://example.test/p.zip.sha256",
            }),
        ];
        let picked = select_asset(Some(&assets), Some(&Platform::WindowsX86_64)).expect("pick");
        assert_eq!(picked.size, 42);
        assert_eq!(picked.digest.as_deref(), Some("abcdef"));
        assert_eq!(
            picked.digest_url.as_deref(),
            Some("https://example.test/p.zip.sha256")
        );
    }

    #[test]
    fn no_assets_and_no_platform_match_report_distinct_errors() {
        assert_eq!(select_asset(None, Some(&Platform::WindowsX86_64)), Err(AssetError::NoAssets));
        assert_eq!(
            select_asset(Some(&Vec::new()), Some(&Platform::WindowsX86_64)),
            Err(AssetError::NoAssets)
        );
        assert_eq!(
            select(
                &["CopperGolemLauncher-0.2.0-windows-x86_64.zip"],
                Some(&Platform::LinuxX86_64)
            ),
            Err(AssetError::NoMatchForPlatform {
                platform: "linux-x86_64".to_string()
            })
        );
    }

    #[test]
    fn deb_asset_is_not_an_installable_candidate() {
        // deb 需要 root，内核不自更新：应报无匹配，而不是投下去再失败。
        let result = select(
            &["coppergolem_0.2.0_amd64.deb"],
            Some(&Platform::LinuxX86_64),
        );
        assert!(result.is_err());
    }
}