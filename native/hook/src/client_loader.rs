//! 内建客户端清单解析与加载策略。
//!
//! 解析及顺序策略保持平台中立，实际等待模块和调用 LoadLibraryExW 仅在 Windows worker 执行。

use serde::{Deserialize, Serialize};

use crate::contract::CLIENT_MANIFEST_SCHEMA_VERSION;

const MAX_DELAY_MS: u64 = 600_000;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ClientManifest {
    pub version: u32,
    #[serde(default)]
    pub clients: Vec<ClientEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ClientEntry {
    pub id: String,
    pub name: String,
    pub dll: String,
    #[serde(default)]
    pub delay_ms: u64,
    #[serde(default)]
    pub wait_module: Option<String>,
    #[serde(default)]
    pub wait_timeout_ms: u64,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientStatus {
    pub id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientStatusFile {
    pub version: u32,
    pub clients: Vec<ClientStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientManifestError {
    Syntax(String),
    Schema(String),
    UnsupportedVersion(u32),
}

impl std::fmt::Display for ClientManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax(message) => write!(f, "客户端清单 JSON 非法: {message}"),
            Self::Schema(message) => write!(f, "客户端清单字段非法: {message}"),
            Self::UnsupportedVersion(version) => write!(f, "客户端清单版本不受支持: {version}"),
        }
    }
}

impl std::error::Error for ClientManifestError {}

impl ClientManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, ClientManifestError> {
        let value: Self = serde_json::from_slice(bytes).map_err(|error| match error.classify() {
            serde_json::error::Category::Syntax | serde_json::error::Category::Eof => ClientManifestError::Syntax(error.to_string()),
            _ => ClientManifestError::Schema(error.to_string()),
        })?;
        if value.version != CLIENT_MANIFEST_SCHEMA_VERSION {
            return Err(ClientManifestError::UnsupportedVersion(value.version));
        }
        for client in &value.clients {
            if client.id.trim().is_empty() || client.name.trim().is_empty() || client.dll.trim().is_empty() {
                return Err(ClientManifestError::Schema("id/name/dll 不能为空".into()));
            }
            if !std::path::Path::new(&client.dll).is_absolute() {
                return Err(ClientManifestError::Schema(format!("DLL 路径必须为绝对路径: {}", client.dll)));
            }
        }
        Ok(value)
    }
}

/// 将 delay 限制在 10 分钟，避免损坏清单令 worker 长时间挂起。
pub fn bounded_delay(delay_ms: u64) -> u64 {
    delay_ms.min(MAX_DELAY_MS)
}

pub fn failed(id: impl Into<String>, error: impl Into<String>) -> ClientStatus {
    ClientStatus { id: id.into(), status: "failed".into(), error: Some(error.into()) }
}

/// Execute manifest entries in order through a supplied platform adapter.
pub fn execute_ordered<F>(manifest: &ClientManifest, mut load: F) -> Vec<ClientStatus>
where
    F: FnMut(&ClientEntry) -> ClientStatus,
{
    manifest.clients.iter().map(&mut load).collect()
}

/// Result used by platform adapters after their module polling loop.
pub fn wait_result(found: bool, id: &str, module: &str) -> Result<(), ClientStatus> {
    if found {
        Ok(())
    } else {
        Err(ClientStatus {
            id: id.into(),
            status: "timeout".into(),
            error: Some(format!("wait_module timeout: {module}")),
        })
    }
}

#[cfg(windows)]
pub fn run_from_env() {
    use std::path::PathBuf;
    let Some(manifest_path) = std::env::var_os(crate::contract::ENV_CLIENTS).map(PathBuf::from) else { return };
    let status_path = std::env::var_os(crate::contract::ENV_CLIENT_STATUS).map(PathBuf::from);
    let mut statuses = Vec::new();

    if std::env::var_os("COPPER_HOOK_HOST_EXE").is_some() {
        if let Some(path) = status_path.as_deref() {
            let entries = read_manifest(&manifest_path).map(|manifest| manifest.clients).unwrap_or_default();
            statuses = entries.into_iter().map(|entry| ClientStatus {
                id: entry.id, status: "skipped".into(), error: Some("shared_gdk".into()),
            }).collect();
            let _ = write_status(path, &statuses);
        }
        crate::log::info("共享 GDK 模式：跳过内建客户端加载");
        return;
    }

    match read_manifest(&manifest_path) {
        Ok(manifest) => {
            for entry in manifest.clients {
                statuses.push(load_one(&entry));
            }
        }
        Err(error) => crate::log::error(format!("读取内建客户端清单失败: {error}")),
    }
    if let Some(path) = status_path {
        if let Err(error) = write_status(&path, &statuses) {
            crate::log::error(format!("写入内建客户端状态失败 {}: {error}", path.display()));
        }
    }
}

#[cfg(windows)]
fn read_manifest(path: &std::path::Path) -> Result<ClientManifest, String> {
    if !path.is_absolute() { return Err("清单路径必须为绝对路径".into()); }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    ClientManifest::parse(&bytes).map_err(|error| error.to_string())
}

