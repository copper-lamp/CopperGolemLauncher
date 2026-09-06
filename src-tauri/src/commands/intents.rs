//! 意图命令：发起意图请求、查询已声明意图。

use serde_json::Value;
use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::state::KernelContext;

/// 发起意图请求（请求 / 响应式模块联动）。
#[tauri::command]
pub fn intents_request(
    kernel: State<'_, KernelContext>,
    intent: String,
    payload: Value,
) -> CommandResult<Value> {
    kernel
        .inner()
        .intents()
        .request(&intent, payload)
        .map_err(into_command_error)
}

/// 已声明意图清单（模块名 -> 意图名）。
#[tauri::command]
pub fn intents_declared(kernel: State<'_, KernelContext>) -> CommandResult<Vec<(String, String)>> {
    Ok(kernel.inner().intents().declared())
}
