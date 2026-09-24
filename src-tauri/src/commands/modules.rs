//! 模块命令：模块清单、启用 / 禁用、权限授权与越权留痕。

use std::collections::BTreeMap;
use std::str::FromStr;

use tauri::State;

use crate::commands::into_command_error;
use crate::error::{CommandResult, KernelError};
use crate::registry::modules::ModuleInfo;
use crate::registry::sandbox::{Grant, ModuleSandbox, Permission, ViolationRecord};
use crate::state::KernelContext;

/// 模块信息列表（供设置页"模块"Tab 展示）。
#[tauri::command]
pub fn modules_list(kernel: State<'_, KernelContext>) -> CommandResult<Vec<ModuleInfo>> {
    Ok(kernel.inner().modules().list(kernel.inner()))
}

/// 切换模块启用状态（下次启动生效）。
#[tauri::command]
pub fn modules_set_enabled(
    kernel: State<'_, KernelContext>,
    id: String,
    enabled: bool,
) -> CommandResult<()> {
    kernel
        .inner()
        .modules()
        .set_enabled(kernel.inner(), &id, enabled)
        .map_err(into_command_error)
}

/// 全部已知权限项（供前端渲染权限勾选列表，避免前端硬编码枚举）。
#[tauri::command]
pub fn sandbox_permissions() -> CommandResult<Vec<String>> {
    Ok(Permission::ALL.iter().map(|p| p.as_str().to_string()).collect())
}

/// 附加模块授权概览（模块 -> 声明 / 实际生效 / 是否停用）。
#[tauri::command]
pub fn sandbox_grants(kernel: State<'_, KernelContext>) -> CommandResult<BTreeMap<String, Grant>> {
    Ok(kernel.inner().sandbox().grants())
}

/// 越权留痕（按发生顺序；前端据此提示风险）。
#[tauri::command]
pub fn sandbox_violations(kernel: State<'_, KernelContext>) -> CommandResult<Vec<ViolationRecord>> {
    Ok(kernel.inner().sandbox().violations())
}

/// 为附加模块登记授权（安装时调用）。
///
/// `declared` 为清单声明的权限；未知权限名直接报错，避免静默降级成"少授权"。
#[tauri::command]
pub fn sandbox_grant(
    kernel: State<'_, KernelContext>,
    id: String,
    declared: Vec<String>,
) -> CommandResult<()> {
    let mut set = std::collections::HashSet::new();
    for name in &declared {
        set.insert(Permission::from_str(name).map_err(into_command_error)?);
    }
    let dir = ModuleSandbox::module_dir(kernel.inner().paths(), &id).map_err(into_command_error)?;
    kernel.inner().sandbox().grant(&id, set, dir);
    log::info!("module `{id}` granted {} permissions", declared.len());
    Ok(())
}

/// 由用户收紧某模块的实际权限（不能超出清单声明）。
#[tauri::command]
pub fn sandbox_revoke_permission(
    kernel: State<'_, KernelContext>,
    id: String,
    permission: String,
) -> CommandResult<()> {
    let perm = Permission::from_str(&permission).map_err(into_command_error)?;
    kernel
        .inner()
        .sandbox()
        .revoke_permission(&id, perm)
        .map_err(into_command_error)
}

/// 注销模块（卸载时调用）：清除授权与留痕。
#[tauri::command]
pub fn sandbox_revoke(kernel: State<'_, KernelContext>, id: String) -> CommandResult<()> {
    if id.trim().is_empty() {
        return Err(into_command_error(KernelError::InvalidArgument(
            "模块 id 不能为空".into(),
        )));
    }
    kernel.inner().sandbox().revoke(&id);
    Ok(())
}
