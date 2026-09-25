//! helper 侧运行时：加载插件、驱动生命周期、把宿主 IPC 请求翻译成插件 ABI 调用。
//!
//! 刻意与进程 IO 分离（IO 循环在 `bin/copper_module_helper.rs`），这样协议分派、
//! 版本拒绝与错误语义可以被单测直接覆盖，不必先启动子进程。
//!
//! # 身份与权限
//!
//! 会话绑定的模块身份由宿主通过 `--module-id` 下发，helper 只把它交给插件上下文，
//! 从不接受插件自报身份。能力 RPC 的授权切片尚未落地，因此 [`deny_capability`]
//! 一律显式拒绝（fail closed），不使用成功桩。

use std::ffi::c_void;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::ipc::{
    negotiate_handshake, read_frame, validate_version, write_frame, HealthResponse,
    InitializeResponse, InvokeResponse, IpcError, IpcMessage, IpcRequest, IpcResponse,
    LifecycleAction, LifecycleResponse, ModuleLifecycleState, ModuleMethod, ModuleRequest,
    METHOD_CAPABILITY_REQUEST, PROTOCOL_VERSION,
};
use crate::module_id::is_valid_module_id;
use crate::plugin_abi::{
    AbiBuffer, AbiBytes, PluginEntry, PluginInstance, ABI_STATUS_BUFFER_TOO_SMALL,
    ABI_STATUS_ERROR, ABI_STATUS_NOT_SUPPORTED, ABI_STATUS_OK, PLUGIN_ENTRY_SYMBOL,
};

/// helper 与宿主之间的帧通道。
///
/// 两个方向共用同一对标准流：主循环读宿主请求；插件发起能力请求时，能力回调在插件
/// 调用栈内写请求并读响应。用同一把锁串行化，且**主循环处理请求期间不持锁**，
/// 否则回调无法取得通道而死锁。
pub struct HelperChannel {
    reader: BufReader<std::io::StdinLock<'static>>,
    writer: BufWriter<std::io::StdoutLock<'static>>,
    /// 能力请求序号，保证 request id 在会话内唯一。
    capability_seq: u64,
}

impl HelperChannel {
    /// 绑定本进程的标准输入输出。
    pub fn stdio() -> Self {
        Self {
            reader: BufReader::new(std::io::stdin().lock()),
            writer: BufWriter::new(std::io::stdout().lock()),
            capability_seq: 0,
        }
    }

    /// 读取一帧消息。
    pub fn recv(&mut self) -> Result<IpcMessage, IpcError> {
        let payload = read_frame(&mut self.reader)?;
        IpcMessage::decode(&payload)
    }

    /// 写出一帧消息并立即刷新：对端正在等这一帧，缓冲会让双方互相等待。
    pub fn send(&mut self, message: &IpcMessage) -> Result<(), IpcError> {
        let payload = message.encode()?;
        write_frame(&mut self.writer, &payload)?;
        self.writer.flush().map_err(IpcError::Io)
    }

    fn next_capability_request_id(&mut self) -> String {
        self.capability_seq += 1;
        format!("cap-{}", self.capability_seq)
    }
}

/// 会转成 IPC 错误 DTO 的失败。
pub struct Failure {
    pub code: &'static str,
    pub message: String,
}

