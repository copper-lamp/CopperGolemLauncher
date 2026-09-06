//! 游戏下载模块命令：清单 / 详情 / 投递下载 / 刷新源 / 取消 / 状态。

use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::modules::game_download::installer::{self, Ctx};
use crate::modules::game_download::manifest::{self, ManifestView};
use crate::state::KernelContext;

/// 版本清单（含已安装 / 下载中状态）。
#[tauri::command]
pub async fn game_download_manifest(
    kernel: State<'_, KernelContext>,
    refresh: Option<bool>,
) -> CommandResult<ManifestView> {
    let ctx = Ctx::from_kernel(&kernel);
    let versions = match manifest::load_manifest(&ctx, refresh.unwrap_or(false)).await {
        Ok(v) => v,
        Err(e) => return Err(into_command_error(e)),
    };
    Ok(manifest::build_view(&ctx, &versions))
}

/// 单版本详情（任务状态）。
#[tauri::command]
pub async fn game_download_detail(
    kernel: State<'_, KernelContext>,
    id: String,
) -> CommandResult<Option<installer::TaskView>> {
    let ctx = Ctx::from_kernel(&kernel);
    installer::status(&ctx, &id).map_err(into_command_error)
}

/// 投递下载（幂等），返回下载任务 id。
#[tauri::command]
pub async fn game_download_enqueue(
    kernel: State<'_, KernelContext>,
    id: String,
) -> CommandResult<u64> {
    let ctx = Ctx::from_kernel(&kernel);
    installer::enqueue(&ctx, &id)
        .await
        .map_err(into_command_error)
}

/// 强制刷新版本清单源。
#[tauri::command]
pub async fn game_download_refresh_source(
    kernel: State<'_, KernelContext>,
) -> CommandResult<()> {
    let ctx = Ctx::from_kernel(&kernel);
    installer::refresh_source(&ctx)
        .await
        .map_err(into_command_error)
}

/// 取消下载任务。
#[tauri::command]
pub async fn game_download_cancel(
    kernel: State<'_, KernelContext>,
    id: String,
) -> CommandResult<()> {
    let ctx = Ctx::from_kernel(&kernel);
    installer::cancel(&ctx, &id).map_err(into_command_error)
}

/// 单版本任务状态。
#[tauri::command]
pub async fn game_download_status(
    kernel: State<'_, KernelContext>,
    id: String,
) -> CommandResult<Option<installer::TaskView>> {
    let ctx = Ctx::from_kernel(&kernel);
    installer::status(&ctx, &id).map_err(into_command_error)
}