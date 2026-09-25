use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;

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

fn deserialize_capability_params<'de, D>(deserializer: D) -> Result<Value, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let params = Value::deserialize(deserializer)?;
    if contains_module_id(&params) {
        return Err(serde::de::Error::custom(
            "capability params must not contain module_id",
        ));
    }
    Ok(params)
}

fn contains_module_id(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key("module_id") || object.values().any(contains_module_id)
        }
        Value::Array(values) => values.iter().any(contains_module_id),
        _ => false,
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleMethod {
    Initialize,
    Start,
    Stop,
    Health,
    Invoke,
}

impl ModuleMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initialize => METHOD_MODULE_INITIALIZE,
            Self::Start => METHOD_MODULE_START,
            Self::Stop => METHOD_MODULE_STOP,
            Self::Health => METHOD_MODULE_HEALTH,
            Self::Invoke => METHOD_MODULE_INVOKE,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            METHOD_MODULE_INITIALIZE => Some(Self::Initialize),
            METHOD_MODULE_START => Some(Self::Start),
            METHOD_MODULE_STOP => Some(Self::Stop),
            METHOD_MODULE_HEALTH => Some(Self::Health),
            METHOD_MODULE_INVOKE => Some(Self::Invoke),
            _ => None,
        }
    }

    pub fn decode_request(self, params: Value) -> Result<ModuleRequest, serde_json::Error> {
        match self {
            Self::Initialize => serde_json::from_value(params).map(ModuleRequest::Initialize),
            Self::Start | Self::Stop => serde_json::from_value(params).map(ModuleRequest::Lifecycle),
            Self::Health => serde_json::from_value(params).map(ModuleRequest::Health),
            Self::Invoke => serde_json::from_value(params).map(ModuleRequest::Invoke),
        }
    }

    pub fn decode_response(self, result: Value) -> Result<ModuleResponse, serde_json::Error> {
        match self {
            Self::Initialize => serde_json::from_value(result).map(ModuleResponse::Initialize),
            Self::Start | Self::Stop => serde_json::from_value(result).map(ModuleResponse::Lifecycle),
            Self::Health => serde_json::from_value(result).map(ModuleResponse::Health),
            Self::Invoke => serde_json::from_value(result).map(ModuleResponse::Invoke),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModuleRequest {
    Initialize(InitializeRequest),
    Lifecycle(LifecycleRequest),
    Health(HealthRequest),
    Invoke(InvokeRequest),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModuleResponse {
    Initialize(InitializeResponse),
    Lifecycle(LifecycleResponse),
    Health(HealthResponse),
    Invoke(InvokeResponse),
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

pub fn read_frame<R: Read>(reader: &mut R) -> Result<Vec<u8>, IpcError> {
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let size = u32::from_be_bytes(length) as usize;
    if size > MAX_FRAME_SIZE {
        return Err(IpcError::FrameTooLarge {
            size,
            max: MAX_FRAME_SIZE,
        });
    }

    let mut payload = vec![0; size];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

pub fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), IpcError> {
    if payload.len() > MAX_FRAME_SIZE {
        return Err(IpcError::FrameTooLarge {
            size: payload.len(),
            max: MAX_FRAME_SIZE,
        });
    }
    let size = u32::try_from(payload.len()).map_err(|_| IpcError::FrameTooLarge {
        size: payload.len(),
        max: MAX_FRAME_SIZE,
    })?;
    writer.write_all(&size.to_be_bytes())?;
    writer.write_all(payload)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        negotiate_handshake, read_frame, validate_response_correlation, validate_version,
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
            ModuleMethod, ModuleRequest, ModuleResponse, METHOD_MODULE_HEALTH,
            METHOD_MODULE_INITIALIZE, METHOD_MODULE_INVOKE, METHOD_MODULE_START,
            METHOD_MODULE_STOP,
        };

        let cases = [
            (ModuleMethod::Initialize, METHOD_MODULE_INITIALIZE),
            (ModuleMethod::Start, METHOD_MODULE_START),
            (ModuleMethod::Stop, METHOD_MODULE_STOP),
            (ModuleMethod::Health, METHOD_MODULE_HEALTH),
            (ModuleMethod::Invoke, METHOD_MODULE_INVOKE),
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
            "params": { "options": [{ "module_id": "attacker-module" }] }
        });
        assert!(serde_json::from_value::<CapabilityRequest>(nested_module_id).is_err());

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
    fn frame_round_trip_writes_u32_big_endian_length() {
        let payload = br#"{"ok":true}"#;
        let mut frame = Vec::new();
        write_frame(&mut frame, payload).unwrap();
        assert_eq!(&frame[..4], &(payload.len() as u32).to_be_bytes());
        assert_eq!(read_frame(&mut Cursor::new(frame)).unwrap(), payload);
    }

    #[test]
    fn frame_reader_rejects_lengths_above_the_limit_before_reading_body() {
        let oversized = (MAX_FRAME_SIZE as u32 + 1).to_be_bytes();
        let error = read_frame(&mut Cursor::new(oversized)).unwrap_err();
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
}