impl Failure {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// helper 侧宿主上下文：能力回调的 `user_data` 指向它。
struct HostContext {
    module_id: String,
    /// 与宿主的帧通道。`None` 表示本运行时没有接宿主（进程内单测），
    /// 此时能力请求一律拒绝而不是假装成功。
    channel: Option<Arc<Mutex<HelperChannel>>>,
}

/// 插件能力请求的宿主入口。
///
/// 把请求转发给宿主并按响应回填输出：宿主是唯一能决定"是否授权"的一方，helper
/// 只负责搬运，绝不自作判断或返回伪造结果。
unsafe extern "C" fn plugin_capability(
    user_data: *mut c_void,
    capability: AbiBytes,
    input: AbiBytes,
    output: *mut AbiBuffer,
) -> i32 {
    let Some(context) = (unsafe { (user_data as *const HostContext).as_ref() }) else {
        return ABI_STATUS_ERROR;
    };
    let Some(channel) = context.channel.as_ref() else {
        eprintln!(
            "[helper] capability request refused for module `{}`: no host channel is attached",
            context.module_id
        );
        return ABI_STATUS_NOT_SUPPORTED;
    };

    let Some(capability_name) = (unsafe { abi_bytes_to_string(capability) }) else {
        return ABI_STATUS_ERROR;
    };
    let params = match unsafe { abi_bytes_to_vec(input) } {
        Some(bytes) if bytes.is_empty() => Value::Null,
        Some(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => Value::Null,
        },
        None => return ABI_STATUS_ERROR,
    };

    let mut guard = match channel.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    let request_id = guard.next_capability_request_id();
    let request = IpcRequest {
        version: PROTOCOL_VERSION,
        request_id: request_id.clone(),
        method: METHOD_CAPABILITY_REQUEST.to_owned(),
        params: json!({ "capability": capability_name, "params": params }),
    };
    if let Err(error) = guard.send(&IpcMessage::Request(request)) {
        eprintln!("[helper] failed to forward a capability request: {error}");
        return ABI_STATUS_ERROR;
    }

    let response = loop {
        match guard.recv() {
            Ok(IpcMessage::Response(response)) => break response,
            // 宿主在能力往返期间不应主动发请求；收到就拒绝，避免把乱序当成响应。
            Ok(IpcMessage::Request(request)) => {
                eprintln!("[helper] host sent a request while a capability call was pending: {}", request.method);
                return ABI_STATUS_ERROR;
            }
            Err(error) => {
                eprintln!("[helper] failed to read a capability response: {error}");
                return ABI_STATUS_ERROR;
            }
        }
    };

    if response.request_id != request_id {
        eprintln!(
            "[helper] capability response id `{}` does not match request id `{request_id}`",
            response.request_id
        );
        return ABI_STATUS_ERROR;
    }
    if let Some(error) = response.error {
        // 宿主未实现该能力与"授权失败"要区分开，插件才能正确处理。
        return if error.code == "capability_not_supported" {
            ABI_STATUS_NOT_SUPPORTED
        } else {
            ABI_STATUS_ERROR
        };
    }
    let Some(result) = response.result else {
        return ABI_STATUS_ERROR;
    };
    let encoded = match serde_json::to_vec(&result) {
        Ok(encoded) => encoded,
        Err(_) => return ABI_STATUS_ERROR,
    };

    if output.is_null() {
        return ABI_STATUS_ERROR;
    }
    let output = unsafe { &mut *output };
    let required = encoded.len() as u64;
    if output.ptr.is_null() || output.capacity < required {
        output.len = required;
        return ABI_STATUS_BUFFER_TOO_SMALL;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(encoded.as_ptr(), output.ptr, encoded.len());
    }
    output.len = required;
    ABI_STATUS_OK
}

/// 读取跨 ABI 的字节块。拷贝而非借用：裸指针无法携带生命周期。
unsafe fn abi_bytes_to_vec(bytes: AbiBytes) -> Option<Vec<u8>> {
    if bytes.len == 0 {
        return Some(Vec::new());
    }
    if bytes.ptr.is_null() {
        return None;
    }
    let len = usize::try_from(bytes.len).ok()?;
    Some(unsafe { std::slice::from_raw_parts(bytes.ptr, len) }.to_vec())
}

unsafe fn abi_bytes_to_string(bytes: AbiBytes) -> Option<String> {
    let raw = unsafe { abi_bytes_to_vec(bytes) }?;
    String::from_utf8(raw).ok()
}

/// 驱动单个附加模块插件的 helper 运行时限。
pub struct HelperRuntime {
    module_id: String,
    plugin_path: PathBuf,
    /// 与宿主的帧通道；`None` 时能力请求被拒绝（进程内单测用）。
    channel: Option<Arc<Mutex<HelperChannel>>>,
    /// 字段顺序即释放顺序：`instance` 必须先于 `library` 释放，
    /// 因为插件 `destroy` 回调的函数指针属于 `library` 的映像。
    instance: Option<PluginInstance<HostContext>>,
    library: Option<libloading::Library>,
    handed_over: bool,
}

impl HelperRuntime {
    /// 构造不带宿主通道的运行时（仅用于不涉及能力请求的进程内测试）。
    pub fn new(module_id: impl Into<String>, plugin_path: impl Into<PathBuf>) -> Self {
        Self {
            module_id: module_id.into(),
            plugin_path: plugin_path.into(),
            channel: None,
            instance: None,
            library: None,
            handed_over: false,
        }
    }

