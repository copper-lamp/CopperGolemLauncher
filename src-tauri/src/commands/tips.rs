//! 提示命令：向所有前端（内核 Shell 与各模块）暴露提示能力。
//!
//! 设计意图：提示是**内核能力**，模块不自行维护提示文案，统一经此取用，
//! 保证全应用提示风格与语言一致。

use serde::Serialize;
use tauri::State;

use crate::error::CommandResult;
use crate::state::KernelContext;

/// 一条提示的视图。`key` 供前端 `t()` 渲染（语言切换即时生效），
/// `text` 为当前语言下的文案（便于非 i18n 场景直接展示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct TipPayload {
    pub key: String,
    pub text: String,
}

/// 取一条随机提示；当前语言无可用提示时返回 `null`。
#[tauri::command]
pub fn tips_next(kernel: State<'_, KernelContext>) -> CommandResult<Option<TipPayload>> {
    Ok(kernel.tips().next().map(|t| TipPayload {
        key: t.key,
        text: t.text,
    }))
}

/// 当前语言下全部提示 key（按序号排序）。
#[tauri::command]
pub fn tips_keys(kernel: State<'_, KernelContext>) -> CommandResult<Vec<String>> {
    Ok(kernel.tips().keys())
}
