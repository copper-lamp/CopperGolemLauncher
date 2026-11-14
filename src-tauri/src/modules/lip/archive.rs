//! 档案访问：ZIP 安全解包、目录档案、LIP 使用的 fnmatch 风格 glob。

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use super::LipError;

/// 已解包或直接引用的档案根目录。
pub struct ArchiveView {
    root: PathBuf,
    temporary: Option<tempfile::TempDir>,
    files: Vec<String>,
}

impl ArchiveView {
    /// 解包 ZIP 到临时目录，并建立文件索引。
    pub fn from_zip(path: &Path) -> Result<Self, LipError> {
        let temp = tempfile::Builder::new().prefix("copper-lip-archive").tempdir().map_err(LipError::Io)?;
        extract_zip(path, temp.path())?;
        let files = collect_files(temp.path())?;
        Ok(Self { root: temp.path().to_path_buf(), temporary: Some(temp), files })
    }

    /// 直接把已解开的目录当作档案。
    pub fn from_directory(root: &Path) -> Result<Self, LipError> {
        let files = collect_files(root)?;
        Ok(Self { root: root.to_path_buf(), temporary: None, files })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 档案中的相对文件路径（统一使用 /）。
    pub fn files(&self) -> &[String] {
        &self.files
    }

    pub fn read(&self, relative: &str) -> Result<Vec<u8>, LipError> {
        fs::read(self.root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR)))
            .map_err(|error| LipError::Archive(format!("cannot read {relative}: {error}")))
    }

    pub fn contains_file(&self, relative: &str) -> bool {
        self.files.binary_search_by(|item| item.as_str().cmp(relative)).is_ok()
    }
}

/// 递归收集普通文件，跳过符号链接目录/文件。
fn collect_files(root: &Path) -> Result<Vec<String>, LipError> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).map_err(LipError::Io)? {
            let entry = entry.map_err(LipError::Io)?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(LipError::Io)?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                if let Ok(relative) = path.strip_prefix(root) {
                    out.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

/// 受限解包：拒绝绝对路径、盘符、.. 段和符号链接，绝不写到 dest 之外。
pub fn extract_zip(zip_path: &Path, dest: &Path) -> Result<(), LipError> {
    let file = File::open(zip_path).map_err(LipError::Io)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| LipError::Archive(format!("invalid zip: {error}")))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| LipError::Archive(error.to_string()))?;
        let Some(name) = entry.enclosed_name() else {
            return Err(LipError::Archive(format!("zip entry escapes destination: {}", entry.name())));
        };
        if entry.unix_mode().map(|mode| mode & 0o170000 == 0o120000).unwrap_or(false) {
            return Err(LipError::Archive(format!("zip entry is a symlink: {}", entry.name())));
        }
        let target = dest.join(&name);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(LipError::Io)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(LipError::Io)?;
        }
        let mut output = File::create(&target).map_err(LipError::Io)?;
        io::copy(&mut entry, &mut output).map_err(LipError::Io)?;
        output.flush().map_err(LipError::Io)?;
    }
    Ok(())
}

/// 从任意可读流写文件（下载复用）。
pub fn write_stream(mut reader: impl Read, dest: &Path) -> Result<u64, LipError> {
    let mut file = File::create(dest).map_err(LipError::Io)?;
    let bytes = io::copy(&mut reader, &mut file).map_err(LipError::Io)?;
    file.flush().map_err(LipError::Io)?;
    Ok(bytes)
}

/// 校验受控相对路径，拒绝绝对路径、盘符、空段和 ..。
pub fn safe_relative(path: &str) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Normal(segment) => {
                let text = segment.to_string_lossy();
                if text.is_empty() {
                    return None;
                }
                out.push(segment);
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// LIP place 使用的 fnmatch：逐段匹配，* 不跨 /，** 跨段，? 单字符，[..] 字符集；
/// 结尾 / 表示该目录及其下所有内容。
pub fn glob_match(pattern: &str, path: &str) -> bool {
    if let Some(directory) = pattern.strip_suffix('/') {
        if directory.is_empty() {
            return true;
        }
        if path == directory {
            return true;
        }
        return path.strip_prefix(directory).map(|rest| rest.starts_with('/')).unwrap_or(false);
    }
    match_segments(&split_segments(pattern), &split_segments(path))
}

fn split_segments(text: &str) -> Vec<&str> {
    text.split('/').filter(|segment| !segment.is_empty()).collect()
}

fn match_segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => {
            if match_segments(rest, path) {
                return true;
            }
            !path.is_empty() && match_segments(pattern, &path[1..])
        }
        Some((segment, rest)) => match path.split_first() {
            Some((candidate, path_rest)) => wildcard_match(segment, candidate) && match_segments(rest, path_rest),
            None => false,
        },
    }
}

