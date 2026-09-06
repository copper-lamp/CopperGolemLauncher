//! 更新命令：检查新版本、下载更新包、安装并重启、查询状态。

use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::services::updater::UpdateStatus;
use crate::state::KernelContext;

/// 检查最新版本（GitHub Releases，semver 比较）。
#[tauri::command]
pub async fn updater_check(kernel: State<'_, KernelContext>) -> CommandResult<UpdateStatus> {
    kernel.updater().check().await.map_err(into_command_error)
}

/// 下载更新包（返回下载任务 id，进度经 `download.*` 事件广播）。
#[tauri::command]
pub async fn updater_apply(kernel: State<'_, KernelContext>) -> CommandResult<u64> {
    kernel.updater().apply().await.map_err(into_command_error)
}

/// 安装已下载的更新：原地替换可执行文件并重启应用。
#[tauri::command]
pub fn updater_install(kernel: State<'_, KernelContext>) -> CommandResult<()> {
    kernel.updater().install().map_err(into_command_error)
}

/// 当前更新状态。
#[tauri::command]
pub fn updater_status(kernel: State<'_, KernelContext>) -> CommandResult<UpdateStatus> {
    Ok(kernel.updater().status())
}
