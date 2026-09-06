//! 内核信息命令：版本、路径快照、主题快照等（供前端启动初始化与"关于"页）。

use serde_json::{json, Value};
use tauri::State;

use crate::error::CommandResult;
use crate::state::KernelContext;

/// 内核信息聚合。
#[tauri::command]
pub fn kernel_info(kernel: State<'_, KernelContext>) -> CommandResult<Value> {
    let kc = kernel.inner();
    Ok(json!({
        "name": "copper-golem",
        "version": env!("CARGO_PKG_VERSION"),
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
