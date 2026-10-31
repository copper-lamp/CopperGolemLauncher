//! UWP 版本目录：微软商店 UWP 渠道历代 MCBE 版本的「版本号 → Windows Update
//! UpdateID」映射，是 UWP 下载的唯一事实源。
//!
//! # 为什么是内嵌目录而不是网络清单
//!
//! GDK 渠道有社区维护的 `historical_versions.json`（含 CDN 直链与 md5），UWP 渠道
//! 没有等价的公开清单：商店分发的 UWP 包没有长期有效的直链，只有一个
//! **Windows Update 的 UpdateID**（UUID），下载时再向 `fe3.delivery.mp.microsoft.com`
//! 换取带签名的临时直链（见 [`super::uwp_source`]）。这份 UpdateID 表无法从公开
//! 接口枚举，只能随版本发布一起内置。
//!
//! 数据文件 `uwp_versions.json` 取自 LeviLauncher（`internal/uwpdownload/versions.json`，
//! GPL-3.0-only，见 `THIRD_PARTY_NOTICES`）：每行是 `[版本号, UpdateID, 渠道]`，
//! 渠道 `0=Release` / `1=Beta` / `2=Preview`。
//!
//! # 与清单页的关系
//!
//! 本目录的条目会**并进** `manifest::ManifestView` 的两张平表（正式版 / 测试版），
//! 前端仍然按「版本号 + 类型徽标」渲染，不区分来源渠道。代号（slug）与 GDK 渠道
//! 完全不相交：UWP 目录收录的是 `1.21.114.1` 及更早的版本（发布渠道）与
//! `1.21.120.20` 及更早的快照，GDK 从 `1.21.120.04` / `1.21.120.21` 起接手，
//! 因此两者不会争同一个版本目录名。
//!
//! # 为什么 Beta 归到「测试版」组
//!
//! Beta 渠道是「正式包身份的开发版」（`Microsoft.MinecraftUWP` 而非
//! `Microsoft.MinecraftWindowsBeta`），所以它在 `version.json` 里仍写
//! `type = "beta"`（决定隔离目录与包身份），但在清单页归入测试版组——页面只有
//! 正式版 / 测试版两个分组，而 Beta 显然不是正式版。

use std::sync::OnceLock;

/// UWP 正式包身份（商店零售包）。
pub const RELEASE_IDENTITY: &str = "Microsoft.MinecraftUWP";
/// UWP 预览包身份（`Minecraft Preview`）。
pub const PREVIEW_IDENTITY: &str = "Microsoft.MinecraftWindowsBeta";
/// 正式包族名。
pub const RELEASE_FAMILY: &str = "Microsoft.MinecraftUWP_8wekyb3d8bbwe";
/// 预览包族名。
pub const PREVIEW_FAMILY: &str = "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe";

/// 内嵌的 UWP 版本目录（LeviLauncher 同源数据）。
const CATALOG_JSON: &str = include_str!("uwp_versions.json");

/// 商店渠道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UwpChannel {
    /// 零售渠道（`Microsoft.MinecraftUWP`）。
    Release,
    /// Beta 渠道（同样是零售包身份，只有版本号不同）。
    Beta,
    /// 预览渠道（`Microsoft.MinecraftWindowsBeta`）。
    Preview,
}

impl UwpChannel {
    /// 目录文件里的渠道编号（`0/1/2`）。
    pub fn code(self) -> u8 {
        match self {
            Self::Release => 0,
            Self::Beta => 1,
            Self::Preview => 2,
        }
    }

    /// 渠道名（写进 `version.json` 的 `type`）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Beta => "beta",
            Self::Preview => "preview",
        }
    }

    /// 清单页分组用的类型：正式版 / 测试版。
    ///
    /// Beta 渠道是开发版，页面上归入「测试版」；它**不是** `preview` 渠道，
    /// 包身份与隔离目录都按正式包处理（见模块注释）。
    pub fn display_kind(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Beta | Self::Preview => "preview",
        }
    }

    /// 该渠道的包身份名（`AppxManifest` 的 `Identity/@Name`）。
    pub fn package_identity(self) -> &'static str {
        match self {
            Self::Release | Self::Beta => RELEASE_IDENTITY,
            Self::Preview => PREVIEW_IDENTITY,
        }
    }

    /// 包族名（`%LOCALAPPDATA%\Packages\<族名>`）。
    pub fn package_family(self) -> &'static str {
        match self {
            Self::Release | Self::Beta => RELEASE_FAMILY,
            Self::Preview => PREVIEW_FAMILY,
        }
    }

    /// 下载文件名里的渠道标签。
    pub fn label(self) -> &'static str {
        match self {
            Self::Release => "Release",
            Self::Beta => "Beta",
            Self::Preview => "Preview",
        }
    }
}

