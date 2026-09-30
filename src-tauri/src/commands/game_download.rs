//! 游戏下载模块命令：清单 / 下载 / APK 导入。

use std::path::PathBuf;
use tauri::State;
use crate::commands::into_command_error;
use crate::error::CommandResult;
use crate::modules::game_download::{apk, installer::{self, Ctx}};
use crate::modules::home::meta::{self, AndroidVersionMeta, VersionMeta};
use crate::modules::game_download::manifest::{self, ManifestView};
use crate::state::KernelContext;

#[tauri::command]
pub async fn game_download_manifest(kernel: State<'_, KernelContext>, refresh: Option<bool>) -> CommandResult<ManifestView> {
    let ctx = Ctx::from_kernel(&kernel);
    let versions = manifest::load_manifest(&ctx, refresh.unwrap_or(false)).await.map_err(into_command_error)?;
    Ok(manifest::build_view(&ctx, &versions))
}
#[tauri::command]
pub async fn game_download_detail(kernel: State<'_, KernelContext>, id: String) -> CommandResult<Option<installer::TaskView>> { installer::status(&Ctx::from_kernel(&kernel), &id).map_err(into_command_error) }
#[tauri::command]
pub async fn game_download_enqueue(kernel: State<'_, KernelContext>, id: String) -> CommandResult<u64> { installer::enqueue(&Ctx::from_kernel(&kernel), &id).await.map_err(into_command_error) }
#[tauri::command]
pub async fn game_download_refresh_source(kernel: State<'_, KernelContext>) -> CommandResult<()> { installer::refresh_source(&Ctx::from_kernel(&kernel)).await.map_err(into_command_error) }
#[tauri::command]
pub async fn game_download_cancel(kernel: State<'_, KernelContext>, id: String) -> CommandResult<()> { installer::cancel(&Ctx::from_kernel(&kernel), &id).map_err(into_command_error) }
#[tauri::command]
pub async fn game_download_status(kernel: State<'_, KernelContext>, id: String) -> CommandResult<Option<installer::TaskView>> { installer::status(&Ctx::from_kernel(&kernel), &id).map_err(into_command_error) }

/// Import an APK/APKS already copied into the app sandbox by the Android file picker.
#[tauri::command]
pub async fn game_download_import_apk(
    kernel: State<'_, KernelContext>, source_path: String, name: String,
    package_name: String, version_name: String, version_code: u64,
) -> CommandResult<apk::ApkPackageInfo> {
    let ctx = Ctx::from_kernel(&kernel);
    let root = ctx.versions_root();
    crate::modules::home::meta::validate_version_name(&root, &name).map_err(into_command_error)?;
    let staging = root.join(".import").join(format!("{}-{}", name.replace(['/', '\\'], "_"), std::process::id()));
    let result = apk::import_file(&PathBuf::from(source_path), &staging, &package_name, &version_name, version_code).map_err(into_command_error);
    if let Ok(ref info) = result {
        let instance_dir = root.join(&name);
        if instance_dir.exists() {
            return Err(crate::commands::into_command_error(crate::error::KernelError::Conflict(format!("实例 `{name}` 已存在，请先删除或重命名"))));
        }
        std::fs::rename(&staging, &instance_dir).map_err(|e| crate::commands::into_command_error(crate::error::KernelError::Io(e)))?;
        let meta = VersionMeta { name: name.clone(), game_version: version_name, version_type: "release".into(), enable_isolation: true, created_at: meta::now_rfc3339(), android: Some(AndroidVersionMeta { package_name: info.package_name.clone(), version_code: info.version_code, abi: info.abi.clone(), package_dir: name.clone(), lib_cache_dir: format!("runtime_libs/{}", name) }), ..Default::default() };
        VersionMeta::write(&instance_dir, &meta).map_err(into_command_error)?;
    } else { let _ = std::fs::remove_dir_all(&staging); }
    if result.is_ok() {
        kernel.events().publish("version.installed", serde_json::json!({ "name": name, "platform": "android" }));
    }
    result
}
