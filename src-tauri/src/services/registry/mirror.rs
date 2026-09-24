//! 镜像层策略：索引层固定镜像、资产层按 `download.mirror` 重排、会话内失败自愈。
//!
//! 依据 [cgl-libs](../../../docs/cgl-libs.md) 2.6：
//!
//! - **索引层**（`index.json` 及其指向的 cgl-libs 内文件）固定走 GitHub raw + jsdelivr + gh-proxy
//!   三段，**绝不**走用户在设置里配置的第三方镜像——索引是所有内容的信任锚，
//!   不能被不可信第三方镜像替换。因此 [`index_mirrors`] 返回的是内置常量。
//! - **资产层**（安装包、图标）遵循 `download.mirror`：`auto` 按声明顺序；
//!   具体镜像名则把匹配该镜像域名的地址提前（顺序调整只影响速度，不影响安全，因为有 sha256 锚定）。
//! - **镜像自愈**：每个镜像记录"最近连续失败次数"，连续失败 3 次后在**本次会话内**排到最后；
//!   会话结束清零、不持久化（避免长期网络环境变化后错误地永久跳过可用镜像）。

use crate::services::registry::model::RegistryEntry;

/// 索引层主入口文件名。客户端唯一硬编码的仓库路径。
pub const INDEX_PATH_IN_REPO: &str = "index.json";

/// 索引层固定镜像模板（三段，顺序即优先级）。
///
/// 与文档 2.8.4 的 CI 生成模板一致：jsdelivr 使用**固定 commit** 以利用其不可变缓存
/// （因此运行时模板里的 `{commit}` 由索引自身声明的 `repo_commit` 填充），
/// gh-proxy 与 raw 使用 `main` 分支以保证最新。
pub const INDEX_MIRROR_TEMPLATES: [&str; 3] = [
    "https://cdn.jsdelivr.net/gh/copper-lamp/cgl-libs@{commit}/{path}",
    "https://gh-proxy.com/https://raw.githubusercontent.com/copper-lamp/cgl-libs/refs/heads/main/{path}",
    "https://raw.githubusercontent.com/copper-lamp/cgl-libs/refs/heads/main/{path}",
];

/// 生成 `index.json` 的候选地址（索引层，固定三段，与用户设置无关）。
///
/// `commit` 为空时（尚未拉到索引，无法得知 commit）jsdelivr 退化为 `main`，
/// 保证任何情况下都至少有一条可用入口。
pub fn index_mirrors(commit: &str) -> Vec<String> {
    let commit = if commit.trim().is_empty() { "main" } else { commit.trim() };
    INDEX_MIRROR_TEMPLATES
        .iter()
        .map(|t| {
            t.replace("{commit}", commit)
                .replace("{path}", INDEX_PATH_IN_REPO)
        })
        .collect()
}

/// 生成仓库内任意文件的索引层候选地址（用于 `index.json.sha256`、分片等）。
pub fn repo_file_mirrors(path: &str, commit: &str) -> Vec<String> {
    let path = path.trim().trim_start_matches('/');
    let commit = if commit.trim().is_empty() { "main" } else { commit.trim() };
    INDEX_MIRROR_TEMPLATES
        .iter()
        .map(|t| {
            t.replace("{commit}", commit)
                .replace("{path}", path)
        })
        .collect()
}

/// 连续失败达到该次数后，本会话内把镜像排到最后（文档 2.6）。
pub const MIRROR_FAILURE_THRESHOLD: u32 = 3;

/// 会话内镜像可用性统计（不持久化）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MirrorStat {
    /// 最近连续失败次数（成功即清零）。
    pub consecutive_failures: u32,
    /// 会话内累计成功次数。
    pub successes: u64,
    /// 会话内累计失败次数。
    pub failures: u64,
}

impl MirrorStat {
    /// 是否应在本会话内被排到最后。
    pub fn is_demoted(&self) -> bool {
        self.consecutive_failures >= MIRROR_FAILURE_THRESHOLD
    }
}

/// 记录镜像使用结果并更新降权状态。
pub fn record_result(stat: &mut MirrorStat, success: bool) {
    if success {
        stat.successes = stat.successes.saturating_add(1);
        stat.consecutive_failures = 0;
    } else {
        stat.failures = stat.failures.saturating_add(1);
        stat.consecutive_failures = stat.consecutive_failures.saturating_add(1);
    }
}

/// 生成候选地址的尝试顺序：健康的在前、被降权的在后；组内保持声明顺序。
///
/// 稳定排序保证同一份元数据在多次调用间得到相同顺序，便于问题复现与日志比对。
pub fn order_candidates(urls: &[String], demoted: impl Fn(&str) -> bool) -> Vec<usize> {
    let mut healthy: Vec<usize> = Vec::with_capacity(urls.len());
    let mut degraded: Vec<usize> = Vec::new();
    for (i, url) in urls.iter().enumerate() {
        if demoted(url) {
            degraded.push(i);
        } else {
            healthy.push(i);
        }
    }
    healthy.extend(degraded);
    healthy
}

