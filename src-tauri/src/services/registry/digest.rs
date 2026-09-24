//! 摘要计算与比对：sha256 计算、`.sha256` 摘要文件解析、镜像摘要一致性判定。
//!
//! 对应 [cgl-libs](../../../docs/cgl-libs.md) 2.5 三级校验链：
//! `index.json.sha256` → `index.json` → `entries[].sha256` → 分片/清单 → `assets[].sha256` → 资产。
//!
//! 本模块只做纯计算与判定，所有网络与磁盘 IO 由调用方（[`super::RegistryService`]）负责，
//! 以便校验逻辑可被单元测试穷尽覆盖。

use sha2::{Digest, Sha256};

/// 计算字节流的 sha256（小写十六进制）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    to_hex(&hasher.finalize())
}

/// 摘要比对结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigestCheck {
    /// 完全一致。
    Match,
    /// 不一致（疑似损坏或被篡改）。
    Mismatch,
    /// 期望摘要缺失或格式非法——**必须视为失败**，缺摘要的条目要被拒绝（文档 2.5 规则 1）。
    Missing,
}

impl DigestCheck {
    pub fn is_ok(self) -> bool {
        matches!(self, Self::Match)
    }
}

/// 比对字节流与期望摘要（大小写不敏感，允许空格）。
pub fn verify_bytes(bytes: &[u8], expected: &str) -> DigestCheck {
    let expected = expected.trim();
    if expected.is_empty() || !super::model::is_sha256_hex(expected) {
        return DigestCheck::Missing;
    }
    if sha256_hex(bytes).eq_ignore_ascii_case(expected) {
        DigestCheck::Match
    } else {
        DigestCheck::Mismatch
    }
}

/// 解析 `.sha256` 摘要文件内容，返回小写十六进制摘要。
///
/// 兼容 `sha256sum` 与 `certutil` 两类常见产物：
/// - `<hex>`（仅摘要）
/// - `<hex>  <filename>`（sha256sum 默认与二进制模式 `*` 前缀）
///
/// 摘要是自 2.5 起用于**多镜像一致性判定**的必需锚点：任何一个镜像给出的
/// `index.json.sha256` 与其余镜像不一致，即判定该镜像被污染。
pub fn parse_sha256_file(text: &str) -> Option<String> {
    let first_line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut parts = first_line.split_whitespace();
    let candidate = parts.next()?;
    let hex = candidate.trim_start_matches('*');
    if super::model::is_sha256_hex(hex) {
        Some(hex.to_ascii_lowercase())
    } else {
        None
    }
}

/// 统计一组摘要的共识：返回出现次数最多的摘要与其出现次数。
///
/// 用于"多镜像 `index.json.sha256` 一致性"判定——多数派摘要才是可信摘要，
/// 少数派镜像返回的内容不得被采用。
pub fn digest_consensus(digests: &[String]) -> Option<(String, usize)> {
    let mut best: Option<(String, usize)> = None;
    for (i, raw) in digests.iter().enumerate() {
        let candidate = raw.trim().to_ascii_lowercase();
        if candidate.is_empty() {
            continue;
        }
        // 只对"首次出现"的摘要计数：保证结果与输入顺序确定（不依赖哈希迭代顺序）。
        let already_counted = digests[..i]
            .iter()
            .any(|prev| prev.trim().eq_ignore_ascii_case(candidate.as_str()));
        if already_counted {
            continue;
        }
        let occurrences = digests
            .iter()
            .filter(|x| x.trim().eq_ignore_ascii_case(candidate.as_str()))
            .count();
        let better = match &best {
            Some((_, count)) => occurrences > *count,
            None => true,
        };
        if better {
            best = Some((candidate, occurrences));
        }
    }
    best
}

/// 字节数是否与声明的 `size` 一致（`size <= 0` 视为未声明，不做判定）。
///
/// 索引与分片清单里的 `size` 用于完整性预检：长度不符时无需计算摘要即可快速拒绝。
pub fn size_matches(actual_len: usize, declared: i64) -> bool {
    if declared <= 0 {
        return true;
    }
    actual_len as i64 == declared
}

