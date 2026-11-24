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

#[cfg(target_os = "android")]
use std::sync::{OnceLock, RwLock};
#[cfg(target_os = "android")]
use jni::{objects::{GlobalRef, JClass, JObject, JValue}, JNIEnv, JavaVM};


use crate::error::KernelError;
use crate::registry::events::EventBus;

pub const PREPARE_EVENT: &str = "android-game-prepare";

#[cfg(target_os = "android")]
static ANDROID_VM: OnceLock<JavaVM> = OnceLock::new();
#[cfg(target_os = "android")]
static ANDROID_HOST: OnceLock<RwLock<Option<GlobalRef>>> = OnceLock::new();
#[cfg(target_os = "android")]
static ANDROID_BRIDGE: OnceLock<GlobalRef> = OnceLock::new();

#[cfg(target_os = "android")]
pub(crate) fn register_android_vm(vm: &JavaVM) {
    // SAFETY: this pointer is owned by the process-wide JVM and remains valid for its lifetime.
    match unsafe { JavaVM::from_raw(vm.get_java_vm_pointer()) } {
        Ok(vm) => { let _ = ANDROID_VM.set(vm); }
        Err(error) => log::error!("[android-launch] unable to retain JavaVM: {error}"),
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_copperlamp_coppergolem_game_CopperGameHostBridge_nativeRegisterBridge(
    env: JNIEnv,
    _class: JClass,
    bridge: JClass,
) {
    match env.new_global_ref(&bridge) {
        Ok(reference) => {
            if ANDROID_BRIDGE.set(reference).is_err() {
                log::warn!("[android-launch] game host bridge already registered");
            }
        }
        Err(error) => log::error!("[android-launch] unable to retain game host bridge class: {error}"),
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_copperlamp_coppergolem_game_CopperGameHostBridge_nativeRegisterHost(
    env: JNIEnv,
    _class: JClass,
    host: JObject,
) {
    match env.new_global_ref(host) {
        Ok(reference) => {
            let slot = ANDROID_HOST.get_or_init(|| RwLock::new(None));
            if let Ok(mut current) = slot.write() { *current = Some(reference); }
        }
        Err(error) => log::error!("[android-launch] unable to retain host Activity: {error}"),
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_copperlamp_coppergolem_game_CopperGameHostBridge_nativeClearHost(
    env: JNIEnv,
    _class: jni::objects::JClass,
    host: JObject,
) {
    let Some(slot) = ANDROID_HOST.get() else { return; };
    let Ok(mut current) = slot.write() else { return; };
    let Some(reference) = current.as_ref() else { return; };
    match env.is_same_object(host, reference.as_obj()) {
        Ok(true) => *current = None,
        Ok(false) => {},
        Err(error) => log::warn!("[android-launch] host identity check failed: {error}"),
    }
}

#[cfg(target_os = "android")]
fn launch_host_prepare(instance: &str) -> Result<(), KernelError> {
    let host = ANDROID_HOST
        .get_or_init(|| RwLock::new(None))
        .read()
        .map_err(|_| KernelError::Config("Android 启动器状态锁已损坏".into()))?
        .clone()
        .ok_or_else(|| KernelError::Config("Android 启动器界面尚未就绪，无法启动游戏".into()))?;
    let vm = ANDROID_VM.get().ok_or_else(|| KernelError::Config("Android Java 虚拟机尚未就绪".into()))?;
    let mut env = vm.attach_current_thread().map_err(|error| KernelError::Config(format!("连接 Android 游戏启动器失败: {error}")))?;
    let value = env.new_string(instance).map_err(|error| KernelError::Config(format!("创建 Android 实例名失败: {error}")))?;
    let bridge = ANDROID_BRIDGE.get().ok_or_else(|| KernelError::Config("Android 游戏启动桥尚未注册".into()))?;
    env.call_static_method(
        bridge,
        "prepare",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&value)],
    ).map_err(|error| KernelError::Config(format!("请求 Android 启动界面失败: {error}")))?;
    Ok(())
}

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
    #[cfg(target_os = "android")]
    launch_host_prepare(instance)?;

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
/// not materialise a file, so the host streams the picked content into a unique
/// transaction directory under the managed versions root. That transaction
/// becomes the final instance directory after validation; the kernel never
/// reads a `content://` URI or accepts a path outside the managed root.
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
    pub icon_png: Option<String>,
    #[serde(default)]
    pub version_name: Option<String>,
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
    if request_id.len() != 36
        || !request_id.as_bytes().iter().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) { *byte == b'-' } else { byte.is_ascii_hexdigit() }
        })
    {
        return Err(KernelError::InvalidArgument("APK 导入请求 ID 无效".into()));
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
        let request_id = "123e4567-e89b-12d3-a456-426614174000";
        let other_request_id = "123e4567-e89b-12d3-a456-426614174001";
        assert!(take_apk_pick_result(&dir, request_id).unwrap().is_none());
        assert!(take_apk_pick_result(&dir, "").is_err());

        std::fs::write(
            dir.join(APK_PICK_RESULT_FILE),
            br#"{"requestId":"123e4567-e89b-12d3-a456-426614174000","path":"/data/versions/.import/req-1/source.apk","displayName":"a.apk"}"#,
        )
        .unwrap();
        let result = take_apk_pick_result(&dir, request_id).unwrap().expect("picked");
        assert!(result.is_ok());
        assert_eq!(result.display_name, "a.apk");

        // 另一个 requestId 不得领取同一份结果
        std::fs::write(
            dir.join(APK_PICK_RESULT_FILE),
            br#"{"requestId":"123e4567-e89b-12d3-a456-426614174000","path":"/data/versions/.import/req-1/source.apk"}"#,
        )
        .unwrap();
        assert!(take_apk_pick_result(&dir, other_request_id).unwrap().is_none());
    }

    #[test]
    fn apk_pick_result_reports_failure() {
        let dir =
            std::env::temp_dir().join(format!("copper_android_pick_err_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let request_id = "123e4567-e89b-12d3-a456-426614174002";
        std::fs::write(
            dir.join(APK_PICK_RESULT_FILE),
            br#"{"requestId":"123e4567-e89b-12d3-a456-426614174002","error":"permission denied"}"#,
        )
        .unwrap();
        let result = take_apk_pick_result(&dir, request_id).unwrap().expect("picked");
        assert!(!result.is_ok());
        assert_eq!(result.error, "permission denied");
    }
}
