//! 更新服务：发现新版本、投递更新包到下载队列、执行替换并重启。
//!
//! 流程：
//! 1. `check()` 从 GitHub Releases 拉候选，按 semver 挑出比当前版本新的最高版本，
//!    再从该 Release 的资产里**精确选出当前平台/架构的产物**（见 `release` 子模块）。
//! 2. `download()` 把产物投进全局下载队列（断点续传 + SHA-256 校验），
//!    进度由下载引擎的事件广播，本服务只跟踪自己的任务 id。
//! 3. `install()` 按产物形态执行替换（自替换 / 静默安装器 / 原子改名）并重启。
//!
//! 事件：`update.status`（常规状态流转）、`update.ready`（更新包已就绪，可提醒重启）。
//!
//! 启动期静默检查见 [`UpdaterService::spawn_background_check`]：不阻塞启动、
//! 失败只写日志、不向前端发任何事件。

mod release;

use std::path::PathBuf;
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
use crate::services::registry::model::Platform;
use crate::services::settings::SettingsService;

const GITHUB_API: &str = "https://api.github.com";

/// 默认发布仓库。设置项 `update.repo` 可覆盖（自建镜像 / fork 验证）。
const DEFAULT_REPO: &str = "copper-lamp/CopperGolemLauncher";

/// 启动后延迟多久发起后台检查。
///
/// 必须给启动流程让路：检查是纯网络 IO，与窗口首帧、模块 boot、设置装载抢资源
/// 只会让用户感到「刚打开就卡了一下」。
const BACKGROUND_CHECK_DELAY: Duration = Duration::from_secs(3);

/// Releases 列表单页条数。30 条足以覆盖「当前版本往后的全部发布」，
/// 且远低于 GitHub 未认证请求的速率上限。
const RELEASES_PER_PAGE: u32 = 30;

/// 更新流程阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    /// 空闲 / 已是最新 / 当前平台无可用产物。
    Idle,
    /// 检查中。
    Checking,
    /// 检测到新版本，等待用户确认下载。
    Available,
    /// 更新包下载中。
    Downloading,
    /// 更新包已就绪，可安装重启。
    Downloaded,
    /// 检查 / 下载失败。
    Failed,
}

/// 失败来源。前端据此决定文案与「是否可重试」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateErrorKind {
    /// 网络 / GitHub API 失败（含未认证限流）。
    Network,
    /// 有新版，但没有当前平台/架构的产物。
    NoAsset,
    /// 更新包下载失败。
    Download,
}

/// 更新包的安装形态，决定 `install()` 走哪条替换路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateKind {
    /// Windows 便携 zip：内含裸 exe，解压后原地自替换。
    Portable,
    /// Windows NSIS 安装器：静默 `/S` 子进程 + 看门狗重启。
    Nsis,
    /// Linux AppImage：原子改名替换自身。
    AppImage,
    /// 产物形态无法自动安装（deb / dmg / apk 等），需用户手动。
    Manual,
}

impl UpdateKind {
    /// 由资产文件名推断安装形态。
    pub fn from_asset_name(name: &str, platform: Option<&Platform>) -> Self {
        let lower = name.to_ascii_lowercase();
        let platform_key = platform.map(Platform::as_str).unwrap_or("");
        if platform_key.starts_with("windows") {
            return if lower.ends_with(".zip") {
                Self::Portable
            } else if lower.ends_with(".exe") {
                Self::Nsis
            } else {
                Self::Manual
            };
        }
        if platform_key.starts_with("linux") && lower.ends_with(".appimage") {
            return Self::AppImage;
        }
        Self::Manual
    }
}

