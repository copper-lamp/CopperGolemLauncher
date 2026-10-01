// SPDX-License-Identifier: GPL-3.0-only
//
// WAM Store-ticket boundary adapted from LeviLauncher
// (internal/xbox/wam_windows.go, wam_provider_windows.go and
// wam_picker_windows.go), GPL-3.0-only. The implementation uses
// WebAuthenticationCoreManager and validates the selected account before
// returning a ticket. It never manufactures credentials.

//! Windows Web Account Manager (WAM) 适配层。
//!
//! 提供两条路径，共用同一套身份校验：
//!
//! - **交互路径** [`sign_in_interactive`]：绑定本进程窗口句柄，系统账户界面会把
//!   登录/授权界面归属到该窗口。用于用户头像入口，是取得 XUID 的唯一途径。
//! - **静默路径** [`acquire_store_ticket_for_xuid`]：复用 WAM 已缓存的票据，不弹窗。
//!   供安装链在后台取 Store content key。
//!
//! 票据材质对调用方保持不透明（[`crate::services::store_entitlement::StoreTicket`]）。
//! XUID 恒为显式绑定：空身份或与响应不一致的身份绝不被静默替换成系统默认账户。

use std::fmt;

use crate::services::store_entitlement::{StoreEntitlementError, StoreTicket};

/// WAM 获取边界上的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WamStoreError {
    /// 宿主必须呈现账户选择/授权界面（静默路径拿不到票据）。
    InteractionRequired,
    /// WAM 返回了与显式请求的 XUID 不同的账户。
    AccountChanged,
    /// 当前目标平台不是 Windows。
    WindowsOnly,
    /// 调用方未提供必需的显式 XUID 绑定。
    InvalidIdentity,
    /// 没有可用于承载系统账户界面的本进程窗口。
    NoAccountWindow,
    /// 用户主动关闭了系统账户界面。
    UserCancelled,
    /// Windows 返回了原生 WinRT/COM 失败。
    Native(String),
}

impl fmt::Display for WamStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InteractionRequired => f.write_str("WAM interaction is required"),
            Self::AccountChanged => f.write_str("WAM account does not match expected XUID"),
            Self::WindowsOnly => f.write_str("WAM Store tickets are only available on Windows"),
            Self::InvalidIdentity => f.write_str("WAM requires an explicit XUID binding"),
            Self::NoAccountWindow => {
                f.write_str("WAM 需要一个属于本进程的窗口来呈现账户界面")
            }
            Self::UserCancelled => f.write_str("用户取消了账户授权"),
            Self::Native(error) => write!(f, "WAM native call failed: {error}"),
        }
    }
}

impl std::error::Error for WamStoreError {}

impl From<WamStoreError> for StoreEntitlementError {
    fn from(error: WamStoreError) -> Self {
        match error {
            WamStoreError::InteractionRequired => StoreEntitlementError::InteractionRequired,
            WamStoreError::AccountChanged => StoreEntitlementError::AccountChanged,
            WamStoreError::WindowsOnly => StoreEntitlementError::WindowsOnly,
            WamStoreError::InvalidIdentity => StoreEntitlementError::InvalidIdentity,
            // 交互路径的失败在静默链路里等价于「拿不到票据」，不新增服务层语义。
            WamStoreError::NoAccountWindow
            | WamStoreError::UserCancelled
            | WamStoreError::Native(error) => StoreEntitlementError::Http(error.to_string()),
        }
    }
}

/// 交互式授权后拿到的本机账户身份。
///
/// WAM 只签发 XUID 形态的身份；`gamertag` 取自系统的 `WebAccount::UserName`，
/// 该字段在部分账户上为空或为登录名，此时调用方应回落到展示 XUID。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WamIdentity {
    pub xuid: String,
    pub gamertag: String,
}

