//! Conservative, locally-derived client compatibility.
use std::{hash::{Hash, Hasher}, path::{Path, PathBuf}, time::UNIX_EPOCH};
use serde::Serialize;
use crate::{error::KernelError, services::paths::Paths};
use super::client::ClientView;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityState { Compatible, Incompatible, Unknown }
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Compatibility { pub state: CompatibilityState, pub reason: Option<String> }
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ClientSummary { pub id: String, pub name: String, pub state: CompatibilityState, pub reason: Option<String> }

fn strip_version_prefix(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s)
}

pub fn matches_version(pattern: &str, version: &str) -> bool {
    let pattern = strip_version_prefix(pattern); let version = strip_version_prefix(version);
    pattern.len() == version.len() && pattern.bytes().zip(version.bytes()).all(|(p, v)| {
        if matches!(p, b'x' | b'X') { v.is_ascii_digit() } else { p.eq_ignore_ascii_case(&v) }
    })
}

pub trait MatrixProvider: Send + Sync {
    fn read_supported_versions(&self, path: &Path) -> Result<Vec<String>, KernelError>;
}

/// No verified matrix source is available in the current official Latite binary.
pub struct DefaultMatrixProvider;
impl MatrixProvider for DefaultMatrixProvider {
    fn read_supported_versions(&self, _path: &Path) -> Result<Vec<String>, KernelError> {
        Err(KernelError::Config("compatibility metadata unavailable".into()))
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CachedMetadata { key: String, versions: Vec<String> }
fn file_key(path: &Path) -> Result<String, KernelError> {
    let metadata = std::fs::metadata(path)?;
    let modified = metadata.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    Ok(format!("{}|{}|{}", std::fs::canonicalize(path)?.to_string_lossy().to_lowercase(), metadata.len(), modified))
}
fn cache_path(paths: &Paths, key: &str) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new(); key.hash(&mut hasher);
    paths.cache_dir().join("client-compatibility").join(format!("{:016x}.json", hasher.finish()))
}

pub fn versions_for<P: MatrixProvider>(paths: &Paths, path: &Path, provider: &P) -> Option<Vec<String>> {
    let key = file_key(path).ok()?;
    let cache = cache_path(paths, &key);
    if let Ok(bytes) = std::fs::read(&cache) {
        if let Ok(saved) = serde_json::from_slice::<CachedMetadata>(&bytes) {
            if saved.key == key && !saved.versions.is_empty() { return Some(saved.versions); }
        }
    }
    let versions = provider.read_supported_versions(path).ok()?;
    if versions.is_empty() { return None; }
    if let Some(parent) = cache.parent() {
        if std::fs::create_dir_all(parent).is_ok() {
            let tmp = cache.with_extension("tmp");
            if let Ok(bytes) = serde_json::to_vec(&CachedMetadata { key, versions: versions.clone() }) {
                if std::fs::write(&tmp, bytes).is_ok() { let _ = std::fs::rename(tmp, cache); }
            }
        }
    }
    Some(versions)
}

pub fn evaluate<P: MatrixProvider>(paths: &Paths, client: &ClientView, version: &str, provider: &P) -> Compatibility {
    if !client.available {
        return Compatibility { state: CompatibilityState::Unknown, reason: Some("metadata_unavailable".into()) };
    }
    let Some(versions) = versions_for(paths, Path::new(&client.path), provider) else {
        return Compatibility { state: CompatibilityState::Unknown, reason: Some("metadata_unavailable".into()) };
    };
    let supported = versions.iter().any(|pattern| matches_version(pattern, version));
    Compatibility { state: if supported { CompatibilityState::Compatible } else { CompatibilityState::Incompatible }, reason: None }
}
pub fn summaries<P: MatrixProvider>(paths: &Paths, clients: &[ClientView], version: &str, provider: &P) -> Vec<ClientSummary> {
    clients.iter().map(|client| { let result = evaluate(paths, client, version, provider); ClientSummary { id: client.id.clone(), name: client.name.clone(), state: result.state, reason: result.reason } }).collect()
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn version_patterns_are_strict() {
        assert!(matches_version(" V1.21.x.20 ", "1.21.5.20"));
        assert!(matches_version("1.21.X.20", "1.21.5.20"));
        assert!(!matches_version("1.21.x.20", "1.21.15.20"));
        assert!(!matches_version("1.21.x", "1.21.5.0"));
        assert!(matches_version("v1.21.A.20", "1.21.a.20"));
    }
    #[test] fn file_identity_changes_when_size_changes() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("client.dll");
        std::fs::write(&path, b"a").unwrap(); let before = file_key(&path).unwrap();
        std::fs::write(&path, b"longer").unwrap(); assert_ne!(before, file_key(&path).unwrap());
    }
}