/// 新版本信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateInfo {
    /// 版本号（无 `v` 前缀）。
    pub version: String,
    /// 原始 tag。
    pub tag: String,
    /// 发行说明。
    pub notes: String,
    /// 发布时间（ISO-8601）。
    pub published_at: String,
    /// 发行页地址。
    pub html_url: String,
    /// 资产文件名。
    pub asset_name: String,
    /// 资产字节数。
    pub asset_size: u64,
    /// 下载直链。
    pub download_url: String,
    /// 安装形态。
    pub kind: UpdateKind,
    /// SHA-256 十六进制摘要；无摘要时 `None`（引擎跳过校验）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// 独立 `.sha256` 文本资产地址；仅在 Release 未提供内联摘要时存在。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256_url: Option<String>,
}

/// 更新状态快照（供前端展示 / 订阅）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateStatus {
    pub phase: UpdatePhase,
    pub current_version: String,
    pub latest: Option<UpdateInfo>,
    /// 本服务投递的下载任务 id。
    pub download_task_id: Option<u64>,
    /// 失败原因原文（可直接展示）。
    pub error: Option<String>,
    /// 失败来源分类。
    pub error_kind: Option<UpdateErrorKind>,
    /// 是否处于「下载中 / 已就绪」这两个需要持续展示进度的阶段。
    pub active: bool,
    /// 上次成功检查的秒级时间戳；从未成功过为 `None`。
    pub last_checked_at: Option<u64>,
}

impl UpdateStatus {
    fn idle(current_version: &str) -> Self {
        Self {
            phase: UpdatePhase::Idle,
            current_version: current_version.to_string(),
            latest: None,
            download_task_id: None,
            error: None,
            error_kind: None,
            active: false,
            last_checked_at: None,
        }
    }

    /// `active` 由阶段派生，不允许调用方各自维护（曾经的状态不同步就是这么来的）。
    fn normalize(&mut self) {
        self.active = matches!(
            self.phase,
            UpdatePhase::Downloading | UpdatePhase::Downloaded
        );
    }
}

