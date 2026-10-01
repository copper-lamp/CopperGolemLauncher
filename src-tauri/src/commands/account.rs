//! 账户命令：当前账户、发起登录、退出登录、刷新令牌、取启动凭证。

use serde_json::Value;
use tauri::{AppHandle, Manager, State};

use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::services::account::{AccountInfo, DeviceCodeInfo};
use crate::state::KernelContext;

/// 承载系统账户界面的主窗口标签。
const MAIN_WINDOW: &str = "main";

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

/// Microsoft 账户授权（WAM）：把系统账户界面归属到主窗口，返回选定账户身份。
///
/// 窗口句柄是硬性前置——WAM 的桌面端授权界面必须有所属窗口，因此这里不接受前端
/// 传入句柄，一律由内核取主窗口，避免前端传进来一个不属于本进程的句柄。
#[tauri::command]
pub async fn account_wam_sign_in(
    app: AppHandle,
    kernel: State<'_, KernelContext>,
) -> CommandResult<AccountInfo> {
    let hwnd = main_window_hwnd(&app)?;
    kernel
        .account()
        .begin_wam_sign_in(hwnd)
        .await
        .map_err(into_command_error)
}

/// 取主窗口句柄；窗口尚未就绪时给出可操作的原因，而不是把 0 交给 WAM。
fn main_window_hwnd(app: &AppHandle) -> Result<isize, crate::error::KernelError> {
    let window = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or_else(|| crate::error::KernelError::Account("主窗口尚未就绪，无法呈现账户授权界面".into()))?;
    let hwnd = window.hwnd().map_err(|e| {
        crate::error::KernelError::Account(format!("无法获取主窗口句柄: {e}"))
    })?;
    Ok(hwnd as isize)
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
