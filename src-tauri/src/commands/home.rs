//! 开始页模块命令：版本清单 / 设置 / 启动 / 内容管理。
//!
//! 所有命令经内核上下文访问 home 模块业务逻辑，错误统一映射为 `CommandError`。

use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::modules::home::content;
use crate::modules::home::launch::LaunchOutcome;
use crate::modules::home::{meta, VersionMetaUpdate, VersionView};
use crate::state::KernelContext;

/// 版本清单。
#[tauri::command]
pub fn home_versions_list(kernel: State<'_, KernelContext>) -> CommandResult<Vec<VersionView>> {
    Ok(crate::modules::home::HomeModule::list_versions(kernel.inner()))
}

/// 单个版本信息。
#[tauri::command]
pub fn home_version_get(
    kernel: State<'_, KernelContext>,
    name: String,
) -> CommandResult<VersionView> {
    crate::modules::home::HomeModule::get_version(kernel.inner(), &name)
        .map_err(into_command_error)
}

/// 部分更新版本设置（渲染龙 / 世界编辑器 / 启动参数等）。
#[tauri::command]
pub fn home_version_save_meta(
    kernel: State<'_, KernelContext>,
    name: String,
    update: VersionMetaUpdate,
) -> CommandResult<VersionView> {
    crate::modules::home::HomeModule::save_meta(kernel.inner(), &name, &update)
        .map_err(into_command_error)
}

/// 重命名版本。
#[tauri::command]
pub fn home_version_rename(
    kernel: State<'_, KernelContext>,
    old_name: String,
    new_name: String,
) -> CommandResult<VersionView> {
    crate::modules::home::HomeModule::rename_version(kernel.inner(), &old_name, &new_name)
        .map_err(into_command_error)
}

/// 删除版本（成功后广播 `version.removed`）。
#[tauri::command]
pub fn home_version_delete(kernel: State<'_, KernelContext>, name: String) -> CommandResult<()> {
    crate::modules::home::HomeModule::delete_version(kernel.inner(), &name)
        .map_err(into_command_error)
}

/// 启动游戏（成功后后台确认进程并广播 `game.launched`）。
#[tauri::command]
pub fn home_launch(kernel: State<'_, KernelContext>, name: String) -> CommandResult<LaunchOutcome> {
    crate::modules::home::HomeModule::launch(kernel.inner(), &name).map_err(into_command_error)
}

/// 保存版本图标（`data:image/png;base64,...`，前端已裁剪 256×256）。
#[tauri::command]
pub fn home_logo_set(
    kernel: State<'_, KernelContext>,
    name: String,
    data_url: String,
) -> CommandResult<()> {
    let root = kernel.inner().paths().versions_dir();
    let dir = meta::resolve_version_dir(root, &name).map_err(into_command_error)?;
    meta::save_logo(&dir, &data_url).map_err(into_command_error)
}

/// 移除版本图标。
#[tauri::command]
pub fn home_logo_remove(kernel: State<'_, KernelContext>, name: String) -> CommandResult<()> {
    let root = kernel.inner().paths().versions_dir();
    let dir = meta::resolve_version_dir(root, &name).map_err(into_command_error)?;
    meta::remove_logo(&dir).map_err(into_command_error)
}

/// 版本已加入资源清单。
#[tauri::command]
pub fn home_content_list(
    kernel: State<'_, KernelContext>,
    name: String,
) -> CommandResult<Vec<content::ContentItem>> {
    content::list_content(kernel.inner(), &name).map_err(into_command_error)
}

/// 启用 / 禁用内容条目。
#[tauri::command]
pub fn home_content_set_enabled(
    kernel: State<'_, KernelContext>,
    name: String,
    item_id: String,
    enabled: bool,
) -> CommandResult<()> {
    content::set_content_enabled(kernel.inner(), &name, &item_id, enabled)
        .map_err(into_command_error)
}

/// 删除内容条目。
#[tauri::command]
pub fn home_content_remove(
    kernel: State<'_, KernelContext>,
    name: String,
    item_id: String,
) -> CommandResult<()> {
    content::remove_content(kernel.inner(), &name, &item_id).map_err(into_command_error)
}
