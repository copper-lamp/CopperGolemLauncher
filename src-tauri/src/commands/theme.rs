//! 主题命令：快照、深浅色模式、强调色。

use serde_json::Value;
use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::services::theme::ThemeMode;
use crate::state::KernelContext;

/// 主题快照（前端启动时一次性应用）。
#[tauri::command]
pub fn theme_snapshot(kernel: State<'_, KernelContext>) -> CommandResult<Value> {
    Ok(kernel.theme().snapshot())
}

/// 设置深浅色模式（dark / light / auto）。
#[tauri::command]
pub fn theme_set_mode(
    kernel: State<'_, KernelContext>,
    mode: ThemeMode,
) -> CommandResult<()> {
    kernel.theme().set_mode(mode).map_err(into_command_error)
}

/// 设置强调色（`#RRGGBB`）。
#[tauri::command]
pub fn theme_set_accent(kernel: State<'_, KernelContext>, hex: String) -> CommandResult<()> {
    kernel.theme().set_accent(&hex).map_err(into_command_error)
}
