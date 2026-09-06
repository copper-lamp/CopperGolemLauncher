//! i18n 命令：语言目录、支持语言、当前语言、切换语言。

use serde_json::Value;
use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::state::KernelContext;

/// 某语言的完整目录（基准 + 模块语言包），供前端一次性加载。
#[tauri::command]
pub fn i18n_catalog(
    kernel: State<'_, KernelContext>,
    locale: String,
) -> CommandResult<Value> {
    Ok(kernel.i18n().catalog(&locale))
}

/// 支持的基准语言列表。
#[tauri::command]
pub fn i18n_supported_locales(kernel: State<'_, KernelContext>) -> CommandResult<Vec<String>> {
    Ok(kernel.i18n().supported_locales())
}

/// 当前语言。
#[tauri::command]
pub fn i18n_current_locale(kernel: State<'_, KernelContext>) -> CommandResult<String> {
    Ok(kernel.i18n().current_locale())
}

/// 切换语言并持久化。
#[tauri::command]
pub fn i18n_set_locale(kernel: State<'_, KernelContext>, locale: String) -> CommandResult<()> {
    kernel.i18n().set_locale(&locale).map_err(into_command_error)
}
