use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use log::{debug, warn};
use parking_lot::Mutex;
use reqwest::header::{HeaderValue, RANGE};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::error::{DownloadError, ExistingFilePolicy};
use crate::task::{DownloadStatus, TaskSnapshot};

const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(200);
const BACKOFF_BASE_MS: u64 = 500;
const BACKOFF_CAP_MS: u64 = 8_000;
/// 速率统计的指数移动平均系数。
const SPEED_EMA_ALPHA: f64 = 0.3;

/// 下载任务的可配置选项。
#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// 是否断点续传（保留 `.part` 临时文件，恢复时以 Range 续传）。
    pub resume: bool,
    /// 取消时是否删除临时文件。
    pub remove_on_cancel: bool,
    /// 期望的 SHA-256 校验和（十六进制），下载完成后校验。
    pub expected_sha256: Option<String>,
    /// 瞬时性错误的最大重试次数。
    pub max_retries: u32,
    /// 附加请求头。
    pub headers: Vec<(String, String)>,
    /// 展示用文件名。
    pub filename: Option<String>,
    /// 目标已存在时的处理策略。
    pub existing_policy: ExistingFilePolicy,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            resume: true,
            remove_on_cancel: true,
            expected_sha256: None,
            max_retries: 3,
            headers: Vec::new(),
            filename: None,
            existing_policy: ExistingFilePolicy::Overwrite,
        }
    }
}

/// 下载引擎对外广播的事件（均携带任务快照）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadEvent {
    Created(TaskSnapshot),
    Progress(TaskSnapshot),
    StatusChanged(TaskSnapshot),
}

/// 单个任务的可变状态。计数用原子量，短临界区用互斥锁，避免长锁竞争。
struct TaskState {
    id: u64,
    url: String,
    dest: PathBuf,
    part_path: PathBuf,
    filename: Option<String>,
    options: DownloadOptions,
    status: Mutex<DownloadStatus>,
    total_bytes: AtomicU64,
    downloaded_bytes: AtomicU64,
    speed_bytes_per_sec: AtomicU64,
    error: Mutex<Option<String>>,
    retry_count: AtomicU32,
    created_at_ms: u64,
    pause_requested: AtomicBool,
    cancel_token: CancellationToken,
    last_progress_emit: Mutex<Option<Instant>>,
}

impl TaskState {
    fn new(id: u64, url: String, dest: PathBuf, options: DownloadOptions) -> Self {
        let part_path = part_path_for(&dest);
        let created_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self {
            id,
            url,
            dest,
            part_path,
            filename: options.filename.clone(),
            options,
            status: Mutex::new(DownloadStatus::Queued),
            total_bytes: AtomicU64::new(0),
            downloaded_bytes: AtomicU64::new(0),
            speed_bytes_per_sec: AtomicU64::new(0),
            error: Mutex::new(None),
            retry_count: AtomicU32::new(0),
            created_at_ms,
            pause_requested: AtomicBool::new(false),
            cancel_token: CancellationToken::new(),
            last_progress_emit: Mutex::new(None),
        }
    }

    fn snapshot(&self) -> TaskSnapshot {
        TaskSnapshot {
            id: self.id,
            filename: self.filename.clone(),
            url: self.url.clone(),
            dest: self.dest.clone(),
            part_path: self.part_path.clone(),
            total_bytes: self.total_bytes.load(Ordering::Relaxed),
            downloaded_bytes: self.downloaded_bytes.load(Ordering::Relaxed),
            speed_bytes_per_sec: self.speed_bytes_per_sec.load(Ordering::Relaxed),
            status: *self.status.lock(),
            error: self.error.lock().clone(),
            retry_count: self.retry_count.load(Ordering::Relaxed),
            created_at_ms: self.created_at_ms,
        }
    }

    fn set_status(&self, status: DownloadStatus) {
        *self.status.lock() = status;
    }

    fn set_error(&self, error: impl Into<String>) {
        *self.error.lock() = Some(error.into());
    }
}

fn part_path_for(dest: &Path) -> PathBuf {
    let mut os = dest.as_os_str().to_owned();
    os.push(".part");
    PathBuf::from(os)
}

/// 下载事件监听器。
type DownloadListener = Arc<dyn Fn(DownloadEvent) + Send + Sync>;

