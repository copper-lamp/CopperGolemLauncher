//! 更新服务：检查 GitHub Releases 新版本并应用更新。
//!
//! 流程：`check()` 拉取仓库最新 Release（GitHub API），与当前版本 semver 比较；
//! 有新版则 `apply()` 把更新包（`.exe`）投递到全局下载队列（支持断点续传与
//! 可选 SHA-256 校验，摘要按约定发布为 `<资产名>.sha256` 附带资产）；
//! 下载完成后 `install()` 用 `self-replace` 原地替换当前可执行文件并重启应用。
//!
//! 事件：`update.status`（Idle / Checking / Available / Downloading / Downloaded / Failed）。

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;

use crate::error::KernelError;
use crate::registry::events::EventBus;
use crate::services::download::DownloadService;
use crate::services::paths::Paths;
use crate::services::settings::SettingsService;

const GITHUB_API: &str = "https://api.github.com";
const DEFAULT_REPO: &str = "copper-lamp/copper-golem";

/// 更新流程阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    /// 空闲 / 已是最新。
    Idle,
    /// 检查中。
    Checking,
    /// 检测到新版本，等待下载。
    Available,
    /// 更新包下载中。
    Downloading,
    /// 更新包已就绪，可安装。
    Downloaded,
    /// 检查或下载失败。
    Failed,
}

/// 可用的新版本信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub published_at: String,
    pub asset_name: String,
    pub asset_size: u64,
    pub download_url: String,
}

/// 更新状态快照（供前端展示 / 订阅）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateStatus {
    pub phase: UpdatePhase,
    pub current_version: String,
    pub latest: Option<UpdateInfo>,
    pub download_task_id: Option<u64>,
    pub error: Option<String>,
}

impl UpdateStatus {
    fn idle(current_version: &str) -> Self {
        Self {
            phase: UpdatePhase::Idle,
            current_version: current_version.to_string(),
            latest: None,
            download_task_id: None,
            error: None,
        }
    }
}

/// 更新服务。
pub struct UpdaterService {
    settings: Arc<SettingsService>,
    download: Arc<DownloadService>,
    paths: Arc<Paths>,
    events: Arc<EventBus>,
    client: Client,
    current_version: String,
    status: Arc<Mutex<UpdateStatus>>,
}

impl UpdaterService {
    pub fn new(
        settings: Arc<SettingsService>,
        download: Arc<DownloadService>,
        paths: Arc<Paths>,
        events: Arc<EventBus>,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("copper-golem/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("failed to build reqwest client");
        let current = env!("CARGO_PKG_VERSION").to_string();
        let service = Self {
            settings,
            download,
            paths,
            events,
            client,
            current_version: current,
            status: Arc::new(Mutex::new(UpdateStatus::idle(env!("CARGO_PKG_VERSION")))),
        };
        service.track_download();
        service
    }

    /// 检查最新版本。有新版且带可更新资产 → Available，否则 Idle。
    pub async fn check(&self) -> Result<UpdateStatus, KernelError> {
        {
            let mut st = self.status.lock();
            st.phase = UpdatePhase::Checking;
            st.error = None;
            st.latest = None;
        }
        let repo = self.settings.get_or("update.repo", DEFAULT_REPO.to_string());
        let url = format!("{GITHUB_API}/repos/{repo}/releases/latest");
        let resp: Value = self
            .client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let tag = resp
            .get("tag_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim_start_matches('v')
            .to_string();
        let current_v = semver::Version::parse(&self.current_version)
            .unwrap_or_else(|_| semver::Version::new(0, 0, 0));
        let latest_v = semver::Version::parse(&tag).ok();

        let mut st = self.status.lock();
        match latest_v {
            Some(v) if v > current_v => {
                if let Some(asset) = pick_asset(resp.get("assets").and_then(Value::as_array)) {
                    st.latest = Some(UpdateInfo {
                        version: tag,
                        notes: resp
                            .get("body")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        published_at: resp
                            .get("published_at")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        asset_name: asset.name,
                        asset_size: asset.size,
                        download_url: asset.url,
                    });
                    st.phase = UpdatePhase::Available;
                    st.error = None;
                } else {
                    // 有新版但发布中无可用资产：保持 Idle，并说明原因。
                    st.latest = None;
                    st.phase = UpdatePhase::Idle;
                    st.error = Some("检测到新版本，但发布中没有可更新的资产".into());
                }
            }
            _ => {
                st.latest = None;
                st.phase = UpdatePhase::Idle;
                st.error = None;
            }
        }
        let snap = st.clone();
        drop(st);
        self.publish_status(&snap);
        Ok(snap)
    }

    /// 下载更新包。返回下载任务 id（进度经下载队列事件广播）。
    pub async fn apply(&self) -> Result<u64, KernelError> {
        let info = {
            let st = self.status.lock();
            if st.phase == UpdatePhase::Downloading {
                return Err(KernelError::Updater("更新已在下载中".into()));
            }
            st.latest
                .clone()
                .ok_or_else(|| KernelError::Updater("无可用更新".into()))?
        };
        let expected = self.fetch_sha256(&info).await?;
        let dir = self.paths.cache_dir().join("updates");
        std::fs::create_dir_all(&dir)?;
        let dest = dir.join(&info.asset_name);
        let task_id = self.download.enqueue(
            &info.download_url,
            &dest,
            copper_downloader::DownloadOptions {
                resume: true,
                remove_on_cancel: true,
                expected_sha256: expected,
                filename: Some(info.asset_name.clone()),
                ..Default::default()
            },
        )?;
        let mut st = self.status.lock();
        st.phase = UpdatePhase::Downloading;
        st.download_task_id = Some(task_id);
        st.error = None;
        let snap = st.clone();
        drop(st);
        self.publish_status(&snap);
        Ok(task_id)
    }

    /// 安装已下载的更新：原地替换可执行文件并重启应用。
    pub fn install(&self) -> Result<(), KernelError> {
        let info = {
            let st = self.status.lock();
            if st.phase != UpdatePhase::Downloaded {
                return Err(KernelError::Updater("更新尚未下载完成".into()));
            }
            st.latest
                .clone()
                .ok_or_else(|| KernelError::Updater("无可用更新".into()))?
        };
        let asset = self.paths.cache_dir().join("updates").join(&info.asset_name);
        if !asset.exists() {
            return Err(KernelError::Updater("更新包不存在，请重新下载".into()));
        }
        self_replace::self_replace(&asset)
            .map_err(|e| KernelError::Updater(format!("应用更新失败: {e}")))?;
        // 替换成功：拉起新进程后退出当前进程。
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).spawn();
        }
        std::process::exit(0);
    }