/// Store 票据的 scope 与客户端 ID（与 LeviLauncher 一致）。
#[cfg(windows)]
const STORE_SCOPE: &str = "service::www.microsoft.com::MBI_SSL";
#[cfg(windows)]
const STORE_CLIENT_ID: &str = "00000000402b5328";
/// Microsoft 消费者账户 provider 的 authority。
#[cfg(windows)]
const MSA_PROVIDER: &str = "https://login.microsoft.com";

/// 取得 Store 票据（无界面），票据与 `expected_xuid` 显式绑定。
///
/// 阻塞调用：内部会等待 WAM 异步操作完成。调用方必须放在阻塞线程上执行。
#[cfg(windows)]
pub fn acquire_store_ticket_for_xuid(expected_xuid: &str) -> Result<StoreTicket, WamStoreError> {
    if expected_xuid.trim().is_empty() {
        return Err(WamStoreError::InvalidIdentity);
    }
    native::initialize()?;
    let result = native::silent_ticket(expected_xuid);
    native::uninitialize();
    result
}

/// 非 Windows 平台无法访问 Web Account Manager。
#[cfg(not(windows))]
pub fn acquire_store_ticket_for_xuid(
    expected_xuid: &str,
) -> Result<StoreTicket, WamStoreError> {
    if expected_xuid.trim().is_empty() {
        return Err(WamStoreError::InvalidIdentity);
    }
    Err(WamStoreError::WindowsOnly)
}

/// 面向使用服务层错误类型的调用方的适配。
pub fn acquire_store_ticket(expected_xuid: &str) -> Result<StoreTicket, StoreEntitlementError> {
    acquire_store_ticket_for_xuid(expected_xuid).map_err(Into::into)
}

/// 交互式账户授权：把系统账户界面归属到 `hwnd`，返回用户选定账户的 XUID。
///
/// 阻塞调用，可能停留数分钟等待用户操作，必须放在阻塞线程上执行。
///
/// # Panics
/// 不 panic：任何原生失败都以 [`WamStoreError`] 返回。
#[cfg(windows)]
pub fn sign_in_interactive(hwnd: isize) -> Result<WamIdentity, WamStoreError> {
    native::validate_owner_window(hwnd)?;
    native::initialize()?;
    let result = native::interactive_identity(hwnd);
    native::uninitialize();
    result
}

/// 非 Windows 平台显式不支持交互式 WAM 授权。
#[cfg(not(windows))]
pub fn sign_in_interactive(_hwnd: isize) -> Result<WamIdentity, WamStoreError> {
    Err(WamStoreError::WindowsOnly)
}

/// Windows 原生互操作。
///
/// `IWebAuthenticationCoreManagerInterop` 未被 windows-rs 生成，只能手工
/// QueryInterface + 虚表调用。虚表布局按 IUnknown(3) + IInspectable(3) 前缀推导，
/// 与 LeviLauncher `wam_windows.go` 中的 ABI 槽位一致。
#[cfg(windows)]
mod native {
    use std::ffi::c_void;
    use std::time::{Duration, Instant};

    use windows::core::{HSTRING, IUnknown, GUID};
    use windows::Foundation::{AsyncStatus, IAsyncInfo, IAsyncOperation};
    use windows::Security::Authentication::Web::Core::{
        WebAuthenticationCoreManager, WebTokenRequest, WebTokenRequestPromptType,
        WebTokenRequestResult, WebTokenRequestStatus,
    };
    use windows::Win32::Foundation::{HWND, FALSE};
    use windows::Win32::System::WinRt::{
        RoGetActivationFactory, RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow};

    use super::{WamStoreError, WamIdentity, MSA_PROVIDER, STORE_CLIENT_ID, STORE_SCOPE};
    use crate::services::store_entitlement::StoreTicket;

