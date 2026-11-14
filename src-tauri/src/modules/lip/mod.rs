//! 独立 Rust LIP 兼容核心（format 4）。
//!
//! 覆盖清单解析、依赖求解、资产下载、事务安装/卸载、hooks 与状态提交；
//! 不依赖 .NET / lipd，也不改动上游协议定义。

pub mod archive;
pub mod git;
pub mod manifest;
pub mod range;
pub mod remote;
pub mod resolve;
pub mod source;
pub mod workspace;

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use serde::{Deserialize, Serialize};

use manifest::PackageFile;
use remote::RemoteClient;
use source::PackageSource;

pub const FORMAT: u32 = 4;

/// 目标运行环境：side 与 .NET RID。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    pub side: String,
    pub platform: String,
}

impl Environment {
    pub fn new(side: impl Into<String>, platform: impl Into<String>) -> Self {
        Self { side: side.into(), platform: platform.into() }
    }

    pub fn current() -> Self {
        Self::new("client", default_platform())
    }
}

pub fn default_platform() -> String {
    if cfg!(target_os = "windows") {
        if cfg!(target_arch = "aarch64") {
            "win-arm64".into()
        } else {
            "win-x64".into()
        }
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "osx-arm64".into()
        } else {
            "osx-x64".into()
        }
    } else if cfg!(target_arch = "aarch64") {
        "linux-arm64".into()
    } else {
        "linux-x64".into()
    }
}