/// 已知镜像域名的设置取值（`download.mirror` 候选值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorPreference {
    /// 按声明顺序（主地址 → 镜像）。
    Auto,
    /// 优先 GitHub 官方域名。
    Github,
    /// 优先 jsdelivr。
    Jsdelivr,
    /// 优先 gh-proxy 代理。
    GhProxy,
    /// 优先 gitcode。
    Gitcode,
    /// 未知取值：按 `auto` 处理（不因新设置值让功能失效）。
    Unknown,
}

impl MirrorPreference {
    /// 解析设置值（大小写不敏感）。
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Self::Auto,
            "github" => Self::Github,
            "jsdelivr" => Self::Jsdelivr,
            "ghproxy" | "gh-proxy" => Self::GhProxy,
            "gitcode" => Self::Gitcode,
            _ => Self::Unknown,
        }
    }

    /// 该偏好匹配的域名关键字（`Auto`/`Unknown` 无偏好）。
    pub fn domain_hints(&self) -> &'static [&'static str] {
        match self {
            Self::Auto | Self::Unknown => &[],
            Self::Github => &["github.com", "githubusercontent.com"],
            Self::Jsdelivr => &["jsdelivr.net"],
            Self::GhProxy => &["gh-proxy.com", "ghproxy.cn", "ghproxy.net"],
            Self::Gitcode => &["gitcode.com", "gitcode.net"],
        }
    }
}

/// 资产层候选地址排序：`auto` 保持声明顺序；指定镜像时把匹配域名的地址提前。
///
/// 只做稳定重排——不丢弃任何地址，避免某个镜像偏好把唯一可用地址排掉。
pub fn order_asset_urls(urls: &[String], preference: MirrorPreference) -> Vec<String> {
    let hints = preference.domain_hints();
    if hints.is_empty() || urls.len() <= 1 {
        return urls.to_vec();
    }
    let (mut preferred, mut rest): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for url in urls {
        if hints.iter().any(|h| url.contains(h)) {
            preferred.push(url.clone());
        } else {
            rest.push(url.clone());
        }
    }
    preferred.extend(rest);
    preferred
}

/// 资产条目候选地址清单（主地址 + mirrors），已按偏好重排。
pub fn asset_candidates(asset: &crate::services::registry::model::ModuleAsset, preference: MirrorPreference) -> Vec<String> {
    let declared = crate::services::registry::model::ModuleEntry::asset_urls(asset);
    order_asset_urls(&declared, preference)
}