    /// 构造绑定宿主通道的运行时：插件的能力请求会经该通道转发给宿主。
    pub fn with_channel(
        module_id: impl Into<String>,
        plugin_path: impl Into<PathBuf>,
        channel: Arc<Mutex<HelperChannel>>,
    ) -> Self {
        Self {
            channel: Some(channel),
            ..Self::new(module_id, plugin_path)
        }
    }

    /// 当前是否已成功加载并初始化插件。
    pub fn is_loaded(&self) -> bool {
        self.instance.is_some()
    }

    /// 处理一条宿主请求，始终返回与之 request id 对应的响应。
    ///
    /// 任何非法输入都 fail closed：未知方法、错误版本、未握手、非法参数、插件失败
    /// 都返回带错误码的响应，而不是 panic 或静默成功。
    pub fn handle(&mut self, request: IpcRequest) -> IpcResponse {
        let request_id = request.request_id.clone();

        if let Err(error) = validate_version(request.version) {
            return reply_error(&request_id, "unsupported_version", error.to_string());
        }

        let Some(method) = ModuleMethod::from_name(&request.method) else {
            return reply_error(
                &request_id,
                "method_not_found",
                format!("unknown method `{}`", request.method),
            );
        };

        // 握手前不接受任何方法：否则宿主可能在版本未协商时驱动生命周期。
        if method != ModuleMethod::Handshake && !self.handed_over {
            return reply_error(
                &request_id,
                "handshake_required",
                "module.handshake must succeed before any other method",
            );
        }

        let decoded = match method.decode_request(request.params) {
            Ok(decoded) => decoded,
            Err(error) => return reply_error(&request_id, "invalid_params", error.to_string()),
        };

        match self.dispatch(decoded) {
            Ok(result) => reply_ok(&request_id, result),
            Err(failure) => reply_error(&request_id, failure.code, failure.message),
        }
    }

    fn dispatch(&mut self, request: ModuleRequest) -> Result<Value, Failure> {
        match request {
            ModuleRequest::Handshake(request) => {
                let response = negotiate_handshake(&request)
                    .map_err(|error| Failure::new("unsupported_version", error.to_string()))?;
                self.handed_over = true;
                encode(response)
            }
            ModuleRequest::Initialize(request) => {
                self.load_plugin(&request.config)?;
                encode(InitializeResponse {})
            }
            ModuleRequest::Lifecycle(request) => match request.action {
                LifecycleAction::Start => {
                    self.instance_mut()?
                        .start()
                        .map_err(|error| Failure::new("start_failed", error.to_string()))?;
                    encode(LifecycleResponse {
                        state: ModuleLifecycleState::Running,
                    })
                }
                LifecycleAction::Stop => {
                    self.instance_mut()?
                        .stop()
                        .map_err(|error| Failure::new("stop_failed", error.to_string()))?;
                    encode(LifecycleResponse {
                        state: ModuleLifecycleState::Stopped,
                    })
                }
            },
            ModuleRequest::Health(_) => encode(HealthResponse {
                healthy: self.is_loaded(),
            }),
            ModuleRequest::Invoke(request) => {
                let input = serde_json::to_vec(&request.args)
                    .map_err(|error| Failure::new("encode_failed", error.to_string()))?;
                let output = self
                    .instance_mut()?
                    .invoke(&request.command, &input)
                    .map_err(|error| Failure::new("invoke_failed", error.to_string()))?;
                let result: Value = serde_json::from_slice(&output).map_err(|error| {
                    Failure::new(
                        "invalid_plugin_output",
                        format!("plugin returned non-JSON output: {error}"),
                    )
                })?;
                encode(InvokeResponse { result })
            }
        }
    }

