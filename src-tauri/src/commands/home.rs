//! 开始页模块命令：版本清单 / 设置 / 启动 / 内容管理 / 模组管理。
//!
//! 所有命令经内核上下文访问 home 模块业务逻辑，错误统一映射为 `CommandError`。

use tauri::State;

use crate::commands::into_command_error;
use crate::error::{CommandResult, KernelError};
use crate::modules::home::content;
use crate::modules::home::launch::LaunchOutcome;
use crate::modules::home::mods;
use crate::modules::home::{meta, OpenDirKind, VersionMetaUpdate, VersionView};
use crate::state::KernelContext;

/// 版本清单。
#[tauri::command]
pub fn home_versions_list(kernel: State<'_, KernelContext>) -> CommandResult<Vec<VersionView>> {
    Ok(crate::modules::home::HomeModule::list_versions(kernel.inner()))
}

/// 当前解析的版本根目录（供前端显示实际游戏目录）。
#[tauri::command]
pub fn home_versions_root(kernel: State<'_, KernelContext>) -> CommandResult<String> {
    Ok(kernel
        .inner()
        .versions_root()
        .to_string_lossy()
        .into_owned())
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
    let dir = meta::resolve_version_dir(&kernel.inner().versions_root(), &name).map_err(into_command_error)?;
    meta::save_logo(&dir, &data_url).map_err(into_command_error)
}

/// 移除版本图标。
#[tauri::command]
pub fn home_logo_remove(kernel: State<'_, KernelContext>, name: String) -> CommandResult<()> {
    let dir = meta::resolve_version_dir(&kernel.inner().versions_root(), &name).map_err(into_command_error)?;
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

// ---------------------------------------------------------------- 模组管理

/// 模组清单（附带缺少清单被跳过的目录数）。
#[tauri::command]
pub fn home_mods_list(
    kernel: State<'_, KernelContext>,
    name: String,
) -> CommandResult<mods::ModListResult> {
    mods::list_mods(kernel.inner(), &name).map_err(into_command_error)
}

/// 从 ZIP 导入模组（重名且 `overwrite` 为假时返回 `conflict`）。
#[tauri::command]
pub fn home_mods_import_zip(
    kernel: State<'_, KernelContext>,
    name: String,
    source_path: String,
    overwrite: bool,
) -> CommandResult<mods::ModView> {
    mods::import_zip(kernel.inner(), &name, &source_path, overwrite).map_err(into_command_error)
}

/// 从单个 DLL 导入模组（自动生成清单）。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn home_mods_import_dll(
    kernel: State<'_, KernelContext>,
    name: String,
    source_path: String,
    mod_name: String,
    mod_type: String,
    version: String,
    overwrite: bool,
) -> CommandResult<mods::ModView> {
    mods::import_dll(
        kernel.inner(),
        &name,
        &source_path,
        &mod_name,
        &mod_type,
        &version,
        overwrite,
    )
    .map_err(into_command_error)
}

/// 启用 / 停用模组。
#[tauri::command]
pub fn home_mods_set_enabled(
    kernel: State<'_, KernelContext>,
    name: String,
    folder: String,
    enabled: bool,
) -> CommandResult<()> {
    mods::set_mod_enabled(kernel.inner(), &name, &folder, enabled).map_err(into_command_error)
}

/// 删除模组。
#[tauri::command]
pub fn home_mods_remove(
    kernel: State<'_, KernelContext>,
    name: String,
    folder: String,
) -> CommandResult<()> {
    mods::remove_mod(kernel.inner(), &name, &folder).map_err(into_command_error)
}

/// 编辑模组清单。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn home_mods_save_manifest(
    kernel: State<'_, KernelContext>,
    name: String,
    folder: String,
    mod_name: String,
    entry: String,
    version: String,
    mod_type: String,
    author: String,
) -> CommandResult<mods::ModView> {
    mods::save_mod_manifest(
        kernel.inner(),
        &name,
        &folder,
        &mod_name,
        &entry,
        &version,
        &mod_type,
        &author,
    )
    .map_err(into_command_error)
}

/// 在系统文件管理器中打开模组目录（目录不存在则创建）。
#[tauri::command]
pub fn home_mods_open_folder(
    kernel: State<'_, KernelContext>,
    name: String,
) -> CommandResult<String> {
    let dir = mods::mods_dir(kernel.inner(), &name, true).map_err(into_command_error)?;
    tauri_plugin_opener::open_path(&dir, None::<&str>)
        .map_err(|e| KernelError::InvalidArgument(format!("打开目录失败: {e}")))
        .map_err(into_command_error)?;
    Ok(dir.to_string_lossy().into_owned())
}

/// 在系统文件管理器中打开版本相关目录（版本目录 / 模组目录 / 存档目录），
/// 返回目录绝对路径。目录不存在则创建。
#[tauri::command]
pub fn home_version_open_dir(
    kernel: State<'_, KernelContext>,
    name: String,
    kind: OpenDirKind,
) -> CommandResult<String> {
    let dir = crate::modules::home::resolve_open_dir(kernel.inner(), &name, kind, true)
        .map_err(into_command_error)?;
    tauri_plugin_opener::open_path(&dir, None::<&str>)
        .map_err(|e| KernelError::InvalidArgument(format!("打开目录失败: {e}")))
        .map_err(into_command_error)?;
    Ok(dir.to_string_lossy().into_owned())
}
