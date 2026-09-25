//! 真实 helper 进程的端到端协议测试。
//!
//! 与 `helper_runtime` 的单测互补：那边验证分派与错误语义，这边验证**进程边界**
//! 上的行为——派生、握手、退出、超时与资源回收。使用 `CARGO_BIN_EXE_*` 指向
//! cargo 构建出的真实可执行文件，不做进程内替身。
//!
//! 每个测试使用独立命名的产物文件：测试是并行执行的，共享同一路径会互相删除。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use copper_module_abi::helper_client::{
    CapabilityDispatcher, CapabilityError, HelperError, HelperProcess, HelperPushHandle, PushError,
};
use copper_module_abi::ipc::{
    CapabilityRequest, ModuleMethod, METHOD_EVENT_DISPATCH, PROTOCOL_VERSION,
};
use copper_module_abi::plugin_abi::{ABI_STATUS_NOT_SUPPORTED, ABI_STATUS_OK};
use serde_json::{json, Value};

/// 宽松超时：helper 是本地进程，正常往返在毫秒级；放宽只为避免慢机器上的假失败。
const TIMEOUT: Duration = Duration::from_secs(10);

const MODULE_ID: &str = "copper-lamp.demo-tools";

fn helper_program() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_copper-module-helper"))
}

/// 一个真实存在、但不是有效动态库的文件：用于让 helper 通过启动校验，
/// 同时把失败推迟到 `module.initialize`。
fn existing_but_invalid_artifact(tag: &str) -> PathBuf {
    let path = fixture_path(tag);
    std::fs::write(&path, b"this file exists but is not a loadable plugin").unwrap();
    path
}

fn missing_artifact(tag: &str) -> PathBuf {
    fixture_path(tag)
}

fn fixture_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "cgl-helper-fixture-{}-{tag}.bin",
        std::process::id()
    ))
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
}

fn spawned(plugin: &Path) -> HelperProcess {
    HelperProcess::spawn(&helper_program(), MODULE_ID, plugin)
        .expect("helper process should spawn successfully")
}

/// 等待 stderr 收集线程读到期望内容，避免与进程退出产生竞态。
fn wait_for_stderr(helper: &HelperProcess, needle: &str) -> bool {
    for _ in 0..50 {
        if helper.stderr().contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// 构建并定位夹具插件产物。
///
/// 集成测试无法像 bin 那样用 `CARGO_BIN_EXE_*` 拿到 cdylib 路径，因此显式构建
/// 一次并在 target 目录中按名字查找。`OnceLock` 避免多个测试重复构建。
fn fixture_plugin() -> PathBuf {
    static PLUGIN: OnceLock<PathBuf> = OnceLock::new();
    PLUGIN
        .get_or_init(|| {
            let status = std::process::Command::new(env!("CARGO"))
                .args(["build", "-p", "abi-fixture-echo"])
                .status()
                .expect("cargo must be available to build the fixture plugin");
            assert!(status.success(), "failed to build the fixture plugin");

            let debug_dir = target_debug_dir();
            find_dylib(&debug_dir).unwrap_or_else(|| {
                panic!(
                    "fixture plugin artifact not found under {}",
                    debug_dir.display()
                )
            })
        })
        .clone()
}

fn target_debug_dir() -> PathBuf {
    let exe = std::env::current_exe().expect("test executable path");
    exe.parent()
        .and_then(Path::parent)
        .expect("test executable should live under target/<profile>/deps")
        .to_path_buf()
}

fn find_dylib(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut candidates: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                return false;
            };
            name.contains("abi_fixture_echo")
                && (name.ends_with(".dll") || name.ends_with(".so") || name.ends_with(".dylib"))
        })
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

#[test]
fn handshake_negotiates_the_protocol_version() {
    let artifact = existing_but_invalid_artifact("handshake");
    let mut helper = spawned(&artifact);

    let version = helper.handshake(TIMEOUT).unwrap();

    assert_eq!(version, PROTOCOL_VERSION);
    assert_eq!(helper.negotiated_version(), Some(PROTOCOL_VERSION));
    assert_eq!(helper.module_id(), MODULE_ID);

    let _ = helper.terminate();
    cleanup(&artifact);
}

