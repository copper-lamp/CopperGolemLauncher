//! 远端访问：GitHub 清单/标签/归档下载、镜像回退、本地档案。
//!
//! 只使用公开 HTTP 接口；不依赖 lip 可执行文件，也不复用上游源码。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value as Json;
use tokio::io::AsyncWriteExt;

use super::archive::ArchiveView;
use super::manifest::{self, PackageFile};
use super::source::{GitRef, PackageSource, Source};
use super::{Cancel, Environment, LipError};

const USER_AGENT: &str = "CopperGolem-LIP/1.0 (+https://github.com/CopperGolem)";
const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// 远端或本地解析出的包。
pub struct RemotePackage {
    pub file: PackageFile,
    /// 规范化的 format 4 JSON（写入 state.json 的 manifest）。
    pub json: Json,
    /// 解析到的 git 引用（tag/branch/commit），本地包为 None。
    pub git_ref: Option<String>,
    /// 仅在包含 self 资产时加载；否则为 None。
    pub archive: Option<ArchiveView>,
}

pub struct RemoteClient {
    http: reqwest::Client,
    cache_dir: PathBuf,
    mirrors: Vec<String>,
    tags: Mutex<HashMap<String, Vec<String>>>,
}

impl RemoteClient {
    pub fn new(cache_dir: PathBuf, mirrors: Vec<String>) -> Result<Self, LipError> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(120))
            .redirect(reqwest::redirect::Policy::limited(8))
            .build()
            .map_err(|error| LipError::Network(error.to_string()))?;
        Ok(Self { http, cache_dir, mirrors, tags: Mutex::new(HashMap::new()) })
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// 解析一个包引用为可安装的包。
    pub async fn fetch_package(&self, source: &PackageSource, environment: &Environment, cancel: &Cancel) -> Result<RemotePackage, LipError> {
        match &source.source {
            Source::Github { owner, repo } => self.fetch_github(owner, repo, source, environment, cancel).await,
            Source::Local { path } => {
                let (archive, text) = if path.is_dir() {
                    let archive = ArchiveView::from_directory(path)?;
                    let text = read_manifest_from_dir(path)?;
                    (archive, text)
                } else {
                    let archive = ArchiveView::from_zip(path)?;
                    let text = read_manifest_from_archive(&archive)?;
                    (archive, text)
                };
                let file = self.parse_manifest(&text, environment)?;
                let needs_archive = file.variants.values().flatten().any(|entry| entry.assets.iter().any(|asset| matches!(asset, manifest::Asset::SelfArchive { .. })));
                let archive = if needs_archive { Some(archive) } else { None };
                let json = serde_json::to_value(&file).map_err(LipError::Json)?;
                Ok(RemotePackage { file, json, git_ref: None, archive })
            }
            Source::ArchiveUrl { url } => {
                cancel.check()?;
                let temp = self.cache_dir.join(format!("archive-{}.zip", short_hash(url)));
                self.download_zip(&[url.clone()], &temp, cancel).await?;
                let archive = ArchiveView::from_zip(&temp)?;
                let text = read_manifest_from_archive(&archive)?;
                let file = self.parse_manifest(&text, environment)?;
                let needs_archive = file.variants.values().flatten().any(|entry| entry.assets.iter().any(|asset| matches!(asset, manifest::Asset::SelfArchive { .. })));
                let json = serde_json::to_value(&file).map_err(LipError::Json)?;
                Ok(RemotePackage { file, json, git_ref: None, archive: if needs_archive { Some(archive) } else { None } })
            }
        }
    }

    /// 列出包的可选版本（仅 GitHub 仓库；本地包返回自身版本）。
    pub async fn versions(&self, source: &PackageSource, cancel: &Cancel) -> Result<Vec<semver::Version>, LipError> {
        match &source.source {
            Source::Github { owner, repo } => {
                let tags = self.tags(owner, repo, cancel).await?;
                let mut versions: Vec<semver::Version> = tags
                    .iter()
                    .filter_map(|tag| semver::Version::parse(tag.trim_start_matches('v')).ok())
                    .collect();
                versions.sort();
                versions.dedup();
                Ok(versions)
            }
            Source::Local { path } => {
                let environment = Environment::current();
                let (text, _) = if path.is_dir() {
                    (read_manifest_from_dir(path)?, ())
                } else {
                    let archive = ArchiveView::from_zip(path)?;
                    (read_manifest_from_archive(&archive)?, ())
                };
                let file = self.parse_manifest(&text, &environment)?;
                Ok(vec![file.version()?])
            }
            Source::ArchiveUrl { .. } => Ok(Vec::new()),
        }
    }

    /// 解析最新发布 tag（优先 releases/latest，其次 tags 排序）。
    pub async fn latest_tag(&self, owner: &str, repo: &str, cancel: &Cancel) -> Result<String, LipError> {
        cancel.check()?;
        let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
        if let Ok(response) = self.http.get(&url).send().await {
            if response.status().is_success() {
                if let Ok(json) = response.json::<Json>().await {
                    if let Some(tag) = json.get("tag_name").and_then(Json::as_str) {
                        return Ok(tag.to_string());
                    }
                }
            }
        }
        let tags = self.tags(owner, repo, cancel).await?;
        let mut versions: Vec<(semver::Version, String)> = tags
            .into_iter()
            .filter_map(|tag| semver::Version::parse(tag.trim_start_matches('v')).ok().map(|version| (version, tag)))
            .collect();
        versions.sort_by(|a, b| a.0.cmp(&b.0));
        versions.pop().map(|(_, tag)| tag).ok_or_else(|| LipError::PackageNotFound(format!("{owner}/{repo}: no released version")))
    }

    /// 仓库 tag 列表，带磁盘缓存（1 小时）。
    pub async fn tags(&self, owner: &str, repo: &str, cancel: &Cancel) -> Result<Vec<String>, LipError> {
        let key = format!("{}/{}", owner.to_ascii_lowercase(), repo.to_ascii_lowercase());
        if let Some(cached) = self.tags.lock().map_err(|_| LipError::Internal("tag cache poisoned".into()))?.get(&key) {
            return Ok(cached.clone());
        }
        let cache_path = self.cache_dir.join("tags").join(format!("{}-{}.json", owner.to_ascii_lowercase(), repo.to_ascii_lowercase()));
        if let Some(tags) = read_tag_cache(&cache_path) {
            if let Ok(mut guard) = self.tags.lock() {
                guard.insert(key.clone(), tags.clone());
            }
            return Ok(tags);
        }
        cancel.check()?;
        let mut tags = Vec::new();
        let mut page = 1;
        loop {
            let url = format!("https://api.github.com/repos/{owner}/{repo}/tags?per_page=100&page={page}");
            let response = self.http.get(&url).send().await.map_err(|error| LipError::Network(format!("{owner}/{repo} tags: {error}")))?;
            if !response.status().is_success() {
                return Err(LipError::Network(format!("{owner}/{repo} tags: HTTP {}", response.status())));
            }
            let json: Json = response.json().await.map_err(|error| LipError::Network(error.to_string()))?;
            let items = json.as_array().ok_or_else(|| LipError::Network("unexpected tags response".into()))?;
            if items.is_empty() {
                break;
            }
            for item in items {
                if let Some(name) = item.get("name").and_then(Json::as_str) {
                    tags.push(name.to_string());
                }
            }
            if items.len() < 100 || page >= 5 {
                break;
            }
            page += 1;
        }
        if let Some(directory) = cache_path.parent() {
            let _ = std::fs::create_dir_all(directory);
        }
        let _ = std::fs::write(&cache_path, serde_json::to_vec(&tags).unwrap_or_default());
        if let Ok(mut guard) = self.tags.lock() {
            guard.insert(key, tags.clone());
        }
        Ok(tags)
    }

    async fn fetch_github(&self, owner: &str, repo: &str, source: &PackageSource, environment: &Environment, cancel: &Cancel) -> Result<RemotePackage, LipError> {
        let git_ref = self.resolve_ref(owner, repo, source.reference.as_ref(), cancel).await?;
        let reference_text = git_ref.as_str();
        let mut file = None;
        let mut json = None;
        let manifest_names = [manifest::PACKAGE_FILE, manifest::LEGACY_PACKAGE_FILE];
        for name in manifest_names {
            let urls = self.manifest_urls(owner, repo, &reference_text, name);
            if let Some(text) = self.try_fetch_text(&urls, cancel).await? {
                let (parsed, parsed_json) = self.parse_manifest_with_json(&text, environment)?;
                file = Some(parsed);
                json = Some(parsed_json);
                break;
            }
        }

        let (file, json, archive) = match (file, json) {
            (Some(file), Some(json)) => (file, json, None),
            _ => {
                let archive = self.download_repo_archive(owner, repo, &reference_text, cancel).await?;
                let text = read_manifest_from_archive(&archive)?;
                let (file, json) = self.parse_manifest_with_json(&text, environment)?;
                (file, json, Some(archive))
            }
        };

        let needs_archive = file.variants.values().flatten().any(|entry| entry.assets.iter().any(|asset| matches!(asset, manifest::Asset::SelfArchive { .. })));
        let archive = match (needs_archive, archive) {
            (true, Some(archive)) => Some(archive),
            (true, None) => Some(self.download_repo_archive(owner, repo, &reference_text, cancel).await?),
            (false, _) => None,
        };
        Ok(RemotePackage { file, json, git_ref: Some(reference_text), archive })
    }

    async fn resolve_ref(&self, owner: &str, repo: &str, reference: Option<&GitRef>, cancel: &Cancel) -> Result<GitRef, LipError> {
        match reference {
            None | Some(GitRef::Latest) => Ok(GitRef::Tag(self.latest_tag(owner, repo, cancel).await?)),
            Some(GitRef::Version(version)) => {
                let candidates = [format!("v{version}"), version.to_string()];
                if let Ok(tags) = self.tags(owner, repo, cancel).await {
                    for candidate in &candidates {
                        if tags.iter().any(|tag| tag == candidate) {
                            return Ok(GitRef::Tag(candidate.clone()));
                        }
                    }
                }
                Ok(GitRef::Tag(candidates[0].clone()))
            }
            Some(reference) => Ok(reference.clone()),
        }
    }

    fn manifest_urls(&self, owner: &str, repo: &str, reference: &str, name: &str) -> Vec<String> {
        let mut urls = Vec::new();
        for mirror in std::iter::once(String::new()).chain(self.mirrors.iter().cloned()) {
            urls.push(format!("{mirror}https://raw.githubusercontent.com/{owner}/{repo}/{reference}/{name}"));
            urls.push(format!("{mirror}https://cdn.jsdelivr.net/gh/{owner}/{repo}@{reference}/{name}"));
            urls.push(format!("{mirror}https://github.com/{owner}/{repo}/raw/{reference}/{name}"));
        }
        urls
    }

    fn archive_urls(&self, owner: &str, repo: &str, reference: &str) -> Vec<String> {
        let mut urls = Vec::new();
        let ref_path = if reference.starts_with("refs/") { reference.to_string() } else { format!("refs/tags/{reference}") };
        for mirror in std::iter::once(String::new()).chain(self.mirrors.iter().cloned()) {
            urls.push(format!("{mirror}https://codeload.github.com/{owner}/{repo}/zip/{ref_path}"));
            urls.push(format!("{mirror}https://github.com/{owner}/{repo}/archive/{reference}.zip"));
        }
        urls
    }

    async fn download_repo_archive(&self, owner: &str, repo: &str, reference: &str, cancel: &Cancel) -> Result<ArchiveView, LipError> {
        let urls = self.archive_urls(owner, repo, reference);
        let temp = self.cache_dir.join(format!("repo-{}-{}-{}.zip", owner.to_ascii_lowercase(), repo.to_ascii_lowercase(), short_hash(reference)));
        self.download_zip(&urls, &temp, cancel).await?;
        ArchiveView::from_zip(&temp)
    }

    /// 依次尝试 URL 列表，下载第一个成功的归档。
    pub async fn download_zip(&self, urls: &[String], dest: &Path, cancel: &Cancel) -> Result<(), LipError> {
        let mut last_error = String::from("no url");
        for url in urls {
            cancel.check()?;
            match self.download_one(url, dest, cancel).await {
                Ok(()) => return Ok(()),
                Err(error) => last_error = format!("{url}: {error}"),
            }
        }
        Err(LipError::Network(format!("all download sources failed: {last_error}")))
    }

    async fn download_one(&self, url: &str, dest: &Path, cancel: &Cancel) -> Result<(), LipError> {
        let response = self.http.get(url).send().await.map_err(|error| LipError::Network(error.to_string()))?;
        if !response.status().is_success() {
            return Err(LipError::Network(format!("HTTP {}", response.status())));
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(LipError::Io)?;
        }
        let mut file = tokio::fs::File::create(dest).await.map_err(LipError::Io)?;
        let mut stream = response.bytes_stream();
        let mut total = 0u64;
        while let Some(chunk) = stream.next().await {
            cancel.check()?;
            let chunk = chunk.map_err(|error| LipError::Network(error.to_string()))?;
            total += chunk.len() as u64;
            if total > MAX_DOWNLOAD_BYTES {
                let _ = tokio::fs::remove_file(dest).await;
                return Err(LipError::Network("download exceeds size limit".into()));
            }
            file.write_all(&chunk).await.map_err(LipError::Io)?;
        }
        file.flush().await.map_err(LipError::Io)?;
        // 归档必须以 ZIP 本地文件头或空档头开始，避免把 HTML 错误页当 zip。
        let header = tokio::fs::read(dest).await.map_err(LipError::Io)?;
        if !(header.starts_with(b"PK\x03\x04") || header.starts_with(b"PK\x05\x06")) {
            let _ = tokio::fs::remove_file(dest).await;
            return Err(LipError::Network("response is not a zip archive".into()));
        }
        Ok(())
    }

    async fn try_fetch_text(&self, urls: &[String], cancel: &Cancel) -> Result<Option<String>, LipError> {
        for url in urls {
            cancel.check()?;
            if let Ok(response) = self.http.get(url).send().await {
                if response.status().is_success() {
                    if let Ok(text) = response.text().await {
                        if !text.trim().is_empty() {
                            return Ok(Some(text));
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    fn parse_manifest(&self, text: &str, environment: &Environment) -> Result<PackageFile, LipError> {
        Ok(self.parse_manifest_with_json(text, environment)?.0)
    }

    /// 自动区分 lip.toml 与 tooth.json。
    fn parse_manifest_with_json(&self, text: &str, environment: &Environment) -> Result<(PackageFile, Json), LipError> {
        let trimmed = text.trim_start();
        if trimmed.starts_with('{') {
            let mut json: Json = serde_json::from_str(text).map_err(|error| LipError::InvalidManifest(format!("invalid tooth.json: {error}")))?;
            render_tooth_templates(&mut json);
            let Some(file) = manifest::from_tooth_json(&json, environment)? else {
                return Err(LipError::InvalidManifest("unsupported legacy tooth format".into()));
            };
            let canonical = serde_json::to_value(&file).map_err(LipError::Json)?;
            Ok((file, canonical))
        } else {
            manifest::parse_package_file(text)
        }
    }
}

/// tooth.json 里的模板变量由包自身字段提供。
fn render_tooth_templates(json: &mut Json) {
    let tooth = json.get("tooth").and_then(Json::as_str).unwrap_or_default().to_string();
    let version = json.get("version").and_then(Json::as_str).unwrap_or_default().to_string();
    fn walk(value: &mut Json, tooth: &str, version: &str) {
        match value {
            Json::String(text) => {
                if text.contains("{{") {
                    *text = text.replace("{{tooth}}", tooth).replace("{{version}}", version).replace("{{path}}", tooth);
                }
            }
            Json::Array(items) => items.iter_mut().for_each(|item| walk(item, tooth, version)),
            Json::Object(map) => map.values_mut().for_each(|item| walk(item, tooth, version)),
            _ => {}
        }
    }
    walk(json, &tooth, &version);
}

fn read_manifest_from_dir(directory: &Path) -> Result<String, LipError> {
    for name in [manifest::PACKAGE_FILE, manifest::LEGACY_PACKAGE_FILE] {
        let path = directory.join(name);
        if path.is_file() {
            return std::fs::read_to_string(&path).map_err(LipError::Io);
        }
    }
    Err(LipError::InvalidManifest(format!("{} has no lip.toml or tooth.json", directory.display())))
}

fn read_manifest_from_archive(archive: &ArchiveView) -> Result<String, LipError> {
    for name in [manifest::PACKAGE_FILE, manifest::LEGACY_PACKAGE_FILE] {
        if archive.contains_file(name) {
            return String::from_utf8(archive.read(name)?).map_err(|_| LipError::InvalidManifest(format!("{name} is not UTF-8")));
        }
    }
    // GitHub 归档通常带一层 <repo>-<ref>/ 前缀。
    if let Some(candidate) = archive.files().iter().find(|file| file.ends_with("/lip.toml") || file.ends_with("/tooth.json")) {
        let bytes = archive.read(candidate)?;
        return String::from_utf8(bytes).map_err(|_| LipError::InvalidManifest(format!("{candidate} is not UTF-8")));
    }
    Err(LipError::InvalidManifest("archive has no lip.toml or tooth.json".into()))
}

fn read_tag_cache(path: &Path) -> Option<Vec<String>> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    if modified.elapsed().ok()? > Duration::from_secs(3600) {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn short_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    digest[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_tooth_templates() {
        let mut json = serde_json::json!({
            "tooth": "github.com/a/b",
            "version": "1.2.3",
            "asset_url": "https://{{tooth}}/releases/v{{version}}/a.zip"
        });
        render_tooth_templates(&mut json);
        assert_eq!(json["asset_url"], "https://github.com/a/b/releases/v1.2.3/a.zip");
    }

    #[test]
    fn manifest_urls_include_mirror() {
        let client = RemoteClient::new(std::env::temp_dir(), vec!["https://mirror.example/".into()]).unwrap();
        let urls = client.manifest_urls("a", "b", "v1", "lip.toml");
        assert!(urls[0].starts_with("https://raw.githubusercontent.com/a/b/v1/lip.toml"));
        assert!(urls.iter().any(|url| url.starts_with("https://mirror.example/https://raw.githubusercontent.com")));
    }
}
