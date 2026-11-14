//! LIP 版本范围。
//!
//! 对齐 lip 协议使用的 .NET Semver SemVersionRange 语义：比较符、通配、插入符、
//! 波浪号、连字符区间、空白分隔的 AND、|| 分隔的 OR。文档来源
//! protocol/lip-package.md（range）与 SemVersionRange.Parse 的默认行为。

use std::fmt;

use semver::Version;

use super::LipError;

/// 一个可被多个版本满足的范围。内部为 OR 的 AND 子句。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRange {
    raw: String,
    alternatives: Vec<Vec<Comparator>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Comparator {
    Any,
    Exact(Version),
    Gt(Version),
    Ge(Version),
    Lt(Version),
    Le(Version),
    Caret(Version),
    Tilde(Version),
    /// 1.2.* / 1.*：major 必填，minor 为 None 表示任意 minor。
    Wildcard { major: u64, minor: Option<u64> },
}

impl VersionRange {
    pub fn parse(raw: &str) -> Result<Self, LipError> {
        let text = raw.trim();
        if text.is_empty() {
            return Err(LipError::InvalidRange(raw.to_string()));
        }
        let mut alternatives = Vec::new();
        for part in text.split("||") {
            let part = part.trim();
            if part.is_empty() {
                return Err(LipError::InvalidRange(raw.to_string()));
            }
            alternatives.push(parse_conjunction(part, raw)?);
        }
        Ok(Self { raw: raw.to_string(), alternatives })
    }

    /// 匹配任意版本（不限制）。
    pub fn any() -> Self {
        Self { raw: "*".into(), alternatives: vec![vec![Comparator::Any]] }
    }

    pub fn matches(&self, version: &Version) -> bool {
        self.alternatives
            .iter()
            .any(|clause| clause.iter().all(|comparator| comparator.matches(version)))
    }

    /// 在候选版本中选出满足范围的最大者。
    pub fn max_satisfying<'a>(&self, versions: impl Iterator<Item = &'a Version>) -> Option<&'a Version> {
        versions.filter(|version| self.matches(version)).max()
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

impl fmt::Display for VersionRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

fn parse_conjunction(text: &str, raw: &str) -> Result<Vec<Comparator>, LipError> {
    // 连字符区间：1.2.3 - 2.0.0。
    if let Some((low, high)) = split_hyphen_range(text) {
        let low = parse_version(low, raw)?;
        let high = parse_version(high, raw)?;
        return Ok(vec![Comparator::Ge(low), Comparator::Le(high)]);
    }
    let tokens = normalize_tokens(text);
    if tokens.is_empty() {
        return Err(LipError::InvalidRange(raw.to_string()));
    }
    let mut out = Vec::new();
    for token in tokens {
        out.push(parse_comparator(&token, raw)?);
    }
    Ok(out)
}

fn split_hyphen_range(text: &str) -> Option<(&str, &str)> {
    let (low, high) = text.split_once(" - ")?;
    let low = low.trim();
    let high = high.trim();
    if low.is_empty() || high.is_empty() {
        return None;
    }
    Some((low, high))
}

/// 把 ">= 1.2.3" 这类带空格的比较符拼回一个 token。
fn normalize_tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in text.split_whitespace() {
        match token {
            ">=" | ">" | "<=" | "<" | "=" | "^" | "~" => out.push(token.to_string()),
            _ => {
                if let Some(previous) = out.last_mut() {
                    if matches!(previous.as_str(), ">=" | ">" | "<=" | "<" | "=" | "^" | "~") {
                        previous.push_str(token);
                        continue;
                    }
                }
                out.push(token.to_string());
            }
        }
    }
    out
}

