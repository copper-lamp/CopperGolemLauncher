//! 工作区事务：journal、原子 state 提交、文件放置、hooks、卸载与崩溃恢复。
//!
//! 对齐 protocol/lip-install.md 与 lip.journal.md：每条变更先写 journal 再执行；
//! 失败或取消按 journal 逆序撤销；成功先写 Commit 记录，再改名 state.json。

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::archive::{self, ArchiveView};
use super::manifest::{Asset, PackageFile, Place};
use super::remote::RemoteClient;
use super::resolve::{InstallItem, Plan};
use super::source::Source;
use super::{identity, Cancel, Environment, InstalledPackage, LipError, Sink, State};

const JOURNAL: &str = ".lip/journal";
const STATE_FILE: &str = ".lip/state.json";
const STATE_TMP: &str = ".lip/state.json.tmp";
const BACKUP_DIR: &str = ".lip/backup";

/// 把「已下载字节 / 总字节」换算成包内进度（下载段占 90%）。
fn byte_reporter<'a>(report: &'a (dyn Fn(f64, &str) + Sync)) -> impl Fn(u64, Option<u64>) + Send + Sync + 'a {
    move |done, total| {
        let fraction = total
            .filter(|total| *total > 0)
            .map(|total| done as f64 / total as f64)
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        report(fraction * 0.9, &format!("下载 {}", bytes_label(done, total)));
    }
}

fn bytes_label(done: u64, total: Option<u64>) -> String {
    let mb = |value: u64| value as f64 / (1024.0 * 1024.0);
    match total {
        Some(total) if total > 0 => format!("{:.1}/{:.1} MB", mb(done), mb(total)),
        _ => format!("{:.1} MB", mb(done)),
    }
}

/// 资产来源：已解包/已下载的档案，或仓库文件树（self 资产按需取 blob）。
///
/// 原版 lip 的 GitTreeSource 就是这样工作的：只列举文件树，等到真正放置某个文件
/// 时才按 object id 取 blob，而不是整仓下载 zip。
enum AssetSource<'a> {
    Archive(&'a ArchiveView),
    Tree(GitTreeSource),
}

struct GitTreeSource {
    owner: String,
    repo: String,
    tree: Arc<super::git::TreeCache>,
    blobs: HashMap<String, Vec<u8>>,
}

impl<'a> AssetSource<'a> {
    fn files(&self) -> Vec<String> {
        match self {
            AssetSource::Archive(view) => view.files().to_vec(),
            AssetSource::Tree(tree) => tree
                .tree
                .entries
                .iter()
                .filter(|entry| entry.mode & 0o170000 == 0o100000)
                .map(|entry| entry.path.clone())
                .collect(),
        }
    }

    fn strip(&self) -> String {
        match self {
            AssetSource::Archive(view) => common_prefix(view.files()),
            // 文件树路径本身就以仓库根为基准，不像 zip 那样带一层仓库目录。
            AssetSource::Tree(_) => String::new(),
        }
    }

    /// 批量取还缺的 blob；档案来源无需准备。
    async fn prepare(&mut self, paths: &[String], client: &RemoteClient, cancel: &Cancel, report: &(dyn Fn(f64, &str) + Sync)) -> Result<(), LipError> {
        let AssetSource::Tree(tree) = self else {
            return Ok(());
        };
        let mut oids: Vec<String> = Vec::new();
        for path in paths {
            if let Some(entry) = tree.tree.entries.iter().find(|entry| &entry.path == path) {
                if !tree.blobs.contains_key(&entry.oid) && !oids.contains(&entry.oid) {
                    oids.push(entry.oid.clone());
                }
            }
        }
        if oids.is_empty() {
            return Ok(());
        }
        let total = oids.len();
        let mut done = 0usize;
        for batch in oids.chunks(64) {
            cancel.check()?;
            let fetched = client.git_client().fetch_blobs(&tree.owner, &tree.repo, batch, cancel).await?;
            tree.blobs.extend(fetched);
            done += batch.len();
            report(0.1 + 0.8 * (done as f64 / total as f64), "获取仓库文件");
        }
        Ok(())
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, LipError> {
        match self {
            AssetSource::Archive(view) => view.read(path),
            AssetSource::Tree(tree) => tree
                .tree
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .and_then(|entry| tree.blobs.get(&entry.oid).cloned())
                .ok_or_else(|| LipError::Archive(format!("cannot read {path}: blob not fetched"))),
        }
    }
}

pub struct Applier<'a> {
    pub workspace: &'a Path,
    pub environment: &'a Environment,
    pub client: &'a RemoteClient,
    pub sink: &'a Sink,
    pub cancel: &'a Cancel,
}

