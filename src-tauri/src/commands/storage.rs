//! Storage configuration is staged for restart, before the live database opens.
//!
//! 占用统计全部走后台扫描：命令层只读内存快照或拉起线程，绝不在 IPC 线程上扫盘。

use serde_json::Value;
use tauri::State;

use crate::services::storage::StorageRequest;
use crate::services::storage_scan::{self, ScanSnapshot};
use crate::services::storage_usage::CleanReport;
use crate::{error::CommandResult, services::storage, state::KernelContext};

#[tauri::command]
pub fn storage_status(kernel: State<'_, KernelContext>) -> CommandResult<Value> {
    storage::status(kernel.paths(), kernel.settings()).map_err(super::into_command_error)
}

#[tauri::command]
pub fn storage_preview(
    kernel: State<'_, KernelContext>,
    request: StorageRequest,
) -> CommandResult<Value> {
    storage::preview(kernel.paths(), kernel.settings(), &request).map_err(super::into_command_error)
}

#[tauri::command]
pub fn storage_apply(
    kernel: State<'_, KernelContext>,
    request: StorageRequest,
) -> CommandResult<Value> {
    storage::apply(kernel.paths(), kernel.settings(), &request).map_err(super::into_command_error)
}

#[tauri::command]
pub fn storage_cancel(kernel: State<'_, KernelContext>) -> CommandResult<()> {
    storage::cancel(kernel.paths()).map_err(super::into_command_error)
}

/// 读占用快照：零重 IO，界面一进来就能画。
#[tauri::command(async)]
pub fn storage_usage_snapshot(kernel: State<'_, KernelContext>) -> CommandResult<ScanSnapshot> {
    Ok(storage_scan::snapshot(kernel.paths(), kernel.settings()))
}

/// 启动后台扫描，立刻返回启动瞬间的快照。
#[tauri::command(async)]
pub fn storage_scan_start(
    kernel: State<'_, KernelContext>,
    force: Option<bool>,
) -> CommandResult<ScanSnapshot> {
    Ok(storage_scan::start(
        std::sync::Arc::clone(kernel.paths()),
        std::sync::Arc::clone(kernel.settings()),
        std::sync::Arc::clone(kernel.db()),
        std::sync::Arc::clone(kernel.events()),
        force.unwrap_or(false),
    ))
}

/// 取消后台扫描，返回是否已停下。
#[tauri::command(async)]
pub fn storage_scan_cancel() -> CommandResult<bool> {
    Ok(storage_scan::cancel_and_wait(std::time::Duration::from_secs(2)))
}

/// 按分类清理；先停扫描再删，释放字节数由清理本身累加。
#[tauri::command(async)]
pub fn storage_clean(
    kernel: State<'_, KernelContext>,
    categories: Vec<String>,
) -> CommandResult<CleanReport> {
    storage_scan::clean(
        kernel.paths(),
        kernel.settings(),
        kernel.db(),
        kernel.events(),
        &categories,
    )
    .map_err(super::into_command_error)
}