    /// 当前更新状态。
    pub fn status(&self) -> UpdateStatus {
        self.status.lock().clone()
    }

    // ---- 内部实现 ----

    /// 订阅下载队列事件：跟踪本服务投递的更新包下载进度。
    fn track_download(&self) {
        let status = self.status.clone();
        let events = self.events.clone();
        self.events.subscribe("download.status", move |_name, payload| {
            let Some(id) = payload.get("id").and_then(Value::as_u64) else {
                return;
            };
            let phase_status = payload.get("status").and_then(Value::as_str).unwrap_or("");
            let mut st = status.lock();
            if st.download_task_id != Some(id) {
                return;
            }
            match phase_status {
                "done" => {
                    st.phase = UpdatePhase::Downloaded;
                    st.error = None;
                }
                "failed" | "cancelled" => {
                    st.phase = UpdatePhase::Failed;
                    st.error = payload
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                _ => return,
            }
            let snap = st.clone();
            drop(st);
            events.publish(
                "update.status",
                serde_json::to_value(&snap).unwrap_or(Value::Null),
            );
        });
    }

    /// 尝试获取更新包的 SHA-256 摘要（发布约定：附带 `<资产名>.sha256` 文本资产）。
    async fn fetch_sha256(&self, info: &UpdateInfo) -> Result<Option<String>, KernelError> {
        let repo = self.settings.get_or("update.repo", DEFAULT_REPO.to_string());
        let url = format!("{GITHUB_API}/repos/{repo}/releases/latest");
        let resp: Value = self
            .client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let digest_name = format!("{}.sha256", info.asset_name);
        let digest_url = resp
            .get("assets")
            .and_then(Value::as_array)
            .and_then(|arr| {
                arr.iter()
                    .find(|a| {
                        a.get("name").and_then(Value::as_str) == Some(digest_name.as_str())
                    })
                    .and_then(|a| a.get("browser_download_url").and_then(Value::as_str))
            });
        let Some(url) = digest_url else {
            return Ok(None);
        };
        let text = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(text.split_whitespace().next().map(str::to_string))
    }

    fn publish_status(&self, snap: &UpdateStatus) {
        self.events
            .publish("update.status", serde_json::to_value(snap).unwrap_or(Value::Null));
    }
}

/// Release 资产（下载所需的最小子集）。
struct ReleaseAsset {
    name: String,
    size: u64,
    url: String,
}

/// 挑选更新资产：优先 Windows 可执行文件（供 `self-replace` 原地替换），
/// 否则取第一个带下载地址的资产。
fn pick_asset(assets: Option<&Vec<Value>>) -> Option<ReleaseAsset> {
    let assets = assets?;
    if assets.is_empty() {
        return None;
    }
    let with_url = |a: &Value| -> Option<ReleaseAsset> {
        let name = a.get("name").and_then(Value::as_str)?.to_string();
        let url = a.get("browser_download_url").and_then(Value::as_str)?.to_string();
        let size = a.get("size").and_then(Value::as_u64).unwrap_or(0);
        Some(ReleaseAsset { name, size, url })
    };
    assets
        .iter()
        .find_map(|a| {
            let ra = with_url(a)?;
            if ra.name.to_ascii_lowercase().ends_with(".exe") {
                Some(ra)
            } else {
                None
            }
        })
        .or_else(|| assets.iter().find_map(with_url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pick_asset_prefers_exe() {
        let assets = vec![
            json!({"name": "copper-golem-0.2.0.zip", "size": 10, "browser_download_url": "https://x/z.zip"}),
            json!({"name": "copper-golem-0.2.0.exe", "size": 20, "browser_download_url": "https://x/z.exe"}),
        ];
        let picked = pick_asset(Some(&assets)).expect("asset should be picked");
        assert_eq!(picked.name, "copper-golem-0.2.0.exe");
        assert_eq!(picked.size, 20);
    }

    #[test]
    fn pick_asset_falls_back_to_first_with_url() {
        let assets = vec![json!({"name": "bundle.zip", "size": 5, "browser_download_url": "https://x/b.zip"})];
        let picked = pick_asset(Some(&assets)).expect("asset should be picked");
        assert_eq!(picked.name, "bundle.zip");
    }

    #[test]
    fn pick_asset_none_when_empty() {
        assert!(pick_asset(Some(&vec![])).is_none());
        assert!(pick_asset(None).is_none());
    }
}
