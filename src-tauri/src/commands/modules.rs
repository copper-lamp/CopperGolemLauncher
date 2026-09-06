//! 模块命令：模块清单、启用 / 禁用。

use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::registry::modules::ModuleInfo;
use crate::state::KernelContext;

/// 模块信息列表（供设置页"模块"Tab 展示）。
#[tauri::command]
pub fn modules_list(kernel: State<'_, KernelContext>) -> CommandResult<Vec<ModuleInfo>> {
    Ok(kernel.inner().modules().list(kernel.inner()))
}

/// 切换模块启用状态（下次启动生效）。
#[tauri::command]
pub fn modules_set_enabled(
    kernel: State<'_, KernelContext>,
    id: String,
    enabled: bool,
) -> CommandResult<()> {
    kernel
        .inner()
        .modules()
        .set_enabled(kernel.inner(), &id, enabled)
        .map_err(into_command_error)
}