impl<'a> Applier<'a> {
    /// 应用计划并返回新的状态。调用方负责在此之前读取 state。
    pub async fn apply(&self, plan: Plan, state: &State) -> Result<State, LipError> {
        let _lock = super::acquire_workspace_lock(self.workspace)?;
        Journal::recover(self.workspace)?;

        // 计划整体落一条日志：出问题时能直接从 kernel.log 看清「这次到底装了什么、卸了什么」，
        // 不必从 UI 阶段文案反推。
        log::info!(
            "[lip] 应用计划 workspace={} 安装={} 卸载={} 保留={}",
            self.workspace.display(),
            plan.installs.iter().map(|item| format!("{}@{}", item.identity, item.package.file.version)).collect::<Vec<_>>().join(", "),
            plan.removals.iter().map(|package| format!("{}#{}@{}", package.manifest.id, package.variant, package.manifest.version)).collect::<Vec<_>>().join(", "),
            plan.kept.len(),
        );
        let mut journal = Journal::begin(self.workspace)?;
        let mut owned = ownership(state);
        for package in &plan.removals {
            for path in &package.files {
                owned.remove(&path.to_ascii_lowercase());
            }
        }

        // 先卸载（更新 = 卸载 + 安装，依赖者先走）。
        for package in &plan.removals {
            self.cancel.check()?;
            let key = identity(&package.manifest.id, &package.variant);
            self.sink.stage("uninstall");
            self.sink.log(format!("卸载 {key} {}", package.manifest.version));
            self.uninstall_package(&mut journal, package, state, &owned).await?;
        }

        let mut new_packages: Vec<InstalledPackage> = plan.kept.iter().map(|item| {
            let mut package = item.package.clone();
            package.explicit = item.explicit;
            package
        }).collect();

        // 整体进度按「包」切分：第 index 个包占据 [index/total, (index+1)/total]，
        // 包内再分两段——下载占 90%，落盘解包占 10%。这样进度条只会向前走，
        // 且百分比来自真实字节数与真实文件数，不是按时间估的。
        let total_packages = plan.installs.len().max(1) as f64;
        for (index, item) in plan.installs.iter().enumerate() {
            self.cancel.check()?;
            self.sink.stage("install");
            self.sink.log(format!("安装 {} {}", item.identity, item.package.file.version));
            let base = index as f64 / total_packages;
            let span = 1.0 / total_packages;
            let report = |fraction: f64, detail: &str| {
                let overall = (base + span * fraction.clamp(0.0, 1.0)) * 100.0;
                self.sink.progress(&format!("{} {detail}", item.identity), Some(overall));
            };
            let files = self.install_package(&mut journal, item, &mut owned, &report).await?;
            new_packages.push(InstalledPackage {
                manifest: item.package.file.clone(),
                variant: item.variant.clone(),
                explicit: item.explicit,
                source_ref: item.source.spec(),
                commit: item.package.git_ref.clone(),
                files,
            });
        }

        let mut new_state = State {
            format: super::FORMAT,
            side: self.environment.side.clone(),
            platform: self.environment.platform.clone(),
            packages: new_packages,
        };
        new_state.packages.sort_by_key(|package| identity(&package.manifest.id, &package.variant).to_ascii_lowercase());
        new_state.validate()?;

        let staged = self.workspace.join(STATE_TMP);
        if let Some(parent) = staged.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::create(&staged)?;
        file.write_all(&serde_json::to_vec_pretty(&new_state)?)?;
        file.sync_all()?;
        journal.commit()?;
        super::replace_file(&staged, &self.workspace.join(STATE_FILE))?;
        journal.finish(self.workspace)?;
        self.sink.stage("done");
        Ok(new_state)
    }