#[test]
fn methods_before_handshake_are_refused_over_the_process_boundary() {
    let artifact = existing_but_invalid_artifact("pre-handshake");
    let mut helper = spawned(&artifact);

    let error = helper
        .request(ModuleMethod::Health, json!({}), TIMEOUT)
        .expect_err("health before handshake must be refused");

    match error {
        HelperError::Remote { code, .. } => assert_eq!(code, "handshake_required"),
        other => panic!("expected a remote refusal, got {other}"),
    }

    let _ = helper.terminate();
    cleanup(&artifact);
}

#[test]
fn health_reports_unhealthy_before_any_plugin_is_loaded() {
    let artifact = existing_but_invalid_artifact("health");
    let mut helper = spawned(&artifact);
    helper.handshake(TIMEOUT).unwrap();

    assert!(!helper.health(TIMEOUT).unwrap());

    let _ = helper.terminate();
    cleanup(&artifact);
}

#[test]
fn initialize_with_an_invalid_artifact_is_reported_as_a_remote_error() {
    let artifact = existing_but_invalid_artifact("invalid-artifact");
    let mut helper = spawned(&artifact);
    helper.handshake(TIMEOUT).unwrap();

    let error = helper
        .initialize(&json!({}), TIMEOUT)
        .expect_err("loading a non-library artifact must fail");

    match error {
        HelperError::Remote { code, .. } => assert_eq!(code, "plugin_load_failed"),
        other => panic!("expected plugin_load_failed, got {other}"),
    }

    let _ = helper.terminate();
    cleanup(&artifact);
}

#[test]
fn missing_plugin_artifact_makes_the_helper_exit_before_replying() {
    let mut helper = spawned(&missing_artifact("missing-artifact"));

    let error = helper
        .request(
            ModuleMethod::Handshake,
            json!({ "supported_versions": [PROTOCOL_VERSION] }),
            TIMEOUT,
        )
        .expect_err("a helper that refused to start must not reply");

    match error {
        HelperError::Exited { .. } => {}
        other => panic!("expected the helper to exit, got {other}"),
    }
    assert!(
        wait_for_stderr(&helper, "does not exist"),
        "stderr tail should explain the refusal, got: {}",
        helper.stderr()
    );
}

#[test]
fn shutdown_lets_the_helper_exit_on_its_own() {
    let artifact = existing_but_invalid_artifact("shutdown");
    let mut helper = spawned(&artifact);
    helper.handshake(TIMEOUT).unwrap();

    let exited_cleanly = helper.shutdown(TIMEOUT).unwrap();

    assert!(exited_cleanly, "helper should exit before the shutdown deadline");
    assert!(helper.has_exited().unwrap());

    cleanup(&artifact);
}

#[test]
fn terminate_reaps_the_process() {
    let artifact = existing_but_invalid_artifact("terminate");
    let mut helper = spawned(&artifact);
    helper.handshake(TIMEOUT).unwrap();
    assert!(!helper.has_exited().unwrap());

    helper.terminate().unwrap();

    assert!(helper.has_exited().unwrap());
    cleanup(&artifact);
}

#[test]
fn plugin_lifecycle_and_invoke_work_end_to_end() {
    let plugin = fixture_plugin();
    let mut helper = spawned(&plugin);
    helper.handshake(TIMEOUT).unwrap();

    helper
        .initialize(&json!({ "greeting": "hi" }), TIMEOUT)
        .expect("a real plugin artifact should initialize");
    assert!(helper.health(TIMEOUT).unwrap());

    helper.start(TIMEOUT).unwrap();
    let echoed = helper
        .invoke("demo.echo", &json!({ "value": 42 }), TIMEOUT)
        .unwrap();
    assert_eq!(echoed["echo"]["value"], json!(42));
    assert_eq!(echoed["started"], json!(true));
    assert!(
        echoed["config"]
            .as_str()
            .is_some_and(|config| config.contains("greeting")),
        "plugin should receive the host-supplied config, got {}",
        echoed["config"]
    );

    helper.stop(TIMEOUT).unwrap();
    let state = helper.invoke("demo.state", &json!({}), TIMEOUT).unwrap();
    assert_eq!(state["started"], json!(true));
    assert_eq!(state["stopped"], json!(true));

    assert!(helper.shutdown(TIMEOUT).unwrap());
}