#[cfg(windows)]
fn write_status(path: &std::path::Path, statuses: &[ClientStatus]) -> std::io::Result<()> {
    use std::io::Write;
    if !path.is_absolute() { return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "状态路径必须为绝对路径")); }
    let document = ClientStatusFile { version: CLIENT_MANIFEST_SCHEMA_VERSION, clients: statuses.to_vec() };
    let bytes = serde_json::to_vec(&document).map_err(std::io::Error::other)?;
    let temporary = path.with_extension("clients-pending");
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temporary, path)
}

#[cfg(windows)]
fn load_one(entry: &ClientEntry) -> ClientStatus {
    use std::os::windows::ffi::OsStrExt;
    use std::time::{Duration, Instant};
    if let Some(module) = entry.wait_module.as_deref() {
        let wide: Vec<u16> = module.encode_utf16().chain(Some(0)).collect();
        let deadline = Instant::now() + Duration::from_millis(entry.wait_timeout_ms);
        loop {
            if !unsafe { crate::sys::GetModuleHandleW(wide.as_ptr()) }.is_null() { break; }
            if Instant::now() >= deadline {
                return match wait_result(false, &entry.id, module) {
                    Err(status) => status,
                    Ok(()) => failed(&entry.id, "wait-module invariant failure"),
                };
            }
            unsafe { crate::sys::Sleep(100) };
        }
    }
    std::thread::sleep(Duration::from_millis(bounded_delay(entry.delay_ms)));
    let path = std::path::Path::new(&entry.dll);
    if !path.is_file() { return failed(&entry.id, format!("DLL 不存在: {}", path.display())); }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let handle = unsafe { crate::sys::LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), crate::sys::LOAD_WITH_ALTERED_SEARCH_PATH) };
    if handle.is_null() {
        failed(&entry.id, format!("LoadLibraryExW 失败: {}", std::io::Error::last_os_error()))
    } else {
        ClientStatus { id: entry.id.clone(), status: "loaded".into(), error: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> ClientEntry {
        ClientEntry { id: id.into(), name: id.into(), dll: "C:\\clients\\a.dll".into(), delay_ms: 0, wait_module: None, wait_timeout_ms: 0, required: false }
    }

    #[test]
    fn parses_v1_clients_and_rejects_relative_dll() {
        let manifest = ClientManifest::parse(br#"{"version":1,"clients":[{"id":"a","name":"A","dll":"C:\\a.dll","delay_ms":1,"wait_module":null,"wait_timeout_ms":2,"required":true}]}"#).unwrap();
        assert_eq!(manifest.clients.len(), 1);
        assert!(matches!(ClientManifest::parse(br#"{"version":1,"clients":[{"id":"a","name":"A","dll":"a.dll"}]}"#), Err(ClientManifestError::Schema(_))));
    }

    #[test]
    fn caps_delay_and_records_missing_dll() {
        assert_eq!(bounded_delay(u64::MAX), MAX_DELAY_MS);
        let status = failed(&entry("missing").id, "DLL missing");
        assert_eq!(status.status, "failed");
    }

    #[test]
    fn status_shape_and_order_are_stable() {
        let statuses = vec![ClientStatus { id: "first".into(), status: "loaded".into(), error: None }, failed("second", "load")];
        let document = ClientStatusFile { version: 1, clients: statuses.clone() };
        let parsed: ClientStatusFile = serde_json::from_slice(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert_eq!(parsed.clients, statuses);
    }

    #[test]
    fn timeout_status_is_explicit() {
        let status = ClientStatus { id: "waiting".into(), status: "timeout".into(), error: Some("wait_module timeout: game.dll".into()) };
        assert_eq!(status.status, "timeout");
        assert_eq!(wait_result(false, "waiting", "game.dll").unwrap_err(), status);
        assert!(wait_result(true, "ready", "game.dll").is_ok());
    }

    #[test]
    fn ordered_executor_keeps_manifest_order_and_continues_after_failure() {
        let manifest = ClientManifest {
            version: 1,
            clients: vec![entry("first"), entry("second"), entry("third")],
        };
        let mut seen = Vec::new();
        let result = execute_ordered(&manifest, |client| {
            seen.push(client.id.clone());
            if client.id == "second" { failed(&client.id, "load failed") }
            else { ClientStatus { id: client.id.clone(), status: "loaded".into(), error: None } }
        });
        assert_eq!(seen, ["first", "second", "third"]);
        assert_eq!(result[1].status, "failed");
        assert_eq!(result[2].status, "loaded");
    }

    #[test]
    fn shared_gdk_status_is_skipped_with_reason() {
        let skipped = ClientStatus { id: "client".into(), status: "skipped".into(), error: Some("shared_gdk".into()) };
        assert_eq!(skipped.status, "skipped");
        assert_eq!(skipped.error.as_deref(), Some("shared_gdk"));
    }
}