    async fn uninstall_package(&self, journal: &mut Journal, package: &InstalledPackage, state: &State, owned: &HashMap<String, String>) -> Result<(), LipError> {
        let entry = entry_for(&package.manifest, &package.variant, self.environment);
        if let Some(entry) = &entry {
            self.run_hooks(journal, &entry.hooks.pre_uninstall, package).await?;
        }
        let preserve = entry.map(|entry| entry.preserve.clone()).unwrap_or_default();
        let key = identity(&package.manifest.id, &package.variant).to_ascii_lowercase();
        for path in &package.files {
            if matches_any(&preserve, path) {
                continue;
            }
            if owned.get(&path.to_ascii_lowercase()).map(|owner| owner != &key).unwrap_or(false) {
                continue;
            }
            journal.delete_file(self.workspace, path)?;
        }
        if let Some(entry) = &entry {
            for pattern in &entry.purge {
                for path in purge_targets(self.workspace, pattern)? {
                    if owned.get(&path.to_ascii_lowercase()).is_some() {
                        continue;
                    }
                    journal.delete_path(self.workspace, &path)?;
                }
            }
            self.run_hooks(journal, &entry.hooks.post_uninstall, package).await?;
        }
        let _ = state;
        Ok(())
    }

    async fn install_package(
        &self,
        journal: &mut Journal,
        item: &InstallItem,
        owned: &mut HashMap<String, String>,
        report: &(dyn Fn(f64, &str) + Sync),
    ) -> Result<Vec<String>, LipError> {
        let key = item.identity.to_ascii_lowercase();
        self.run_hooks_files(journal, &item.entry.hooks.pre_install, item).await?;

        let mut written: Vec<String> = Vec::new();
        for asset in &item.entry.assets {
            self.cancel.check()?;
            match asset {
                Asset::SelfArchive { place } => {
                    let mut source = match item.package.archive.as_ref() {
                        Some(archive) => AssetSource::Archive(archive),
                        None => {
                            let commit = item
                                .package
                                .git_ref
                                .clone()
                                .ok_or_else(|| LipError::InvalidState(format!("{} 的 self 资产缺少提交信息", item.identity)))?;
                            let (owner, repo) = match &item.source.source {
                                Source::Github { owner, repo } => (owner.clone(), repo.clone()),
                                _ => return Err(LipError::Archive("self asset needs a git source".into())),
                            };
                            self.sink.log(format!("读取 {} 的仓库文件树", item.identity));
                            let tree = self.client.git_client().list_tree(&owner, &repo, &commit, self.cancel).await?;
                            AssetSource::Tree(GitTreeSource { owner, repo, tree, blobs: HashMap::new() })
                        }
                    };
                    written.extend(self.place_archive(journal, &mut source, place, &item.entry.preserve, &key, owned, report).await?);
                }
                Asset::Zip { urls, place } => {
                    self.sink.log(format!("下载 {} 的归档 asset", item.identity));
                    let temp = self.client.cache_dir().join(format!("asset-{}.zip", short_hash(&format!("{}-{}", item.identity, urls.join(",")))));
                    let downloader = byte_reporter(report);
                    self.client.download_zip(urls, &temp, self.cancel, Some(&downloader)).await?;
                    let archive = ArchiveView::from_zip(&temp)?;
                    let mut source = AssetSource::Archive(&archive);
                    written.extend(self.place_archive(journal, &mut source, place, &item.entry.preserve, &key, owned, report).await?);
                }
                Asset::Mcmod { urls } => {
                    self.sink.log(format!("下载 {} 的 mcmod", item.identity));
                    let temp = self.client.cache_dir().join(format!("mcmod-{}.zip", short_hash(&format!("{}-{}", item.identity, urls.join(",")))));
                    let downloader = byte_reporter(report);
                    self.client.download_zip(urls, &temp, self.cancel, Some(&downloader)).await?;
                    let archive = ArchiveView::from_zip(&temp)?;
                    let name = mod_name(&archive)?;
                    if self.workspace.join("mods").is_symlink() {
                        return Err(LipError::Conflict("workspace mods is a symlink; refusing to write mods".into()));
                    }
                    let dest = format!("mods/{name}/");
                    let mut source = AssetSource::Archive(&archive);
                    written.extend(self.place_archive(journal, &mut source, &[Place { src: String::new(), dest }], &item.entry.preserve, &key, owned, report).await?);
                }
            }
        }

        self.run_hooks_files(journal, &item.entry.hooks.post_install, item).await?;
        written.sort();
        written.dedup();
        Ok(written)
    }

