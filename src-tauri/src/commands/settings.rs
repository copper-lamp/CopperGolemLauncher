//! 设置命令：KV 读 / 写 / 批量写 / 全量快照。
#[path="settings_contributions.rs"] mod contributions;
#[tauri::command]
pub fn settings_module_register(kernel: State<'_, KernelContext>, module_id:String, section:contributions::Section)->CommandResult<()> {
 check_module(kernel.inner(),&module_id,&section.id).map_err(into_command_error)?;
 let raw=std::fs::read(kernel.paths().modules_dir().join(&module_id).join("module.json")).map_err(|e|into_command_error(e.into()))?;
 let manifest=crate::registry::manifest::ModuleManifest::parse(&raw).map_err(into_command_error)?;
 contributions::register(&module_id,&manifest.i18n_namespace,section).map_err(into_command_error)
}
#[tauri::command]
pub fn settings_module_unregister(kernel:State<'_,KernelContext>,module_id:String,section_id:String)->CommandResult<()> {if !crate::registry::modules::is_valid_module_id(&module_id) {return Err(into_command_error(crate::error::KernelError::InvalidArgument("Invalid settings owner".into())));}let _=kernel;contributions::unregister(&module_id,&section_id);Ok(())}

fn check_module(kernel: &KernelContext, module_id: &str, key: &str) -> Result<(), crate::error::KernelError> {
    use crate::registry::sandbox::Permission;
    if !crate::registry::modules::is_valid_module_id(module_id) || key.is_empty() || key.len() > 160 || !key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) || !kernel.modules().is_enabled(kernel, module_id) {
        contributions::unregister(module_id,"*");return Err(crate::error::KernelError::InvalidArgument("Invalid or disabled module settings owner".into()));
    }
    kernel.sandbox().enforce(module_id, Permission::Settings, "settings.contribution", key)?;
    kernel.sandbox().enforce(module_id, Permission::Ui, "settings.contribution", key)
}
#[tauri::command]
pub fn settings_module_get(kernel: State<'_, KernelContext>, module_id: String, key: String) -> CommandResult<Option<Value>> {
    check_module(kernel.inner(), &module_id, &key).map_err(into_command_error)?;
    let field=contributions::field(&module_id,&key).map_err(into_command_error)?;
    Ok(Some(kernel.settings().get::<Value>(&format!("module-settings:{module_id}:{key}")).filter(|v|contributions::validate(&field,v).is_ok()).unwrap_or(field.default)))
}
#[tauri::command]
pub fn settings_module_set(kernel: State<'_, KernelContext>, module_id: String, key: String, value: Value) -> CommandResult<()> {
    check_module(kernel.inner(), &module_id, &key).map_err(into_command_error)?;
    let field=contributions::field(&module_id,&key).map_err(into_command_error)?;
    contributions::validate(&field,&value).map_err(into_command_error)?;
    kernel.settings().set(&format!("module-settings:{module_id}:{key}"), &value).map_err(into_command_error)
}


use std::collections::HashMap;

use serde_json::Value;
use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::state::KernelContext;

/// 全量设置快照（含默认值）。
#[tauri::command]
pub fn settings_all(kernel: State<'_, KernelContext>) -> CommandResult<HashMap<String, Value>> {
    Ok(kernel.settings().all().into_iter().filter(|(key,_)|!key.starts_with("module-settings:")).collect())
}

/// 读取单个设置（不存在返回 null）。
#[tauri::command]
pub fn settings_get(
    kernel: State<'_, KernelContext>,
    key: String,
) -> CommandResult<Option<Value>> {
    Ok(kernel.settings().get::<Value>(&key))
}

/// 写入单个设置并广播 `settings.changed`。
#[tauri::command]
pub fn settings_set(
    kernel: State<'_, KernelContext>,
    key: String,
    value: Value,
) -> CommandResult<()> {
    kernel.settings().set(&key, &value).map_err(into_command_error)
}

/// 批量写入并广播一次 `settings.changed`。
#[tauri::command]
pub fn settings_set_many(
    kernel: State<'_, KernelContext>,
    entries: HashMap<String, Value>,
) -> CommandResult<()> {
    kernel
        .settings()
        .set_many(&entries)
        .map_err(into_command_error)
}