    /// 加载插件产物并完成初始化。重复调用会被拒绝，避免同一会话加载两个实例。
    fn load_plugin(&mut self, config: &Value) -> Result<(), Failure> {
        if self.instance.is_some() {
            return Err(Failure::new(
                "already_initialized",
                "this helper session already owns an initialized plugin instance",
            ));
        }

        // SAFETY: 路径来自宿主下发；按 ABI 版本与函数表校验过才会继续使用。
        let library = unsafe { libloading::Library::new(&self.plugin_path) }
            .map_err(|error| Failure::new("plugin_load_failed", error.to_string()))?;

        let entry: PluginEntry = unsafe {
            let symbol = library
                .get::<PluginEntry>(PLUGIN_ENTRY_SYMBOL)
                .map_err(|error| Failure::new("plugin_entry_missing", error.to_string()))?;
            *symbol
        };

        let config_bytes = serde_json::to_vec(config)
            .map_err(|error| Failure::new("encode_failed", error.to_string()))?;

        let instance = unsafe {
            PluginInstance::instantiate(
                entry,
                &self.module_id,
                &config_bytes,
                HostContext {
                    module_id: self.module_id.clone(),
                    channel: self.channel.clone(),
                },
                plugin_capability,
            )
        }
        .map_err(|error| Failure::new("plugin_init_failed", error.to_string()))?;

        self.library = Some(library);
        self.instance = Some(instance);
        Ok(())
    }

