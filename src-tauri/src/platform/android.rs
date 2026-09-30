//! Android host bridge.
//!
//! Ownership split, and the reason for it:
//!
//! - The **Java host** owns Activity lifecycle, native library loading and
//!   `AssetManager` mounting. `libminecraftpe.so` cannot be unloaded once
//!   loaded, so this state is process-wide and must not be duplicated in Rust.
//! - **Rust** owns validation, the instance registry and the event channel.
//!   It never touches native code; it only hands the Java host a validated
//!   instance name and version.
//!
//! Exit reporting goes the other way through a file mailbox instead of the
//! event bus: the Java host writes `<data>/game_exit.json` and the kernel
//! drains it via `take_exit_record`. See `game::CopperGameExitRecord` for why
//! a broadcast/event round-trip cannot be relied on here.

use serde_json::json;

use crate::error::KernelError;
use crate::registry::events::EventBus;

pub const PREPARE_EVENT: &str = "android-game-prepare";

/// Ask the Java host to start an instance.
///
/// `version_name` is passed through because the native library load order is
/// selected from it; an empty value is rejected rather than defaulted, so a
/// manifest that failed to parse surfaces as an error instead of a wrong
/// library set.
pub fn request_prepare(
    events: &EventBus,
    instance: &str,
    package_name: &str,
    version_name: &str,
) -> Result<(), KernelError> {
    if instance.is_empty() || package_name.is_empty() {
        return Err(KernelError::InvalidArgument("Android 游戏启动参数为空".into()));
    }
    if version_name.is_empty() {
        return Err(KernelError::InvalidArgument(
            "Android 实例缺少版本号，无法确定原生库加载顺序".into(),
        ));
    }
    events.publish(
        PREPARE_EVENT,
        json!({
            "instance_name": instance,
            "package_name": package_name,
            "version_name": version_name,
        }),
    );
    Ok(())
}

/// Exit record written by the Java host, relative to the data root.
pub const EXIT_RECORD_FILE: &str = "game_exit.json";

/// SAF pick result written by the Java host, relative to the data root.
pub const APK_PICK_RESULT_FILE: &str = "apk_pick_result.json";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitRecord {
    pub instance_name: String,
    pub reason: String,
    #[serde(default)]
    pub exited_at: i64,
}

/// Result of a system file-picker round trip performed by the Java host.
///
/// `tauri-plugin-dialog` on Android resolves to a `content://` URI and does
/// not materialise a file, so the host copies the picked stream into
/// `cache/inbox/` first. Only that private copy is ever handed to the kernel:
/// the kernel never reads a `content://` URI, and never sees a path outside
/// app-private storage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApkPickResult {
    pub request_id: String,
    /// Absolute path inside app-private storage; empty on failure.
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub error: String,
}

impl ApkPickResult {
    pub fn is_ok(&self) -> bool {
        self.error.is_empty() && !self.path.is_empty()
    }
}

/// Drain the exit mailbox: read the record and remove it in one step.
///
/// Take semantics matter — the Java host may write again for the next session,
/// and a read-without-delete would replay an old exit into the UI.
pub fn take_exit_record(data_root: &std::path::Path) -> Result<Option<ExitRecord>, KernelError> {
    match take_json::<ExitRecord>(data_root, EXIT_RECORD_FILE)? {
        Some(record) if !record.instance_name.is_empty() => Ok(Some(record)),
        _ => Ok(None),
    }
}

/// Drain the APK pick mailbox, discarding results that belong to an earlier
/// request so a stale file cannot be attributed to the current pick.
pub fn take_apk_pick_result(
    data_root: &std::path::Path,
    request_id: &str,
) -> Result<Option<ApkPickResult>, KernelError> {
    if request_id.is_empty() {
        return Err(KernelError::InvalidArgument("缺少请求 ID".into()));
    }
    Ok(take_json::<ApkPickResult>(data_root, APK_PICK_RESULT_FILE)?
        .filter(|result| result.request_id == request_id))
}

fn take_json<T: serde::de::DeserializeOwned>(
    data_root: &std::path::Path,
    file_name: &str,
) -> Result<Option<T>, KernelError> {
    let path = data_root.join(file_name);
    if !path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read(&path)?;
    // Remove first: a half-written or unparsable record must not be replayed.
    let _ = std::fs::remove_file(&path);
    Ok(serde_json::from_slice::<T>(&raw).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_prepare_request() {
        let events = EventBus::new();
        assert!(request_prepare(&events, "", "com.mojang.minecraftpe", "1.21.130.20").is_err());
        assert!(request_prepare(&events, "v", "", "1.21.130.20").is_err());
    }

    #[test]
    fn rejects_missing_version_name() {
        let events = EventBus::new();
        assert!(request_prepare(&events, "v", "com.mojang.minecraftpe", "").is_err());
    }

    #[test]
    fn take_exit_record_consumes_once() {
        let dir = std::env::temp_dir().join(format!("copper_android_exit_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(take_exit_record(&dir).unwrap().is_none());

        std::fs::write(
            dir.join(EXIT_RECORD_FILE),
            br#"{"instanceName":"demo","reason":"normal","exitedAt":42}"#,
        )
        .unwrap();
        let record = take_exit_record(&dir).unwrap().expect("record present");
        assert_eq!(record.instance_name, "demo");
        assert_eq!(record.reason, "normal");
        assert_eq!(record.exited_at, 42);
        // 第二次取不到，保证不重放
        assert!(take_exit_record(&dir).unwrap().is_none());
    }

    #[test]
    fn take_exit_record_drops_malformed() {
        let dir = std::env::temp_dir().join(format!("copper_android_exit_bad_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(EXIT_RECORD_FILE), b"{not json").unwrap();
        assert!(take_exit_record(&dir).unwrap().is_none());
        assert!(!dir.join(EXIT_RECORD_FILE).exists());
    }

    #[test]
    fn take_apk_pick_result_matches_request_id() {
        let dir = std::env::temp_dir().join(format!("copper_android_pick_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(take_apk_pick_result(&dir, "req-1").unwrap().is_none());
        assert!(take_apk_pick_result(&dir, "").is_err());

        std::fs::write(
            dir.join(APK_PICK_RESULT_FILE),
            br#"{"requestId":"req-1","path":"/data/cache/inbox/a.apk","displayName":"a.apk"}"#,
        )
        .unwrap();
        let result = take_apk_pick_result(&dir, "req-1").unwrap().expect("picked");
        assert!(result.is_ok());
        assert_eq!(result.display_name, "a.apk");

        // 另一个 requestId 不得领取同一份结果
        std::fs::write(
            dir.join(APK_PICK_RESULT_FILE),
            br#"{"requestId":"req-1","path":"/data/cache/inbox/a.apk"}"#,
        )
        .unwrap();
        assert!(take_apk_pick_result(&dir, "req-2").unwrap().is_none());
    }

    #[test]
    fn apk_pick_result_reports_failure() {
        let dir =
            std::env::temp_dir().join(format!("copper_android_pick_err_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(APK_PICK_RESULT_FILE),
            br#"{"requestId":"r","error":"permission denied"}"#,
        )
        .unwrap();
        let result = take_apk_pick_result(&dir, "r").unwrap().expect("picked");
        assert!(!result.is_ok());
        assert_eq!(result.error, "permission denied");
    }
}
