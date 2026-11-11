//! Storage configuration is staged for restart, before the live database opens.
use tauri::State;
use serde_json::Value;
use crate::{error::CommandResult,state::KernelContext,services::storage::{self,StorageRequest}};
#[tauri::command]
pub fn storage_status(kernel:State<'_,KernelContext>)->CommandResult<Value>{storage::status(kernel.paths(),kernel.settings()).map_err(super::into_command_error)}
#[tauri::command]
pub fn storage_preview(kernel:State<'_,KernelContext>,request:StorageRequest)->CommandResult<Value>{storage::preview(kernel.paths(),kernel.settings(),&request).map_err(super::into_command_error)}
#[tauri::command]
pub fn storage_apply(kernel:State<'_,KernelContext>,request:StorageRequest)->CommandResult<Value>{storage::apply(kernel.paths(),kernel.settings(),&request).map_err(super::into_command_error)}
#[tauri::command]
pub fn storage_cancel(kernel:State<'_,KernelContext>)->CommandResult<()>{storage::cancel(kernel.paths()).map_err(super::into_command_error)}
