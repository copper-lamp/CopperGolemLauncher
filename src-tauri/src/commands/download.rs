//! 下载命令：投递任务、任务快照、暂停 / 恢复 / 取消 / 重试 / 移除。
//!
//! 投递参数 `EnqueueOptions` 采用 camelCase + 全可选，前端只传需要覆盖的字段。
//! 任务进度 / 状态变化经 `download.*` 事件推送到前端。

use std::path::PathBuf;

use copper_downloader::error::ExistingFilePolicy;
use copper_downloader::DownloadOptions;
use serde::Deserialize;
use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::services::download::DownloadTaskView;
use crate::state::KernelContext;

/// 投递任务的可选参数（对应引擎 [`DownloadOptions`]，全字段可选）。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct EnqueueOptions {
    pub resume: Option<bool>,
    pub remove_on_cancel: Option<bool>,
    pub expected_sha256: Option<String>,
    pub max_retries: Option<u32>,
    pub headers: Vec<(String, String)>,
    pub filename: Option<String>,
    /// "overwrite"（默认）| "skip_if_valid"。
    pub existing_policy: Option<String>,
}

impl From<EnqueueOptions> for DownloadOptions {
    fn from(o: EnqueueOptions) -> Self {
        let mut d = DownloadOptions::default();
        if let Some(v) = o.resume {
            d.resume = v;
        }
        if let Some(v) = o.remove_on_cancel {
            d.remove_on_cancel = v;
        }
        d.expected_sha256 = o.expected_sha256;
        if let Some(v) = o.max_retries {
            d.max_retries = v;
        }
        d.headers = o.headers;
        d.filename = o.filename;
        if let Some(p) = o.existing_policy.as_deref() {
            d.existing_policy = match p {
                "skip_if_valid" => ExistingFilePolicy::SkipIfValid,
                _ => ExistingFilePolicy::Overwrite,
            };
        }
        d
    }
}

/// 投递下载任务，返回任务 id。
#[tauri::command]
pub fn download_enqueue(
    kernel: State<'_, KernelContext>,
    url: String,
    dest: PathBuf,
    options: Option<EnqueueOptions>,
) -> CommandResult<u64> {
    kernel
        .download()
        .enqueue(&url, &dest, options.unwrap_or_default().into())
        .map_err(into_command_error)
}

/// 全部任务快照。
#[tauri::command]
pub fn download_tasks(kernel: State<'_, KernelContext>) -> CommandResult<Vec<DownloadTaskView>> {
    Ok(kernel
        .download()
        .tasks()
        .into_iter()
        .map(DownloadTaskView::from)
        .collect())
}

/// 单个任务快照。
#[tauri::command]
pub fn download_task(
    kernel: State<'_, KernelContext>,
    id: u64,
) -> CommandResult<Option<DownloadTaskView>> {
    Ok(kernel.download().task(id).map(DownloadTaskView::from))
}

#[tauri::command]
pub fn download_pause(kernel: State<'_, KernelContext>, id: u64) -> CommandResult<()> {
    kernel.download().pause(id).map_err(into_command_error)
}

#[tauri::command]
pub fn download_resume(kernel: State<'_, KernelContext>, id: u64) -> CommandResult<()> {
    kernel.download().resume(id).map_err(into_command_error)
}

#[tauri::command]
pub fn download_cancel(kernel: State<'_, KernelContext>, id: u64) -> CommandResult<()> {
    kernel.download().cancel(id).map_err(into_command_error)
}

#[tauri::command]
pub fn download_retry(kernel: State<'_, KernelContext>, id: u64) -> CommandResult<()> {
    kernel.download().retry(id).map_err(into_command_error)
}

#[tauri::command]
pub fn download_remove(kernel: State<'_, KernelContext>, id: u64) -> CommandResult<()> {
    kernel.download().remove(id).map_err(into_command_error)
}

#[tauri::command]
pub fn download_pause_all(kernel: State<'_, KernelContext>) -> CommandResult<()> {
    kernel.download().pause_all();
    Ok(())
}

#[tauri::command]
pub fn download_resume_all(kernel: State<'_, KernelContext>) -> CommandResult<()> {
    kernel.download().resume_all();
    Ok(())
}
