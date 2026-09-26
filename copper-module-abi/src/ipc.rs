use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Write};

pub const PROTOCOL_VERSION: u32 = 2;
pub const WIRE_PROTOCOL_ID: &str = "copper-addon.ndjson";
pub const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireFrame {
    Hello {
        version: u32,
        protocol: String,
        supported_versions: Vec<u32>,
        runtime: String,
        capabilities: Vec<String>,
    },
    Request {
        version: u32,
        id: String,
        method: String,
        params: Value,
    },
    Response {
        version: u32,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<WireError>,
    },
    Notification {
        version: u32,
        method: String,
        params: Value,
    },
    Event {
        version: u32,
        event: String,
        payload: Value,
    },
    Fatal {
        version: u32,
        error: WireError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireError {
    pub code: String,
    pub message: String,
}

impl WireFrame {
    pub fn request(id: impl Into<String>, method: impl Into<String>, params: Value) -> Self {
        Self::Request {
            version: PROTOCOL_VERSION,
            id: id.into(),
            method: method.into(),
            params,
        }
    }

    pub fn response(id: impl Into<String>, result: Value) -> Self {
        Self::Response {
            version: PROTOCOL_VERSION,
            id: id.into(),
            result: Some(result),
            error: None,
        }
    }

    pub fn encode_line(&self) -> Result<Vec<u8>, IpcError> {
        validate_wire_frame(self)?;
        let mut bytes = serde_json::to_vec(self).map_err(IpcError::Json)?;
        if bytes.len() > MAX_FRAME_SIZE {
            return Err(IpcError::FrameTooLarge { size: bytes.len(), max: MAX_FRAME_SIZE });
        }
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn decode_line(bytes: &[u8]) -> Result<Self, IpcError> {
        let payload = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        let payload = payload.strip_suffix(b"\r").unwrap_or(payload);
        if payload.contains(&b'\n') || payload.contains(&b'\r') {
            return Err(IpcError::InvalidFrame);
        }
        if payload.len() > MAX_FRAME_SIZE {
            return Err(IpcError::FrameTooLarge { size: payload.len(), max: MAX_FRAME_SIZE });
        }
        let frame: Self = serde_json::from_slice(payload).map_err(IpcError::Json)?;
        validate_wire_frame(&frame)?;
        Ok(frame)
    }
}

fn validate_wire_frame(frame: &WireFrame) -> Result<(), IpcError> {
    let version = match frame {
        WireFrame::Hello { version, .. }
        | WireFrame::Request { version, .. }
        | WireFrame::Response { version, .. }
        | WireFrame::Notification { version, .. }
        | WireFrame::Event { version, .. }
        | WireFrame::Fatal { version, .. } => *version,
    };
    if version != PROTOCOL_VERSION {
        return Err(IpcError::UnsupportedVersion { received: version, supported: PROTOCOL_VERSION });
    }
    if let WireFrame::Response { result, error, .. } = frame {
        if result.is_some() == error.is_some() {
            return Err(IpcError::InvalidFrame);
        }
    }
    if let WireFrame::Hello { protocol, .. } = frame {
        if protocol != WIRE_PROTOCOL_ID {
            return Err(IpcError::InvalidFrame);
        }
    }
    Ok(())
}

pub fn read_ndjson_frame<R: std::io::BufRead>(reader: &mut R) -> Result<WireFrame, IpcError> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Err(IpcError::Io(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated NDJSON frame")));
        }
        let count = available.iter().position(|byte| *byte == b'\n').map_or(available.len(), |index| index + 1);
        if line.len() + count > MAX_FRAME_SIZE + 1 {
            return Err(IpcError::FrameTooLarge { size: line.len() + count - 1, max: MAX_FRAME_SIZE });
        }
        let has_newline = available[count - 1] == b'\n';
        line.extend_from_slice(&available[..count]);
        reader.consume(count);
        if has_newline {
            return WireFrame::decode_line(&line);
        }
    }
}

pub fn write_ndjson_frame<W: Write>(writer: &mut W, encoded_line: &[u8]) -> Result<(), IpcError> {
    let frame = WireFrame::decode_line(encoded_line)?;
    let canonical_line = frame.encode_line()?;
    writer.write_all(&canonical_line)?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeRequest {
    pub supported_versions: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeResponse {
    pub version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleAction {
    Start,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleRequest {
    pub action: LifecycleAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LifecycleResponse {
    pub state: ModuleLifecycleState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleLifecycleState {
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitializeRequest {
    pub config: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitializeResponse {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthRequest {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthResponse {
    pub healthy: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvokeRequest {
    pub command: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvokeResponse {
    pub result: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRequest {
    pub capability: String,
    #[serde(deserialize_with = "deserialize_capability_params")]
    pub params: Value,
}

/// 宿主推送一条事件给 helper（**单向通知**，helper 不回帧）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventDispatchRequest {
    pub event: String,
    pub payload: Value,
}

/// 宿主请 helper 把一次意图请求转给插件（请求 / 响应）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentHandleRequest {
    pub intent: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentHandleResponse {
    pub result: Value,
}

/// 能力参数**顶层**不得携带 `module_id`。
///
/// 这只是一道廉价的第一道闸：真正的防护是宿主始终使用**会话绑定**的模块身份，从不
/// 读取请求里的任何身份字段（见内核的 `registry::capability`）。因此这里只检查顶层，
/// 不再递归——递归会把模块存储里一条恰好含 `module_id` 字段的正常记录也拒掉，
/// 那是把防御变成了功能缺陷。
fn deserialize_capability_params<'de, D>(deserializer: D) -> Result<Value, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let params = Value::deserialize(deserializer)?;
    if let Value::Object(object) = &params {
        if object.contains_key("module_id") {
            return Err(serde::de::Error::custom(
                "capability params must not carry module_id at the top level",
            ));
        }
    }
    Ok(params)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityResponse {
    pub result: Value,
}

pub const METHOD_MODULE_INITIALIZE: &str = "module.initialize";
pub const METHOD_MODULE_START: &str = "module.start";
pub const METHOD_MODULE_STOP: &str = "module.stop";
pub const METHOD_MODULE_HEALTH: &str = "module.health";
pub const METHOD_MODULE_INVOKE: &str = "module.invoke";
/// 版本协商。必须在任何其它方法之前调用，双方各自拒绝不支持的版本。
pub const METHOD_MODULE_HANDSHAKE: &str = "module.handshake";
/// 插件向宿主请求能力时使用的方法名（方向为 helper → host）。
pub const METHOD_CAPABILITY_REQUEST: &str = "capability.request";
/// 宿主向 helper 推送事件的方法名（方向为 host → helper）。
///
/// **这是单向通知，不是请求**：helper 派发到插件后**不写任何响应帧**。
/// 一旦它回了帧，宿主正在等待某个 invoke 响应的调用方会读到不属于它的响应，
/// 把一次成功的调用判成 [`IpcError::RequestIdMismatch`] 并污染后续配对。
/// 因此它**不进入** [`ModuleMethod`] 的请求 / 响应方法表。
pub const METHOD_EVENT_DISPATCH: &str = "event.dispatch";
/// 宿主请 helper 把一次意图请求转给插件的方法名（请求 / 响应）。
pub const METHOD_INTENT_HANDLE: &str = "intent.handle";

/// 写入 Agent 的凭据（请求 / 响应）。
pub const METHOD_AGENT_CREDENTIALS_SET: &str = "agent.credentials.set";
/// 清除 Agent 已写入的凭据（请求 / 响应）。
pub const METHOD_AGENT_CREDENTIALS_CLEAR: &str = "agent.credentials.clear";
/// 选择 Agent 使用的模型（请求 / 响应）。
pub const METHOD_AGENT_MODEL_SET: &str = "agent.model.set";
/// 加载 / 恢复一次 Agent 会话（请求 / 响应）。
pub const METHOD_AGENT_SESSION_LOAD: &str = "agent.session.load";
/// 发起一轮 Agent 提示，流式结果通过 `event` 帧单向回推（请求 / 响应 + 事件流）。
pub const METHOD_AGENT_PROMPT: &str = "agent.prompt";
/// 取消当前正在进行的 Agent 轮次（请求 / 响应）。
pub const METHOD_AGENT_CANCEL: &str = "agent.cancel";
/// 探活（请求 / 响应）。宿主用它确认受监管 Node 进程仍存活且协议正常。
pub const METHOD_AGENT_PING: &str = "agent.ping";
/// 请求 Agent 进程优雅退出（请求 / 响应）。
pub const METHOD_AGENT_SHUTDOWN: &str = "agent.shutdown";

/// 受监管 Node Agent runtime 支持的方法集合（用于 hello 帧的能力声明）。
///
/// 顺序即能力声明顺序：host hello 直接按此数组生成 `capabilities`，顺序变动会被
/// golden frame 断言抓住。宿主侧目前只发起其中一部分方法，但整份集合都要声明，
/// 以便 Agent 侧据此判断宿主理解了协议全貌。
pub const AGENT_METHODS: &[&str] = &[
    METHOD_AGENT_CREDENTIALS_SET,
    METHOD_AGENT_CREDENTIALS_CLEAR,
    METHOD_AGENT_MODEL_SET,
    METHOD_AGENT_SESSION_LOAD,
    METHOD_AGENT_PROMPT,
    METHOD_AGENT_CANCEL,
    METHOD_AGENT_PING,
    METHOD_AGENT_SHUTDOWN,
];

/// 插件命令命名空间：宿主把事件 / 意图派发转成 `plugin.invoke("<前缀><名>", payload)`。
///
/// 插件的命令空间由模块自定（如夹具的 `demo.echo`），不保留命名空间就无法区分
/// 「宿主派发的通知」与「模块自己的命令」。这两个前缀由宿主保留，模块不得注册同名命令。
pub const PLUGIN_COMMAND_EVENT_PREFIX: &str = "event.";
pub const PLUGIN_COMMAND_INTENT_PREFIX: &str = "intent.";

/// 双向通道上的消息信封。
///
/// 内核与 helper 之间**两个方向都会发请求**：宿主驱动模块生命周期，插件则请求宿主
/// 能力。因此每一帧都必须自描述方向，不能靠"收到的一定是响应"来推断——那样一旦
/// 能力 RPC 上线就会出现帧被解析成错误类型的问题。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IpcMessage {
    Hello {
        version: u32,
        protocol: String,
        supported_versions: Vec<u32>,
        runtime: String,
        capabilities: Vec<String>,
    },
    Request(IpcRequest),
    Response(IpcResponse),
    Notification { version: u32, method: String, params: Value },
    Event { version: u32, event: String, payload: Value },
    Fatal { version: u32, error: WireError },
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostNotification {
    pub method: String,
    pub params: Value,
}

impl IpcMessage {
    pub fn encode(&self) -> Result<Vec<u8>, IpcError> {
        match self {
            Self::Hello { version, protocol, supported_versions, runtime, capabilities } => WireFrame::Hello {
                version: *version,
                protocol: protocol.clone(),
                supported_versions: supported_versions.clone(),
                runtime: runtime.clone(),
                capabilities: capabilities.clone(),
            }.encode_line(),
            Self::Request(request) => WireFrame::Request {
                version: request.version,
                id: request.request_id.clone(),
                method: request.method.clone(),
                params: request.params.clone(),
            }
            .encode_line(),
            Self::Response(response) => WireFrame::Response {
                version: response.version,
                id: response.request_id.clone(),
                result: response.result.clone(),
                error: response.error.as_ref().map(|error| WireError {
                    code: error.code.clone(),
                    message: error.message.clone(),
                }),
            }
            .encode_line(),
            Self::Notification { version, method, params } => WireFrame::Notification {
                version: *version,
                method: method.clone(),
                params: params.clone(),
            }
            .encode_line(),
            Self::Event { version, event, payload } => WireFrame::Event {
                version: *version,
                event: event.clone(),
                payload: payload.clone(),
            }
            .encode_line(),
            Self::Fatal { version, error } => WireFrame::Fatal {
                version: *version,
                error: error.clone(),
            }
            .encode_line(),
        }
    }

    pub fn decode(payload: &[u8]) -> Result<Self, IpcError> {
        match WireFrame::decode_line(payload)? {
            WireFrame::Hello { version, protocol, supported_versions, runtime, capabilities } => Ok(Self::Hello {
                version, protocol, supported_versions, runtime, capabilities,
            }),
            WireFrame::Notification { version, method, params } => Ok(Self::Notification { version, method, params }),
            WireFrame::Event { version, event, payload } => Ok(Self::Event { version, event, payload }),
            WireFrame::Fatal { version, error } => Ok(Self::Fatal { version, error }),
            WireFrame::Request { version, id, method, params } => Ok(Self::Request(IpcRequest {
                version,
                request_id: id,
                method,
                params,
            })),
            WireFrame::Response { version, id, result, error } => Ok(Self::Response(IpcResponse {
                version,
                request_id: id,
                result,
                error: error.map(|error| IpcErrorDto {
                    code: error.code,
                    message: error.message,
                }),
            })),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleMethod {
    Handshake,
    Initialize,
    Start,
    Stop,
    Health,
    Invoke,
    /// 把一次意图请求转给插件（请求 / 响应）。与 [`METHOD_EVENT_DISPATCH`] 不同，
    /// 它有真实响应，因此属于本方法表。
    IntentHandle,
}

impl ModuleMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Handshake => METHOD_MODULE_HANDSHAKE,
            Self::Initialize => METHOD_MODULE_INITIALIZE,
            Self::Start => METHOD_MODULE_START,
            Self::Stop => METHOD_MODULE_STOP,
            Self::Health => METHOD_MODULE_HEALTH,
            Self::Invoke => METHOD_MODULE_INVOKE,
            Self::IntentHandle => METHOD_INTENT_HANDLE,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            METHOD_MODULE_HANDSHAKE => Some(Self::Handshake),
            METHOD_MODULE_INITIALIZE => Some(Self::Initialize),
            METHOD_MODULE_START => Some(Self::Start),
            METHOD_MODULE_STOP => Some(Self::Stop),
            METHOD_MODULE_HEALTH => Some(Self::Health),
            METHOD_MODULE_INVOKE => Some(Self::Invoke),
            METHOD_INTENT_HANDLE => Some(Self::IntentHandle),
            _ => None,
        }
    }

    pub fn decode_request(self, params: Value) -> Result<ModuleRequest, serde_json::Error> {
        match self {
            Self::Handshake => serde_json::from_value(params).map(ModuleRequest::Handshake),
            Self::Initialize => serde_json::from_value(params).map(ModuleRequest::Initialize),
            Self::Start | Self::Stop => serde_json::from_value(params).map(ModuleRequest::Lifecycle),
            Self::Health => serde_json::from_value(params).map(ModuleRequest::Health),
            Self::Invoke => serde_json::from_value(params).map(ModuleRequest::Invoke),
            Self::IntentHandle => serde_json::from_value(params).map(ModuleRequest::IntentHandle),
        }
    }

    pub fn decode_response(self, result: Value) -> Result<ModuleResponse, serde_json::Error> {
        match self {
            Self::Handshake => serde_json::from_value(result).map(ModuleResponse::Handshake),
            Self::Initialize => serde_json::from_value(result).map(ModuleResponse::Initialize),
            Self::Start | Self::Stop => serde_json::from_value(result).map(ModuleResponse::Lifecycle),
            Self::Health => serde_json::from_value(result).map(ModuleResponse::Health),
            Self::Invoke => serde_json::from_value(result).map(ModuleResponse::Invoke),
            Self::IntentHandle => serde_json::from_value(result).map(ModuleResponse::IntentHandle),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModuleRequest {
    Handshake(HandshakeRequest),
    Initialize(InitializeRequest),
    Lifecycle(LifecycleRequest),
    Health(HealthRequest),
    Invoke(InvokeRequest),
    IntentHandle(IntentHandleRequest),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModuleResponse {
    Handshake(HandshakeResponse),
    Initialize(InitializeResponse),
    Lifecycle(LifecycleResponse),
    Health(HealthResponse),
    Invoke(InvokeResponse),
    IntentHandle(IntentHandleResponse),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcRequest {
    pub version: u32,
    pub request_id: String,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcResponse {
    pub version: u32,
    pub request_id: String,
    pub result: Option<Value>,
    pub error: Option<IpcErrorDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpcErrorDto {
    pub code: String,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("IPC I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("IPC response request ID {received} does not match request ID {expected}")]
    RequestIdMismatch { expected: String, received: String },
    #[error("IPC JSON encoding or decoding failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IPC frame size {size} exceeds maximum {max}")]
    FrameTooLarge { size: usize, max: usize },
    #[error("invalid IPC frame")]
    InvalidFrame,
    #[error("unsupported IPC protocol version {received}; supported version is {supported}")]
    UnsupportedVersion { received: u32, supported: u32 },
    #[error("module ID must not be empty")]
    InvalidModuleId,
}

pub fn validate_version(version: u32) -> Result<(), IpcError> {
    if version == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(IpcError::UnsupportedVersion {
            received: version,
            supported: PROTOCOL_VERSION,
        })
    }
}

pub fn negotiate_hello(frame: &WireFrame) -> Result<u32, IpcError> {
    let WireFrame::Hello { version, protocol, supported_versions, .. } = frame else {
        return Err(IpcError::InvalidFrame);
    };
    if protocol != WIRE_PROTOCOL_ID || *version != PROTOCOL_VERSION {
        return Err(IpcError::InvalidFrame);
    }
    if supported_versions.contains(&PROTOCOL_VERSION) {
        Ok(PROTOCOL_VERSION)
    } else {
        Err(IpcError::UnsupportedVersion {
            received: supported_versions.iter().copied().max().unwrap_or(0),
            supported: PROTOCOL_VERSION,
        })
    }
}

pub fn negotiate_handshake(request: &HandshakeRequest) -> Result<HandshakeResponse, IpcError> {
    if request.supported_versions.contains(&PROTOCOL_VERSION) {
        Ok(HandshakeResponse {
            version: PROTOCOL_VERSION,
        })
    } else {
        Err(IpcError::UnsupportedVersion {
            received: request.supported_versions.iter().copied().max().unwrap_or(0),
            supported: PROTOCOL_VERSION,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleRpcState {
    module_id: String,
}

impl ModuleRpcState {
    pub fn new(module_id: impl Into<String>) -> Result<Self, IpcError> {
        let module_id = module_id.into();
        if module_id.trim().is_empty() {
            return Err(IpcError::InvalidModuleId);
        }
        Ok(Self { module_id })
    }

    pub fn module_id(&self) -> &str {
        &self.module_id
    }
}

pub fn validate_response_correlation(
    request: &IpcRequest,
    response: &IpcResponse,
) -> Result<(), IpcError> {
    validate_version(response.version)?;
    if response.request_id == request.request_id {
        Ok(())
    } else {
        Err(IpcError::RequestIdMismatch {
            expected: request.request_id.clone(),
            received: response.request_id.clone(),
        })
    }
}

pub fn read_frame<R: std::io::BufRead>(reader: &mut R) -> Result<Vec<u8>, IpcError> {
    let frame = read_ndjson_frame(reader)?;
    frame.encode_line()
}

pub fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), IpcError> {
    write_ndjson_frame(writer, payload)
}

#[cfg(test)]
mod tests {
    use super::{
        negotiate_handshake, read_ndjson_frame, validate_response_correlation, validate_version,
        write_frame, CapabilityRequest, HandshakeRequest, IpcError, IpcErrorDto, IpcRequest,
        IpcResponse, LifecycleAction, LifecycleRequest, ModuleRpcState, MAX_FRAME_SIZE,
        PROTOCOL_VERSION,
    };
    use serde_json::json;
    use std::io::Cursor;

    #[test]
    fn request_and_response_json_preserve_version_request_id_and_error() {
        let request = IpcRequest {
            version: PROTOCOL_VERSION,
            request_id: "req-17".to_owned(),
            method: "module.start".to_owned(),
            params: json!({ "enabled": true }),
        };
        let encoded = serde_json::to_vec(&request).unwrap();
        let decoded: IpcRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.version, PROTOCOL_VERSION);
        assert_eq!(decoded.request_id, "req-17");
        assert_eq!(decoded.method, "module.start");
        assert_eq!(decoded.params, json!({ "enabled": true }));

        let response = IpcResponse {
            version: PROTOCOL_VERSION,
            request_id: request.request_id,
            result: None,
            error: Some(IpcErrorDto {
                code: "method_not_found".to_owned(),
                message: "Unknown method".to_owned(),
            }),
        };
        let encoded = serde_json::to_vec(&response).unwrap();
        let decoded: IpcResponse = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.version, PROTOCOL_VERSION);
        assert_eq!(decoded.request_id, "req-17");
        assert_eq!(decoded.error.unwrap().code, "method_not_found");
    }

    #[test]
    fn version_validation_rejects_unsupported_versions() {
        assert!(validate_version(PROTOCOL_VERSION).is_ok());
        assert!(matches!(
            validate_version(PROTOCOL_VERSION + 1),
            Err(IpcError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn handshake_negotiates_the_highest_supported_protocol_version() {
        let result = negotiate_handshake(&HandshakeRequest {
            supported_versions: vec![0, PROTOCOL_VERSION, PROTOCOL_VERSION + 1],
        })
        .unwrap();
        assert_eq!(result.version, PROTOCOL_VERSION);

        let unsupported = negotiate_handshake(&HandshakeRequest {
            supported_versions: vec![PROTOCOL_VERSION + 1],
        });
        assert!(matches!(unsupported, Err(IpcError::UnsupportedVersion { .. })));
    }

    #[test]
    fn initialize_health_and_invoke_dtos_round_trip() {
        use super::{
            HealthRequest, HealthResponse, InitializeRequest, InitializeResponse, InvokeRequest,
            InvokeResponse,
        };

        let initialize = InitializeRequest {
            config: json!({ "locale": "zh-CN" }),
        };
        let encoded = serde_json::to_vec(&initialize).unwrap();
        let decoded: InitializeRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, initialize);

        let initialized = InitializeResponse {};
        let encoded = serde_json::to_vec(&initialized).unwrap();
        let decoded: InitializeResponse = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, initialized);

        let health = HealthRequest {};
        let encoded = serde_json::to_vec(&health).unwrap();
        let decoded: HealthRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, health);

        let health_result = HealthResponse { healthy: true };
        let encoded = serde_json::to_vec(&health_result).unwrap();
        let decoded: HealthResponse = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, health_result);

        let invoke = InvokeRequest {
            command: "open_world".to_owned(),
            args: json!({ "world_id": "world-1" }),
        };
        let encoded = serde_json::to_vec(&invoke).unwrap();
        let decoded: InvokeRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, invoke);

        let invoked = InvokeResponse {
            result: json!({ "opened": true }),
        };
        let encoded = serde_json::to_vec(&invoked).unwrap();
        let decoded: InvokeResponse = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, invoked);
    }

    #[test]
    fn module_method_names_map_to_typed_request_and_response_dtos() {
        use super::{
            ModuleMethod, ModuleRequest, ModuleResponse, METHOD_MODULE_HANDSHAKE,
            METHOD_MODULE_HEALTH, METHOD_MODULE_INITIALIZE, METHOD_MODULE_INVOKE,
            METHOD_MODULE_START, METHOD_MODULE_STOP, METHOD_INTENT_HANDLE,
        };

        let cases = [
            (ModuleMethod::Handshake, METHOD_MODULE_HANDSHAKE),
            (ModuleMethod::Initialize, METHOD_MODULE_INITIALIZE),
            (ModuleMethod::Start, METHOD_MODULE_START),
            (ModuleMethod::Stop, METHOD_MODULE_STOP),
            (ModuleMethod::Health, METHOD_MODULE_HEALTH),
            (ModuleMethod::Invoke, METHOD_MODULE_INVOKE),
            (ModuleMethod::IntentHandle, METHOD_INTENT_HANDLE),
        ];
        for (method, name) in cases {
            assert_eq!(method.as_str(), name);
            assert_eq!(ModuleMethod::from_name(name), Some(method));
        }

        assert_eq!(
            ModuleMethod::Initialize
                .decode_request(json!({ "config": {} }))
                .unwrap(),
            ModuleRequest::Initialize(super::InitializeRequest { config: json!({}) })
        );
        assert_eq!(
            ModuleMethod::Initialize
                .decode_response(json!({}))
                .unwrap(),
            ModuleResponse::Initialize(super::InitializeResponse {})
        );
        assert_eq!(
            ModuleMethod::Start.decode_request(json!({ "action": "start" })).unwrap(),
            ModuleRequest::Lifecycle(super::LifecycleRequest {
                action: super::LifecycleAction::Start,
            })
        );
        assert_eq!(
            ModuleMethod::Stop.decode_response(json!({ "state": "stopped" })).unwrap(),
            ModuleResponse::Lifecycle(super::LifecycleResponse {
                state: super::ModuleLifecycleState::Stopped,
            })
        );
        assert_eq!(
            ModuleMethod::Health.decode_request(json!({})).unwrap(),
            ModuleRequest::Health(super::HealthRequest {})
        );
        assert_eq!(
            ModuleMethod::Health.decode_response(json!({ "healthy": true })).unwrap(),
            ModuleResponse::Health(super::HealthResponse { healthy: true })
        );
        assert_eq!(
            ModuleMethod::Invoke
                .decode_request(json!({ "command": "open_world", "args": {} }))
                .unwrap(),
            ModuleRequest::Invoke(super::InvokeRequest {
                command: "open_world".to_owned(),
                args: json!({}),
            })
        );
        assert_eq!(
            ModuleMethod::Invoke
                .decode_response(json!({ "result": { "opened": true } }))
                .unwrap(),
            ModuleResponse::Invoke(super::InvokeResponse {
                result: json!({ "opened": true }),
            })
        );
        assert_eq!(ModuleMethod::from_name("module.unknown"), None);
    }

    #[test]
    fn capability_request_rejects_module_identity_in_wire_params() {
        let state = ModuleRpcState::new("module-a").unwrap();
        let encoded_params = json!({
            "capability": "world.read",
            "params": { "module_id": "attacker-module" }
        });
        assert!(serde_json::from_value::<CapabilityRequest>(encoded_params).is_err());

        let nested_module_id = json!({
            "capability": "world.read",
            "params": { "options": [{ "module_id": "some-value" }] }
        });
        assert!(
            serde_json::from_value::<CapabilityRequest>(nested_module_id).is_ok(),
            "嵌套的 module_id 是模块自己的数据，不构成身份伪造"
        );

        let encoded_field = json!({
            "capability": "world.read",
            "params": {},
            "module_id": "attacker-module"
        });
        assert!(serde_json::from_value::<CapabilityRequest>(encoded_field).is_err());
        assert_eq!(state.module_id(), "module-a");
    }

    #[test]
    fn lifecycle_and_capability_rpc_dtos_round_trip() {
        let lifecycle = LifecycleRequest {
            action: LifecycleAction::Start,
        };
        let encoded = serde_json::to_vec(&lifecycle).unwrap();
        let decoded: LifecycleRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, lifecycle);

        let capability = CapabilityRequest {
            capability: "world.read".to_owned(),
            params: json!({ "world_id": "world-1" }),
        };
        let encoded = serde_json::to_vec(&capability).unwrap();
        let decoded: CapabilityRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, capability);
    }

    #[test]
    fn capability_request_does_not_serialize_host_bound_module_identity() {
        let state = ModuleRpcState::new("module-a").unwrap();
        let request = CapabilityRequest {
            capability: "world.read".to_owned(),
            params: json!({}),
        };
        let encoded = serde_json::to_value(request).unwrap();
        assert_eq!(state.module_id(), "module-a");
        assert!(encoded.get("module_id").is_none());
    }

    #[test]
    fn response_validation_requires_matching_protocol_version_and_request_id() {
        let request = IpcRequest {
            version: PROTOCOL_VERSION,
            request_id: "req-17".to_owned(),
            method: "module.start".to_owned(),
            params: json!({}),
        };
        let matching = IpcResponse {
            version: PROTOCOL_VERSION,
            request_id: "req-17".to_owned(),
            result: Some(json!({ "started": true })),
            error: None,
        };
        assert!(validate_response_correlation(&request, &matching).is_ok());

        let mismatched_id = IpcResponse {
            request_id: "req-18".to_owned(),
            ..matching.clone()
        };
        assert!(validate_response_correlation(&request, &mismatched_id).is_err());

        let mismatched_version = IpcResponse {
            version: PROTOCOL_VERSION + 1,
            ..matching
        };
        assert!(validate_response_correlation(&request, &mismatched_version).is_err());
    }

    #[test]
    fn unified_ndjson_request_uses_v2_wire_envelope() {
        use super::{WireFrame, WIRE_PROTOCOL_ID};

        let frame = WireFrame::request("req-1", "module.start", json!({}));
        let encoded = frame.encode_line().unwrap();
        assert_eq!(
            std::str::from_utf8(&encoded).unwrap(),
            concat!(r#"{"kind":"request","version":2,"id":"req-1","method":"module.start","params":{}}"#, "\n")
        );
        assert_eq!(WireFrame::decode_line(&encoded).unwrap(), frame);
        assert_eq!(WIRE_PROTOCOL_ID, "copper-addon.ndjson");
    }

    #[test]
    fn host_event_dispatch_uses_an_idless_notification_frame() {
        use super::{IpcMessage, IpcRequest, METHOD_EVENT_DISPATCH};

        let frame = IpcMessage::Notification {
            version: PROTOCOL_VERSION,
            method: METHOD_EVENT_DISPATCH.to_owned(),
            params: json!({ "event": "demo.activity", "payload": { "n": 1 } }),
        };
        let encoded = frame.encode().unwrap();
        assert_eq!(
            std::str::from_utf8(&encoded).unwrap(),
            concat!(r#"{"kind":"notification","version":2,"method":"event.dispatch","params":{"event":"demo.activity","payload":{"n":1}}}"#, "\n")
        );
        assert!(!std::str::from_utf8(&encoded).unwrap().contains("\\\"id\\\""));
        assert_eq!(IpcMessage::decode(&encoded).unwrap(), frame);
        let legacy = IpcMessage::Request(IpcRequest {
            version: PROTOCOL_VERSION,
            request_id: "push-1".to_owned(),
            method: METHOD_EVENT_DISPATCH.to_owned(),
            params: json!({ "event": "demo.activity", "payload": { "n": 1 } }),
        });
        assert_ne!(legacy.encode().unwrap(), encoded);
    }

    #[test]
    fn hello_negotiation_rejects_a_peer_without_protocol_v2() {
        use super::{negotiate_hello, WireFrame, WIRE_PROTOCOL_ID};

        let hello = WireFrame::Hello {
            version: PROTOCOL_VERSION,
            protocol: WIRE_PROTOCOL_ID.to_owned(),
            supported_versions: vec![1],
            runtime: "test-helper".to_owned(),
            capabilities: Vec::new(),
        };
        assert!(negotiate_hello(&hello).is_err());
    }

    #[test]
    fn hello_event_and_fatal_frames_have_stable_golden_lines() {
        use super::{IpcMessage, WireError, WIRE_PROTOCOL_ID};

        // 这些字面量是 Rust 与 TypeScript 共用的 golden frame：任何一端改了信封形状，
        // 另一端的一致性测试必须同时失败，而不是各自悄悄演进。
        let hello = IpcMessage::Hello {
            version: PROTOCOL_VERSION,
            protocol: WIRE_PROTOCOL_ID.to_owned(),
            supported_versions: vec![PROTOCOL_VERSION],
            runtime: "copper-module-helper".to_owned(),
            capabilities: vec!["event.dispatch".to_owned()],
        };
        assert_eq!(
            std::str::from_utf8(&hello.encode().unwrap()).unwrap(),
            concat!(
                r#"{"kind":"hello","version":2,"protocol":"copper-addon.ndjson","supported_versions":[2],"runtime":"copper-module-helper","capabilities":["event.dispatch"]}"#,
                "\n"
            )
        );

        let event = IpcMessage::Event {
            version: PROTOCOL_VERSION,
            event: "agent.delta".to_owned(),
            payload: json!({ "runId": "run-1", "sequence": 1 }),
        };
        assert_eq!(
            std::str::from_utf8(&event.encode().unwrap()).unwrap(),
            concat!(
                r#"{"kind":"event","version":2,"event":"agent.delta","payload":{"runId":"run-1","sequence":1}}"#,
                "\n"
            )
        );

        let fatal = IpcMessage::Fatal {
            version: PROTOCOL_VERSION,
            error: WireError {
                code: "protocol_violation".to_owned(),
                message: "bad frame".to_owned(),
            },
        };
        assert_eq!(
            std::str::from_utf8(&fatal.encode().unwrap()).unwrap(),
            concat!(
                r#"{"kind":"fatal","version":2,"error":{"code":"protocol_violation","message":"bad frame"}}"#,
                "\n"
            )
        );

        let response = IpcMessage::Response(IpcResponse {
            version: PROTOCOL_VERSION,
            request_id: "req-1".to_owned(),
            result: None,
            error: Some(IpcErrorDto {
                code: "method_not_found".to_owned(),
                message: "unknown method".to_owned(),
            }),
        });
        assert_eq!(
            std::str::from_utf8(&response.encode().unwrap()).unwrap(),
            concat!(
                r#"{"kind":"response","version":2,"id":"req-1","error":{"code":"method_not_found","message":"unknown method"}}"#,
                "\n"
            )
        );
        for frame in [hello, event, fatal, response] {
            assert_eq!(IpcMessage::decode(&frame.encode().unwrap()).unwrap(), frame);
        }
    }

    #[test]
    fn the_hard_limit_counts_utf8_bytes_without_the_newline() {
        // 上限按 UTF-8 单行字节数（不含换行）计算，且是**闭区间**：恰好 8 MiB 合法，
        // 多 1 字节即拒绝。分帧器与解码器必须采用同一口径，否则合法帧会被误杀。
        let prefix = r#"{"kind":"event","version":2,"event":"e","payload":""#;
        let suffix = r#""}"#;
        let padding = "x".repeat(MAX_FRAME_SIZE - prefix.len() - suffix.len());
        let exact = format!("{prefix}{padding}{suffix}");
        assert_eq!(exact.len(), MAX_FRAME_SIZE);
        assert!(super::WireFrame::decode_line(exact.as_bytes()).is_ok());

        let oversized = format!("{exact}x");
        assert!(matches!(
            super::WireFrame::decode_line(oversized.as_bytes()),
            Err(IpcError::FrameTooLarge { .. })
        ));

        // 经分帧器读取时（带换行）也必须接受恰好到上限的帧。
        let mut line = exact.into_bytes();
        line.push(b'\n');
        assert!(read_ndjson_frame(&mut std::io::BufReader::new(Cursor::new(line))).is_ok());
    }

    #[test]
    fn ndjson_reader_accepts_one_line_and_rejects_truncated_eof() {
        use super::{read_ndjson_frame, WireFrame};
        use std::io::BufReader;

        let frame = WireFrame::response("req-1", json!({ "started": true }));
        let mut encoded = frame.encode_line().unwrap();
        assert_eq!(read_ndjson_frame(&mut BufReader::new(Cursor::new(&encoded))).unwrap(), frame);
        encoded.pop();
        assert!(read_ndjson_frame(&mut BufReader::new(Cursor::new(encoded))).is_err());
    }

    #[test]
    fn ndjson_writer_rejects_frame_above_hard_limit() {
        use super::{write_ndjson_frame, IpcError};

        let oversized = vec![b'x'; MAX_FRAME_SIZE + 1];
        let mut output = Vec::new();
        assert!(matches!(
            write_ndjson_frame(&mut output, &oversized),
            Err(IpcError::FrameTooLarge { .. })
        ));
        assert!(output.is_empty());
    }

    #[test]
    fn response_envelope_requires_exactly_one_result_or_error() {
        use super::WireFrame;

        assert!(WireFrame::decode_line(
            br#"{"version":2,"kind":"response","id":"req-1","result":{},"error":{"code":"x","message":"y"}}"#
        ).is_err());
        assert!(WireFrame::decode_line(
            br#"{"version":2,"kind":"response","id":"req-1"}"#
        ).is_err());
    }

    #[test]
    fn ndjson_reader_rejects_frames_above_the_limit_before_json_parsing() {
        use std::io::BufReader;

        let oversized = vec![b'x'; MAX_FRAME_SIZE + 2];
        let error = read_ndjson_frame(&mut BufReader::new(Cursor::new(oversized))).unwrap_err();
        assert!(matches!(error, IpcError::FrameTooLarge { .. }));
    }

    #[test]
    fn frame_writer_rejects_payloads_above_the_limit() {
        let payload = vec![0; MAX_FRAME_SIZE + 1];
        let mut output = Vec::new();
        let error = write_frame(&mut output, &payload).unwrap_err();
        assert!(matches!(error, IpcError::FrameTooLarge { .. }));
        assert!(output.is_empty());
    }

    #[test]
    fn event_dispatch_is_not_a_request_response_method() {
        use super::{ModuleMethod, METHOD_EVENT_DISPATCH, METHOD_INTENT_HANDLE};

        // 事件推送是单向通知：一旦它被登记进方法表，helper 就会按"请求必须回帧"
        // 的路径给它写响应，而宿主没有任何人在等这一帧——那会打乱请求/响应配对。
        // 本测试把该设计约束钉在方法表上，防止后来者顺手把它加进去。
        assert_eq!(ModuleMethod::from_name(METHOD_EVENT_DISPATCH), None);
        assert_ne!(METHOD_EVENT_DISPATCH, METHOD_INTENT_HANDLE);
        assert_eq!(
            ModuleMethod::from_name(METHOD_INTENT_HANDLE),
            Some(ModuleMethod::IntentHandle)
        );
    }

    #[test]
    fn event_and_intent_dtos_round_trip_and_reject_unknown_fields() {
        use super::{EventDispatchRequest, IntentHandleRequest, IntentHandleResponse};

        let dispatch = EventDispatchRequest {
            event: "version.installed".to_owned(),
            payload: json!({ "slug": "1.21.0" }),
        };
        let decoded: EventDispatchRequest =
            serde_json::from_slice(&serde_json::to_vec(&dispatch).unwrap()).unwrap();
        assert_eq!(decoded, dispatch);

        let handle = IntentHandleRequest {
            intent: "game.list".to_owned(),
            payload: json!({ "installed_only": true }),
        };
        let decoded: IntentHandleRequest =
            serde_json::from_slice(&serde_json::to_vec(&handle).unwrap()).unwrap();
        assert_eq!(decoded, handle);

        let handled = IntentHandleResponse {
            result: json!({ "games": [] }),
        };
        let decoded: IntentHandleResponse =
            serde_json::from_slice(&serde_json::to_vec(&handled).unwrap()).unwrap();
        assert_eq!(decoded, handled);

        // 多余字段必须拒绝：拼错字段名不能变成静默成功。
        assert!(serde_json::from_value::<EventDispatchRequest>(
            json!({ "event": "a.b", "payload": null, "extra": 1 })
        )
        .is_err());
        assert!(serde_json::from_value::<IntentHandleRequest>(
            json!({ "intent": "a.b", "payload": null, "module_id": "forged" })
        )
        .is_err());
    }

    #[test]
    fn agent_method_names_are_exactly_the_eight_agent_namespaced_methods() {
        use super::{
            AGENT_METHODS, METHOD_AGENT_CANCEL, METHOD_AGENT_CREDENTIALS_CLEAR,
            METHOD_AGENT_CREDENTIALS_SET, METHOD_AGENT_MODEL_SET, METHOD_AGENT_PROMPT,
            METHOD_AGENT_PING, METHOD_AGENT_SESSION_LOAD, METHOD_AGENT_SHUTDOWN,
        };

        // 顺序即 host hello 的能力声明顺序：这里钉死完整集合与顺序，防止后来者
        // 增删常量时悄悄改变对端看到的 capabilities。
        let expected = [
            METHOD_AGENT_CREDENTIALS_SET,
            METHOD_AGENT_CREDENTIALS_CLEAR,
            METHOD_AGENT_MODEL_SET,
            METHOD_AGENT_SESSION_LOAD,
            METHOD_AGENT_PROMPT,
            METHOD_AGENT_CANCEL,
            METHOD_AGENT_PING,
            METHOD_AGENT_SHUTDOWN,
        ];
        assert_eq!(AGENT_METHODS, &expected[..]);
        assert!(
            AGENT_METHODS.iter().all(|method| method.starts_with("agent.")),
            "Agent 方法必须全部位于 agent. 命名空间"
        );
    }
}
