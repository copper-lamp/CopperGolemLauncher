//! 账户命令：当前账户、发起登录、退出登录、刷新令牌、取启动凭证。

use serde_json::Value;
use tauri::State;

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::services::account::{AccountInfo, DeviceCodeInfo};
use crate::state::KernelContext;

/// 当前登录账户。
#[tauri::command]
pub fn account_current(kernel: State<'_, KernelContext>) -> CommandResult<Option<AccountInfo>> {
    Ok(kernel.account().current())
}

/// 发起设备码登录：返回授权信息（前端展示链接与用户码），后台自动轮询。
#[tauri::command]
pub async fn account_begin_login(
    kernel: State<'_, KernelContext>,
) -> CommandResult<DeviceCodeInfo> {
    kernel.account().begin_login().await.map_err(into_command_error)
}

/// 退出登录（清除密钥环与数据库记录）。
#[tauri::command]
pub fn account_logout(kernel: State<'_, KernelContext>) -> CommandResult<()> {
    kernel.account().logout().map_err(into_command_error)
}

/// 刷新当前账户令牌。
#[tauri::command]
pub async fn account_refresh(kernel: State<'_, KernelContext>) -> CommandResult<()> {
    kernel.account().refresh().await.map_err(into_command_error)
}

/// 取当前账户的 MSA + XSTS 凭证（自动刷新，供启动游戏使用）。
#[tauri::command]
pub async fn account_credentials(kernel: State<'_, KernelContext>) -> CommandResult<Value> {
    kernel.account().credentials().await.map_err(into_command_error)
}
