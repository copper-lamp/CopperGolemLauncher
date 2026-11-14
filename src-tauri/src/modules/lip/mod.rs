//! 独立 Rust LIP 兼容核心。
//!
//! 当前实现 format-4 状态读写、包引用解析、列表兼容输出、工作区锁和原子状态提交。
//! 资产解析、依赖求解及安装事务尚未实现，调用方不得将其当作完整 lip install 替代品。

use std::{collections::{HashMap, HashSet}, fs::{self, File, OpenOptions}, io::Write, path::{Component, Path}};
use serde::{Deserialize, Serialize};

const FORMAT: u32 = 4;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct State { pub format: u32, pub side: String, pub platform: String, pub packages: Vec<InstalledPackage> }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledPackage {
    pub manifest: PackageManifest, pub variant: String, pub explicit: bool,
    #[serde(rename = "ref")] pub source_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")] pub commit: Option<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageManifest {
    pub format: u32, pub id: String, pub version: String,
    #[serde(default)] pub variants: HashMap<String, Vec<PackageEntry>>,
    #[serde(flatten)] pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageEntry {
    pub side: String, pub platform: String,
    #[serde(default)] pub deps: HashMap<String, String>,
    #[serde(flatten)] pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageState { pub installed: bool, pub explicit_installed: bool, pub installed_version: String }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRef { pub id: String, pub variant: String, pub version: Option<String> }

#[derive(Debug, thiserror::Error)]
pub enum LipError {
    #[error("invalid lip package reference: {0}")] InvalidPackageRef(String),
    #[error("invalid LIP state: {0}")] InvalidState(String),
    #[error("workspace is locked by another operation")] WorkspaceLocked,
    #[error("workspace path is not a directory")] InvalidWorkspace,
    #[error(transparent)] Io(#[from] std::io::Error),
    #[error(transparent)] Json(#[from] serde_json::Error),
}

impl State {
    pub fn empty(side: impl Into<String>, platform: impl Into<String>) -> Self { Self { format: FORMAT, side: side.into(), platform: platform.into(), packages: Vec::new() } }

    pub fn validate(&self) -> Result<(), LipError> {
        if self.format != FORMAT { return Err(LipError::InvalidState(format!("unsupported format {}; expected {FORMAT}", self.format))); }
        if !matches!(self.side.as_str(), "client" | "server") || self.platform.is_empty() || matches!(self.platform.as_str(), "client" | "server") { return Err(LipError::InvalidState("invalid side or platform".into())); }
        let mut identities = HashSet::new();
        for p in &self.packages {
            validate_manifest(&p.manifest)?; validate_variant(&p.variant)?;
            if !identities.insert(package_identity(&p.manifest.id, &p.variant).to_ascii_lowercase()) { return Err(LipError::InvalidState("duplicate package identity".into())); }
            if let Some(commit) = &p.commit { if commit.len() != 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) { return Err(LipError::InvalidState("invalid commit".into())); } }
            for path in &p.files { validate_workspace_path(path)?; }
        }
        Ok(())
    }

    pub fn list_json(&self) -> Result<serde_json::Value, LipError> {
        self.validate()?; let mut explicit = Vec::new(); let mut implicit = Vec::new();
        for p in &self.packages { let item = format!("{}@{}", package_identity(&p.manifest.id, &p.variant), p.manifest.version); if p.explicit { explicit.push(item) } else { implicit.push(item) } }
        Ok(serde_json::json!({"ExplicitInstalled": explicit, "ImplicitInstalled": implicit}))
    }

    pub fn package_states(&self) -> Result<HashMap<String, PackageState>, LipError> {
        self.validate()?; Ok(self.packages.iter().map(|p| (package_identity(&p.manifest.id, &p.variant).to_ascii_lowercase(), PackageState { installed: true, explicit_installed: p.explicit, installed_version: p.manifest.version.clone() })).collect())
    }
}