#[test]
fn capability_requests_fail_closed_until_the_rpc_slice_lands() {
    let plugin = fixture_plugin();
    let mut helper = spawned(&plugin);
    helper.handshake(TIMEOUT).unwrap();
    helper.initialize(&json!({}), TIMEOUT).unwrap();

    let probe = helper
        .invoke("demo.probe_capability", &json!({}), TIMEOUT)
        .unwrap();

    assert_eq!(
        probe["capability_status"],
        json!(ABI_STATUS_NOT_SUPPORTED),
        "capability RPC is not implemented yet and must be refused, never faked"
    );
}

/// 记录被请求的身份，并回一个可断言的结果。
#[derive(Default)]
struct EchoDispatcher {
    seen_module_ids: Mutex<Vec<String>>,
}

impl CapabilityDispatcher for EchoDispatcher {
    fn dispatch(
        &self,
        module_id: &str,
        request: CapabilityRequest,
    ) -> Result<Value, CapabilityError> {
        self.seen_module_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(module_id.to_owned());
        Ok(json!({ "echoed": request.capability, "module": module_id }))
    }
}

#[test]
fn capability_requests_round_trip_through_the_host() {
    let dispatcher = Arc::new(EchoDispatcher::default());
    let plugin = fixture_plugin();
    let mut helper = HelperProcess::spawn_with_dispatcher(
        &helper_program(),
        MODULE_ID,
        &plugin,
        Arc::clone(&dispatcher) as Arc<dyn CapabilityDispatcher>,
    )
    .expect("helper should spawn");
    helper.handshake(TIMEOUT).unwrap();
    helper.initialize(&json!({}), TIMEOUT).unwrap();

    let probe = helper
        .invoke("demo.probe_capability", &json!({}), TIMEOUT)
        .unwrap();

    // 结果必须真的经输出缓冲回到插件，而不只是"调用没报错"。
    assert_eq!(probe["capability_status"], json!(ABI_STATUS_OK));
    assert_eq!(probe["capability_payload"]["echoed"], json!("world.read"));
    assert_eq!(probe["capability_payload"]["module"], json!(MODULE_ID));
    // 派发器看到的身份来自宿主会话绑定，不是插件自报。
    assert_eq!(
        dispatcher
            .seen_module_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        &[MODULE_ID.to_owned()]
    );
}

/// 启动一个已握手、已初始化并在运行的夹具插件会话。
fn started_fixture_helper() -> HelperProcess {
    let plugin = fixture_plugin();
    let mut helper = spawned(&plugin);
    helper.handshake(TIMEOUT).unwrap();
    helper.initialize(&json!({}), TIMEOUT).unwrap();
    helper.start(TIMEOUT).unwrap();
    helper
}

#[test]
fn pushed_events_reach_the_plugin_without_disturbing_request_pairing() {
    let mut helper = started_fixture_helper();
    let push = helper.push_handle();

    push.notify(
        METHOD_EVENT_DISPATCH,
        json!({ "event": "demo.activity", "payload": { "n": 1 } }),
    )
    .unwrap();
    push.notify(
        METHOD_EVENT_DISPATCH,
        json!({ "event": "version.installed", "payload": { "n": 2 } }),
    )
    .unwrap();

    // 事件通知**没有响应帧**。若 helper 回了帧，这里会先读到那一帧，于是 request id
    // 不匹配、本次调用失败——这正是本测试要守住的配对不变式。
    let state = helper.invoke("demo.state", &json!({}), TIMEOUT).unwrap();
    let received = state["received"]
        .as_array()
        .expect("demo.state 必须回报派发记账数组")
        .clone();
    assert_eq!(received.len(), 2, "两条事件都必须到达插件，实际：{received:?}");
    assert_eq!(received[0]["command"], json!("event.demo.activity"));
    assert_eq!(received[0]["payload"]["n"], json!(1));
    assert_eq!(received[1]["command"], json!("event.version.installed"));
    assert_eq!(received[1]["payload"]["n"], json!(2));

    assert!(helper.shutdown(TIMEOUT).unwrap());
}