struct ManagerInner {
    tasks: Mutex<HashMap<u64, Arc<TaskState>>>,
    semaphore: Arc<tokio::sync::Semaphore>,
    next_id: AtomicU64,
    listeners: Mutex<Vec<DownloadListener>>,
    client: reqwest::Client,
    runtime: tokio::runtime::Handle,
}

impl ManagerInner {
    fn emit(&self, event: DownloadEvent) {
        let listeners = self.listeners.lock().clone();
        for l in listeners {
            l(event.clone());
        }
    }

    fn emit_status(&self, state: &TaskState) {
        self.emit(DownloadEvent::StatusChanged(state.snapshot()));
    }

    /// 进度广播：节流到 ≥200ms 一次，保证 UI 不频繁重渲染。
    fn emit_progress(&self, state: &TaskState) {
        let mut last = state.last_progress_emit.lock();
        let now = Instant::now();
        let due = last
            .map(|t| now.duration_since(t) >= PROGRESS_EMIT_INTERVAL)
            .unwrap_or(true);
        if due {
            *last = Some(now);
            drop(last);
            self.emit(DownloadEvent::Progress(state.snapshot()));
        }
    }
}

/// 下载管理器：并发队列 + 任务生命周期控制 + 事件广播。
///
/// 线程安全，可跨任务共享。需在 tokio 运行时内创建（持有运行时句柄用于派生下载任务）。
#[derive(Clone)]
pub struct DownloadManager {
    inner: Arc<ManagerInner>,
}

