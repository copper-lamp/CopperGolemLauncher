//! 内核信息命令：版本、路径快照、主题快照等（供前端启动初始化与"关于"页）。

use serde_json::{json, Value};
use tauri::State;

use crate::error::CommandResult;
use crate::state::KernelContext;

/// 内核信息聚合。
#[tauri::command]
pub fn kernel_info(kernel: State<'_, KernelContext>) -> CommandResult<Value> {
    let kc = kernel.inner();
    let backends = kc.backends();
    Ok(json!({
        "name": "copper-golem",
        "version": env!("CARGO_PKG_VERSION"),
        // 平台标识与形态：驱动前端 Shell 的桌面/移动两态布局（见 docs/平台适配.md 2.5）。
        "platform": backends.platform_id(),
        "formFactor": backends.form_factor().as_str(),
        "paths": kc.paths().snapshot(),
        "theme": kc.theme().snapshot(),
        "locales": kc.i18n().supported_locales(),
    }))
}

/// 路径体系快照（调试 / 展示用）。
#[tauri::command]
pub fn paths_snapshot(kernel: State<'_, KernelContext>) -> CommandResult<Value> {
    Ok(kernel.inner().paths().snapshot())
}

/// 前端调试日志转发（临时诊断用：把 render / console 错误打到内核日志）。
#[tauri::command]
pub fn debug_log(level: String, message: String) -> CommandResult<()> {
    match level.as_str() {
        "error" => log::error!("[frontend] {message}"),
        "warn" => log::warn!("[frontend] {message}"),
        _ => log::info!("[frontend] {message}"),
    }
    Ok(())
}