/// 字节 → 小写十六进制。
fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空串的 SHA-256（公开标准值，用于验证实现正确而非自证）。
    const EMPTY_SHA256: &str =
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    /// `"abc"` 的 SHA-256（NIST 测试向量）。
    const ABC_SHA256: &str =
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn sha256_matches_known_test_vectors() {
        assert_eq!(sha256_hex(b""), EMPTY_SHA256);
        assert_eq!(sha256_hex(b"abc"), ABC_SHA256);
        // 大输入分块无关性（一次性 update 与流式一致）。
        let big = vec![7u8; 200_000];
        let direct = sha256_hex(&big);
        let mut hasher = Sha256::new();
        for chunk in big.chunks(4096) {
            hasher.update(chunk);
        }
        assert_eq!(direct, to_hex(&hasher.finalize()));
    }

    #[test]
    fn verify_bytes_reports_match_mismatch_and_missing() {
        assert_eq!(verify_bytes(b"abc", ABC_SHA256), DigestCheck::Match);
        assert_eq!(verify_bytes(b"abc", &ABC_SHA256.to_uppercase()), DigestCheck::Match);
        assert_eq!(verify_bytes(b"abcd", ABC_SHA256), DigestCheck::Mismatch);
        assert_eq!(verify_bytes(b"abc", ""), DigestCheck::Missing);
        assert_eq!(verify_bytes(b"abc", "   "), DigestCheck::Missing);
        assert_eq!(verify_bytes(b"abc", "deadbeef"), DigestCheck::Missing);
        assert_eq!(verify_bytes(b"abc", &"z".repeat(64)), DigestCheck::Missing);
        assert!(verify_bytes(b"abc", ABC_SHA256).is_ok());
        assert!(!verify_bytes(b"abc", "").is_ok(), "缺摘要必须判定为失败");
    }

    #[test]
    fn parse_sha256_file_accepts_common_formats() {
        let hex = ABC_SHA256;
        assert_eq!(parse_sha256_file(hex).as_deref(), Some(hex));
        assert_eq!(
            parse_sha256_file(&format!("{hex}  index.json\n")).as_deref(),
            Some(hex)
        );
        assert_eq!(
            parse_sha256_file(&format!("{hex} *index.json")).as_deref(),
            Some(hex)
        );
        assert_eq!(
            parse_sha256_file(&format!("\n\n  {}   \n", hex.to_uppercase())).as_deref(),
            Some(hex),
            "应取首行非空内容并统一为小写"
        );
        assert!(parse_sha256_file("").is_none());
        assert!(parse_sha256_file("not a digest").is_none());
        assert!(parse_sha256_file("abc123  index.json").is_none(), "长度不足必须拒绝");
    }

    #[test]
    fn digest_consensus_picks_majority() {
        let good = ABC_SHA256.to_string();
        let evil = "f".repeat(64);
        let (winner, count) = digest_consensus(&[
            good.clone(),
            evil.clone(),
            good.clone(),
            good.clone(),
        ])
        .expect("应有共识摘要");
        assert_eq!(winner, good);
        assert_eq!(count, 3);

        // 多数派为污染摘要时同样如实返回（由调用方决定是否采用）。
        let (winner2, count2) =
            digest_consensus(&[evil.clone(), good.clone(), evil.clone()]).expect("应有共识");
        assert_eq!(winner2, evil);
        assert_eq!(count2, 2);

        // 空输入与空摘要被忽略。
        assert!(digest_consensus(&[]).is_none());
        assert!(digest_consensus(&["".to_string(), "  ".to_string()]).is_none());
    }

    #[test]
    fn size_precheck_flags_length_mismatch() {
        assert!(size_matches(1462, 1462));
        assert!(!size_matches(1463, 1462));
        assert!(size_matches(9999, 0), "未声明 size 不参与判定");
        assert!(size_matches(9999, -1), "非法 size 不参与判定");
    }
}