fn parse_comparator(token: &str, raw: &str) -> Result<Comparator, LipError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(LipError::InvalidRange(raw.to_string()));
    }
    if token == "*" || token.eq_ignore_ascii_case("x") {
        return Ok(Comparator::Any);
    }
    for (prefix, build) in [
        (">=", Comparator::Ge as fn(Version) -> Comparator),
        ("<=", Comparator::Le as fn(Version) -> Comparator),
        (">", Comparator::Gt as fn(Version) -> Comparator),
        ("<", Comparator::Lt as fn(Version) -> Comparator),
        ("=", Comparator::Exact as fn(Version) -> Comparator),
        ("^", Comparator::Caret as fn(Version) -> Comparator),
        ("~", Comparator::Tilde as fn(Version) -> Comparator),
    ] {
        if let Some(rest) = token.strip_prefix(prefix) {
            let rest = rest.trim();
            if rest.is_empty() {
                return Err(LipError::InvalidRange(raw.to_string()));
            }
            if let Some(wildcard) = parse_wildcard(rest) {
                // >=1.* 之类的通配与比较符组合：按通配的上下界折叠。
                return Ok(wildcard);
            }
            let version = parse_version(rest, raw)?;
            return Ok(build(version));
        }
    }
    if let Some(wildcard) = parse_wildcard(token) {
        return Ok(wildcard);
    }
    Ok(Comparator::Exact(parse_version(token, raw)?))
}

/// 解析 1.2.* / 1.* / 1.2.x。
fn parse_wildcard(token: &str) -> Option<Comparator> {
    let token = token.trim().trim_start_matches('v');
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let last = parts.last().copied()?;
    if !matches!(last, "*" | "x" | "X") {
        return None;
    }
    let major = parts.first()?.parse::<u64>().ok()?;
    let minor = if parts.len() >= 3 { Some(parts[1].parse::<u64>().ok()?) } else { None };
    Some(Comparator::Wildcard { major, minor })
}

fn parse_version(text: &str, raw: &str) -> Result<Version, LipError> {
    let text = text.trim().trim_start_matches('v');
    Version::parse(text).map_err(|_| LipError::InvalidRange(raw.to_string()))
}

impl Comparator {
    fn matches(&self, version: &Version) -> bool {
        match self {
            Comparator::Any => true,
            Comparator::Exact(want) => version == want,
            Comparator::Gt(want) => version > want,
            Comparator::Ge(want) => version >= want,
            Comparator::Lt(want) => version < want,
            Comparator::Le(want) => version <= want,
            Comparator::Caret(want) => version >= want && version < &caret_upper(want),
            Comparator::Tilde(want) => {
                version >= want && version < &Version::new(want.major, want.minor.saturating_add(1), 0)
            }
            Comparator::Wildcard { major, minor } => match minor {
                Some(minor) => version.major == *major && version.minor == *minor,
                None => version.major == *major,
            },
        }
    }
}

fn caret_upper(version: &Version) -> Version {
    // ^1.2.3 => <2.0.0；^0.2.3 => <0.3.0；^0.0.3 => <0.0.4。
    if version.major > 0 {
        Version::new(version.major + 1, 0, 0)
    } else if version.minor > 0 {
        Version::new(0, version.minor + 1, 0)
    } else {
        Version::new(0, 0, version.patch + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn matches_wildcard_and_caret() {
        assert!(VersionRange::parse("1.3.*").unwrap().matches(&v("1.3.9")));
        assert!(!VersionRange::parse("1.3.*").unwrap().matches(&v("1.4.0")));
        assert!(VersionRange::parse("^1.2.3").unwrap().matches(&v("1.9.0")));
        assert!(!VersionRange::parse("^1.2.3").unwrap().matches(&v("2.0.0")));
        assert!(VersionRange::parse("^0.2.3").unwrap().matches(&v("0.2.9")));
        assert!(!VersionRange::parse("^0.2.3").unwrap().matches(&v("0.3.0")));
    }

    #[test]
    fn matches_and_or_and_bounds() {
        let range = VersionRange::parse(">=1.0.0 <2.0.0").unwrap();
        assert!(range.matches(&v("1.5.0")));
        assert!(!range.matches(&v("2.0.0")));
        assert!(VersionRange::parse("1.2.3 || 2.0.0").unwrap().matches(&v("2.0.0")));
        assert!(VersionRange::parse("1.2.3 - 2.0.0").unwrap().matches(&v("1.9.9")));
        assert!(VersionRange::parse(">= 1.2.3").unwrap().matches(&v("1.2.4")));
        assert!(VersionRange::parse("*").unwrap().matches(&v("0.0.1")));
    }

    #[test]
    fn rejects_garbage() {
        assert!(VersionRange::parse("latest").is_err());
        assert!(VersionRange::parse("").is_err());
        assert!(VersionRange::parse(">= ").is_err());
    }
}