    async fn place_archive(
        &self,
        journal: &mut Journal,
        source: &mut AssetSource<'_>,
        place: &[Place],
        preserve: &[String],
        owner: &str,
        owned: &mut HashMap<String, String>,
        report: &(dyn Fn(f64, &str) + Sync),
    ) -> Result<Vec<String>, LipError> {
        let mut written = Vec::new();
        let files = source.files();
        let total_files = files.len().max(1) as f64;
        let mut processed = 0usize;
        let strip = source.strip();
        for item in place {
            self.cancel.check()?;
            let matches: Vec<(String, String)> = if item.src.is_empty() || item.src.ends_with('/') {
                let prefix = item.src.trim_end_matches('/');
                let mut out = Vec::new();
                for file in &files {
                    let logical = strip_prefix(&file, &strip);
                    let relative = if prefix.is_empty() { logical } else { match logical.strip_prefix(prefix) { Some(rest) => rest.trim_start_matches('/'), None => continue } };
                    if relative.is_empty() {
                        continue;
                    }
                    out.push((file.clone(), join(&item.dest, relative)));
                }
                out
            } else if item.src.contains(['*', '?', '[']) {
                let fixed = archive::glob_static_prefix(&item.src);
                let mut out = Vec::new();
                for file in &files {
                    let logical = strip_prefix(&file, &strip);
                    if !archive::glob_match(&item.src, logical) {
                        continue;
                    }
                    let relative = logical.strip_prefix(&fixed).unwrap_or(logical).trim_start_matches('/');
                    if relative.is_empty() {
                        continue;
                    }
                    out.push((file.clone(), join(&item.dest, relative)));
                }
                out
            } else {
                let mut out = Vec::new();
                for file in &files {
                    let logical = strip_prefix(&file, &strip);
                    if logical == item.src {
                        out.push((file.clone(), item.dest.clone()));
                    }
                }
                out
            };
            if matches.is_empty() {
                return Err(LipError::Archive(format!("asset source {:?} matched nothing", item.src)));
            }
            let needed: Vec<String> = matches.iter().map(|(path, _)| path.clone()).collect();
            source.prepare(&needed, self.client, self.cancel, report).await?;
            for (origin, dest) in matches {
                let dest = normalize(&dest);
                self.check_destination(&dest)?;
                if let Some(existing) = owned.get(&dest.to_ascii_lowercase()) {
                    if existing != owner {
                        return Err(LipError::Conflict(format!("{dest} is owned by {existing}")));
                    }
                }
                let bytes = source.read(&origin)?;
                if self.workspace.join(&dest).exists() && matches_any(preserve, &dest) {
                    owned.insert(dest.to_ascii_lowercase(), owner.to_string());
                    if !written.contains(&dest) {
                        written.push(dest);
                    }
                    continue;
                }
                journal.write_file(self.workspace, &dest, &bytes)?;
                owned.insert(dest.to_ascii_lowercase(), owner.to_string());
                written.push(dest);
                processed += 1;
                // 解包进度按「已处理文件 / 归档文件总数」计算：这是真实工作量，
                // 与包大小无关（压缩率差很大的包也能匀速前进）。
                if processed % 16 == 0 {
                    report(0.9 + 0.1 * (processed as f64 / total_files).min(1.0), "解包文件");
                }
            }
        }
        Ok(written)
    }

