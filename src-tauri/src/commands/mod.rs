//! Tauri 命令层：把内核全部能力暴露给前端。
//!
//! 每个命令返回 `CommandResult<T>`（见 [`crate::error`]），错误经统一结构
//! 序列化，前端据此做 Toast / 弹窗反馈。事件（下载进度、账户状态、设置变更等）
//! 由事件总线自动桥接到前端，前端经 `@tauri-apps/api/event` 监听同名事件。

pub mod account;
pub mod content_download;
pub mod download;
pub mod game_download;
pub mod home;
pub mod i18n;
pub mod intents;
pub mod kernel;
pub mod modules;
pub mod settings;
pub mod theme;
pub mod updater;

use crate::error::CommandError;

/// 统一错误映射：`KernelError` → 命令层可序列化错误。
pub(crate) fn into_command_error(e: crate::error::KernelError) -> CommandError {
    CommandError::from_kernel(&e)
}