/// 索引条目候选地址（`entries[].urls`），按同一套自愈顺序排。
pub fn entry_candidates(entry: &RegistryEntry, demoted: impl Fn(&str) -> bool) -> Vec<String> {
    let order = order_candidates(&entry.urls, demoted);
    order
        .into_iter()
        .filter_map(|i| entry.urls.get(i).cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::registry::model::ModuleAsset;

    #[test]
    fn index_mirrors_are_fixed_and_do_not_follow_user_setting() {
        let urls = index_mirrors("7f3c1ab9e4d2b6c8a0f5e1d3c7b9a2f4e6d8c0b1");
        assert_eq!(urls.len(), 3, "索引层固定三段");
        assert!(urls[0].contains("cdn.jsdelivr.net"));
        assert!(urls[0].contains("7f3c1ab9e4d2b6c8a0f5e1d3c7b9a2f4e6d8c0b1"));
        assert!(urls[0].ends_with("/index.json"));
        assert!(urls[1].contains("gh-proxy.com"));
        assert!(urls[2].starts_with("https://raw.githubusercontent.com/"));
        // 不含任何用户可配置的第三方镜像域名。
        for u in &urls {
            assert!(!u.contains("gitcode"), "索引层不得走用户配置的第三方镜像");
        }

        // commit 未知时退化为 main，仍有三条入口。
        let fallback = index_mirrors("");
        assert_eq!(fallback.len(), 3);
        assert!(fallback[0].ends_with("/index.json"));
        assert!(!fallback[0].contains("{commit}"));

        // 仓库内任意文件的地址生成。
        let sha = repo_file_mirrors("/index.json.sha256", "abc123");
        assert_eq!(sha.len(), 3);
        assert!(sha[0].contains("abc123/index.json.sha256"));
        assert!(sha[1].ends_with("/index.json.sha256"));
    }

    #[test]
    fn mirror_failure_streak_demotes_after_three_failures() {
        let mut stat = MirrorStat::default();
        record_result(&mut stat, false);
        assert!(!stat.is_demoted());
        record_result(&mut stat, false);
        assert!(!stat.is_demoted(), "连续 2 次失败仍排在原位");
        record_result(&mut stat, false);
        assert!(stat.is_demoted(), "连续 3 次失败后本会话排到最后");
        assert_eq!(stat.consecutive_failures, 3);
        assert_eq!(stat.failures, 3);

        // 一次成功即恢复。
        record_result(&mut stat, true);
        assert!(!stat.is_demoted());
        assert_eq!(stat.consecutive_failures, 0);
        assert_eq!(stat.successes, 1);
        // 失败计数保留（用于诊断），只有"连续"计数清零。
        assert_eq!(stat.failures, 3);
    }

    #[test]
    fn candidate_ordering_keeps_declaration_order_among_healthy_mirrors() {
        let urls: Vec<String> = vec![
            "https://raw.githubusercontent.com/a".into(),
            "https://gh-proxy.com/a".into(),
            "https://cdn.jsdelivr.net/gh/a".into(),
        ];
        // 无降权：保持声明顺序。
        assert_eq!(order_candidates(&urls, |_| false), vec![0, 1, 2]);
        // 降权第 2 条：它被排到最后，其余保持相对顺序。
        assert_eq!(
            order_candidates(&urls, |u| u.contains("gh-proxy")),
            vec![0, 2, 1]
        );
        // 全部降权：仍返回全部下标（不丢地址，只是顺序退化）。
        assert_eq!(order_candidates(&urls, |_| true), vec![0, 1, 2]);
        assert_eq!(order_candidates(&[], |_| false), Vec::<usize>::new());
    }

    #[test]
    fn asset_ordering_follows_download_mirror_preference() {
        let urls = vec![
            "https://github.com/example-dev/world-editor/releases/download/v1/a.cglm".to_string(),
            "https://gh-proxy.com/https://github.com/example-dev/world-editor/releases/download/v1/a.cglm".to_string(),
            "https://cdn.jsdelivr.net/gh/example-dev/world-editor@v1/a.cglm".to_string(),
        ];

        // auto：保持声明顺序。
        assert_eq!(order_asset_urls(&urls, MirrorPreference::Auto), urls);
        // ghproxy：匹配域名的地址提前，其余保持相对顺序。
        let ordered = order_asset_urls(&urls, MirrorPreference::GhProxy);
        assert!(ordered[0].contains("gh-proxy.com"));
        assert_eq!(ordered.len(), 3, "重排不得丢弃地址");
        // jsdelivr：同理。
        let ordered = order_asset_urls(&urls, MirrorPreference::Jsdelivr);
        assert!(ordered[0].contains("cdn.jsdelivr.net"));
        // github：`github.com` 与 `githubusercontent.com` 都算 GitHub 域。
        let ordered = order_asset_urls(&urls, MirrorPreference::Github);
        assert!(ordered[0].starts_with("https://github.com/"));
        // gitcode：没有匹配地址时顺序不变。
        assert_eq!(order_asset_urls(&urls, MirrorPreference::Gitcode), urls);
        // 未知取值按 auto 处理，不破坏功能。
        assert_eq!(MirrorPreference::parse("bogus"), MirrorPreference::Unknown);
        assert_eq!(order_asset_urls(&urls, MirrorPreference::Unknown), urls);
        // 设置值解析。
        assert_eq!(MirrorPreference::parse("  GHProxy "), MirrorPreference::GhProxy);
        assert_eq!(MirrorPreference::parse(""), MirrorPreference::Auto);
    }

    #[test]
    fn asset_candidates_include_primary_and_mirrors_in_order() {
        let asset = ModuleAsset {
            platform: crate::services::registry::model::Platform::WindowsX86_64,
            url: "https://github.com/a/b/releases/download/v1/x.cglm".into(),
            mirrors: vec![
                "https://gh-proxy.com/https://github.com/a/b/releases/download/v1/x.cglm".into(),
                "".into(),
            ],
            size: 10,
            sha256: "a".repeat(64),
        };
        // auto：主地址在前，空镜像被过滤。
        let auto = asset_candidates(&asset, MirrorPreference::Auto);
        assert_eq!(auto.len(), 2);
        assert!(auto[0].starts_with("https://github.com/"));
        assert!(auto[1].starts_with("https://gh-proxy.com/"));

        // 偏好代理：镜像提前，但主地址仍在候选集合里（不丢弃）。
        let proxied = asset_candidates(&asset, MirrorPreference::GhProxy);
        assert_eq!(proxied.len(), 2);
        assert!(proxied[0].contains("gh-proxy.com"));
        assert!(proxied.iter().any(|u| u.starts_with("https://github.com/")));
    }

    #[test]
    fn entry_candidates_use_caller_ordering() {
        let entry = RegistryEntry {
            urls: vec![
                "https://raw.githubusercontent.com/a".into(),
                "https://gh-proxy.com/a".into(),
            ],
            ..Default::default()
        };
        let ordered = entry_candidates(&entry, |u| u.contains("gh-proxy"));
        assert_eq!(ordered[0], "https://raw.githubusercontent.com/a");
        assert_eq!(ordered[1], "https://gh-proxy.com/a");
        let none = entry_candidates(&entry, |_| false);
        assert_eq!(none.len(), 2);
    }
}