    /// `IWebAuthenticationCoreManagerInterop` 的 IID。
    const IID_MANAGER_INTEROP: GUID = GUID::from_values(
        0xf4b8e804,
        0x811e,
        0x4436,
        [0xb6, 0x9c, 0x44, 0xcb, 0x67, 0xb7, 0x20, 0x84],
    );
    /// `IAsyncOperation<WebTokenRequestResult>` 的 IID。
    const IID_TOKEN_RESULT_OPERATION: GUID = GUID::from_values(
        0x0a815852,
        0x7c44,
        0x5674,
        [0xb3, 0xd2, 0xfa, 0x2e, 0x4c, 0x1e, 0x46, 0xc9],
    );
    const MANAGER_CLASS: &str =
        "Windows.Security.Authentication.Web.Core.WebAuthenticationCoreManager";

    /// 用户操作可以持续很久，超时按交互场景放宽。
    const INTERACTIVE_TIMEOUT: Duration = Duration::from_secs(600);
    /// 静默路径不需要用户操作，短超时足够。
    const SILENT_TIMEOUT: Duration = Duration::from_secs(30);
    const POLL_INTERVAL: Duration = Duration::from_millis(40);

    #[repr(C)]
    struct IUnknownVtbl {
        query_interface: unsafe extern "system" fn(
            *mut c_void,
            *const GUID,
            *mut *mut c_void,
        ) -> windows::core::HRESULT,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
    }

    /// `IWebAuthenticationCoreManagerInterop`：`RequestTokenForWindowAsync` 位于槽 6。
    #[repr(C)]
    struct ManagerInteropVtbl {
        base: [usize; 6],
        request_token_for_window_async: unsafe extern "system" fn(
            *mut c_void,
            isize,
            *mut c_void,
            *const GUID,
            *mut *mut c_void,
        ) -> windows::core::HRESULT,
    }

    pub(super) fn initialize() -> Result<(), WamStoreError> {
        // SAFETY: 必须在同一线程上配对 uninitialize，由调用方保证。
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(native_error)?;
        Ok(())
    }

    pub(super) fn uninitialize() {
        // SAFETY: 与 initialize 配对。
        unsafe { RoUninitialize() };
    }

    fn native_error(error: windows::core::Error) -> WamStoreError {
        WamStoreError::Native(error.to_string())
    }