/// 目录条目：一个可下载的 UWP 版本。
#[derive(Debug, Clone)]
pub struct UwpVersion {
    /// 数值版本号（如 `1.21.114.1`）。
    pub version: String,
    /// Windows Update 的 UpdateID（小写 UUID）。
    pub update_id: String,
    pub channel: UwpChannel,
}

impl UwpVersion {
    /// 版本唯一键（= 版本目录名基底 = 整包暂存文件名）。
    ///
    /// 与 GDK 渠道同一套规则：快照版追加 `_preview`，正式版 / Beta 版用纯版本号。
    /// 三个渠道之间没有同版本号重复（目录去重键是 `渠道:版本`，实测无冲突），
    /// 所以正式版不需要额外后缀。
    pub fn slug(&self) -> String {
        match self.channel {
            UwpChannel::Preview => format!("{}_preview", self.version),
            _ => self.version.clone(),
        }
    }

    /// 整包包名（与下载引擎的 `filename` 一致）。
    pub fn package_file_name(&self) -> String {
        format!(
            "Minecraft-UWP-{}-{}.appx",
            self.channel.label(),
            self.version
        )
    }
}

/// 解析目录文本（可单测；生产入口是 [`all`]）。
///
/// 严格校验：行必须是三元组、版本号必须是四段数字、UpdateID 必须是 UUID 形状、
/// 渠道必须是 `0/1/2`。任何一行非法都整体失败——半份目录会让用户看到一堆
/// 下载必然失败的版本，比直接报错更难排查。
pub fn parse_catalog(raw: &str) -> Result<Vec<UwpVersion>, String> {
    let rows: Vec<(String, String, u8)> =
        serde_json::from_str(raw).map_err(|e| format!("UWP 版本目录解析失败: {e}"))?;
    let mut out = Vec::with_capacity(rows.len());
    let mut seen = std::collections::HashSet::new();
    for (index, (version, update_id, code)) in rows.into_iter().enumerate() {
        if !is_four_part_version(&version) {
            return Err(format!("UWP 目录第 {index} 行的版本号非法: {version}"));
        }
        let update_id = update_id.trim().to_ascii_lowercase();
        if !is_uuid(&update_id) {
            return Err(format!("UWP 目录第 {index} 行的 UpdateID 非法: {update_id}"));
        }
        let channel = match code {
            0 => UwpChannel::Release,
            1 => UwpChannel::Beta,
            2 => UwpChannel::Preview,
            other => return Err(format!("UWP 目录第 {index} 行的渠道非法: {other}")),
        };
        let entry = UwpVersion {
            version,
            update_id,
            channel,
        };
        // 同名 slug 会让两个版本抢同一个实例目录名与同一份整包，必须拒绝。
        if !seen.insert(entry.slug()) {
            return Err(format!(
                "UWP 目录存在重复版本键: {}（渠道 {}）",
                entry.slug(),
                channel.as_str()
            ));
        }
        out.push(entry);
    }
    Ok(out)
}

/// 全部目录条目（进程内解析一次）。
pub fn all() -> &'static [UwpVersion] {
    static CATALOG: OnceLock<Vec<UwpVersion>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        parse_catalog(CATALOG_JSON).unwrap_or_else(|error| {
            // 内嵌数据是编译期常量，解析失败属于构建缺陷：这里宁可 panic 也不要
            // 静默退化成「UWP 列表为空」，否则整条 UWP 链路会无声消失。
            panic!("内嵌 UWP 版本目录不可用: {error}")
        })
    })
}

/// 按 slug 查找（版本 id 即 slug）。
pub fn find(slug: &str) -> Option<&'static UwpVersion> {
    all().iter().find(|entry| entry.slug() == slug)
}

/// 是否为 UWP 版本 id。
pub fn is_uwp(slug: &str) -> bool {
    find(slug).is_some()
}