#[test]
fn pushing_to_a_stopped_helper_reports_a_closed_channel() {
    let mut helper = started_fixture_helper();
    let push = helper.push_handle();

    assert!(helper.shutdown(TIMEOUT).unwrap());

    let error = push
        .notify(
            METHOD_EVENT_DISPATCH,
            json!({ "event": "demo.activity", "payload": null }),
        )
        .expect_err("向已停机 helper 推送必须报错，不能静默丢弃");
    assert!(matches!(error, PushError::Closed), "got {error}");
}

/// 在能力往返期间顺手推一条事件：验证 helper 把它**延后**而不是打断能力调用。
struct PushingDispatcher {
    push: Arc<OnceLock<HelperPushHandle>>,
    pushed: AtomicBool,
    failure: Mutex<Option<String>>,
}

impl PushingDispatcher {
    fn record_failure(&self, message: String) {
        *self.failure.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(message);
    }
}

impl CapabilityDispatcher for PushingDispatcher {
    fn dispatch(
        &self,
        module_id: &str,
        request: CapabilityRequest,
    ) -> Result<Value, CapabilityError> {
        // 只在第一次派发时推：模拟"插件正卡在一次能力往返里，宿主这边又有事件发生"。
        if !self.pushed.swap(true, Ordering::SeqCst) {
            match self.push.get() {
                Some(handle) => {
                    if let Err(error) = handle.notify(
                        METHOD_EVENT_DISPATCH,
                        json!({ "event": "demo.activity", "payload": { "during": "capability" } }),
                    ) {
                        self.record_failure(error.to_string());
                    }
                }
                None => self.record_failure(
                    "push handle was not installed before the first dispatch".to_owned(),
                ),
            }
        }
        Ok(json!({ "echoed": request.capability, "module": module_id }))
    }
}

#[test]
fn an_event_pushed_during_a_capability_round_trip_is_deferred_not_dropped() {
    let push_slot: Arc<OnceLock<HelperPushHandle>> = Arc::new(OnceLock::new());
    let dispatcher = Arc::new(PushingDispatcher {
        push: Arc::clone(&push_slot),
        pushed: AtomicBool::new(false),
        failure: Mutex::new(None),
    });

    let plugin = fixture_plugin();
    let mut helper = HelperProcess::spawn_with_dispatcher(
        &helper_program(),
        MODULE_ID,
        &plugin,
        Arc::clone(&dispatcher) as Arc<dyn CapabilityDispatcher>,
    )
    .expect("helper should spawn");
    push_slot
        .set(helper.push_handle())
        .ok()
        .expect("push slot must be empty before installation");

    helper.handshake(TIMEOUT).unwrap();
    helper.initialize(&json!({}), TIMEOUT).unwrap();
    helper.start(TIMEOUT).unwrap();

    // 这次 invoke 会在插件内发起能力请求，宿主在写回响应**之前**先把一条事件推进
    // 管道——于是 helper 在能力往返窗口内读到它。
    let probe = helper
        .invoke("demo.probe_capability", &json!({}), TIMEOUT)
        .unwrap();

    // 窗口内的事件若被打断处理，这次能力调用会以 ABI_STATUS_ERROR 收场。
    assert_eq!(
        probe["capability_status"],
        json!(ABI_STATUS_OK),
        "能力往返不得被事件推送打断"
    );
    // 先取出再加断言：同一个 std::sync::Mutex 不能在断言表达式与消息里重复加锁。
    let push_failure = dispatcher
        .failure
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert!(
        push_failure.is_none(),
        "宿主在能力往返期间推送失败：{push_failure:?}"
    );

    // 被延后的事件必须在当前请求处理完成后补派给插件，而不是丢弃。
    let state = helper.invoke("demo.state", &json!({}), TIMEOUT).unwrap();
    let received = state["received"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        received.len(),
        1,
        "窗口内的事件必须被延后补派，实际：{received:?}"
    );
    assert_eq!(received[0]["command"], json!("event.demo.activity"));
    assert_eq!(received[0]["payload"]["during"], json!("capability"));

    assert!(helper.shutdown(TIMEOUT).unwrap());
}