impl DownloadManager {
    /// 创建下载管理器。
    ///
    /// `concurrency`：同时下载的任务数；`runtime`：用于派生下载任务的 tokio 运行时句柄。
    pub fn new(concurrency: usize, runtime: tokio::runtime::Handle) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .user_agent(concat!("copper-downloader/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .expect("failed to build reqwest client");
        Self {
            inner: Arc::new(ManagerInner {
                tasks: Mutex::new(HashMap::new()),
                semaphore: Arc::new(tokio::sync::Semaphore::new(concurrency.max(1))),
                next_id: AtomicU64::new(1),
                listeners: Mutex::new(Vec::new()),
                client,
                runtime,
            }),
        }
    }

    /// 注册全局事件监听器。内核用它把事件转发到事件总线与前端。
    pub fn add_listener<F>(&self, f: F)
    where
        F: Fn(DownloadEvent) + Send + Sync + 'static,
    {
        self.inner.listeners.lock().push(Arc::new(f));
    }

    /// 清空全部监听器（重配置场景）。
    pub fn clear_listeners(&self) {
        self.inner.listeners.lock().clear();
    }

    /// 投递下载任务。目标已存在且校验通过时（`SkipIfValid`）直接返回完成态任务。
    pub fn enqueue(
        &self,
        url: impl Into<String>,
        dest: impl Into<PathBuf>,
        options: DownloadOptions,
    ) -> Result<u64, DownloadError> {
        let url = url.into();
        let dest = dest.into();
        if url.trim().is_empty() || reqwest::Url::parse(&url).is_err() {
            return Err(DownloadError::InvalidArgument(format!("url: {url}")));
        }
        if dest.as_os_str().is_empty() {
            return Err(DownloadError::InvalidArgument(
                "empty destination path".into(),
            ));
        }

        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(TaskState::new(id, url, dest, options));
        self.inner.tasks.lock().insert(id, state.clone());
        self.inner.emit(DownloadEvent::Created(state.snapshot()));
        self.spawn_run(state);
        Ok(id)
    }

    /// 暂停单个任务（保留临时文件）。等待中的任务立即进入暂停态。
    pub fn pause(&self, id: u64) -> Result<(), DownloadError> {
        let state = self.require_task(id)?;
        if state.status.lock().is_active() {
            state.pause_requested.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    /// 暂停全部活跃任务。
    pub fn pause_all(&self) {
        let tasks: Vec<Arc<TaskState>> = self
            .inner
            .tasks
            .lock()
            .values()
            .filter(|s| s.status.lock().is_active())
            .cloned()
            .collect();
        for t in tasks {
            t.pause_requested.store(true, Ordering::SeqCst);
        }
    }

    /// 恢复单个任务。
    pub fn resume(&self, id: u64) -> Result<(), DownloadError> {
        let state = self.require_task(id)?;
        if *state.status.lock() == DownloadStatus::Paused {
            state.pause_requested.store(false, Ordering::SeqCst);
            state.set_status(DownloadStatus::Queued);
            self.inner.emit_status(&state);
            self.spawn_run(state);
        }
        Ok(())
    }

    /// 恢复全部暂停任务。
    pub fn resume_all(&self) {
        let tasks: Vec<Arc<TaskState>> = self
            .inner
            .tasks
            .lock()
            .values()
            .filter(|s| *s.status.lock() == DownloadStatus::Paused)
            .cloned()
            .collect();
        for t in tasks {
            t.pause_requested.store(false, Ordering::SeqCst);
            t.set_status(DownloadStatus::Queued);
            self.inner.emit_status(&t);
            self.spawn_run(t);
        }
    }

    /// 取消单个任务。`remove_on_cancel` 为真时删除临时文件。
    pub fn cancel(&self, id: u64) -> Result<(), DownloadError> {
        let state = self.require_task(id)?;
        self.cancel_state(state);
        Ok(())
    }

    /// 取消全部任务。
    pub fn cancel_all(&self) {
        let tasks: Vec<Arc<TaskState>> = self.inner.tasks.lock().values().cloned().collect();
        for t in tasks {
            self.cancel_state(t);
        }
    }

    fn cancel_state(&self, state: Arc<TaskState>) {
        if matches!(
            *state.status.lock(),
            DownloadStatus::Done | DownloadStatus::Cancelled
        ) {
            return;
        }
        state.cancel_token.cancel();
        state.pause_requested.store(false, Ordering::SeqCst);
        state.set_status(DownloadStatus::Cancelled);
        if state.options.remove_on_cancel {
            let _ = std::fs::remove_file(&state.part_path);
            state.downloaded_bytes.store(0, Ordering::Relaxed);
        }
        self.inner.emit_status(&state);
    }

    /// 失败 / 取消任务重试（保留已下载字节，从断点续传）。
    pub fn retry(&self, id: u64) -> Result<(), DownloadError> {
        let state = self.require_task(id)?;
        if !matches!(
            *state.status.lock(),
            DownloadStatus::Failed | DownloadStatus::Cancelled
        ) {
            return Ok(());
        }
        state.error.lock().take();
        state.pause_requested.store(false, Ordering::SeqCst);
        state.set_status(DownloadStatus::Queued);
        self.spawn_run(state);
        Ok(())
    }

    /// 移除任务记录（已完成 / 已取消 / 已失败的任务）。
    pub fn remove(&self, id: u64) -> Result<(), DownloadError> {
        let state = self.require_task(id)?;
        if state.status.lock().is_active() {
            return Err(DownloadError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "active task cannot be removed",
            )));
        }
        self.inner.tasks.lock().remove(&id);
        Ok(())
    }

    /// 查询单个任务快照。
    pub fn snapshot(&self, id: u64) -> Result<TaskSnapshot, DownloadError> {
        Ok(self.require_task(id)?.snapshot())
    }

    /// 查询全部任务快照（按创建时间排序）。
    pub fn snapshots(&self) -> Vec<TaskSnapshot> {
        let mut list: Vec<TaskSnapshot> = self
            .inner
            .tasks
            .lock()
            .values()
            .map(|s| s.snapshot())
            .collect();
        list.sort_by_key(|s| s.created_at_ms);
        list
    }

    fn require_task(&self, id: u64) -> Result<Arc<TaskState>, DownloadError> {
        self.inner
            .tasks
            .lock()
            .get(&id)
            .cloned()
            .ok_or(DownloadError::TaskNotFound(id))
    }

    /// 派生单个任务的运行循环。
    fn spawn_run(&self, state: Arc<TaskState>) {
        let inner = self.inner.clone();
        self.inner.runtime.spawn(async move {
            if let Err(e) = run_task(inner, state.clone()).await {
                warn!("download task {} exited with error: {e}", state.id);
            }
        });
    }
}

/// 任务运行：单次执行直到完成 / 暂停 / 取消 / 失败（暂停或取消后由上层重新派发）。
async fn run_task(inner: Arc<ManagerInner>, state: Arc<TaskState>) -> Result<(), DownloadError> {
    // 处理用户中断：暂停 / 取消优先于开始或重试。
    if state.pause_requested.load(Ordering::SeqCst) {
        state.set_status(DownloadStatus::Paused);
        inner.emit_status(&state);
        return Ok(());
    }
    if state.cancel_token.is_cancelled() {
        state.set_status(DownloadStatus::Cancelled);
        inner.emit_status(&state);
        return Ok(());
    }

    // 目标已存在且校验通过（幂等策略）→ 直接完成。
    if matches!(
        state.options.existing_policy,
        ExistingFilePolicy::SkipIfValid
    ) && state.dest.exists()
    {
        let valid = match &state.options.expected_sha256 {
            Some(expected) => hash_file(&state.dest)
                .await
                .map(|h| h.eq_ignore_ascii_case(expected))
                .unwrap_or(false),
            None => true,
        };
        if valid {
            state.set_status(DownloadStatus::Done);
            inner.emit_status(&state);
            return Ok(());
        }
    }

    // 等待并发名额，期间可响应暂停 / 取消。
    let permit = {
        let acquire = inner.semaphore.clone().acquire_owned();
        tokio::pin!(acquire);
        tokio::select! {
            permit = &mut acquire => permit.expect("semaphore closed"),
            _ = state.cancel_token.cancelled() => {
                state.set_status(DownloadStatus::Cancelled);
                inner.emit_status(&state);
                return Ok(());
            }
            _ = wait_pause(&state) => {
                state.set_status(DownloadStatus::Paused);
                inner.emit_status(&state);
                return Ok(());
            }
        }
    };

    state.set_status(DownloadStatus::Downloading);
    inner.emit_status(&state);

    let attempts = state.options.max_retries;
    let mut attempt = 0u32;
    let outcome = loop {
        match download_once(&inner.client, &state, inner.as_ref()).await {
            Ok(total) => {
                state.total_bytes.store(total, Ordering::Relaxed);
                break Ok(());
            }
            Err(e) if e.is_user_interrupt() => break Err(e),
            Err(e) if e.is_transient() && attempt < attempts => {
                attempt += 1;
                state.retry_count.store(attempt, Ordering::Relaxed);
                state.set_error(format!("{e}（第 {attempt} 次重试）"));
                inner.emit_status(&state);
                let delay = Duration::from_millis(
                    (BACKOFF_BASE_MS * 2u64.pow(attempt.saturating_sub(1)))
                        .min(BACKOFF_CAP_MS),
                );
                debug!(
                    "download {} attempt {}/{} failed: {e}, backing off {delay:?}",
                    state.id, attempt, attempts
                );
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = state.cancel_token.cancelled() => {
                        state.set_status(DownloadStatus::Cancelled);
                        inner.emit_status(&state);
                        return Ok(());
                    }
                    _ = wait_pause(&state) => {
                        state.set_status(DownloadStatus::Paused);
                        inner.emit_status(&state);
                        return Ok(());
                    }
                }
                continue;
            }
            Err(e) => break Err(e),
        }
    };
    drop(permit);