    /// 句柄必须指向本进程仍然存在的窗口，否则系统账户界面无处归属。
    pub(super) fn validate_owner_window(hwnd: isize) -> Result<(), WamStoreError> {
        if hwnd == 0 {
            return Err(WamStoreError::NoAccountWindow);
        }
        let handle = HWND(hwnd as *mut c_void);
        // SAFETY: 仅查询窗口存在性与所属进程，不触碰窗口内存。
        unsafe {
            if !IsWindow(Some(handle)).as_bool() {
                return Err(WamStoreError::NoAccountWindow);
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(Some(handle), Some(&mut pid));
            if pid != std::process::id() {
                return Err(WamStoreError::NoAccountWindow);
            }
        }
        let _ = FALSE;
        Ok(())
    }

    /// 静默取票：已缓存票据直接复用，缺账户即失败，绝不弹窗。
    pub(super) fn silent_ticket(expected_xuid: &str) -> Result<StoreTicket, WamStoreError> {
        let provider = find_msa_provider()?;
        let account = WebAuthenticationCoreManager::FindAccountAsync(
            &provider,
            &HSTRING::from(expected_xuid),
        )
        .map_err(native_error)?
        .get()
        .map_err(map_wait_error)?;

        let request = WebTokenRequest::Create(
            &provider,
            &HSTRING::from(STORE_SCOPE),
            &HSTRING::from(STORE_CLIENT_ID),
        )
        .map_err(native_error)?;

        let operation = WebAuthenticationCoreManager::GetTokenSilentlyWithWebAccountAsync(
            &request,
            &account,
        )
        .map_err(native_error)?;
        let result = wait_operation(&operation, SILENT_TIMEOUT)?;
        let (token, actual) = read_success(&result, false)?;
        if actual != expected_xuid {
            return Err(WamStoreError::AccountChanged);
        }
        Ok(StoreTicket::from_wam(token, actual))
    }

    /// 交互取身份：先以默认提示请求，若 WAM 未呈现界面则强制提示重试一次。
    pub(super) fn interactive_identity(hwnd: isize) -> Result<WamIdentity, WamStoreError> {
        let provider = find_msa_provider()?;
        match request_with_prompt(&provider, hwnd, WebTokenRequestPromptType::Default) {
            Ok(identity) => Ok(identity),
            Err(WamStoreError::InteractionRequired) => request_with_prompt(
                &provider,
                hwnd,
                WebTokenRequestPromptType::ForceAuthentication,
            ),
            Err(other) => Err(other),
        }
    }

    fn request_with_prompt(
        provider: &windows::Security::Credentials::WebAccountProvider,
        hwnd: isize,
        prompt: WebTokenRequestPromptType,
    ) -> Result<WamIdentity, WamStoreError> {
        let request = WebTokenRequest::CreateWithPromptType(
            provider,
            &HSTRING::from(STORE_SCOPE),
            &HSTRING::from(STORE_CLIENT_ID),
            prompt,
        )
        .map_err(native_error)?;

        let operation = request_token_for_window(hwnd, &request)?;
        let result = wait_operation(&operation, INTERACTIVE_TIMEOUT)?;
        let (token, actual) = read_success(&result, true)?;
        if token.is_empty() || actual.is_empty() {
            return Err(WamStoreError::Native(
                "WAM returned an empty token or account".into(),
            ));
        }
        let gamertag = result
            .ResponseData()
            .map_err(native_error)
            .and_then(|data| data.GetAt(0))
            .map_err(native_error)?
            .WebAccount()
            .map_err(native_error)?
            .UserName()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        Ok(WamIdentity {
            xuid: actual,
            gamertag,
        })
    }

    fn find_msa_provider() -> Result<windows::Security::Credentials::WebAccountProvider, WamStoreError>
    {
        WebAuthenticationCoreManager::FindAccountProviderWithAuthorityAsync(
            &HSTRING::from(MSA_PROVIDER),
            &HSTRING::from("consumers"),
        )
        .map_err(native_error)?
        .get()
        .map_err(map_wait_error)
    }

    /// 窗口作用域的取票请求：`RequestTokenForWindowAsync`。
    ///
    /// 这是唯一会让系统账户界面归属到本进程窗口的入口；无窗口作用域的
    /// `RequestTokenAsync` 在桌面上不保证呈现界面。
    fn request_token_for_window(
        hwnd: isize,
        request: &WebTokenRequest,
    ) -> Result<IAsyncOperation<WebTokenRequestResult>, WamStoreError> {
        // SAFETY: 全部指针来自下方激活的工厂与 QI 结果，且在本函数返回前由
        // `IAsyncOperation` 持有引用计数；虚表布局见 `ManagerInteropVtbl`。
        unsafe {
            let factory: IUnknown = RoGetActivationFactory(&HSTRING::from(MANAGER_CLASS))
                .map_err(native_error)?;
            let interop = query_interface(factory.as_raw(), &IID_MANAGER_INTEROP)?;
            let vtable = *(interop as *const *const ManagerInteropVtbl);
            let mut raw: *mut c_void = std::ptr::null_mut();
            (vtable.request_token_for_window_async)(
                interop,
                hwnd,
                request.as_raw() as *mut c_void,
                &IID_TOKEN_RESULT_OPERATION,
                &mut raw,
            )
            .ok()
            .map_err(native_error)?;
            if raw.is_null() {
                return Err(WamStoreError::Native(
                    "WAM window token request returned no operation".into(),
                ));
            }
            Ok(IAsyncOperation::<WebTokenRequestResult>::from_raw(raw))
        }
    }

    unsafe fn query_interface(
        object: *mut c_void,
        iid: &GUID,
    ) -> Result<*mut c_void, WamStoreError> {
        let vtable = *(object as *const *const IUnknownVtbl);
        let mut out: *mut c_void = std::ptr::null_mut();
        (vtable.query_interface)(object, iid, &mut out)
            .ok()
            .map_err(native_error)?;
        if out.is_null() {
            return Err(WamStoreError::Native(
                "WAM interop interface is unavailable".into(),
            ));
        }
        Ok(out)
    }

    /// 轮询直到完成，超时则请求取消，避免等待线程被界面永久占用。
    fn wait_operation(
        operation: &IAsyncOperation<WebTokenRequestResult>,
        timeout: Duration,
    ) -> Result<WebTokenRequestResult, WamStoreError> {
        let deadline = Instant::now() + timeout;
        loop {
            let status = operation.Status().map_err(native_error)?;
            match status {
                AsyncStatus::Completed => {
                    return operation.GetResults().map_err(map_wait_error)
                }
                AsyncStatus::Canceled => return Err(WamStoreError::UserCancelled),
                AsyncStatus::Error => {
                    return operation.GetResults().map_err(map_wait_error);
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                if let Ok(info) = operation.cast::<IAsyncInfo>() {
                    let _ = info.Cancel();
                }
                return Err(WamStoreError::Native(format!(
                    "WAM token request timed out after {}s",
                    timeout.as_secs()
                )));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn map_wait_error(error: windows::core::Error) -> WamStoreError {
        WamStoreError::Native(error.to_string())
    }

    /// 校验响应状态并取出票据与账户标识。
    ///
    /// `interactive` 为真时，用户主动关闭界面判为取消而非「需要交互」，
    /// 否则重试逻辑会把它当成信号再弹一次。
    fn read_success(
        result: &WebTokenRequestResult,
        interactive: bool,
    ) -> Result<(String, String), WamStoreError> {
        let status = result.ResponseStatus().map_err(native_error)?;
        match status {
            WebTokenRequestStatus::Success => {}
            WebTokenRequestStatus::UserCancel if interactive => {
                return Err(WamStoreError::UserCancelled)
            }
            WebTokenRequestStatus::UserInteractionRequired
            | WebTokenRequestStatus::UserCancel
            | WebTokenRequestStatus::AccountProviderNotAvailable => {
                return Err(WamStoreError::InteractionRequired)
            }
            WebTokenRequestStatus::AccountSwitch => return Err(WamStoreError::AccountChanged),
            _ => return Err(WamStoreError::Native("WAM token request failed".into())),
        }

        let responses = result.ResponseData().map_err(native_error)?;
        if responses.Size().map_err(native_error)? != 1 {
            return Err(WamStoreError::Native(
                "WAM returned an unexpected response count".into(),
            ));
        }
        let response = responses.GetAt(0).map_err(native_error)?;
        let token = response.Token().map_err(native_error)?.to_string_lossy().to_string();
        let account = response.WebAccount().map_err(native_error)?;
        let account_id = account.Id().map_err(native_error)?.to_string_lossy().to_string();
        Ok((token, account_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn non_windows_is_explicitly_rejected() {
        assert!(matches!(
            acquire_store_ticket_for_xuid("xuid"),
            Err(WamStoreError::WindowsOnly)
        ));
    }

    #[test]
    fn empty_identity_is_rejected_before_platform_dispatch() {
        assert!(matches!(
            acquire_store_ticket_for_xuid("  "),
            Err(WamStoreError::InvalidIdentity)
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn interactive_sign_in_is_explicitly_rejected_off_windows() {
        assert!(matches!(
            sign_in_interactive(0),
            Err(WamStoreError::WindowsOnly)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn interactive_sign_in_rejects_a_zero_window_handle() {
        assert!(matches!(
            sign_in_interactive(0),
            Err(WamStoreError::NoAccountWindow)
        ));
    }
}