/// 一次检查的产物：命中的 Release + 它的资产数组。
struct Candidate {
    info: release::ReleaseInfo,
    assets: Option<Vec<Value>>,
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
            .timeout(Duration::from_secs(20))
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
            current_version: current.clone(),
            status: Arc::new(Mutex::new(UpdateStatus::idle(&current))),
        };
        service.track_download();
        service
    }

    // -----------------------------------------------------------------
    // 检查
    // -----------------------------------------------------------------

    /// 手动检查：结果一定广播到前端（无论成败）。
    pub async fn check(&self) -> Result<UpdateStatus, KernelError> {
        self.run_check(false).await
    }

    /// 执行一次检查。
    ///
    /// `silent = true` 用于启动后台检查：失败只写日志且**不发事件**，
    /// 前端因此完全无感（R3）。两种模式共用同一条网络与选择链路，避免两套行为。
    async fn run_check(&self, silent: bool) -> Result<UpdateStatus, KernelError> {
        {
            let mut st = self.status.lock();
            st.phase = UpdatePhase::Checking;
            st.error = None;
            st.error_kind = None;
        }
        if !silent {
            self.publish_current();
        }

        let platform = Platform::current();
        let discovered = self.discover().await;
        // 失败时先落到 Failed 再发布：手动检查必须让用户看到原因，
        // 静默模式则在发布前回落 Idle（见末尾）。
        let mut failure: Option<(UpdateErrorKind, String)> = None;

        match discovered {
            Err(message) => {
                failure = Some((UpdateErrorKind::Network, message));
            }
            Ok(None) => {
                let mut st = self.status.lock();
                st.latest = None;
                st.download_task_id = None;
                st.phase = UpdatePhase::Idle;
                st.last_checked_at = Some(now_secs());
            }
            Ok(Some(candidate)) => {
                match release::select_asset(candidate.assets.as_ref(), platform.as_ref()) {
                    Ok(asset) => {
                        let kind = UpdateKind::from_asset_name(&asset.name, platform.as_ref());
                        log::info!(
                            "[updater] 发现新版本 {}（{}，{} 字节，形态 {:?}）",
                            candidate.info.version,
                            asset.name,
                            asset.size,
                            kind
                        );
                        let mut st = self.status.lock();
                        st.latest = Some(UpdateInfo {
                            version: candidate.info.version.clone(),
                            tag: candidate.info.tag.clone(),
                            notes: candidate.info.notes.clone(),
                            published_at: candidate.info.published_at.clone(),
                            html_url: candidate.info.html_url.clone(),
                            asset_name: asset.name.clone(),
                            asset_size: asset.size,
                            download_url: asset.url.clone(),
                            kind,
                            sha256: asset.digest.clone(),
                            sha256_url: asset.digest_url.clone(),
                        });
                        st.download_task_id = None;
                        st.phase = UpdatePhase::Available;
                        st.error = None;
                        st.error_kind = None;
                        st.last_checked_at = Some(now_secs());
                    }
                    Err(error) => {
                        let mut st = self.status.lock();
                        st.latest = None;
                        st.download_task_id = None;
                        st.phase = UpdatePhase::Idle;
                        st.error = Some(error.message());
                        st.error_kind = Some(UpdateErrorKind::NoAsset);
                        st.last_checked_at = Some(now_secs());
                        failure = Some((UpdateErrorKind::NoAsset, error.message()));
                    }
                }
            }
        }

        if let Some((kind, message)) = &failure {
            log::info!("[updater] 检查更新失败（{kind:?}）：{message}");
        }
        let snap = self.status.lock().clone();
        let failed = snap.phase == UpdatePhase::Failed || snap.error.is_some();
        if silent && failed {
            // 静默模式的语义就是「失败不留痕」：状态回落 Idle 且不广播。
            let mut st = self.status.lock();
            st.phase = UpdatePhase::Idle;
            st.error = None;
            st.error_kind = None;
            drop(st);
            return Ok(self.status.lock().clone());
        }
        self.publish(&snap);
        Ok(snap)
    }

    /// 拉取候选并挑出比当前版本新的最高版本。
    async fn discover(&self) -> Result<Option<Candidate>, String> {
        let repo = self
            .settings
            .get_or("update.repo", DEFAULT_REPO.to_string())
            .trim()
            .trim_matches('/')
            .to_string();
        let channel = self.settings.get_or("update.channel", "stable".to_string());

        let url = format!("{GITHUB_API}/repos/{repo}/releases?per_page={RELEASES_PER_PAGE}");
        let response = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| format!("连接 GitHub 失败：{e}"))?;

        let status_code = response.status().as_u16();
        if status_code == 403 || status_code == 429 {
            return Err(format!(
                "GitHub 接口受限（HTTP {status_code}）：未认证请求每小时仅 60 次，请稍后再试"
            ));
        }
        if status_code >= 400 {
            return Err(format!("GitHub 接口返回 HTTP {status_code}"));
        }

        let payload: Value = response
            .json()
            .await
            .map_err(|e| format!("解析 GitHub 响应失败：{e}"))?;
        let releases = release::collect_releases(&payload);
        let allow_prerelease = release::channel_allows_prerelease(&channel);
        let Some(info) =
            release::pick_newer(&releases, &self.current_version, allow_prerelease)
        else {
            return Ok(None);
        };
        // 资产数组仍留在原始响应里，这里按 tag 就地取回，避免把整个 JSON 层层搬运。
        let assets = payload
            .as_array()
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("tag_name")
                        .and_then(Value::as_str)
                        .is_some_and(|tag| tag.trim() == info.tag)
                })
            })
            .and_then(|item| item.get("assets").and_then(Value::as_array).cloned());
        Ok(Some(Candidate {
            info: info.clone(),
            assets,
        }))
    }

    // -----------------------------------------------------------------
    // 下载
    // -----------------------------------------------------------------

    /// 把更新包投进全局下载队列，返回下载任务 id。
    ///
    /// 幂等：已处于 `Downloading` 时返回既有任务 id，不重复投递
    /// （避免用户连点两次装出两个任务各下一份）。
    pub async fn download(&self) -> Result<u64, KernelError> {
        let info = {
            let st = self.status.lock();
            if let (UpdatePhase::Downloading, Some(id)) = (st.phase, st.download_task_id) {
                return Ok(id);
            }
            st.latest
                .clone()
                .ok_or_else(|| KernelError::Updater("当前没有可安装的新版本".into()))?
        };
        if info.kind == UpdateKind::Manual {
            return Err(KernelError::Updater(
                "该版本为手动安装包，请到发行页下载后自行安装".into(),
            ));
        }
        let expected_sha256 = self.resolve_sha256(&info).await?;

        let dir = self.paths.cache_dir().join("updates");
        std::fs::create_dir_all(&dir)?;
        let dest = dir.join(&info.asset_name);
        let task_id = self.download.enqueue(
            &info.download_url,
            &dest,
            copper_downloader::DownloadOptions {
                resume: true,
                remove_on_cancel: true,
                expected_sha256,
                filename: Some(info.asset_name.clone()),
                ..Default::default()
            },
        )?;

        let mut st = self.status.lock();
        st.phase = UpdatePhase::Downloading;
        st.download_task_id = Some(task_id);
        st.error = None;
        st.error_kind = None;
        st.normalize();
        let snap = st.clone();
        drop(st);
        self.publish(&snap);
        Ok(task_id)
    }

    /// 取消下载（保留 `.part`，再次下载可续传）。
    pub fn cancel_download(&self) -> Result<(), KernelError> {
        let id = self.status.lock().download_task_id;
        if let Some(id) = id {
            let _ = self.download.cancel(id);
        }
        let mut st = self.status.lock();
        st.phase = if st.latest.is_some() {
            UpdatePhase::Available
        } else {
            UpdatePhase::Idle
        };
        st.download_task_id = None;
        st.error = None;
        st.error_kind = None;
        st.normalize();
        let snap = st.clone();
        drop(st);
        self.publish(&snap);
        Ok(())
    }

    /// 当前更新状态。
    pub fn status(&self) -> UpdateStatus {
        self.status.lock().clone()
    }

    /// 更新包在缓存目录中的落点。
    fn asset_path(&self, info: &UpdateInfo) -> PathBuf {
        self.paths.cache_dir().join("updates").join(&info.asset_name)
    }

    // -----------------------------------------------------------------
    // 启动期静默检查
    // -----------------------------------------------------------------

    /// 在启动完成后静默检查一次更新。
    ///
    /// 三个约束都由构造保证：
    /// - **不阻塞启动**：全程 `spawn`，`setup()` 不等待；
    /// - **不打扰用户**：失败只写日志、不发事件；
    /// - **可关闭**：受设置 `update.auto_check` 控制。
    pub fn spawn_background_check(self: &Arc<Self>) {
        let enabled = self
            .settings
            .get_or("update.auto_check", Value::Bool(true))
            .as_bool()
            .unwrap_or(true);
        if !enabled {
            log::info!("[updater] 已关闭启动自动检查（设置 update.auto_check）");
            return;
        }
        let this = Arc::clone(self);
        drop(tauri::async_runtime::spawn(async move {
            tokio::time::sleep(BACKGROUND_CHECK_DELAY).await;
            if let Err(error) = this.run_check(true).await {
                log::info!("[updater] 后台检查未完成：{error}");
            }
        }));
    }

    // -----------------------------------------------------------------
    // 内部
    // -----------------------------------------------------------------

    /// 订阅下载队列事件，跟踪本服务投递的那个任务。
    ///
    /// 用 `subscribe_from_now` 之外的普通 `subscribe` 即可：事件总带有任务 id，
    /// 与本任务 id 不符时立刻返回，不存在误认可能。
    fn track_download(&self) {
        let status = self.status.clone();
        let events = self.events.clone();
        self.events.subscribe("download.status", move |_name, payload| {
            let Some(id) = payload.get("id").and_then(Value::as_u64) else {
                return;
            };
            let task_status = payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let error = payload
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string);
            let mut st = status.lock();
            if st.download_task_id != Some(id) {
                return;
            }
            let ready = match task_status.as_str() {
                "done" => {
                    st.phase = UpdatePhase::Downloaded;
                    st.error = None;
                    st.error_kind = None;
                    true
                }
                "failed" | "cancelled" => {
                    st.phase = UpdatePhase::Failed;
                    st.error_kind = Some(UpdateErrorKind::Download);
                    st.error = Some(error.unwrap_or_else(|| "更新包下载失败".into()));
                    false
                }
                _ => return,
            };
            st.normalize();
            let snap = st.clone();
            drop(st);
            events.publish(
                "update.status",
                serde_json::to_value(&snap).unwrap_or(Value::Null),
            );
            if ready {
                // 单独发一次「可重启」：全局提示层据此弹「立即重启」，
                // 不必轮询下载任务。
                events.publish(
                    "update.ready",
                    serde_json::json!({
                        "version": snap.latest.as_ref().map(|i| i.version.clone()).unwrap_or_default(),
                        "download_task_id": id,
                    }),
                );
            }
        });
    }

    /// 确定期望的 SHA-256。
    ///
    /// 优先用 GitHub 资产的内联 `digest`（无需额外请求）；缺失时才去读发布方
    /// 另附的 `<资产名>.sha256` 文本资产。两者都没有则返回 `None`，
    /// 由引擎跳过校验——但这会被记为已知风险（见文档 R-1）。
    async fn resolve_sha256(&self, info: &UpdateInfo) -> Result<Option<String>, KernelError> {
        if let Some(digest) = &info.sha256 {
            return Ok(Some(digest.clone()));
        }
        let Some(url) = &info.sha256_url else {
            log::warn!(
                "[updater] 产物 {} 没有校验信息，本次下载不做完整性校验",
                info.asset_name
            );
            return Ok(None);
        };
        let text = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| KernelError::Updater(format!("读取校验文件失败：{e}")))?
            .text()
            .await
            .map_err(|e| KernelError::Updater(format!("读取校验文件失败：{e}")))?;
        let digest = text
            .split_whitespace()
            .next()
            .filter(|token| token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit()))
            .map(|token| token.to_ascii_lowercase());
        if digest.is_none() {
            log::warn!("[updater] 校验文件 {} 内容无法解析", url);
        }
        Ok(digest)
    }

    fn publish_current(&self) {
        let snap = self.status.lock().clone();
        self.publish(&snap);
    }

    fn publish(&self, snap: &UpdateStatus) {
        self.events
            .publish("update.status", serde_json::to_value(snap).unwrap_or(Value::Null));
    }
}

