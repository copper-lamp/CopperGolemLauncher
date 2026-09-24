// SPDX-License-Identifier: GPL-3.0-only
//
// WAM Store-ticket boundary adapted from LeviLauncher
// (internal/xbox/wam_windows.go and auth_windows.go), GPL-3.0-only.
// The implementation uses WebAuthenticationCoreManager and validates the
// selected account before returning a ticket. It never manufactures credentials.

//! Windows Web Account Manager (WAM) Store-ticket adapter.
//!
//! Ticket material remains opaque inside [`store_entitlement::StoreTicket`].
//! Callers must provide the XUID displayed by the account UI; an empty or
//! mismatching identity is never silently replaced with the Windows default.

use std::fmt;

use crate::services::store_entitlement::{StoreEntitlementError, StoreTicket};
#[cfg(windows)]
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

/// Errors specific to the WAM acquisition boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WamStoreError {
    /// The native WAM account picker/consent flow must be shown by the host.
    InteractionRequired,
    /// WAM returned a different account than the explicitly requested XUID.
    AccountChanged,
    /// The current target is not Windows.
    WindowsOnly,
    /// The caller did not provide the required explicit XUID binding.
    InvalidIdentity,
    /// Windows returned a native WinRT/COM failure.
    Native(String),
}

impl fmt::Display for WamStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InteractionRequired => f.write_str("WAM interaction is required"),
            Self::AccountChanged => f.write_str("WAM account does not match expected XUID"),
            Self::WindowsOnly => f.write_str("WAM Store tickets are only available on Windows"),
            Self::InvalidIdentity => f.write_str("WAM requires an explicit XUID binding"),
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
            WamStoreError::Native(error) => StoreEntitlementError::Http(error),
        }
    }
}

/// Acquire a Store ticket bound to `expected_xuid` using the real Windows WAM API.
#[cfg(windows)]
pub fn acquire_store_ticket_for_xuid(expected_xuid: &str) -> Result<StoreTicket, WamStoreError> {
    if expected_xuid.trim().is_empty() { return Err(WamStoreError::InvalidIdentity); }
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(native_error)?;
    let result = acquire_native_inner(expected_xuid);
    unsafe { RoUninitialize(); }
    result
}

#[cfg(windows)]
fn acquire_native_inner(expected_xuid: &str) -> Result<StoreTicket, WamStoreError> {
    use windows::Security::Authentication::Web::Core::{WebAuthenticationCoreManager, WebTokenRequest, WebTokenRequestStatus as Status};
    let provider = WebAuthenticationCoreManager::FindAccountProviderWithAuthorityAsync(&windows::core::HSTRING::from("https://login.microsoft.com"), &windows::core::HSTRING::from("consumers")).map_err(native_error)?.get().map_err(native_error)?;
    let account = WebAuthenticationCoreManager::FindAccountAsync(&provider, &windows::core::HSTRING::from(expected_xuid)).map_err(native_error)?.get().map_err(native_error)?;
    let request = WebTokenRequest::Create(&provider, &windows::core::HSTRING::from("service::www.microsoft.com::MBI_SSL"), &windows::core::HSTRING::from("00000000402b5328")).map_err(native_error)?;
    let result = WebAuthenticationCoreManager::GetTokenSilentlyWithWebAccountAsync(&request, &account).map_err(native_error)?.get().map_err(native_error)?;
    let status = result.ResponseStatus().map_err(native_error)?;
    if status == Status::UserInteractionRequired || status == Status::UserCancel || status == Status::AccountProviderNotAvailable { return Err(WamStoreError::InteractionRequired); }
    if status == Status::AccountSwitch { return Err(WamStoreError::AccountChanged); }
    if status != Status::Success { return Err(WamStoreError::Native("WAM token request failed".into())); }
    let responses = result.ResponseData().map_err(native_error)?;
    if responses.Size().map_err(native_error)? != 1 { return Err(WamStoreError::Native("WAM returned an unexpected response count".into())); }
    let response = responses.GetAt(0).map_err(native_error)?;
    let token = response.Token().map_err(native_error)?.to_string_lossy();
    let actual = response.WebAccount().map_err(native_error)?.Id().map_err(native_error)?.to_string_lossy();
    if token.is_empty() || actual.is_empty() { return Err(WamStoreError::Native("WAM returned an empty token or account".into())); }
    if actual != expected_xuid { return Err(WamStoreError::AccountChanged); }
    Ok(StoreTicket::from_wam(token, actual))
}

#[cfg(windows)]
fn native_error(error: windows::core::Error) -> WamStoreError { WamStoreError::Native(error.to_string()) }

/// Non-Windows builds cannot access Web Account Manager.
#[cfg(not(windows))]
pub fn acquire_store_ticket_for_xuid(
    expected_xuid: &str,
) -> Result<StoreTicket, WamStoreError> {
    if expected_xuid.trim().is_empty() { return Err(WamStoreError::InvalidIdentity); }
    Err(WamStoreError::WindowsOnly)
}

/// Store-entitlement-shaped adapter for callers that use the service error type.
/// The XUID remains explicit and is checked against the WAM response.
pub fn acquire_store_ticket(
    expected_xuid: &str,
) -> Result<StoreTicket, StoreEntitlementError> {
    acquire_store_ticket_for_xuid(expected_xuid).map_err(Into::into)
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
}
