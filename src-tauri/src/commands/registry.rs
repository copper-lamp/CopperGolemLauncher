//! 元数据命令：`cgl-libs` 索引状态、刷新与远端模块列表。
//!
//! 与既有 `commands/modules.rs` 的分工：本模块只负责**远端元数据**（拉取 / 校验 / 折叠），
//! 本地已注册模块仍由 `modules_list` 提供；两侧由前端按 `id` 关联（文档 2.9.1 / 2.9.4）。
//!
//! 错误一律经 [`into_command_error`] 收敛为统一的 `CommandError`。

use tauri::State;

use crate::commands::into_command_error;
use crate::error::{CommandResult, KernelError};
use crate::services::registry::RegistryStatusView;
use crate::services::registry::RemoteModuleView;
use crate::state::KernelContext;

/// 元数据状态：可用性、是否陈旧、来源、防降级锚点、schema 与签名状态。
///
/// 只读内存态，**不发起网络请求**——前端可在任意时机轮询而不会产生流量。
#[tauri::command]
pub fn registry_status(kernel: State<'_, KernelContext>) -> CommandResult<RegistryStatusView> {
    let registry = kernel
        .inner()
        .registry()
        .ok_or_else(|| into_command_error(KernelError::Config("元数据服务未初始化".into())))?;
    Ok(registry.status())
}

/// 刷新元数据索引。
///
/// `force = true` 绕过 TTL（用户手动刷新），但仍受防降级校验约束：
/// 检测到 `generated_at` 回退且 `repo_commit` 不同时会拒绝新数据并保留本地缓存。
#[tauri::command]
pub async fn registry_refresh(
    kernel: State<'_, KernelContext>,
    force: Option<bool>,
) -> CommandResult<RegistryStatusView> {
    let registry = kernel
        .inner()
        .registry()
        .ok_or_else(|| into_command_error(KernelError::Config("元数据服务未初始化".into())))?;
    registry
        .load_index(force.unwrap_or(false))
        .await
        .map_err(into_command_error)?;
    Ok(registry.status())
}

/// 远端模块条目列表（已按 `registry.channel` 过滤）。
///
/// 展示名与简介按 `locale` 折叠：locale → `en-US` → `id` / 空串（文档 2.9.4 回退链）。
#[tauri::command]
pub async fn registry_modules(
    kernel: State<'_, KernelContext>,
    locale: Option<String>,
) -> CommandResult<Vec<RemoteModuleView>> {
    let registry = kernel
        .inner()
        .registry()
        .ok_or_else(|| into_command_error(KernelError::Config("元数据服务未初始化".into())))?;
    let locale = locale
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| kernel.inner().settings().get_or("locale", "zh-CN".to_string()));
    registry
        .list_module_views(&locale)
        .await
        .map_err(into_command_error)
}