/// 单段 fnmatch（* ? [..]），不跨 /。也用于 tooth 的 RID 模式匹配。
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    match_chars(&pattern, &text)
}

fn match_chars(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => {
            if match_chars(rest, text) {
                return true;
            }
            !text.is_empty() && match_chars(pattern, &text[1..])
        }
        Some(('?', rest)) => !text.is_empty() && match_chars(rest, &text[1..]),
        Some(('[', rest)) => {
            let Some(close) = rest.iter().position(|character| *character == ']') else {
                return match text.split_first() {
                    Some((&'[', tail)) => match_chars(rest, tail),
                    _ => false,
                };
            };
            let Some((&candidate, tail)) = text.split_first() else { return false };
            let (class, after) = rest.split_at(close);
            if !class_matches(class, candidate) {
                return false;
            }
            match_chars(&after[1..], tail)
        }
        Some((&expected, rest)) => match text.split_first() {
            Some((&candidate, tail)) => expected == candidate && match_chars(rest, tail),
            None => false,
        },
    }
}

fn class_matches(class: &[char], candidate: char) -> bool {
    let (negate, class) = match class.split_first() {
        Some(('!', rest)) => (true, rest),
        Some(('^', rest)) => (true, rest),
        _ => (false, class),
    };
    let mut matched = false;
    let mut index = 0;
    while index < class.len() {
        if index + 2 < class.len() && class[index + 1] == '-' {
            if class[index] <= candidate && candidate <= class[index + 2] {
                matched = true;
            }
            index += 3;
        } else {
            if class[index] == candidate {
                matched = true;
            }
            index += 1;
        }
    }
    matched != negate
}

/// glob 中第一个通配符之前的固定目录前缀（用于把匹配结果映射到 dest）。
pub fn glob_static_prefix(pattern: &str) -> String {
    let mut prefix = Vec::new();
    for segment in pattern.split('/') {
        if segment.contains(['*', '?', '[']) || segment == "**" {
            break;
        }
        if segment.is_empty() {
            continue;
        }
        prefix.push(segment);
    }
    prefix.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_respects_segments_and_double_star() {
        assert!(glob_match("plugins/*.dll", "plugins/a.dll"));
        assert!(!glob_match("plugins/*.dll", "plugins/sub/a.dll"));
        assert!(glob_match("plugins/**/*.dll", "plugins/sub/a.dll"));
        assert!(glob_match("plugins/**/*.dll", "plugins/a.dll"));
        assert!(glob_match("plugins/", "plugins/sub/a.dll"));
        assert!(glob_match("a?c.txt", "abc.txt"));
        assert!(glob_match("[abc]*.txt", "b1.txt"));
        assert!(!glob_match("[!a]*.txt", "a1.txt"));
        assert_eq!(glob_static_prefix("plugins/GMSidebar/*"), "plugins/GMSidebar");
    }

    #[test]
    fn rid_wildcards() {
        assert!(wildcard_match("win-*", "win-x64"));
        assert!(wildcard_match("*-x64", "win-x64"));
        assert!(!wildcard_match("win-arm64", "win-x64"));
    }

    #[test]
    fn safe_relative_rejects_escape() {
        assert!(safe_relative("../x").is_none());
        assert!(safe_relative("C:/x").is_none());
        assert!(safe_relative("/x").is_none());
        assert_eq!(safe_relative("plugins/a.dll").unwrap().to_string_lossy(), "plugins/a.dll");
    }
}
