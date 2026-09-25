use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;

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
    #[error("IPC JSON encoding or decoding failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IPC frame size {size} exceeds maximum {max}")]
    FrameTooLarge { size: usize, max: usize },
    #[error("unsupported IPC protocol version {received}; supported version is {supported}")]
    UnsupportedVersion { received: u32, supported: u32 },
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
        read_frame, validate_version, write_frame, IpcError, IpcErrorDto, IpcRequest, IpcResponse,
        MAX_FRAME_SIZE, PROTOCOL_VERSION,
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
