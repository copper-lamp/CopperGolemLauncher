//! LLM 配置命令：状态查询、保存非敏感配置、写入 / 清除 API Key。
//!
//! 密钥绝不回显：`llm_status` 只回「是否已配置」，密钥走独立命令、不进普通设置写入。

use serde::Serialize;
use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::services::llm::LlmConfig;
use crate::state::KernelContext;

/// LLM 配置状态（**不含密钥本体**）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LlmStatus {
    pub base_url: String,
    pub model: String,
    pub api_key_configured: bool,
}

/// 读取 LLM 配置状态（密钥只回「是否已配置」）。
#[tauri::command]
pub fn llm_status(kernel: State<'_, KernelContext>) -> CommandResult<LlmStatus> {
    let config = kernel.llm().config();
    Ok(LlmStatus {
        base_url: config.base_url,
        model: config.model,
        api_key_configured: kernel.llm().api_key_configured(),
    })
}

/// 保存非敏感配置（base URL / 模型名）；密钥走 `llm_set_api_key`。
#[tauri::command]
pub fn llm_save_config(
    kernel: State<'_, KernelContext>,
    base_url: String,
    model: String,
) -> CommandResult<()> {
    kernel
        .llm()
        .save_config(&LlmConfig { base_url, model })
        .map_err(into_command_error)
}

/// 写入 API Key（不回显）。
#[tauri::command]
pub fn llm_set_api_key(kernel: State<'_, KernelContext>, api_key: String) -> CommandResult<()> {
    kernel
        .llm()
        .set_api_key(&api_key)
        .map_err(into_command_error)
}

/// 清除 API Key（幂等）。
#[tauri::command]
pub fn llm_clear_api_key(kernel: State<'_, KernelContext>) -> CommandResult<()> {
    kernel
        .llm()
        .clear_api_key()
        .map_err(into_command_error)
}