    match outcome {
        Ok(_) => {
            // 校验。
            if let Some(expected) = &state.options.expected_sha256 {
                match hash_file(&state.part_path).await {
                    Ok(actual) if actual.eq_ignore_ascii_case(expected) => {}
                    Ok(actual) => {
                        let err = DownloadError::ChecksumMismatch {
                            expected: expected.clone(),
                            actual,
                        };
                        state.set_error(err.to_string());
                        state.set_status(DownloadStatus::Failed);
                        inner.emit_status(&state);
                        return Ok(());
                    }
                    Err(e) => {
                        state.set_error(e.to_string());
                        state.set_status(DownloadStatus::Failed);
                        inner.emit_status(&state);
                        return Ok(());
                    }
                }
            }
            // 落位。
            if let Err(e) = finalize(&state.part_path, &state.dest).await {
                state.set_error(e.to_string());
                state.set_status(DownloadStatus::Failed);
                inner.emit_status(&state);
                return Ok(());
            }
            state.set_status(DownloadStatus::Done);
            inner.emit_status(&state);
            Ok(())
        }
        Err(e) if e.is_user_interrupt() => {
            if e.is_paused() {
                state.set_status(DownloadStatus::Paused);
            } else {
                state.set_status(DownloadStatus::Cancelled);
            }
            inner.emit_status(&state);
            Ok(())
        }
        Err(e) => {
            state.set_error(e.to_string());
            state.set_status(DownloadStatus::Failed);
            inner.emit_status(&state);
            Ok(())
        }
    }
}

