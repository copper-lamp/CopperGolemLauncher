//! 包引用解析：github.com 仓库、本地目录/压缩包、直接归档 URL，以及版本/引用部分。

use std::fmt;
use std::path::PathBuf;

use semver::Version;

use super::LipError;

/// 包的内容来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// github.com/<owner>/<repo>，identity 为 owner/repo 小写。
    Github { owner: String, repo: String },
    /// 本地目录或归档（绝对路径）。
    Local { path: PathBuf },
    /// 直接归档 URL 且没有清单来源（无法解析依赖，只做单包安装）。
    ArchiveUrl { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitRef {
    Latest,
    Version(Version),
    Tag(String),
    Branch(String),
    Commit(String),
}

/// 完整包引用：来源 + 变体 + 可选版本/引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSource {
    pub source: Source,
    pub variant: String,
    pub reference: Option<GitRef>,
}

impl PackageSource {
    /// 解析 install 参数，例如 github.com/LiteLDev/LeviLamina#client@1.3.1。
    pub fn parse(input: &str) -> Result<Self, LipError> {
        let text = input.trim();
        if text.is_empty() {
            return Err(LipError::InvalidReference(input.to_string()));
        }
        let (body, reference) = split_reference(text);
        let (body, variant) = split_variant(body);
        let source = if body.starts_with("https://") || body.starts_with("http://") {
            Source::ArchiveUrl { url: body.to_string() }
        } else if let Some(rest) = body.strip_prefix("github.com/") {
            let mut parts = rest.split('/');
            let owner = parts.next().filter(|part| !part.is_empty());
            let repo = parts.next().filter(|part| !part.is_empty());
            match (owner, repo, parts.next()) {
                (Some(owner), Some(repo), None) => Source::Github { owner: owner.to_string(), repo: repo.to_string() },
                _ => return Err(LipError::InvalidReference(input.to_string())),
            }
        } else {
            let path = PathBuf::from(body);
            if !path.is_absolute() {
                return Err(LipError::InvalidReference(format!("{input}: use an absolute path or github.com/<owner>/<repo>")));
            }
            Source::Local { path }
        };
        Ok(Self { source, variant: variant.to_string(), reference })
    }

    /// state.json 中记录的来源字符串。
    pub fn spec(&self) -> String {
        let body = match &self.source {
            Source::Github { owner, repo } => format!("github.com/{owner}/{repo}"),
            Source::Local { path } => path.to_string_lossy().to_string(),
            Source::ArchiveUrl { url } => url.clone(),
        };
        if self.variant.is_empty() {
            body
        } else {
            format!("{body}#{}", self.variant)
        }
    }

    pub fn identity(&self) -> String {
        match &self.source {
            Source::Github { owner, repo } => format!("{}/{}", owner.to_ascii_lowercase(), repo.to_ascii_lowercase()),
            Source::Local { path } => path.to_string_lossy().replace('\\', "/").to_ascii_lowercase(),
            Source::ArchiveUrl { url } => url.to_ascii_lowercase(),
        }
    }
}

impl fmt::Display for PackageSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.spec())
    }
}

impl GitRef {
    pub fn parse(text: &str) -> Self {
        let text = text.trim();
        if text.is_empty() || text.eq_ignore_ascii_case("latest") {
            return GitRef::Latest;
        }
        if let Ok(version) = Version::parse(text.trim_start_matches('v')) {
            return GitRef::Version(version);
        }
        if text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return GitRef::Commit(text.to_string());
        }
        if text.starts_with("refs/heads/") {
            return GitRef::Branch(text.trim_start_matches("refs/heads/").to_string());
        }
        if text.starts_with("refs/tags/") {
            return GitRef::Tag(text.trim_start_matches("refs/tags/").to_string());
        }
        GitRef::Tag(text.to_string())
    }

    pub fn as_str(&self) -> String {
        match self {
            GitRef::Latest => "latest".into(),
            GitRef::Version(version) => version.to_string(),
            GitRef::Tag(tag) => tag.clone(),
            GitRef::Branch(branch) => branch.clone(),
            GitRef::Commit(commit) => commit.clone(),
        }
    }
}

fn split_variant(text: &str) -> (&str, &str) {
    match text.rsplit_once('#') {
        Some((body, variant)) if !variant.contains('/') => (body, variant),
        _ => (text, ""),
    }
}

fn split_reference(text: &str) -> (&str, Option<GitRef>) {
    // 只在最后一段没有 / 时按 @ 切版本，避免 URL 里的 @。
    if let Some((body, reference)) = text.rsplit_once('@') {
        if !reference.is_empty() && !reference.contains('/') && !body.ends_with('/') {
            return (body, Some(GitRef::parse(reference)));
        }
    }
    (text, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_with_variant_and_version() {
        let source = PackageSource::parse("github.com/LiteLDev/LeviLamina#client@1.3.1").unwrap();
        assert_eq!(source.variant, "client");
        assert_eq!(source.identity(), "liteldev/levilamina");
        assert_eq!(source.reference, Some(GitRef::Version(Version::parse("1.3.1").unwrap())));
    }

    #[test]
    fn parses_latest_and_local() {
        assert_eq!(PackageSource::parse("github.com/a/b").unwrap().reference, None);
        let local = PackageSource::parse("D:/mods/demo#client").unwrap();
        assert_eq!(local.variant, "client");
        assert!(matches!(local.source, Source::Local { .. }));
    }

    #[test]
    fn rejects_relative_and_bad_github() {
        assert!(PackageSource::parse("relative/path").is_err());
        assert!(PackageSource::parse("github.com/a").is_err());
    }
}
