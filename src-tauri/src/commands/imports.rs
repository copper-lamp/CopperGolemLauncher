//! User-confirmed import tickets. External paths never reach the commit command.
use std::{collections::HashMap, path::{Path, PathBuf}, time::{Duration, Instant}};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{Manager, State};
use crate::{error::{CommandResult, KernelError}, state::KernelContext};
use super::into_command_error;
use crate::modules::content_download::install_target::{self, ExtKind};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview { pub ticket: String, pub filename: String, pub kind: String, pub name: String, pub version: Option<String>, pub permissions: Vec<String>, pub module_id: Option<String>, pub publisher_verified: bool, pub installed_version: Option<String>, pub sha256: String }
#[derive(Clone)]
struct Ticket { path: PathBuf, work: PathBuf, created: Instant, preview: ImportPreview }
#[derive(Default)]
pub struct ImportCoordinator { tickets: Mutex<HashMap<String, Ticket>>, pending: Mutex<Vec<PathBuf>> }
impl ImportCoordinator {
    pub fn receive(&self, paths: impl IntoIterator<Item=PathBuf>) { let mut pending = self.pending.lock(); for path in paths { if pending.len() >= 32 { break; } if accepted(&path) { pending.push(path); } } }
}
fn accepted(path: &Path) -> bool { path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "mcpack"|"mcaddon"|"addon"|"mcworld"|"mctemplate"|"zip"|"cglm")) }
fn inspect(path: &Path, cache: &Path) -> Result<Ticket, KernelError> {
    use std::io::{Read, Write};
    if !accepted(path) || std::fs::symlink_metadata(path)?.file_type().is_symlink() { return Err(KernelError::InvalidArgument("Unsupported or linked import source".into())); }
    let work = cache.join(format!("import-{}", uuid::Uuid::new_v4())); std::fs::create_dir_all(&work)?;
    let result = (|| {
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("resource").to_owned();
        let snapshot = work.join(format!("snapshot.{}", path.extension().unwrap().to_string_lossy()));
        let mut input = std::fs::File::open(path)?;
        if !input.metadata()?.is_file() || input.metadata()?.len() > 512 * 1024 * 1024 { return Err(KernelError::InvalidArgument("Import must be a regular file up to 512 MiB".into())); }
        let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(&snapshot)?;
        if std::io::copy(&mut Read::by_ref(&mut input).take(512 * 1024 * 1024 + 1), &mut output)? > 512 * 1024 * 1024 { return Err(KernelError::InvalidArgument("Import exceeds size limit".into())); }
        output.flush()?; output.sync_all()?;
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        let mut file = std::fs::File::open(&snapshot)?;
        let mut buffer = [0u8; 65536];
        loop { let count = file.read(&mut buffer)?; if count == 0 { break; } digest.update(&buffer[..count]); }
        let sha256 = format!("{:x}", digest.finalize());
        let ticket = uuid::Uuid::new_v4().to_string();
        let mut preview = ImportPreview { ticket, filename: filename.clone(), kind: String::new(), name: install_target::dir_name_hint(&filename), version: None, permissions: vec![], module_id: None, publisher_verified: false, installed_version: None, sha256 };
        if snapshot.extension().is_some_and(|e| e.eq_ignore_ascii_case("cglm")) {
            let package = crate::registry::package::extract_package(&snapshot, &work.join("inspect"))?;
            let m = package.manifest; m.validate()?;
            if !m.supports_current_platform() || !m.accepts_launcher(env!("CARGO_PKG_VERSION")) { return Err(KernelError::InvalidArgument("Module platform or launcher version is incompatible".into())); }
            preview.kind = "module".into(); preview.name = m.display_name; preview.version = Some(m.version); preview.permissions = m.permissions; preview.module_id = Some(m.id);
        } else {
            let extracted = work.join("inspect"); std::fs::create_dir(&extracted)?;
            install_target::extract_archive(&snapshot, &extracted)?;
            let ext = install_target::classify(&filename).ok_or_else(|| KernelError::InvalidArgument("Unsupported import".into()))?;
            preview.kind = match ext { ExtKind::World => "world", ExtKind::Template => "template", ExtKind::Addon => "addon", ExtKind::Pack => "pack", ExtKind::Zip => {
                if extracted.join("level.dat").is_file() { "world" } else {
                    let manifests = count_manifests(&extracted, 0)?;
                    if manifests == 1 { "pack" } else if manifests > 1 { "addon" } else { return Err(KernelError::InvalidArgument("Zip does not uniquely identify a supported resource".into())); }
                }
            }, _ => return Err(KernelError::InvalidArgument("Unsupported import".into())) }.into();
            if preview.kind == "world" && !extracted.join("level.dat").is_file() { return Err(KernelError::InvalidArgument("World archive has no level.dat".into())); }
            if matches!(preview.kind.as_str(), "pack"|"addon"|"template") && count_manifests(&extracted, 0)? == 0 { return Err(KernelError::InvalidArgument("Resource has no manifest".into())); }
        }
        Ok(Ticket { path: snapshot, work: work.clone(), created: Instant::now(), preview })
    })();
    if result.is_err() { let _ = std::fs::remove_dir_all(&work); } result
}
fn count_manifests(root: &Path, depth: usize) -> Result<usize, KernelError> {
    if root.join("manifest.json").is_file() { return Ok(1); }
    if depth >= 4 { return Ok(0); } let mut count = 0;
    for entry in std::fs::read_dir(root)? { let entry = entry?; if entry.file_type()?.is_dir() { count += count_manifests(&entry.path(), depth + 1)?; } } Ok(count)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn inspected_snapshot_survives_source_change_and_unknown_zip_is_rejected() {
        let root = std::env::temp_dir().join(format!("import-ticket-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("pack.mcpack");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        zip.start_file("manifest.json", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(br#"{"header":{"name":"Example"},"modules":[{"type":"resources"}]}"#).unwrap(); zip.finish().unwrap();
        let item = inspect(&archive, &root).unwrap();
        let snapshot = std::fs::read(&item.path).unwrap();
        std::fs::write(&archive, b"changed externally").unwrap();
        assert_eq!(std::fs::read(&item.path).unwrap(), snapshot);
        assert_eq!(item.preview.kind, "pack"); assert_eq!(item.preview.sha256.len(), 64);
        let unknown = root.join("unknown.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&unknown).unwrap());
        zip.start_file("readme.txt", zip::write::SimpleFileOptions::default()).unwrap(); zip.write_all(b"not a resource").unwrap(); zip.finish().unwrap();
        assert!(inspect(&unknown, &root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

fn enqueue(app: &tauri::AppHandle, kernel: &KernelContext, paths: Vec<PathBuf>) -> CommandResult<Vec<ImportPreview>> {
    let state = app.state::<ImportCoordinator>(); let mut previews = vec![];
    {
        let mut tickets = state.tickets.lock();
        tickets.retain(|_, ticket| { if ticket.created.elapsed() > Duration::from_secs(1800) { let _ = std::fs::remove_dir_all(&ticket.work); false } else { true } });
        if tickets.len() + paths.len().min(32) > 32 { return Err(into_command_error(KernelError::Conflict("Import queue is full; finish or cancel pending imports".into()))); }
    }
    for path in paths.into_iter().take(32) {
        let mut ticket = match inspect(&path, &kernel.paths().cache_dir()) {
            Ok(ticket) => ticket,
            Err(error) => {
                for preview in &previews {
                    let preview: &ImportPreview = preview;
                    if let Some(ticket) = state.tickets.lock().remove(&preview.ticket) { let _ = std::fs::remove_dir_all(ticket.work); }
                }
                return Err(into_command_error(error));
            }
        };
        if let Some(id) = &ticket.preview.module_id {
            ticket.preview.installed_version = std::fs::read(kernel.paths().modules_dir().join(id).join("module.json")).ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value.get("version").and_then(|v| v.as_str()).map(str::to_owned));
        }
        previews.push(ticket.preview.clone()); state.tickets.lock().insert(ticket.preview.ticket.clone(), ticket);
    } Ok(previews)
}
#[tauri::command]
pub fn imports_pick(app: tauri::AppHandle, kernel: State<'_, KernelContext>) -> CommandResult<Vec<ImportPreview>> {
    #[cfg(target_os = "android")]
    {
        use tauri_plugin_opener::OpenerExt;
        app.opener().open_url("coppergolem://import-resource", None::<&str>)
            .map_err(|_| into_command_error(KernelError::InvalidArgument("Unable to open Android file picker".into())))?;
        return Ok(Vec::new());
    }
    #[cfg(not(target_os = "android"))]
    {
    use tauri_plugin_dialog::DialogExt;
    let paths = app.dialog().file().add_filter("Minecraft / Copper", &["mcpack","mcaddon","addon","mcworld","mctemplate","zip","cglm"]).blocking_pick_files().unwrap_or_default();
    let paths = paths.into_iter().map(|p| p.into_path().map_err(|_| into_command_error(KernelError::InvalidArgument("Invalid file selection".into())))).collect::<CommandResult<Vec<_>>>()?;
    enqueue(&app, kernel.inner(), paths)
    }
}
#[tauri::command]
pub fn imports_drop(app: tauri::AppHandle, kernel: State<'_, KernelContext>, paths: Vec<String>) -> CommandResult<Vec<ImportPreview>> {
    enqueue(&app, kernel.inner(), paths.into_iter().take(32).map(PathBuf::from).collect())
}

#[tauri::command]
pub fn imports_pending(app: tauri::AppHandle, kernel: State<'_, KernelContext>) -> CommandResult<Vec<ImportPreview>> {
    let paths = std::mem::take(&mut *app.state::<ImportCoordinator>().pending.lock());
    #[cfg(target_os = "android")]
    let paths = {
        let mut paths = paths;
        let inbox = kernel.paths().cache_dir().join("global-import");
        if let Ok(entries) = std::fs::read_dir(&inbox) {
            for entry in entries.flatten().take(32) {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "error") {
                    let _ = std::fs::remove_file(&path);
                    return Err(into_command_error(KernelError::InvalidArgument("Android resource copy failed; select the file again".into())));
                }
                if accepted(&path) { paths.push(path); }
            }
        }
        paths
    };
    let result = enqueue(&app, kernel.inner(), paths.clone());
    #[cfg(target_os = "android")]
    for path in paths { if path.starts_with(kernel.paths().cache_dir().join("global-import")) { let _ = std::fs::remove_file(path); } }
    result
}
#[tauri::command]
pub fn imports_cancel(app: tauri::AppHandle, ticket: String) {
    if let Some(item) = app.state::<ImportCoordinator>().tickets.lock().remove(&ticket) { let _ = std::fs::remove_dir_all(item.work); }
}
#[tauri::command]
pub fn imports_commit(app: tauri::AppHandle, kernel: State<'_, KernelContext>, ticket: String, instance: Option<String>, replace: bool, acknowledge_risk: bool) -> CommandResult<()> {
    let state = app.state::<ImportCoordinator>();
    let mut tickets = state.tickets.lock();
    let item = tickets.get(&ticket).cloned().ok_or_else(|| into_command_error(KernelError::InvalidArgument("Import ticket expired or already used".into())))?;
    let result = (|| {
        if item.created.elapsed() > Duration::from_secs(1800) { return Err(KernelError::InvalidArgument("Import ticket expired".into())); }
        use sha2::{Digest, Sha256};
        use std::io::Read;
        let mut hash = Sha256::new();
        let mut source = std::fs::File::open(&item.path)?;
        let mut buffer = [0u8; 65536];
        loop { let n = source.read(&mut buffer)?; if n == 0 { break; } hash.update(&buffer[..n]); }
        if format!("{:x}", hash.finalize()) != item.preview.sha256 { return Err(KernelError::Conflict("Import snapshot changed; inspect the source again".into())); }
        if item.preview.kind == "module" {
            if !acknowledge_risk { return Err(KernelError::InvalidArgument("Module risk acknowledgement required".into())); }
            crate::registry::install::install_local_package(&item.path, kernel.paths().modules_dir(), |id| {
                if kernel.modules().is_registered(id) && (kernel.modules().origin_of(id).is_builtin() || kernel.modules().is_running(id)) { return Err(KernelError::Conflict("Disable the module and restart before replacing it".into())); }
                if kernel.paths().modules_dir().join(id).exists() && !replace { return Err(KernelError::Conflict("Explicit module replacement confirmation required".into())); } Ok(())
            })?;
        } else {
            let instance = instance.ok_or_else(|| KernelError::InvalidArgument("Select an instance".into()))?;
            let roots = crate::modules::home::content::content_roots_for_write(kernel.inner(), &instance)?;
            let ext = match item.preview.kind.as_str() { "world" => ExtKind::World, "template" => ExtKind::Template, "addon" => ExtKind::Addon, _ => ExtKind::Pack };
            install_target::install_confirmed(&item.path, ext, &roots, &item.preview.name, replace)?;
        } Ok(())
    })();
    if result.is_ok() {
        kernel.events().publish("imports.completed", serde_json::json!({ "kind": item.preview.kind, "moduleId": item.preview.module_id }));
        if item.preview.kind == "module" { kernel.events().publish("modules.changed", serde_json::json!({})); }
        else { kernel.events().publish("home.content-changed", serde_json::json!({})); }
    }
    if result.is_ok() || item.created.elapsed() > Duration::from_secs(1800) {
        tickets.remove(&ticket);
        let _ = std::fs::remove_dir_all(item.work);
    }
    result.map_err(into_command_error)
}
