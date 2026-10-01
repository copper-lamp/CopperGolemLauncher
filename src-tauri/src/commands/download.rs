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
    Ok(kernel.download().tasks())
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

/// 清空下载记录（终态条目），返回删除的持久化行数。
///
/// 只删记录不删文件，也不碰在跑 / 排队 / 安装中的任务：见
/// [`crate::services::download::DownloadService::clear_history`]。
#[tauri::command]
pub fn download_clear_history(kernel: State<'_, KernelContext>) -> CommandResult<u64> {
    kernel
        .download()
        .clear_history()
        .map(|n| n as u64)
        .map_err(into_command_error)
}

/// 在系统文件管理器中定位下载产物。
///
/// 不接收任务 id 而接收路径：下载中心同时展示内核任务与各模块自己的记录
/// （内容下载的落点是模组目录、游戏安装的落点是版本目录），让每个来源各自
/// 解析落点比在这里塞一张「id → 路径」的映射表更稳，也不会因为某条记录被
/// 清掉而打不开文件夹。
#[tauri::command]
pub fn download_reveal(path: String) -> CommandResult<()> {
    let path = path.trim();
    if path.is_empty() {
        return Err(into_command_error(crate::error::KernelError::InvalidArgument(
            "该条目没有可打开的文件路径".into(),
        )));
    }
    crate::platform::shell::reveal(std::path::Path::new(path))
        .map_err(|message| into_command_error(crate::error::KernelError::InvalidArgument(message)))
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

/// 并发上限的合法区间：与前端下拉框 1~5 一致，后端做最后一道夹紧。
const CONCURRENCY_MIN: usize = 1;
const CONCURRENCY_MAX: usize = 5;

/// 读取当前同时下载数。
#[tauri::command]
pub fn download_concurrency(kernel: State<'_, KernelContext>) -> CommandResult<usize> {
    Ok(kernel.download().concurrency())
}

/// 设置同时下载数并即时生效。越界值夹紧到 [1, 5]，不报错——
/// 前端已用下拉框约束取值，夹紧只是防御异常入参。
#[tauri::command]
pub fn download_set_concurrency(
    kernel: State<'_, KernelContext>,
    concurrency: usize,
) -> CommandResult<usize> {
    let clamped = concurrency.clamp(CONCURRENCY_MIN, CONCURRENCY_MAX);
    kernel.download().set_concurrency(clamped);
    // 同步写回设置，使重启后仍是用户选择的值。
    let mut entries = std::collections::HashMap::new();
    entries.insert(
        "download.concurrency".to_string(),
        serde_json::Value::Number(clamped.into()),
    );
    kernel
        .settings()
        .set_many(&entries)
        .map_err(into_command_error)?;
    Ok(clamped)
}