    fn check_destination(&self, dest: &str) -> Result<(), LipError> {
        if dest.is_empty() || dest.split('/').any(|segment| segment == ".lip") {
            return Err(LipError::Conflict(format!("refusing to write reserved path {dest}")));
        }
        if dest.starts_with("/") || dest.contains("..") || dest.contains('\\') {
            return Err(LipError::Conflict(format!("unsafe package path {dest}")));
        }
        if cfg!(windows) {
            for segment in dest.split('/') {
                if segment.is_empty() || segment.contains(':') || segment.ends_with('.') || segment.ends_with(' ') {
                    return Err(LipError::Conflict(format!("path segment is not valid on Windows: {dest}")));
                }
            }
        }
        Ok(())
    }

    async fn run_hooks(&self, journal: &mut Journal, hooks: &[String], package: &InstalledPackage) -> Result<(), LipError> {
        self.run_commands(hooks, &identity(&package.manifest.id, &package.variant), journal).await
    }

    async fn run_hooks_files(&self, journal: &mut Journal, hooks: &[String], item: &InstallItem) -> Result<(), LipError> {
        self.run_commands(hooks, &item.identity, journal).await
    }

    async fn run_commands(&self, hooks: &[String], label: &str, journal: &mut Journal) -> Result<(), LipError> {
        for hook in hooks {
            self.cancel.check()?;
            self.sink.log(format!("{label}: 执行 {hook}"));
            let mut command = if cfg!(windows) {
                let mut command = tokio::process::Command::new("cmd");
                command.arg("/C").arg(hook);
                command
            } else {
                let mut command = tokio::process::Command::new("sh");
                command.arg("-c").arg(hook);
                command
            };
            command.current_dir(self.workspace).stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
            #[cfg(windows)]
            {
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                command.creation_flags(CREATE_NO_WINDOW);
            }
            let child = command.spawn().map_err(|error| LipError::HookFailed(format!("{hook}: {error}")))?;
            let output = tokio::time::timeout(Duration::from_secs(900), child.wait_with_output())
                .await
                .map_err(|_| LipError::HookFailed(format!("{hook}: timeout after 900s")))?
                .map_err(|error| LipError::HookFailed(format!("{hook}: {error}")))?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            for line in stdout.lines().chain(stderr.lines()) {
                if !line.trim().is_empty() {
                    self.sink.log(line.to_string());
                }
            }
            if !output.status.success() {
                return Err(LipError::HookFailed(format!("{hook} exited with {}", output.status.code().map(|code| code.to_string()).unwrap_or_else(|| "unknown".into()))));
            }
            let _ = journal;
        }
        Ok(())
    }
}

fn entry_for<'a>(manifest: &'a PackageFile, variant: &str, environment: &Environment) -> Option<&'a super::manifest::Entry> {
    manifest
        .entry(variant, environment)
        .or_else(|| manifest.variants.get(variant).and_then(|entries| entries.iter().find(|entry| entry.side == environment.side)))
        .or_else(|| manifest.variants.values().flatten().next())
}

fn ownership(state: &State) -> HashMap<String, String> {
    let mut owned = HashMap::new();
    for package in &state.packages {
        let key = identity(&package.manifest.id, &package.variant).to_ascii_lowercase();
        for path in &package.files {
            owned.insert(path.to_ascii_lowercase(), key.clone());
        }
    }
    owned
}

fn matches_any(patterns: &[String], path: &str) -> bool {
    patterns.iter().any(|pattern| archive::glob_match(pattern, path))
}

fn purge_targets(workspace: &Path, pattern: &str) -> Result<Vec<String>, LipError> {
    let mut out = Vec::new();
    let prefix = archive::glob_static_prefix(pattern);
    let base = if prefix.is_empty() { workspace.to_path_buf() } else { workspace.join(prefix.replace('/', std::path::MAIN_SEPARATOR_STR)) };
    if !base.exists() {
        return Ok(out);
    }
    let mut stack = vec![base];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let Some(relative) = path.strip_prefix(workspace).ok().map(|path| path.to_string_lossy().replace('\\', "/")) else { continue };
            if relative.starts_with(".lip") {
                continue;
            }
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                if archive::glob_match(pattern, &format!("{relative}/")) {
                    out.push(relative);
                } else {
                    stack.push(path);
                }
            } else if archive::glob_match(pattern, &relative) {
                out.push(relative);
            }
        }
    }
    Ok(out)
}

