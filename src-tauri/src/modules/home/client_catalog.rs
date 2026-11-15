//! Official third-party client catalog and downloads.
//! The catalog is metadata only; downloaded DLLs always pass the existing client importer.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::error::KernelError;
use crate::modules::home::client::{self, ClientView};
use crate::registry::events::EventBus;
use crate::services::database::DatabaseService;
use crate::services::paths::Paths;

const LATITE_ASSET: &str = "Latite.dll";
const GITHUB_API: &str = "https://api.github.com/repos/LatiteClient/Latite/releases/latest";
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
const CACHE_KEY: &str = "github/LatiteClient/Latite/releases/latest/latite-client-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ClientCatalogEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source_url: String,
    pub asset_name: String,
    pub compatible: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

pub async fn list(cache_root: &std::path::Path) -> Result<Vec<ClientCatalogEntry>, KernelError> {
    crate::services::metadata_cache::load(
        cache_root,
        CACHE_KEY,
        crate::services::metadata_cache::CONTENT_REFRESH,
        fetch_catalog,
    )
    .await
}

async fn fetch_catalog() -> Result<Vec<ClientCatalogEntry>, KernelError> {
    let http = crate::services::http_client::client_builder(Duration::from_secs(15))
        .user_agent(concat!("CopperGolem/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let release: Release = http
        .get(GITHUB_API)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == LATITE_ASSET)
        .ok_or_else(|| KernelError::Config("Latite Release 缺少 Latite.dll 资产".into()))?;
    validate_asset(asset)?;
    Ok(vec![ClientCatalogEntry {
        id: format!("latite:{}", release.tag_name),
        name: "Latite Client".into(),
        version: release.tag_name,
        source_url: asset.browser_download_url.clone(),
        asset_name: asset.name.clone(),
        // Latite does not publish a machine-readable Minecraft compatibility matrix.
        compatible: None,
    }])
}

fn validate_asset(asset: &Asset) -> Result<(), KernelError> {
    let url = reqwest::Url::parse(&asset.browser_download_url)
        .map_err(|_| KernelError::Config("Latite Release 下载地址无效".into()))?;
    if asset.name != LATITE_ASSET
        || asset.size == 0
        || asset.size > MAX_DOWNLOAD_BYTES
        || url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.path().starts_with("/LatiteClient/Latite/releases/download/")
    {
        return Err(KernelError::Config("Latite Release 资产信息异常".into()));
    }
    Ok(())
}

pub async fn install(
    paths: &Paths,
    db: &DatabaseService,
    events: &EventBus,
    id: &str,
    name: Option<&str>,
) -> Result<ClientView, KernelError> {
    client::ensure_supported()?;
    let entries = list(&paths.data_dir()).await?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| KernelError::InvalidArgument("客户端版本不存在或已更新".into()))?;
    // list() is cached, so revalidate the API-provided official URL before accepting bytes.
    if entry.asset_name != LATITE_ASSET {
        return Err(KernelError::Config("Latite Release 资产名称异常".into()));
    }
    validate_download_url(&entry.source_url)?;
    let http = crate::services::http_client::client_builder(Duration::from_secs(90))
        .user_agent(concat!("CopperGolem/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?;
    let response = http.get(&entry.source_url).send().await?.error_for_status()?;
    if response.content_length().is_some_and(|length| length == 0 || length > MAX_DOWNLOAD_BYTES) {
        return Err(KernelError::Config("客户端资产体积异常".into()));
    }

    let staging_root = paths.data_dir().join("clients").join(".downloads");
    std::fs::create_dir_all(&staging_root)?;
    let staging = staging_root.join(format!("{}.dll", uuid::Uuid::new_v4()));
    let result = stream_to_file(response, &staging).await.and_then(|()| {
        let display_name = name.filter(|value| !value.trim().is_empty()).unwrap_or("Latite Client");
        client::import(paths, db, events, &staging.to_string_lossy(), display_name)
    });
    let _ = std::fs::remove_file(&staging);
    result
}

fn validate_download_url(raw: &str) -> Result<(), KernelError> {
    let url = reqwest::Url::parse(raw)
        .map_err(|_| KernelError::Config("Latite Release 下载地址无效".into()))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.path().starts_with("/LatiteClient/Latite/releases/download/")
    {
        return Err(KernelError::Config("拒绝非 GitHub 官方 HTTPS 资产地址".into()));
    }
    Ok(())
}

async fn stream_to_file(response: reqwest::Response, path: &std::path::Path) -> Result<(), KernelError> {
    let mut file = tokio::fs::File::create(path).await?;
    let mut stream = response.bytes_stream();
    let mut received = 0u64;
    while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
        let chunk = chunk?;
        received = received.saturating_add(chunk.len() as u64);
        if received > MAX_DOWNLOAD_BYTES {
            return Err(KernelError::Config("客户端下载超过 512 MB 限制".into()));
        }
        file.write_all(&chunk).await?;
    }
    if received == 0 {
        return Err(KernelError::Config("客户端下载结果为空".into()));
    }
    file.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_the_exact_dll_asset_and_official_url() {
        let valid = Asset {
            name: "Latite.dll".into(),
            browser_download_url: "https://github.com/LatiteClient/Latite/releases/download/v1/Latite.dll".into(),
            size: 2,
        };
        assert!(validate_asset(&valid).is_ok());
        let invalid = Asset { name: "Latite.zip".into(), ..valid };
        assert!(validate_asset(&invalid).is_err());
    }
}