pub fn parse_package_ref(input: &str) -> Result<PackageRef, LipError> {
    let raw = input.trim(); if raw.is_empty() { return Err(LipError::InvalidPackageRef(input.into())); }
    let (source, version) = match raw.rsplit_once('@') { Some((s, v)) if !s.is_empty() && !v.is_empty() => (s, Some(v.to_owned())), Some(_) => return Err(LipError::InvalidPackageRef(input.into())), None => (raw, None) };
    let source = source.strip_prefix("https://").or_else(|| source.strip_prefix("http://")).unwrap_or(source);
    let (location, variant) = source.split_once('#').map_or((source, ""), |(a,b)| (a,b));
    let mut parts = location.trim_matches('/').split('/'); let first = parts.next().unwrap_or_default();
    let (id, variant) = if first.contains('.') {
        let owner = parts.next().unwrap_or_default(); let repo = parts.next().unwrap_or_default(); if parts.next().is_some() { return Err(LipError::InvalidPackageRef(input.into())); }
        (format!("{first}/{owner}/{repo}"), if variant.is_empty() { "client" } else { variant })
    } else {
        let owner = first; let repo = parts.next().unwrap_or_default(); if parts.next().is_some() { return Err(LipError::InvalidPackageRef(input.into())); }
        (format!("{owner}/{repo}"), if variant.is_empty() { "client" } else { variant })
    };
    validate_id(&id).map_err(|_| LipError::InvalidPackageRef(input.into()))?; validate_variant(variant).map_err(|_| LipError::InvalidPackageRef(input.into()))?;
    if let Some(v) = &version { semver::Version::parse(v).map_err(|_| LipError::InvalidPackageRef(input.into()))?; }
    Ok(PackageRef { id: id.to_ascii_lowercase(), variant: variant.to_owned(), version })
}

pub fn read_state(workspace: &Path) -> Result<State, LipError> {
    if !workspace.is_dir() { return Err(LipError::InvalidWorkspace); }
    let path = workspace.join(".lip/state.json"); if !path.exists() { return Ok(State::empty("client", "win-x64")); }
    let state: State = serde_json::from_slice(&fs::read(path)?)?; state.validate()?; Ok(state)
}

pub fn acquire_workspace_lock(workspace: &Path) -> Result<WorkspaceLock, LipError> {
    if !workspace.is_dir() { return Err(LipError::InvalidWorkspace); }
    let dir = workspace.join(".lip"); fs::create_dir_all(&dir)?;
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join("lock"))?; lock_file(&file)?; Ok(WorkspaceLock { _file: file })
}
pub struct WorkspaceLock { _file: File }

pub fn write_state_atomic(workspace: &Path, state: &State) -> Result<(), LipError> {
    if !workspace.is_dir() { return Err(LipError::InvalidWorkspace); } state.validate()?;
    let dir = workspace.join(".lip"); fs::create_dir_all(&dir)?; let staged = dir.join("state.json.tmp"); let target = dir.join("state.json");
    let mut file = File::create(&staged)?; file.write_all(&serde_json::to_vec_pretty(state)?)?; file.sync_all()?; replace_file(&staged, &target)
}

fn validate_manifest(m: &PackageManifest) -> Result<(), LipError> {
    if m.format != FORMAT || validate_id(&m.id).is_err() || semver::Version::parse(&m.version).is_err() { return Err(LipError::InvalidState("invalid package manifest".into())); }
    for (variant, entries) in &m.variants { validate_variant(variant)?; if entries.is_empty() { return Err(LipError::InvalidState("empty variant entries".into())); } } Ok(())
}
fn validate_id(id: &str) -> Result<(), ()> {
    if id.is_empty() || id.starts_with('/') || id.ends_with('/') || id.contains("..") || id.contains('\\') { return Err(()); }
    let parts: Vec<_> = id.split('/').collect(); if parts.len() < 2 || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_~".contains(&b))) { return Err(()); } Ok(())
}
fn validate_variant(v: &str) -> Result<(), LipError> {
    if v.is_empty() || (v.split('/').count() <= 2 && v.split('/').all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))) { Ok(()) } else { Err(LipError::InvalidState("invalid package variant".into())) }
}
fn validate_workspace_path(raw: &str) -> Result<(), LipError> {
    let path = Path::new(raw); if raw.is_empty() || raw.contains('\\') || path.is_absolute() || path.components().any(|c| !matches!(c, Component::Normal(_))) || raw.split('/').any(|s| s == ".lip") { return Err(LipError::InvalidState("unsafe package-owned path".into())); } Ok(())
}
fn package_identity(id: &str, variant: &str) -> String { if variant.is_empty() { id.into() } else { format!("{id}#{variant}") } }