    fn instance_mut(&mut self) -> Result<&mut PluginInstance<HostContext>, Failure> {
        self.instance.as_mut().ok_or_else(|| {
            Failure::new(
                "not_initialized",
                "module.initialize must succeed before this method",
            )
        })
    }
}

/// 校验 helper 启动参数中的模块身份。
pub fn validate_module_id(module_id: &str) -> Result<(), Failure> {
    if is_valid_module_id(module_id) {
        Ok(())
    } else {
        Err(Failure::new(
            "invalid_module_id",
            format!("`{module_id}` is not a valid two-segment module id"),
        ))
    }
}

/// 插件产物路径必须存在，否则启动即失败好过延迟到 initialize 才报错。
pub fn validate_plugin_path(path: &Path) -> Result<(), Failure> {
    if path.is_file() {
        Ok(())
    } else {
        Err(Failure::new(
            "plugin_not_found",
            format!("plugin artifact `{}` does not exist", path.display()),
        ))
    }
}

fn reply_ok(request_id: &str, result: Value) -> IpcResponse {
    IpcResponse {
        version: PROTOCOL_VERSION,
        request_id: request_id.to_owned(),
        result: Some(result),
        error: None,
    }
}

fn reply_error(request_id: &str, code: &str, message: impl Into<String>) -> IpcResponse {
    IpcResponse {
        version: PROTOCOL_VERSION,
        request_id: request_id.to_owned(),
        result: None,
        error: Some(crate::ipc::IpcErrorDto {
            code: code.to_owned(),
            message: message.into(),
        }),
    }
}

fn encode(value: impl serde::Serialize) -> Result<Value, Failure> {
    serde_json::to_value(value).map_err(|error| Failure::new("encode_failed", error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::HandshakeRequest;
    use serde_json::json;

    fn request(request_id: &str, method: ModuleMethod, params: Value) -> IpcRequest {
        IpcRequest {
            version: PROTOCOL_VERSION,
            request_id: request_id.to_owned(),
            method: method.as_str().to_owned(),
            params,
        }
    }

    fn runtime() -> HelperRuntime {
        HelperRuntime::new(
            "copper-lamp.demo-tools",
            "definitely-not-a-real-plugin-artifact",
        )
    }

    fn handshake(runtime: &mut HelperRuntime) {
        let response = runtime.handle(request(
            "req-1",
            ModuleMethod::Handshake,
            json!({ "supported_versions": [PROTOCOL_VERSION] }),
        ));
        assert!(response.error.is_none());
        assert_eq!(response.result.unwrap()["version"], json!(PROTOCOL_VERSION));
    }

    #[test]
    fn methods_other_than_handshake_are_refused_before_negotiation() {
        let mut runtime = runtime();

        let response = runtime.handle(request("req-health", ModuleMethod::Health, json!({})));

        assert_eq!(response.request_id, "req-health");
        assert_eq!(response.error.unwrap().code, "handshake_required");
    }

    #[test]
    fn handshake_negotiates_the_supported_protocol_version() {
        let mut runtime = runtime();

        let response = runtime.handle(request(
            "req-1",
            ModuleMethod::Handshake,
            json!({ "supported_versions": [PROTOCOL_VERSION] }),
        ));

        assert!(response.error.is_none());
        assert_eq!(response.result.unwrap()["version"], json!(PROTOCOL_VERSION));
    }

    #[test]
    fn handshake_refuses_a_version_the_helper_does_not_support() {
        let mut runtime = runtime();

        let response = runtime.handle(request(
            "req-1",
            ModuleMethod::Handshake,
            json!({ "supported_versions": [PROTOCOL_VERSION + 9] }),
        ));

        assert_eq!(response.error.unwrap().code, "unsupported_version");
    }

    #[test]
    fn absolute_version_mismatch_is_refused_before_dispatch() {
        let mut runtime = runtime();
        let mut ipc = request("req-1", ModuleMethod::Handshake, json!({}));
        ipc.version = PROTOCOL_VERSION + 1;

        let response = runtime.handle(ipc);

        assert_eq!(response.error.unwrap().code, "unsupported_version");
    }

    #[test]
    fn unknown_method_is_reported_not_ignored() {
        let mut runtime = runtime();
        handshake(&mut runtime);

        let response = runtime.handle(IpcRequest {
            version: PROTOCOL_VERSION,
            request_id: "req-2".to_owned(),
            method: "module.launch_minecraft".to_owned(),
            params: json!({}),
        });

        assert_eq!(response.error.unwrap().code, "method_not_found");
    }

    #[test]
    fn lifecycle_before_initialize_reports_not_initialized() {
        let mut runtime = runtime();
        handshake(&mut runtime);

        let response = runtime.handle(request(
            "req-2",
            ModuleMethod::Start,
            json!({ "action": "start" }),
        ));

        let error = response.error.unwrap();
        assert_eq!(error.code, "not_initialized");
    }

    #[test]
    fn health_reports_unhealthy_until_a_plugin_is_loaded() {
        let mut runtime = runtime();
        handshake(&mut runtime);

        let response = runtime.handle(request("req-2", ModuleMethod::Health, json!({})));

        assert_eq!(response.result.unwrap()["healthy"], json!(false));
    }

    #[test]
    fn initialize_reports_a_missing_plugin_artifact() {
        let mut runtime = runtime();
        handshake(&mut runtime);

        let response = runtime.handle(request(
            "req-2",
            ModuleMethod::Initialize,
            json!({ "config": {} }),
        ));

        assert_eq!(response.error.unwrap().code, "plugin_load_failed");
    }

    #[test]
    fn handshake_request_shape_is_enforced() {
        let mut runtime = runtime();

        let response = runtime.handle(request(
            "req-1",
            ModuleMethod::Handshake,
            json!({ "unexpected": true }),
        ));

        assert_eq!(response.error.unwrap().code, "invalid_params");
    }

    #[test]
    fn handshake_dto_round_trips_through_the_method_table() {
        let decoded = ModuleMethod::Handshake
            .decode_request(json!({ "supported_versions": [PROTOCOL_VERSION] }))
            .unwrap();

        assert_eq!(
            decoded,
            ModuleRequest::Handshake(HandshakeRequest {
                supported_versions: vec![PROTOCOL_VERSION],
            })
        );
    }

    #[test]
    fn invalid_module_ids_are_rejected() {
        assert!(validate_module_id("../escape").is_err());
        assert!(validate_module_id("single").is_err());
        assert!(validate_module_id("copper-lamp.demo-tools").is_ok());
    }
}
