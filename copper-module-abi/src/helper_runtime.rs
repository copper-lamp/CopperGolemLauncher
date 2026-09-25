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
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::ipc::{
    negotiate_handshake, validate_version, HealthResponse, InitializeResponse, InvokeResponse,
    IpcRequest, IpcResponse, LifecycleAction, LifecycleResponse, ModuleLifecycleState,
    ModuleMethod, ModuleRequest, PROTOCOL_VERSION,
};
use crate::module_id::is_valid_module_id;
use crate::plugin_abi::{
    AbiBuffer, AbiBytes, PluginEntry, PluginInstance, ABI_STATUS_NOT_SUPPORTED,
    PLUGIN_ENTRY_SYMBOL,
};

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
}

/// 能力 RPC 一律显式拒绝。
///
/// 授权切片尚未实现时返回 `ABI_STATUS_NOT_SUPPORTED` 而不是成功或空结果：插件因此
/// 能明确区分"被拒绝"与"拿到结果"，不会把未实现当作可用。
unsafe extern "C" fn deny_capability(
    user_data: *mut c_void,
    capability: AbiBytes,
    _input: AbiBytes,
    _output: *mut AbiBuffer,
) -> i32 {
    let module_id = if user_data.is_null() {
        "<unknown>".to_owned()
    } else {
        // SAFETY: `user_data` 由 `PluginInstance` 持有，始终指向本模块的 HostContext。
        let context = unsafe { &*(user_data as *const HostContext) };
        context.module_id.clone()
    };
    eprintln!(
        "[helper] capability request ({} bytes) denied for module `{module_id}`: \
         capability RPC is not implemented in this build",
        capability.len
    );
    ABI_STATUS_NOT_SUPPORTED
}

/// 驱动单个附加模块插件的 helper 运行时限。
pub struct HelperRuntime {
    module_id: String,
    plugin_path: PathBuf,
    /// 字段顺序即释放顺序：`instance` 必须先于 `library` 释放，
    /// 因为插件 `destroy` 回调的函数指针属于 `library` 的映像。
    instance: Option<PluginInstance<HostContext>>,
    library: Option<libloading::Library>,
    handed_over: bool,
}

impl HelperRuntime {
    /// 构造运行时。此时尚未加载任何插件代码。
    pub fn new(module_id: impl Into<String>, plugin_path: impl Into<PathBuf>) -> Self {
        Self {
            module_id: module_id.into(),
            plugin_path: plugin_path.into(),
            instance: None,
            library: None,
            handed_over: false,
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
                },
                deny_capability,
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