/// 四段纯数字版本号（每段 `u16` 范围）。与 `msixvc`/PE 版本字段的口径一致。
fn is_four_part_version(raw: &str) -> bool {
    let parts: Vec<&str> = raw.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 5
                && part.bytes().all(|b| b.is_ascii_digit())
                && part.parse::<u16>().is_ok()
        })
}

/// `8-4-4-4-12` 十六进制 UUID 形状。
fn is_uuid(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let expected_dash = matches!(index, 8 | 13 | 18 | 23);
        if expected_dash {
            if *byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_is_valid_and_unique() {
        let catalog = all();
        assert!(catalog.len() > 400, "目录条目数异常: {}", catalog.len());
        // 三个渠道都有条目（否则说明数据文件被裁剪过）。
        for channel in [UwpChannel::Release, UwpChannel::Beta, UwpChannel::Preview] {
            assert!(
                catalog.iter().any(|entry| entry.channel == channel),
                "缺少渠道 {}",
                channel.as_str()
            );
        }
        // slug 唯一（parse_catalog 已保证，这里再对最终目录钉一次）。
        let mut slugs = std::collections::HashSet::new();
        for entry in catalog {
            assert!(slugs.insert(entry.slug()), "重复 slug: {}", entry.slug());
        }
    }

    #[test]
    fn preview_slug_is_disjoint_from_release() {
        let preview = find("1.21.120.20_preview").expect("预览条目");
        assert_eq!(preview.channel, UwpChannel::Preview);
        assert_eq!(preview.channel.package_identity(), PREVIEW_IDENTITY);
        assert_eq!(preview.channel.display_kind(), "preview");
        let release = find("1.21.114.1").expect("正式条目");
        assert_eq!(release.channel, UwpChannel::Release);
        assert_eq!(release.channel.package_identity(), RELEASE_IDENTITY);
        assert_eq!(release.channel.display_kind(), "release");
        // 版本目录名（slug）与目录里没有的版本不匹配。
        assert!(find("9.9.9.9").is_none());
        assert!(!is_uwp("1.26.52.03"));
    }

    #[test]
    fn beta_channel_uses_release_identity_but_preview_group() {
        let beta = all()
            .iter()
            .find(|entry| entry.channel == UwpChannel::Beta)
            .expect("Beta 条目");
        assert_eq!(beta.channel.package_identity(), RELEASE_IDENTITY);
        assert_eq!(beta.channel.package_family(), RELEASE_FAMILY);
        // `version.json` 里仍是 beta：隔离目录与包身份都按正式包处理。
        assert_eq!(beta.channel.as_str(), "beta");
        assert_eq!(beta.channel.display_kind(), "preview");
        // Beta 的 slug 不带后缀，与正式版按版本号天然区分（目录内无重复版本）。
        assert_eq!(beta.slug(), beta.version);
    }

    #[test]
    fn parser_rejects_dirty_rows() {
        assert!(parse_catalog(r#"[["1.21.114.1","3738c248-0603-4560-abf4-fdd66e7cd852",0]]"#).is_ok());
        // 版本号不是四段
        assert!(parse_catalog(r#"[["1.21.114","3738c248-0603-4560-abf4-fdd66e7cd852",0]]"#).is_err());
        // UpdateID 不是 UUID
        assert!(parse_catalog(r#"[["1.21.114.1","not-a-uuid",0]]"#).is_err());
        // 渠道越界
        assert!(parse_catalog(r#"[["1.21.114.1","3738c248-0603-4560-abf4-fdd66e7cd852",3]]"#).is_err());
        // 行长度不是三元组
        assert!(parse_catalog(r#"[["1.21.114.1","3738c248-0603-4560-abf4-fdd66e7cd852"]]"#).is_err());
        // 同版本同渠道重复
        assert!(parse_catalog(
            r#"[["1.21.114.1","3738c248-0603-4560-abf4-fdd66e7cd852",0],["1.21.114.1","3738c248-0603-4560-abf4-fdd66e7cd853",0]]"#
        )
        .is_err());
    }

    #[test]
    fn package_file_name_carries_channel_and_version() {
        let entry = find("1.21.114.1").expect("正式条目");
        assert_eq!(entry.package_file_name(), "Minecraft-UWP-Release-1.21.114.1.appx");
        let preview = find("1.21.120.20_preview").expect("预览条目");
        assert_eq!(preview.package_file_name(), "Minecraft-UWP-Preview-1.21.120.20.appx");
    }
}