fn mod_name(archive: &ArchiveView) -> Result<String, LipError> {
    let candidates: Vec<&String> = archive.files().iter().filter(|file| file.as_str() == "manifest.json" || file.ends_with("/manifest.json") || file.ends_with("pack_manifest.json")).collect();
    let Some(path) = candidates.first() else {
        return Err(LipError::Archive("mcmod archive has no manifest.json".into()));
    };
    let bytes = archive.read(path)?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| LipError::Archive(format!("mcmod manifest.json: {error}")))?;
    let header = json.get("header").unwrap_or(&json);
    let name = header.get("name").and_then(|value| value.as_str()).unwrap_or_default();
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) || name == "." || name == ".." {
        return Err(LipError::Archive("mcmod manifest.json has an invalid name".into()));
    }
    Ok(name.to_string())
}

fn common_prefix(files: &[String]) -> String {
    let Some(first) = files.first() else { return String::new() };
    let Some((head, _)) = first.split_once('/') else { return String::new() };
    if files.iter().all(|file| file.starts_with(&format!("{head}/"))) {
        head.to_string()
    } else {
        String::new()
    }
}

fn strip_prefix<'a>(file: &'a str, prefix: &str) -> &'a str {
    if prefix.is_empty() {
        file
    } else {
        file.strip_prefix(&format!("{prefix}/")).unwrap_or(file)
    }
}

fn join(dest: &str, relative: &str) -> String {
    if dest.is_empty() {
        relative.to_string()
    } else if dest.ends_with('/') {
        format!("{dest}{relative}")
    } else {
        format!("{dest}/{relative}")
    }
}

fn normalize(path: &str) -> String {
    path.split('/').filter(|segment| !segment.is_empty() && *segment != ".").collect::<Vec<_>>().join("/")
}

fn short_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    digest[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum Record {
    Begin,
    CreateDirectory { path: String },
    CreateFile { path: String },
    Backup { path: String, to: String, #[serde(default)] hidden: bool },
    DeleteDirectoryOnCommit { path: String },
    Commit,
}

struct Journal {
    file: File,
    records: Vec<Record>,
    backup_seq: u64,
}

impl Journal {
    fn begin(workspace: &Path) -> Result<Self, LipError> {
        let path = workspace.join(JOURNAL);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).write(true).truncate(true).open(&path)?;
        let mut journal = Self { file, records: Vec::new(), backup_seq: 0 };
        journal.push(&Record::Begin)?;
        Ok(journal)
    }

    fn push(&mut self, record: &Record) -> Result<(), LipError> {
        let mut line = serde_json::to_vec(record)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.flush()?;
        self.file.sync_data()?;
        self.records.push(record.clone());
        Ok(())
    }

    fn create_directory(&mut self, workspace: &Path, path: &str) -> Result<(), LipError> {
        let target = workspace.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if target.is_dir() {
            return Ok(());
        }
        self.push(&Record::CreateDirectory { path: path.to_string() })?;
        fs::create_dir_all(&target)?;
        Ok(())
    }

    fn write_file(&mut self, workspace: &Path, relative: &str, bytes: &[u8]) -> Result<(), LipError> {
        let safe = archive::safe_relative(relative).ok_or_else(|| LipError::Conflict(format!("unsafe path {relative}")))?;
        let target = workspace.join(&safe);
        let parent_relative = relative.rsplit_once('/').map(|(parent, _)| parent.to_string());
        if let Some(parent) = &parent_relative {
            self.create_directory(workspace, parent)?;
        }
        if target.exists() {
            self.backup(workspace, relative)?;
        }
        self.push(&Record::CreateFile { path: relative.to_string() })?;
        let mut file = File::create(&target)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }

    fn backup(&mut self, workspace: &Path, relative: &str) -> Result<(), LipError> {
        self.backup_seq += 1;
        let to = format!("{BACKUP_DIR}/{}-{}", self.backup_seq, short_hash(relative));
        let target = workspace.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        let backup_path = workspace.join(to.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = backup_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&target, &backup_path)?;
        self.push(&Record::Backup { path: relative.to_string(), to, hidden: false })
    }

    fn delete_file(&mut self, workspace: &Path, relative: &str) -> Result<(), LipError> {
        let target = workspace.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        if !target.exists() {
            return Ok(());
        }
        self.backup(workspace, relative)
    }

    fn delete_path(&mut self, workspace: &Path, relative: &str) -> Result<(), LipError> {
        let target = workspace.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        if target.is_dir() {
            self.push(&Record::DeleteDirectoryOnCommit { path: relative.to_string() })
        } else if target.exists() {
            self.delete_file(workspace, relative)
        } else {
            Ok(())
        }
    }

    fn commit(&mut self) -> Result<(), LipError> {
        self.push(&Record::Commit)
    }

    fn finish(self, workspace: &Path) -> Result<(), LipError> {
        finalize_commit(workspace)?;
        Ok(())
    }

    /// 启动时恢复：有 Commit 记录则完成提交，否则逆序撤销。
    fn recover(workspace: &Path) -> Result<(), LipError> {
        let path = workspace.join(JOURNAL);
        if !path.exists() {
            return Ok(());
        }
        let records = read_records(&path)?;
        if records.iter().any(|record| matches!(record, Record::Commit)) {
            finalize_commit(workspace)?;
        } else {
            undo(workspace, &records)?;
            let _ = fs::remove_file(&path);
            let _ = fs::remove_dir(workspace.join(BACKUP_DIR));
        }
        Ok(())
    }
}

fn read_records(path: &Path) -> Result<Vec<Record>, LipError> {
    let file = File::open(path)?;
    let mut records = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<Record>(&line) {
            records.push(record);
        }
    }
    Ok(records)
}