/// 等待暂停请求被置位（轻量轮询，仅用于队列等待与退避期间）。
async fn wait_pause(state: &TaskState) {
    loop {
        if state.pause_requested.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 单次下载尝试：断点续传、流式写入、进度与速率统计。
///
/// 返回响应提供的总字节数（0 表示未知）。
async fn download_once(
    client: &reqwest::Client,
    state: &TaskState,
    inner: &ManagerInner,
) -> Result<u64, DownloadError> {
    let start = if state.options.resume {
        match tokio::fs::metadata(&state.part_path).await {
            Ok(m) => m.len(),
            Err(_) => 0,
        }
    } else {
        0
    };

    let mut builder = client.get(&state.url);
    if start > 0 {
        builder =
            builder.header(RANGE, HeaderValue::from_str(&format!("bytes={start}-")).unwrap());
    }
    for (k, v) in &state.options.headers {
        builder = builder.header(k, v);
    }

    let resp = builder.send().await?;

    // 总大小：优先 Content-Range 的 total，其次 Content-Length。
    let total = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit('/').next())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .or_else(|| resp.content_length())
        .unwrap_or(0);
    state.total_bytes.store(total, Ordering::Relaxed);

    let stream = match resp.status().as_u16() {
        200 => {
            // 服务器忽略 Range，从头重下：截断临时文件。
            let file = tokio::fs::File::create(&state.part_path).await?;
            file.set_len(0).await?;
            state.downloaded_bytes.store(0, Ordering::Relaxed);
            resp.bytes_stream()
        }
        206 => {
            // 追加模式写入，续传位置由文件长度决定。
            let file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&state.part_path)
                .await?;
            drop(file);
            resp.bytes_stream()
        }
        code => return Err(DownloadError::HttpStatus(code)),
    };

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&state.part_path)
        .await?;

    let mut downloaded = start;
    state.downloaded_bytes.store(downloaded, Ordering::Relaxed);

    let mut ema_speed: f64 = 0.0;
    let mut last_tick = Instant::now();
    let mut last_bytes = downloaded as f64;

    futures_util::pin_mut!(stream);
    while let Some(chunk) = stream.next().await {
        // 暂停 / 取消响应（每次收到数据块时检查）。
        if state.pause_requested.load(Ordering::SeqCst) {
            return Err(DownloadError::Paused);
        }
        if state.cancel_token.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }

        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        state.downloaded_bytes.store(downloaded, Ordering::Relaxed);

        let now = Instant::now();
        let elapsed = now.duration_since(last_tick);
        if elapsed >= PROGRESS_EMIT_INTERVAL {
            let dt = elapsed.as_secs_f64().max(0.001);
            let inst = (downloaded as f64 - last_bytes) / dt;
            ema_speed = if ema_speed == 0.0 {
                inst
            } else {
                SPEED_EMA_ALPHA * inst + (1.0 - SPEED_EMA_ALPHA) * ema_speed
            };
            state
                .speed_bytes_per_sec
                .store(ema_speed.max(0.0) as u64, Ordering::Relaxed);
            last_tick = now;
            last_bytes = downloaded as f64;
            inner.emit_progress(state);
        }
    }
    file.flush().await?;

    // 收尾：速率归零，强制发一次进度，确保 UI 到 100%。
    state.speed_bytes_per_sec.store(0, Ordering::Relaxed);
    *state.last_progress_emit.lock() = None;
    inner.emit_progress(state);

    Ok(total)
}

/// 校验文件 SHA-256。
async fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(hex(&digest))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 临时文件落位：Windows 下先移除目标再重命名。
async fn finalize(part: &Path, dest: &Path) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let _ = tokio::fs::remove_file(dest).await;
    tokio::fs::rename(part, dest).await
}
