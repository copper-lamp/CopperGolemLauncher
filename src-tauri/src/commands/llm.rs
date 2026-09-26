//! LLM 模型表命令：列出、增删改模型，写入 / 清除单条模型的 API Key。
//!
//! 密钥绝不回显：`llm_list_models` 只回「是否已配置」，密钥走独立命令、不进普通
//! 设置写入。内核不标记「默认 / 当前」模型——调用方按 id 指定要用的条目。

use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::services::llm::LlmModelRow;
use crate::state::KernelContext;

/// 列出模型表（**不含密钥**，仅标注每条是否已配置）。
#[tauri::command]
pub fn llm_list_models(kernel: State<'_, KernelContext>) -> CommandResult<Vec<LlmModelRow>> {
    Ok(kernel.llm().rows())
}

/// 新增模型，返回新生成的 id。
#[tauri::command]
pub fn llm_add_model(
    kernel: State<'_, KernelContext>,
    display_name: String,
    base_url: String,
    model: String,
) -> CommandResult<String> {
    kernel
        .llm()
        .add(display_name, base_url, model)
        .map_err(into_command_error)
}

/// 按 id 更新模型。
#[tauri::command]
pub fn llm_update_model(
    kernel: State<'_, KernelContext>,
    id: String,
    display_name: String,
    base_url: String,
    model: String,
) -> CommandResult<()> {
    kernel
        .llm()
        .update(&id, display_name, base_url, model)
        .map_err(into_command_error)
}

/// 按 id 删除模型（同时清除其密钥）。
#[tauri::command]
pub fn llm_remove_model(kernel: State<'_, KernelContext>, id: String) -> CommandResult<()> {
    kernel.llm().remove(&id).map_err(into_command_error)
}

/// 写入某条模型的 API Key（不回显）。
#[tauri::command]
pub fn llm_set_api_key(
    kernel: State<'_, KernelContext>,
    id: String,
    api_key: String,
) -> CommandResult<()> {
    kernel
        .llm()
        .set_api_key(&id, &api_key)
        .map_err(into_command_error)
}

/// 清除某条模型的 API Key。
#[tauri::command]
pub fn llm_clear_api_key(kernel: State<'_, KernelContext>, id: String) -> CommandResult<()> {
    kernel.llm().clear_api_key(&id).map_err(into_command_error)
}