fn undo(workspace: &Path, records: &[Record]) -> Result<(), LipError> {
    for record in records.iter().rev() {
        match record {
            Record::CreateFile { path } => {
                let target = workspace.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
                if target.is_file() {
                    let _ = fs::remove_file(target);
                }
            }
            Record::Backup { path, to, .. } => {
                let source = workspace.join(to.replace('/', std::path::MAIN_SEPARATOR_STR));
                let target = workspace.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
                if source.exists() {
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    if target.exists() {
                        let _ = fs::remove_file(&target);
                    }
                    fs::rename(&source, &target)?;
                }
            }
            Record::CreateDirectory { path } => {
                let target = workspace.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
                let _ = fs::remove_dir(target);
            }
            _ => {}
        }
    }
    Ok(())
}

fn finalize_commit(workspace: &Path) -> Result<(), LipError> {
    let staged = workspace.join(STATE_TMP);
    if staged.exists() {
        super::replace_file(&staged, &workspace.join(STATE_FILE))?;
    }
    let records = read_records(&workspace.join(JOURNAL))?;
    for record in records {
        if let Record::DeleteDirectoryOnCommit { path } = record {
            let target = workspace.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            let _ = fs::remove_dir_all(target);
        }
    }
    let backup = workspace.join(BACKUP_DIR);
    if backup.exists() {
        let _ = fs::remove_dir_all(&backup);
    }
    let _ = fs::remove_file(workspace.join(JOURNAL));
    Ok(())
}

/// 供 Engine 复用：确保 .lip 目录存在。
pub fn ensure_lip_dir(workspace: &Path) -> Result<(), LipError> {
    if !workspace.is_dir() {
        return Err(LipError::InvalidWorkspace);
    }
    fs::create_dir_all(workspace.join(".lip"))?;
    Ok(())
}

/// 工作区已安装文件索引（供 UI / 冲突检查）。
pub fn installed_files(state: &State) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for package in &state.packages {
        let key = identity(&package.manifest.id, &package.variant).to_ascii_lowercase();
        for path in &package.files {
            map.insert(path.clone(), key.clone());
        }
    }
    map
}