/// 秒级时间戳（更新只关心「多久前查过」，不需要更高精度）。
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 各平台的替换实现（仅桌面）
// ---------------------------------------------------------------------------

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod install_impl {
    use super::{KernelError, Paths};
    use std::path::{Path, PathBuf};

    /// Windows 便携 zip：解压出裸 exe，`self_replace` 原地替换并重启。
    pub(super) fn portable(paths: &Paths, asset: &Path, current_exe: &Path) -> Result<(), KernelError> {
        let staging = paths.cache_dir().join("updates").join("staging");
        std::fs::create_dir_all(&staging)?;
        let binary = extract_first_executable(asset, &staging)?;
        log::info!("[updater] 便携包已解出：{}", binary.display());
        let replaced = self_replace::self_replace(&binary);
        drop(binary);
        let _ = std::fs::remove_dir_all(&staging);
        replaced.map_err(|e| KernelError::Updater(format!("替换启动器文件失败：{e}")))?;
        relaunch(current_exe)
    }

    /// Windows NSIS：静默安装 + 看门狗重启。
    ///
    /// 运行中的 exe 无法被覆盖，这是 Windows 的硬限制，NSIS 安装器同样绕不开。
    /// 因此把「等安装器结束、再拉起新版本」交给一个脱离父进程的看门狗
    /// （系统自带 PowerShell），随后当前进程直接退出。
    #[cfg(windows)]
    pub(super) fn nsis(installer: &Path, current_exe: &Path) -> Result<(), KernelError> {
        use std::os::windows::process::CommandExt;

        // CREATE_NO_WINDOW(0x0800_0000) | DETACHED_PROCESS(0x0000_0008)
        const FLAGS: u32 = 0x0800_0000 | 0x0000_0008;

        let child = std::process::Command::new(installer)
            .arg("/S")
            .arg("/NCRC")
            .creation_flags(FLAGS)
            .spawn()
            .map_err(|e| KernelError::Updater(format!("无法启动安装程序：{e}")))?;

        let script = format!(
            "$ErrorActionPreference='SilentlyContinue'; \
             Wait-Process -Id {}; \
             Start-Process -FilePath '{}'",
            child.id(),
            current_exe.display()
        );
        std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &script,
            ])
            .creation_flags(FLAGS)
            .spawn()
            .map_err(|e| KernelError::Updater(format!("无法启动重启看门狗：{e}")))?;

        log::info!(
            "[updater] 静默安装已启动（pid {}），当前进程退出后由看门狗拉起新版本",
            child.id()
        );
        std::process::exit(0);
    }

    #[cfg(not(windows))]
    pub(super) fn nsis(_installer: &Path, _current_exe: &Path) -> Result<(), KernelError> {
        Err(KernelError::Updater(
            "当前平台不支持安装器形态的更新".into(),
        ))
    }

    /// Linux AppImage：写 `.new` 后同目录 `rename` 原子覆盖，再重启。
    pub(super) fn appimage(asset: &Path, current_exe: &Path) -> Result<(), KernelError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let staged = current_exe.with_extension("appimage.new");
            std::fs::copy(asset, &staged)
                .map_err(|e| KernelError::Updater(format!("写入新 AppImage 失败：{e}")))?;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| KernelError::Updater(format!("设置执行权限失败：{e}")))?;
            std::fs::rename(&staged, current_exe)
                .map_err(|e| KernelError::Updater(format!("替换 AppImage 失败：{e}")))?;
            relaunch(current_exe)
        }
        #[cfg(not(unix))]
        {
            let _ = (asset, current_exe);
            Err(KernelError::Updater(
                "当前平台不支持 AppImage 形态的更新".into(),
            ))
        }
    }

    /// 解压 zip，取其中第一个裸 exe 写到暂存目录。
    ///
    /// 只接受路径分量全为 `Normal` 的条目（阻断 zip slip），且只认 zip 根附近的
    /// `.exe`：便携包里除了启动器本体还有 WebView2 运行时等其它可执行文件，
    /// 挑错就装不上。
    fn extract_first_executable(zip_path: &Path, staging: &Path) -> Result<PathBuf, KernelError> {
        let file = std::fs::File::open(zip_path)
            .map_err(|e| KernelError::Updater(format!("打开更新包失败：{e}")))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| KernelError::Updater(format!("更新包不是有效的 zip：{e}")))?;
        let mut picked: Option<PathBuf> = None;
        for index in 0..archive.len() {
            let Ok(mut entry) = archive.by_index(index) else {
                continue;
            };
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_string();
            let Some(file_name) = name.rsplit('/').next() else {
                continue;
            };
            if !file_name.to_ascii_lowercase().ends_with(".exe") {
                continue;
            }
            if !Path::new(&name)
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
            {
                continue;
            }
            let target = staging.join(file_name);
            let mut out = std::fs::File::create(&target)
                .map_err(|e| KernelError::Updater(format!("创建替换文件失败：{e}")))?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|e| KernelError::Updater(format!("写入替换文件失败：{e}")))?;
            picked = Some(target);
            break;
        }
        picked.ok_or_else(|| {
            KernelError::Updater("更新包内没有找到启动器可执行文件".into())
        })
    }

    /// 拉起新版本并退出当前进程。
    pub(super) fn relaunch(current_exe: &Path) -> Result<(), KernelError> {
        std::process::Command::new(current_exe)
            .spawn()
            .map_err(|e| KernelError::Updater(format!("重启启动器失败：{e}")))?;
        std::process::exit(0);
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl UpdaterService {
    /// 执行替换并重启；桌面平台按产物形态分派。
    pub fn install(&self) -> Result<(), KernelError> {
        let info = {
            let st = self.status.lock();
            if st.phase != UpdatePhase::Downloaded {
                return Err(KernelError::Updater("更新包尚未下载完成".into()));
            }
            st.latest
                .clone()
                .ok_or_else(|| KernelError::Updater("当前没有可安装的新版本".into()))?
        };
        let asset = self.asset_path(&info);
        if !asset.exists() {
            return Err(KernelError::Updater("更新包不存在，请重新下载".into()));
        }
        let current_exe = std::env::current_exe()
            .map_err(|e| KernelError::Updater(format!("定位当前程序失败：{e}")))?;
        match info.kind {
            UpdateKind::Portable => install_impl::portable(&self.paths, &asset, &current_exe),
            UpdateKind::Nsis => install_impl::nsis(&asset, &current_exe),
            UpdateKind::AppImage => install_impl::appimage(&asset, &current_exe),
            UpdateKind::Manual => Err(KernelError::Updater(
                "该版本为手动安装包，请到发行页下载后自行安装".into(),
            )),
        }
    }
}

/// 移动端：更新由应用商店负责，不存在自替换通道，显式报错。
#[cfg(any(target_os = "android", target_os = "ios"))]
impl UpdaterService {
    pub fn install(&self) -> Result<(), KernelError> {
        Err(KernelError::Updater(
            "移动端更新由应用商店分发，内核不支持原地替换".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_inference_matches_asset_shape() {
        assert_eq!(
            UpdateKind::from_asset_name(
                "CopperGolemLauncher-0.2.0-windows-x86_64.zip",
                Some(&Platform::WindowsX86_64)
            ),
            UpdateKind::Portable
        );
        assert_eq!(
            UpdateKind::from_asset_name(
                "CopperGolemLauncher-0.2.0-windows-x86_64.exe",
                Some(&Platform::WindowsX86_64)
            ),
            UpdateKind::Nsis
        );
        assert_eq!(
            UpdateKind::from_asset_name(
                "CopperGolemLauncher-0.2.0-linux-x86_64.AppImage",
                Some(&Platform::LinuxX86_64)
            ),
            UpdateKind::AppImage
        );
        assert_eq!(
            UpdateKind::from_asset_name(
                "coppergolem_0.2.0_amd64.deb",
                Some(&Platform::LinuxX86_64)
            ),
            UpdateKind::Manual
        );
    }

    #[test]
    fn idle_status_is_not_active() {
        let mut status = UpdateStatus::idle("0.1.0");
        assert_eq!(status.phase, UpdatePhase::Idle);
        status.normalize();
        assert!(!status.active);
        assert!(status.latest.is_none());
    }

    #[test]
    fn active_tracks_only_download_phases() {
        let mut status = UpdateStatus::idle("0.1.0");
        status.phase = UpdatePhase::Available;
        status.normalize();
        assert!(!status.active, "等待用户确认下载不算活动态");
        status.phase = UpdatePhase::Downloading;
        status.normalize();
        assert!(status.active);
        status.phase = UpdatePhase::Downloaded;
        status.normalize();
        assert!(status.active);
    }
}