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
            other => StoreEntitlementError::Http(other.to_string()),
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

/// 交互式取 Store 票据，归属到本进程窗口 `hwnd`。
///
/// 静默取票被系统拒绝时的回落路径：同一次请求改用窗口作用域的
/// `RequestTokenForWindowAsync`，由系统账户界面当场签发票据。票据与
/// `expected_xuid` 显式绑定，用户在界面上切换账户会被拒绝而不是被静默接受。
///
/// 阻塞调用，会等待用户在系统界面上完成，可能长达数分钟，必须放在阻塞线程上执行。
/// `hwnd` 必须属于本进程（由 [`crate::services::window::MainWindow`] 登记），
/// 因此系统账户界面会归属到本应用的窗口。
#[cfg(windows)]
pub fn acquire_store_ticket_for_window(
    hwnd: isize,
    expected_xuid: &str,
) -> Result<StoreTicket, WamStoreError> {
    if expected_xuid.trim().is_empty() {
        return Err(WamStoreError::InvalidIdentity);
    }
    native::validate_owner_window(hwnd)?;
    native::initialize()?;
    let result = native::interactive_ticket(hwnd, expected_xuid);
    native::uninitialize();
    result
}

/// 非 Windows 平台没有窗口作用域的 WAM，回落路径显式不支持。
#[cfg(not(windows))]
pub fn acquire_store_ticket_for_window(
    _hwnd: isize,
    expected_xuid: &str,
) -> Result<StoreTicket, WamStoreError> {
    if expected_xuid.trim().is_empty() {
        return Err(WamStoreError::InvalidIdentity);
    }
    Err(WamStoreError::WindowsOnly)
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

    use windows::core::{HSTRING, IUnknown, GUID, Interface};
    use windows::Foundation::{AsyncStatus, IAsyncInfo, IAsyncOperation};
    use windows::Security::Authentication::Web::Core::{
        WebAuthenticationCoreManager, WebTokenRequest, WebTokenRequestPromptType,
        WebTokenRequestResult, WebTokenRequestStatus,
    };
    use windows::Win32::System::WinRT::{
        RoGetActivationFactory, RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED,
    };

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
        step("RoInitialize", unsafe { RoInitialize(RO_INIT_MULTITHREADED) })?;
        Ok(())
    }

    pub(super) fn uninitialize() {
        // SAFETY: 与 initialize 配对。
        unsafe { RoUninitialize() };
    }

    /// 给原生失败打上步骤名。
    ///
    /// WAM 边界上的每个 WinRT 调用都可能以同一个 HRESULT 失败；不带步骤名时
    /// 上层日志无法区分是「找 provider」「找账户」「建请求」还是「静默取票」。
    fn step<T>(name: &str, result: windows::core::Result<T>) -> Result<T, WamStoreError> {
        result.map_err(|error| WamStoreError::Native(format!("{name}: {error}")))
    }

    /// 句柄必须可用，否则系统账户界面无处归属。
    ///
    /// 句柄只由内核从自身主窗口取得（见 `commands::account::main_window_hwnd`），
    /// 归属本进程由构造保证，前端无法注入，因此这里只校验非空。
    pub(super) fn validate_owner_window(hwnd: isize) -> Result<(), WamStoreError> {
        if hwnd == 0 {
            return Err(WamStoreError::NoAccountWindow);
        }
        Ok(())
    }

    /// 静默取票：已缓存票据直接复用，缺账户即失败，绝不弹窗。
    ///
    /// 这里刻意使用**不带账户**的 `GetTokenSilentlyAsync(request)`，而不是
    /// `GetTokenSilentlyWithWebAccountAsync(request, account)`。后者在受限进程
    /// （低完整性令牌或受限令牌）下被系统直接拒绝：`E_ACCESSDENIED (0x80070005)`，
    /// 且发生在此前的 `RoInitialize`、provider 查找、`FindAccountAsync`、
    /// `WebTokenRequest::Create` **全部成功之后**——即账户确实存在、参数确实正确，
    /// 只有「显式 WebAccount」这一条路被拒。同进程内不带账户的静默请求则成功，
    /// 且响应的 `WebAccount.Id` 与 expected XUID 逐字一致（本机实测确认）。
    ///
    /// 身份绑定不因此放松：绑定校验改为在拿到响应后做（见下方 `actual != expected_xuid`），
    /// 账户不匹配仍然报 `AccountChanged`，绝不把别的账户的票据交出去。
    pub(super) fn silent_ticket(expected_xuid: &str) -> Result<StoreTicket, WamStoreError> {
        let provider = find_msa_provider()?;

        let request = step(
            "WebTokenRequest::Create",
            WebTokenRequest::Create(
                &provider,
                &HSTRING::from(STORE_SCOPE),
                &HSTRING::from(STORE_CLIENT_ID),
            ),
        )?;

        let operation = silent_token_request(&request)?;
        let result = wait_operation(&operation, SILENT_TIMEOUT, "GetTokenSilently")?;
        let (token, actual) = read_success(&result, false)?;
        if actual != expected_xuid {
            return Err(WamStoreError::AccountChanged);
        }
        Ok(StoreTicket::from_wam(token, actual))
    }

    /// 静默取票请求：不带账户，只复用 WAM 当前缓存的默认账户。
    ///
    /// 曾经的失败假设：`GetTokenSilentlyWithWebAccountAsync(request, account)`
    /// 在本机被系统以 `E_ACCESSDENIED` 拒绝，而同样参数在参考实现上可以通过。
    /// 已排除的原因：提权、scope/clientID/provider、账户口径（响应的
    /// `WebAccount.Id` 与数据库中的 XUID 逐字一致）。真正的原因是**显式传入
    /// WebAccount 的那条重载在受限进程下会被拒绝**：同一个进程里换成不带账户的
    /// 重载即可拿到票据，并且响应账户仍与 expected XUID 一致。
    fn silent_token_request(
        request: &WebTokenRequest,
    ) -> Result<IAsyncOperation<WebTokenRequestResult>, WamStoreError> {
        step(
            "GetTokenSilentlyAsync",
            WebAuthenticationCoreManager::GetTokenSilentlyAsync(request),
        )
    }

    /// 交互取身份：先以默认提示请求，若 WAM 未呈现界面则强制提示重试一次。
    pub(super) fn interactive_identity(hwnd: isize) -> Result<WamIdentity, WamStoreError> {
        let provider = find_msa_provider()?;
        let (_, actual, result) = request_token_interactively(&provider, hwnd)?;
        Ok(WamIdentity {
            xuid: actual,
            gamertag: display_name(&result),
        })
    }

    /// 交互取票：与身份路径同一套请求，只是把票据交给安装链。
    ///
    /// 静默取票被系统拒绝时的回落路径。票据与 `expected_xuid` 显式绑定，用户在
    /// 系统界面上换账户不会被静默接受。
    pub(super) fn interactive_ticket(
        hwnd: isize,
        expected_xuid: &str,
    ) -> Result<StoreTicket, WamStoreError> {
        let provider = find_msa_provider()?;
        let (token, actual, _) = request_token_interactively(&provider, hwnd)?;
        if actual != expected_xuid {
            return Err(WamStoreError::AccountChanged);
        }
        Ok(StoreTicket::from_wam(token, actual))
    }

    /// 先以默认提示请求；WAM 未呈现界面时用强制提示重试一次。
    ///
    /// 返回票据、实际账户 ID 与原始响应（供身份路径取展示名）。
    fn request_token_interactively(
        provider: &windows::Security::Credentials::WebAccountProvider,
        hwnd: isize,
    ) -> Result<(String, String, WebTokenRequestResult), WamStoreError> {
        match request_with_prompt(provider, hwnd, WebTokenRequestPromptType::Default) {
            Ok(granted) => Ok(granted),
            Err(WamStoreError::InteractionRequired) => request_with_prompt(
                provider,
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
    ) -> Result<(String, String, WebTokenRequestResult), WamStoreError> {
        let request = step(
            "WebTokenRequest::CreateWithPromptType",
            WebTokenRequest::CreateWithPromptType(
                provider,
                &HSTRING::from(STORE_SCOPE),
                &HSTRING::from(STORE_CLIENT_ID),
                prompt,
            ),
        )?;

        let operation = request_token_for_window(hwnd, &request)?;
        let result = wait_operation(&operation, INTERACTIVE_TIMEOUT, "RequestTokenForWindow")?;
        let (token, actual) = read_success(&result, true)?;
        if token.is_empty() || actual.is_empty() {
            return Err(WamStoreError::Native(
                "WAM returned an empty token or account".into(),
            ));
        }
        Ok((token, actual, result))
    }

    /// 系统侧账户显示名。
    ///
    /// WAM 只签发 XUID；`UserName` 在部分账户上为空或为登录名，取不到不是错误，
    /// 由调用方决定回落策略（当前回落为展示 XUID）。
    fn display_name(result: &WebTokenRequestResult) -> String {
        result
            .ResponseData()
            .and_then(|data| data.GetAt(0))
            .and_then(|response| response.WebAccount())
            .and_then(|account| account.UserName())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    fn find_msa_provider() -> Result<windows::Security::Credentials::WebAccountProvider, WamStoreError>
    {
        let found = step(
            "FindAccountProviderWithAuthorityAsync",
            WebAuthenticationCoreManager::FindAccountProviderWithAuthorityAsync(
                &HSTRING::from(MSA_PROVIDER),
                &HSTRING::from("consumers"),
            ),
        )?;
        step("FindAccountProviderWithAuthorityAsync.get", found.get())
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
            let factory: IUnknown =
                step("RoGetActivationFactory", RoGetActivationFactory(&HSTRING::from(MANAGER_CLASS)))?;
            let interop = query_interface(factory.as_raw(), &IID_MANAGER_INTEROP)?;
            let vtable: *const *const ManagerInteropVtbl =
                interop as *const *const ManagerInteropVtbl;
            let table: *const ManagerInteropVtbl = *vtable;
            let mut raw: *mut c_void = std::ptr::null_mut();
            let request_for_window = (*table).request_token_for_window_async;
            step(
                "RequestTokenForWindowAsync",
                request_for_window(
                    interop,
                    hwnd,
                    request.as_raw() as *mut c_void,
                    &IID_TOKEN_RESULT_OPERATION,
                    &mut raw,
                )
                .ok(),
            )?;
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
        let vtable: *const *const IUnknownVtbl = object as *const *const IUnknownVtbl;
        let table: *const IUnknownVtbl = *vtable;
        let query = (*table).query_interface;
        let mut out: *mut c_void = std::ptr::null_mut();
        step(
            "QueryInterface(IWebAuthenticationCoreManagerInterop)",
            query(object, iid, &mut out).ok(),
        )?;
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
        label: &str,
    ) -> Result<WebTokenRequestResult, WamStoreError> {
        let deadline = Instant::now() + timeout;
        loop {
            let status = step("AsyncOperation.Status", operation.Status())?;
            match status {
                AsyncStatus::Completed => {
                    return step(&format!("{label}.GetResults"), operation.GetResults());
                }
                AsyncStatus::Canceled => return Err(WamStoreError::UserCancelled),
                AsyncStatus::Error => {
                    return step(&format!("{label}.GetResults"), operation.GetResults());
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

    /// 校验响应状态并取出票据与账户标识。
    ///
    /// `interactive` 为真时，用户主动关闭界面判为取消而非「需要交互」，
    /// 否则重试逻辑会把它当成信号再弹一次。
    fn read_success(
        result: &WebTokenRequestResult,
        interactive: bool,
    ) -> Result<(String, String), WamStoreError> {
        let status = step("ResponseStatus", result.ResponseStatus())?;
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

        let responses = step("ResponseData", result.ResponseData())?;
        if step("ResponseData.Size", responses.Size())? != 1 {
            return Err(WamStoreError::Native(
                "WAM returned an unexpected response count".into(),
            ));
        }
        let response = step("ResponseData.GetAt", responses.GetAt(0))?;
        let token = step("WebTokenResponse.Token", response.Token())?.to_string_lossy().to_string();
        let account = step("WebTokenResponse.WebAccount", response.WebAccount())?;
        let account_id = step("WebAccount.Id", account.Id())?.to_string_lossy().to_string();
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
