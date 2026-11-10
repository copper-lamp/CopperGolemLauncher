//! 外部组件受控命令；来源与路径不接受前端输入。
use tauri::State;
use crate::{error::CommandResult, state::KernelContext, services::components::{ComponentId,ComponentStatus}};
use super::into_command_error;
#[tauri::command]
pub fn components_list(kernel:State<'_,KernelContext>)->Vec<ComponentStatus>{kernel.components().snapshot()}
#[tauri::command]
pub async fn components_probe(kernel:State<'_,KernelContext>,id:ComponentId)->CommandResult<bool>{kernel.components().probe(id).await.map(|_|true).map_err(into_command_error)}
#[tauri::command]
pub async fn components_check(kernel:State<'_,KernelContext>,id:ComponentId)->CommandResult<ComponentStatus>{kernel.components().check(id).await.map_err(into_command_error)}
#[tauri::command]
pub async fn components_install(kernel:State<'_,KernelContext>,id:ComponentId)->CommandResult<()>{kernel.components().install(id).await.map_err(into_command_error)}