/// 协作式取消标记。
#[derive(Debug, Default)]
pub struct Cancel(AtomicBool);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn check(&self) -> Result<(), LipError> {
        if self.is_cancelled() {
            Err(LipError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// 进度/日志事件。
#[derive(Debug, Clone)]
pub enum LipEvent {
    Stage(&'static str),
    Log(String),
    Progress { item: String, percent: Option<f64> },
}

#[derive(Clone)]
pub struct Sink(Arc<dyn Fn(LipEvent) + Send + Sync>);

impl Sink {
    pub fn new(callback: impl Fn(LipEvent) + Send + Sync + 'static) -> Self {
        Self(Arc::new(callback))
    }

    pub fn noop() -> Self {
        Self(Arc::new(|_| {}))
    }

    pub fn emit(&self, event: LipEvent) {
        (self.0)(event);
    }

    pub fn stage(&self, stage: &'static str) {
        self.emit(LipEvent::Stage(stage));
    }

    pub fn log(&self, message: impl Into<String>) {
        self.emit(LipEvent::Log(message.into()));
    }

    pub fn progress(&self, item: &str, percent: Option<f64>) {
        self.emit(LipEvent::Progress { item: item.to_string(), percent });
    }
}

impl Default for Sink {
    fn default() -> Self {
        Self::noop()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct State {
    #[serde(default = "default_format")]
    pub format: u32,
    #[serde(default)]
    pub side: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub packages: Vec<InstalledPackage>,
}

fn default_format() -> u32 {
    FORMAT
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstalledPackage {
    pub manifest: PackageFile,
    pub variant: String,
    #[serde(default)]
    pub explicit: bool,
    #[serde(default, rename = "ref")]
    pub source_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageState {
    pub installed: bool,
    pub explicit_installed: bool,
    pub installed_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRef {
    pub id: String,
    pub variant: String,
    pub version: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LipError {
    #[error("invalid lip package reference: {0}")]
    InvalidPackageRef(String),
    #[error("invalid version range: {0}")]
    InvalidRange(String),
    #[error("invalid package manifest: {0}")]
    InvalidManifest(String),
    #[error("invalid package reference: {0}")]
    InvalidReference(String),
    #[error("invalid LIP state: {0}")]
    InvalidState(String),
    #[error("package not found: {0}")]
    PackageNotFound(String),
    #[error("network request failed: {0}")]
    Network(String),
    #[error("archive error: {0}")]
    Archive(String),
    #[error("package conflict: {0}")]
    Conflict(String),
    #[error("hook failed: {0}")]
    HookFailed(String),
    #[error("operation cancelled")]
    Cancelled,
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("workspace is locked by another operation")]
    WorkspaceLocked,
    #[error("workspace path is not a directory")]
    InvalidWorkspace,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

impl State {
    pub fn empty(side: impl Into<String>, platform: impl Into<String>) -> Self {
        Self { format: FORMAT, side: side.into(), platform: platform.into(), packages: Vec::new() }
    }

    pub fn validate(&self) -> Result<(), LipError> {
        if self.format != FORMAT {
            return Err(LipError::InvalidState(format!("unsupported format {}; expected {FORMAT}", self.format)));
        }
        if !matches!(self.side.as_str(), "client" | "server") || self.platform.is_empty() || matches!(self.platform.as_str(), "client" | "server") {
            return Err(LipError::InvalidState("invalid side or platform".into()));
        }
        let mut identities = HashSet::new();
        for package in &self.packages {
            validate_manifest(&package.manifest)?;
            validate_variant(&package.variant)?;
            if !identities.insert(identity(&package.manifest.id, &package.variant).to_ascii_lowercase()) {
                return Err(LipError::InvalidState("duplicate package identity".into()));
            }
            if let Some(commit) = &package.commit {
                if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) {
                    return Err(LipError::InvalidState("invalid commit".into()));
                }
            }
            for path in &package.files {
                validate_workspace_path(path)?;
            }
        }
        Ok(())
    }

    pub fn list_json(&self) -> Result<serde_json::Value, LipError> {
        self.validate()?;
        let mut explicit = Vec::new();
        let mut implicit = Vec::new();
        for package in &self.packages {
            let item = format!("{}@{}", identity(&package.manifest.id, &package.variant), package.manifest.version);
            if package.explicit {
                explicit.push(item);
            } else {
                implicit.push(item);
            }
        }
        Ok(serde_json::json!({"ExplicitInstalled": explicit, "ImplicitInstalled": implicit}))
    }

    pub fn package_states(&self) -> Result<HashMap<String, PackageState>, LipError> {
        self.validate()?;
        let mut states = HashMap::new();
        for package in &self.packages {
            let state = PackageState {
                installed: true,
                explicit_installed: package.explicit,
                installed_version: package.manifest.version.clone(),
            };
            states.insert(identity(&package.manifest.id, &package.variant).to_ascii_lowercase(), state.clone());
            if package.variant.is_empty() {
                states.insert(package.manifest.id.to_ascii_lowercase(), state);
            }
        }
        Ok(states)
    }

    pub fn find(&self, id: &str, variant: &str) -> Option<&InstalledPackage> {
        self.packages.iter().find(|package| package.manifest.id.eq_ignore_ascii_case(id) && package.variant == variant)
    }
}

pub fn identity(id: &str, variant: &str) -> String {
    resolve::identity(id, variant)
}

/// 兼容旧调用：解析 owner/repo、github.com/owner/repo 与 #variant@version 形式。
pub fn parse_package_ref(input: &str) -> Result<PackageRef, LipError> {
    let source = PackageSource::parse(input)?;
    let id = source.identity();
    let version = match source.reference {
        Some(source::GitRef::Version(version)) => Some(version.to_string()),
        Some(source::GitRef::Latest) => None,
        Some(source::GitRef::Tag(tag)) | Some(source::GitRef::Branch(tag)) => Some(tag),
        Some(source::GitRef::Commit(commit)) => Some(commit),
        None => None,
    };
    Ok(PackageRef { id, variant: source.variant, version })
}

pub fn read_state(workspace: &Path) -> Result<State, LipError> {
    if !workspace.is_dir() {
        return Err(LipError::InvalidWorkspace);
    }
    let path = workspace.join(".lip/state.json");
    if !path.exists() {
        return Ok(State::empty("client", default_platform()));
    }
    let state: State = serde_json::from_slice(&fs::read(path)?)?;
    state.validate()?;
    Ok(state)
}

pub fn acquire_workspace_lock(workspace: &Path) -> Result<WorkspaceLock, LipError> {
    if !workspace.is_dir() {
        return Err(LipError::InvalidWorkspace);
    }
    let dir = workspace.join(".lip");
    fs::create_dir_all(&dir)?;
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join("lock"))?;
    lock_file(&file)?;
    Ok(WorkspaceLock { _file: file })
}

pub struct WorkspaceLock {
    _file: File,
}

pub fn write_state_atomic(workspace: &Path, state: &State) -> Result<(), LipError> {
    if !workspace.is_dir() {
        return Err(LipError::InvalidWorkspace);
    }
    state.validate()?;
    let dir = workspace.join(".lip");
    fs::create_dir_all(&dir)?;
    let staged = dir.join("state.json.tmp");
    let target = dir.join("state.json");
    let mut file = File::create(&staged)?;
    file.write_all(&serde_json::to_vec_pretty(state)?)?;
    file.sync_all()?;
    replace_file(&staged, &target)
}

fn validate_manifest(manifest: &PackageFile) -> Result<(), LipError> {
    if manifest.format != FORMAT || !manifest::valid_id(&manifest.id) || semver::Version::parse(&manifest.version).is_err() {
        return Err(LipError::InvalidState("invalid package manifest".into()));
    }
    for (variant, entries) in &manifest.variants {
        validate_variant(variant)?;
        if entries.is_empty() {
            return Err(LipError::InvalidState("empty variant entries".into()));
        }
    }
    Ok(())
}

fn validate_variant(variant: &str) -> Result<(), LipError> {
    if manifest::valid_variant(variant) {
        Ok(())
    } else {
        Err(LipError::InvalidState("invalid package variant".into()))
    }
}

fn validate_workspace_path(raw: &str) -> Result<(), LipError> {
    let path = Path::new(raw);
    if raw.is_empty() || raw.contains('\\') || path.is_absolute() || path.components().any(|component| !matches!(component, Component::Normal(_))) {
        return Err(LipError::InvalidState("unsafe package-owned path".into()));
    }
    if raw.split('/').any(|segment| segment == ".lip") {
        return Err(LipError::InvalidState("unsafe package-owned path".into()));
    }
    Ok(())
}

/// 面向调用方的引擎：解析清单、求解、下载、事务安装。
pub struct Engine {
    pub environment: Environment,
    pub remote: RemoteClient,
}

impl Engine {
    pub fn new(environment: Environment, cache_dir: PathBuf) -> Result<Self, LipError> {
        let remote = RemoteClient::new(cache_dir, default_mirrors())?;
        Ok(Self { environment, remote })
    }

    pub fn with_remote(environment: Environment, remote: RemoteClient) -> Self {
        Self { environment, remote }
    }

    pub fn state(&self, workspace: &Path) -> Result<State, LipError> {
        read_state(workspace)
    }

    pub fn list(&self, workspace: &Path) -> Result<serde_json::Value, LipError> {
        self.state(workspace)?.list_json()
    }

    /// 安装包。dependencies=false 只装点名的包（内容下载装模组用），
    /// true 展开完整依赖闭包（游戏下载补装加载器用）。
    pub async fn install(
        &self,
        workspace: &Path,
        packages: &[String],
        refresh: bool,
        dependencies: bool,
        sink: &Sink,
        cancel: &Cancel,
    ) -> Result<State, LipError> {
        let requests = parse_sources(packages)?;
        let state = read_state(workspace)?;
        let plan = resolve::plan_install(&self.remote, &self.environment, &state, &requests, refresh, dependencies, cancel, sink).await?;
        if plan.installs.is_empty() && plan.removals.is_empty() {
            sink.log("所有请求的包都已是最新");
            return Ok(state);
        }
        workspace::Applier { workspace, environment: &self.environment, client: &self.remote, sink, cancel }
            .apply(plan, &state)
            .await
    }

    /// 更新：packages 为空时更新所有显式安装的包。
    pub async fn update(
        &self,
        workspace: &Path,
        packages: &[String],
        dependencies: bool,
        sink: &Sink,
        cancel: &Cancel,
    ) -> Result<State, LipError> {
        let state = read_state(workspace)?;
        let refs: Vec<String> = if packages.is_empty() {
            state.packages.iter().filter(|package| package.explicit).map(|package| package.source_ref.clone()).collect()
        } else {
            packages.to_vec()
        };
        if refs.is_empty() {
            sink.log("没有可更新的包");
            return Ok(state);
        }
        self.install(workspace, &refs, true, dependencies, sink, cancel).await
    }

    pub async fn uninstall(&self, workspace: &Path, packages: &[String], no_dependencies: bool, sink: &Sink, cancel: &Cancel) -> Result<State, LipError> {
        cancel.check()?;
        let state = read_state(workspace)?;
        let plan = resolve::plan_uninstall(&state, packages, no_dependencies)?;
        if plan.removals.is_empty() {
            sink.log("没有匹配的已安装包");
            return Ok(state);
        }
        workspace::Applier { workspace, environment: &self.environment, client: &self.remote, sink, cancel }
            .apply(plan, &state)
            .await
    }
}

fn parse_sources(packages: &[String]) -> Result<Vec<PackageSource>, LipError> {
    packages.iter().map(|package| PackageSource::parse(package)).collect()
}

fn default_mirrors() -> Vec<String> {
    let mut mirrors = Vec::new();
    if let Ok(value) = std::env::var("COPPER_LIP_MIRROR") {
        mirrors.extend(value.split(',').map(|item| item.trim().trim_end_matches('/').to_string()).filter(|item| !item.is_empty()));
    }
    mirrors
}

#[cfg(windows)]
fn lock_file(file: &File) -> Result<(), LipError> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY};
    use windows::Win32::System::IO::OVERLAPPED;
    let mut overlapped = OVERLAPPED::default();
    unsafe { LockFileEx(HANDLE(file.as_raw_handle()), LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY, 0, u32::MAX, u32::MAX, &mut overlapped) }
        .map_err(|_| LipError::WorkspaceLocked)
}

#[cfg(not(windows))]
fn lock_file(file: &File) -> Result<(), LipError> {
    use std::os::fd::AsRawFd;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        Ok(())
    } else {
        Err(LipError::WorkspaceLocked)
    }
}

pub(crate) fn replace_file(staged: &Path, target: &Path) -> Result<(), LipError> {
    #[cfg(windows)]
    if target.exists() {
        fs::remove_file(target)?;
    }
    fs::rename(staged, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> State {
        let toml = r#"
format = 4
id = "github.com/example/mod"
version = "1.2.3"

[info]
name = "Example"

[[variants.client]]
side = "client"
platform = "win-x64"
"#;
        let (manifest, _) = manifest::parse_package_file(toml).unwrap();
        State {
            format: 4,
            side: "client".into(),
            platform: "win-x64".into(),
            packages: vec![InstalledPackage {
                manifest,
                variant: "client".into(),
                explicit: true,
                source_ref: "github.com/example/mod#client".into(),
                commit: None,
                files: vec!["mods/mod/file.dll".into()],
            }],
        }
    }

    #[test]
    fn parses_package_refs_and_rejects_bad_versions() {
        let parsed = parse_package_ref("github.com/LiteLDev/LeviLamina#client@1.2.3").unwrap();
        assert_eq!(parsed.id, "liteldev/levilamina");
        assert_eq!(parsed.variant, "client");
        assert_eq!(parsed.version.as_deref(), Some("1.2.3"));
        assert!(parse_package_ref("owner/repo@latest").is_err());
    }

    #[test]
    fn list_state_has_official_format_and_preserves_install_order() {
        let state = sample();
        let list = state.list_json().unwrap();
        assert_eq!(list["ExplicitInstalled"][0], "github.com/example/mod#client@1.2.3");
        assert!(state.package_states().unwrap()["github.com/example/mod#client"].explicit_installed);
    }

    #[test]
    fn validates_v4_and_rejects_unsafe_owned_paths() {
        let mut state = sample();
        assert!(state.validate().is_ok());
        state.packages[0].files = vec!["../escape".into()];
        assert!(state.validate().is_err());
        state.packages[0].files = vec!["mods/good/file".into()];
        state.format = 3;
        assert!(state.validate().is_err());
    }

    #[test]
    fn state_round_trips_atomically_and_lock_is_exclusive() {
        let root = std::env::temp_dir().join(format!("lip-module-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let first = acquire_workspace_lock(&root).unwrap();
        assert!(matches!(acquire_workspace_lock(&root), Err(LipError::WorkspaceLocked)));
        let state = sample();
        write_state_atomic(&root, &state).unwrap();
        assert_eq!(read_state(&root).unwrap(), state);
        drop(first);
        assert!(acquire_workspace_lock(&root).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    /// 真实网络探针：把一个只含无 label 变体的 self 资产包（GMLIB）装进临时工作区。
    /// 需要联网，默认忽略；用 cargo test ... probe_install_gmlib -- --ignored --nocapture 运行。
    #[tokio::test]
    #[ignore]
    async fn probe_install_gmlib_from_github() {
        let root = std::env::temp_dir().join(format!("lip-probe-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("ws");
        fs::create_dir_all(&workspace).unwrap();
        let cache = root.join("cache");
        fs::create_dir_all(&cache).unwrap();
        let engine = Engine::new(Environment::current(), cache).unwrap();
        let sink = Sink::new(|event| println!("EVENT {event:?}"));
        let cancel = Cancel::new();
        let request = "github.com/GroupMountain/GMLIB-Release#client@26.51.3".to_string();
        let result = engine.install(&workspace, &[request], false, false, &sink, &cancel).await;
        match result {
            Ok(state) => {
                let packages: Vec<_> = state.packages.iter().map(|package| (package.manifest.id.clone(), package.variant.clone(), package.files.clone())).collect();
                println!("PROBE_INSTALL_OK {packages:?}");
            }
            Err(error) => println!("PROBE_INSTALL_ERR {error}"),
        }
        let mut found: Vec<String> = Vec::new();
        walk_files(&workspace, &workspace, &mut found);
        found.sort();
        println!("PROBE_FILES {found:?}");
        let _ = fs::remove_dir_all(&root);
    }

    fn walk_files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_files(root, &path, out);
            } else if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}
