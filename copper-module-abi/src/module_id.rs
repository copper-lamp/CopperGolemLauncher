//! 模块 id 的规范形态与校验。
//!
//! 这条规则是多个子系统的**共同前提**，因此收敛到这里作为单一来源：模块清单校验、
//! 远端元数据索引、目录解析与插件 ABI 都按同一规则判定。
//!
//! 规则：**两段式** `author.module`，每段由小写字母数字组成、可用单个 `-` 连接，
//! 段数 >= 2，总长 <= 128。校验是安全边界的第一道闸：模块 id 会被直接当作
//! **目录名**使用，若放行 `..`、`/`、`\`、盘符或 Windows 保留名，扫描与卸载就会
//! 越过 `modules_dir`。

/// 模块 id 长度上界（与目录名限制对齐，防病态输入）。
pub const MODULE_ID_MAX_LEN: usize = 128;

/// 模块 id 是否符合两段式规范 `^[a-z0-9]+(-[a-z0-9]+)*(\.[a-z0-9]+(-[a-z0-9]+)*)+$`。
pub fn is_valid_module_id(id: &str) -> bool {
    // 长度上界防止病态输入（目录名超长在 Windows 上会直接失败）。
    if id.is_empty() || id.len() > MODULE_ID_MAX_LEN {
        return false;
    }
    let mut count = 0usize;
    for seg in id.split('.') {
        count += 1;
        if !is_valid_id_segment(seg) {
            return false;
        }
    }
    // 必须两段及以上（`author.module`），单段 id 不符合规范。
    count >= 2
}

/// 单个 id 段：`[a-z0-9]+(-[a-z0-9]+)*`。
fn is_valid_id_segment(seg: &str) -> bool {
    if seg.is_empty() {
        return false;
    }
    for part in seg.split('-') {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_two_segment_lowercase_ids() {
        assert!(is_valid_module_id("copper-lamp.server-manager"));
        assert!(is_valid_module_id("thirdparty.world-editor"));
        assert!(is_valid_module_id("a.b"));
        assert!(is_valid_module_id("author9.mod2"));
        assert!(is_valid_module_id("a.b.c"));
    }

    #[test]
    fn rejects_single_segment_and_bad_case() {
        assert!(!is_valid_module_id("home"));
        assert!(!is_valid_module_id("game-download"));
        assert!(!is_valid_module_id("Author.Module"));
        assert!(!is_valid_module_id(""));
    }

    #[test]
    fn rejects_empty_and_dangling_segments() {
        assert!(!is_valid_module_id("author."));
        assert!(!is_valid_module_id(".module"));
        assert!(!is_valid_module_id("author..module"));
        assert!(!is_valid_module_id("author.-module"));
        assert!(!is_valid_module_id("author.mod-"));
        assert!(!is_valid_module_id("author._mod"));
    }

    #[test]
    fn rejects_path_traversal_shaped_ids() {
        assert!(!is_valid_module_id(".."));
        assert!(!is_valid_module_id("../evil"));
        assert!(!is_valid_module_id("author.../evil"));
        assert!(!is_valid_module_id("..\\evil.x"));
        assert!(!is_valid_module_id("C:\\Windows.x"));
        assert!(!is_valid_module_id("author.module/../../etc"));
        assert!(!is_valid_module_id("author.mod\u{0}ule"));
    }

    #[test]
    fn rejects_ids_above_the_length_bound() {
        assert!(!is_valid_module_id(&format!("author.{}", "a".repeat(MODULE_ID_MAX_LEN))));
    }
}
