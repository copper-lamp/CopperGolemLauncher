//! 设置命令：KV 读 / 写 / 批量写 / 全量快照。

use std::collections::HashMap;

use serde_json::Value;
use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::state::KernelContext;

/// 全量设置快照（含默认值）。
#[tauri::command]
pub fn settings_all(kernel: State<'_, KernelContext>) -> CommandResult<HashMap<String, Value>> {
    Ok(kernel.settings().all())
}

/// 读取单个设置（不存在返回 null）。
#[tauri::command]
pub fn settings_get(
    kernel: State<'_, KernelContext>,
    key: String,
) -> CommandResult<Option<Value>> {
    Ok(kernel.settings().get::<Value>(&key))
}

/// 写入单个设置并广播 `settings.changed`。
#[tauri::command]
pub fn settings_set(
    kernel: State<'_, KernelContext>,
    key: String,
    value: Value,
) -> CommandResult<()> {
    kernel.settings().set(&key, &value).map_err(into_command_error)
}

/// 批量写入并广播一次 `settings.changed`。
#[tauri::command]
pub fn settings_set_many(
    kernel: State<'_, KernelContext>,
    entries: HashMap<String, Value>,
) -> CommandResult<()> {
    kernel
        .settings()
        .set_many(&entries)
        .map_err(into_command_error)
}
