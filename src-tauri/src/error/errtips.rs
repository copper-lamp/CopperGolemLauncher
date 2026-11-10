//! Safe structured evidence at the command boundary. Never serialize source bodies or URLs.
use serde::Serialize;
use super::KernelError;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ERROR: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize)]
pub struct ErrorContext {
    pub schema_version: u8,
    pub error_id: String,
    pub platform: &'static str,
    pub code: &'static str,
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_scope: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_code: Option<i32>,
    /// Only a validated DNS host, without credentials, path, query or fragment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub retry_safe: bool,
}

impl ErrorContext {
    pub fn from_kernel(error: &KernelError) -> Self {
        let mut context = Self {
            schema_version: 1,
            error_id: format!("{}-{}", std::process::id(), NEXT_ERROR.fetch_add(1, Ordering::Relaxed)),
            platform: std::env::consts::OS,
            code: "unknown", source: "kernel", timeout_scope: None,
            http_status: None, os_code: None, host: None, retry_safe: false,
        };
        context.inspect(error);
        context
    }

    fn inspect(&mut self, error: &KernelError) {
        match error {
            KernelError::Http(e) => {
                self.source = "reqwest";
                self.http_status = e.status().map(|s| s.as_u16());
                // Record only known public service hosts. Arbitrary hosts may contain identifiers.
                self.host = e.url().and_then(|u| u.host_str()).filter(|host| matches!(*host,
                    "github.com" | "api.github.com" | "raw.githubusercontent.com" |
                    "objects.githubusercontent.com" | "licensing.mp.microsoft.com" |
                    "displaycatalog.mp.microsoft.com")).map(str::to_owned);
                self.code = if e.is_timeout() { self.timeout_scope = Some("network"); "network_timeout" }
                    else if e.status().is_some() { "http_status" }
                    else if e.is_connect() { "network_connect" }
                    else { "network_request" };
            }
            KernelError::Io(e) => {
                self.source = "io";
                self.os_code = e.raw_os_error();
                self.code = match e.kind() {
                    std::io::ErrorKind::PermissionDenied => "permission_denied",
                    std::io::ErrorKind::NotFound => "not_found",
                    std::io::ErrorKind::AlreadyExists => "already_exists",
                    std::io::ErrorKind::TimedOut => { self.timeout_scope = Some("io"); "io_timeout" },
                    _ if matches!(e.raw_os_error(), Some(28 | 112)) => "disk_full",
                    _ => "io_failure",
                };
            }
            KernelError::Startup { source, .. } => self.inspect(source),
            KernelError::InvalidArgument(_) => self.code = "invalid_argument",
            KernelError::Conflict(_) => self.code = "conflict",
            KernelError::Account(_) => self.code = "account_unclassified",
            KernelError::StoreEntitlement(e) => {
                use crate::services::store_entitlement::StoreEntitlementError as E;
                self.source = "store_entitlement";
                self.code = match e {
                    E::InteractionRequired => "interaction_required",
                    E::AccountChanged | E::InvalidIdentity => "account_changed",
                    E::NoContentKey | E::KeyIdNotUnique => "no_content_key",
                    E::ServiceRefused(_) => "service_refused",
                    E::HttpStatus(status) => { self.http_status = Some(status.as_u16()); "http_status" },
                    E::MalformedLicenseResponse => "malformed_license_response",
                    _ => "account_unclassified",
                };
            },
            KernelError::Secret(_) => self.code = "credential_storage",
            KernelError::Download(_) => self.code = "download_unclassified",
            KernelError::Module(_) => self.code = "module_unclassified",
            KernelError::Database(_) => self.code = "database",
            KernelError::Serde(_) => self.code = "invalid_response",
            _ => (),
        }
    }
}

/// Fail closed for free-form diagnostics containing credentials, paths or URLs.
/// This is intentionally conservative: structured evidence remains available.
pub fn redact_message(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if ["token", "authorization", "bearer", "cookie", "password", "secret", "ticket", "contentkey", "xuid", "http://", "https://", "file://", "\\", ":/", "/users/", "/home/", "@"]
        .iter().any(|marker| lower.contains(marker)) || lower.contains(char::from(92)) {
        return "Diagnostic text withheld to protect private information".into();
    }
    message.chars().take(1024).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_io_evidence_survives_startup_without_path() {
        let error = KernelError::startup("secret path", KernelError::Io(std::io::Error::from_raw_os_error(5)));
        let context = ErrorContext::from_kernel(&error);
        assert_eq!(context.source, "io");
        assert_eq!(context.os_code, Some(5));
        assert!(!context.retry_safe);
        assert!(!serde_json::to_string(&context).unwrap().contains("secret path"));
    }
    #[test]
    fn opaque_account_errors_are_never_purchase_verdicts() {
        let context = ErrorContext::from_kernel(&KernelError::Account("NotEntitled device limit".into()));
        assert_eq!(context.code, "account_unclassified");
        assert_eq!(context.timeout_scope, None);
    }
    #[test]
    fn typed_entitlement_refusal_and_http_status_survive() {
        use crate::services::store_entitlement::StoreEntitlementError as E;
        let refused = ErrorContext::from_kernel(&KernelError::StoreEntitlement(E::ServiceRefused("private server reason".into())));
        assert_eq!(refused.code, "service_refused");
        assert!(!serde_json::to_string(&refused).unwrap().contains("private server reason"));
        let http = ErrorContext::from_kernel(&KernelError::StoreEntitlement(E::HttpStatus(reqwest::StatusCode::FORBIDDEN)));
        assert_eq!(http.http_status, Some(403));
        assert_eq!(http.source, "store_entitlement");
    }
    #[test]
    fn secret_url_path_and_multiline_text_is_withheld() {
        for text in ["Authorization: Bearer abc", "https://host/path?key=abc", "D:\\Users\\private", "a@b.com", "first\ncookie=abc"] {
            assert_eq!(redact_message(text), "Diagnostic text withheld to protect private information");
        }
        assert_eq!(redact_message("普通参数错误"), "普通参数错误");
    }
}
