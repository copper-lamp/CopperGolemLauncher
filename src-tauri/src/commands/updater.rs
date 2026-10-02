//! 更新命令：手动检查、下载更新包、取消、安装并重启、查询状态。

use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::services::updater::UpdateStatus;
use crate::state::KernelContext;

/// 手动检查新版本。
///
/// 与启动期的后台检查共用同一条链路，差别只在**失败一定会返回给调用方并广播**：
/// 用户主动点的按钮没有提示就是坏了。
#[tauri::command]
pub async fn updater_check(kernel: State<'_, KernelContext>) -> CommandResult<UpdateStatus> {
    kernel.updater().check().await.map_err(into_command_error)
}

/// 把更新包投进全局下载队列，返回下载任务 id（进度经 `download-*` 事件广播）。
#[tauri::command]
pub async fn updater_download(kernel: State<'_, KernelContext>) -> CommandResult<u64> {
    kernel.updater().download().await.map_err(into_command_error)
}

/// 取消更新包下载（保留断点，可再次续传）。
#[tauri::command]
pub fn updater_cancel(kernel: State<'_, KernelContext>) -> CommandResult<()> {
    kernel.updater().cancel_download().map_err(into_command_error)
}

/// 执行替换并重启启动器。
#[tauri::command]
pub fn updater_install(kernel: State<'_, KernelContext>) -> CommandResult<()> {
    kernel.updater().install().map_err(into_command_error)
}

/// 当前更新状态。
#[tauri::command]
pub fn updater_status(kernel: State<'_, KernelContext>) -> CommandResult<UpdateStatus> {
    Ok(kernel.updater().status())
}