#[cfg(windows)]
fn lock_file(file: &File) -> Result<(), LipError> {
    use std::os::windows::io::AsRawHandle; use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY};
    use windows::Win32::System::IO::OVERLAPPED;
    let mut overlapped = OVERLAPPED::default();
    unsafe { LockFileEx(HANDLE(file.as_raw_handle()), LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY, 0, u32::MAX, u32::MAX, &mut overlapped) }.map_err(|_| LipError::WorkspaceLocked)
}
#[cfg(not(windows))]
fn lock_file(file: &File) -> Result<(), LipError> {
    use std::os::fd::AsRawFd; let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }; if result == 0 { Ok(()) } else { Err(LipError::WorkspaceLocked) }
}
#[cfg(windows)]
fn replace_file(staged: &Path, target: &Path) -> Result<(), LipError> {
    if target.exists() { fs::remove_file(target)?; } fs::rename(staged, target)?; Ok(())
}
#[cfg(not(windows))]
fn replace_file(staged: &Path, target: &Path) -> Result<(), LipError> { fs::rename(staged, target)?; Ok(()) }

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> State {
        let manifest = PackageManifest { format: 4, id: "github.com/example/mod".into(), version: "1.2.3".into(), variants: HashMap::from([("client".into(), vec![PackageEntry { side: "client".into(), platform: "win-x64".into(), deps: HashMap::new(), extra: Default::default() }])]), extra: Default::default() };
        State { format: 4, side: "client".into(), platform: "win-x64".into(), packages: vec![InstalledPackage { manifest, variant: "client".into(), explicit: true, source_ref: "github.com/example/mod#client".into(), commit: None, files: vec!["mods/mod/file.dll".into()] }] }
    }
    #[test] fn parses_package_refs_and_rejects_bad_versions() {
        assert_eq!(parse_package_ref("https://github.com/LiteLDev/LeviLamina#client@1.2.3").unwrap(), PackageRef { id: "github.com/liteldev/levilamina".into(), variant: "client".into(), version: Some("1.2.3".into()) });
        assert!(parse_package_ref("owner/repo@latest").is_err()); assert!(parse_package_ref("owner/repo#Bad").is_err());
    }
    #[test] fn list_state_has_official_format_and_preserves_install_order() {
        let state = sample(); let list = state.list_json().unwrap(); assert_eq!(list["ExplicitInstalled"][0], "github.com/example/mod#client@1.2.3"); assert!(state.package_states().unwrap()["github.com/example/mod#client"].explicit_installed);
    }
    #[test] fn validates_v4_and_rejects_unsafe_owned_paths() {
        let mut state = sample(); assert!(state.validate().is_ok()); state.packages[0].files = vec!["../escape".into()]; assert!(state.validate().is_err()); state.packages[0].files = vec!["mods/good/file".into()]; state.format = 3; assert!(state.validate().is_err());
    }
    #[test] fn state_round_trips_atomically_and_lock_is_exclusive() {
        let root = std::env::temp_dir().join(format!("lip-module-test-{}", uuid::Uuid::new_v4())); fs::create_dir_all(&root).unwrap();
        let first = acquire_workspace_lock(&root).unwrap(); assert!(matches!(acquire_workspace_lock(&root), Err(LipError::WorkspaceLocked)));
        let state = sample(); write_state_atomic(&root, &state).unwrap(); assert_eq!(read_state(&root).unwrap(), state); drop(first); assert!(acquire_workspace_lock(&root).is_ok()); fs::remove_dir_all(root).unwrap();
    }
}
