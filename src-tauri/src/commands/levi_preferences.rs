//! On-demand installed Levi preferences; never scans instances.
use tauri::State;
use crate::{error::CommandResult,state::KernelContext,services::levi_preferences};
#[tauri::command]
pub async fn levi_preferences_read()->Result<levi_preferences::Preferences,String>{tokio::task::spawn_blocking(levi_preferences::read).await.map_err(|e|format!("Levi preference reader failed: {e}"))}
#[tauri::command]
pub async fn levi_preferences_select_versions(kernel:State<'_,KernelContext>,versions_directory:String)->CommandResult<serde_json::Value>{
 if std::env::var_os("COPPER_VERSIONS_DIR").is_some_and(|s|!s.is_empty()){return Err(super::into_command_error(crate::error::KernelError::Config("COPPER_VERSIONS_DIR overrides the selected directory; remove it first".into())));}
 let preferences=tokio::task::spawn_blocking(levi_preferences::read).await.map_err(|e|super::into_command_error(crate::error::KernelError::Config(e.to_string())))?;
 if !preferences.installations.iter().any(|i|i.versions_directory==versions_directory){return Err(super::into_command_error(crate::error::KernelError::Config("The selected Levi versions directory is no longer available".into())));}
 let entries=std::collections::HashMap::from([("game.directory".into(),serde_json::json!(versions_directory)),("game.versions_selection".into(),serde_json::json!(versions_directory))]);
 kernel.settings().set_many(&entries).map_err(super::into_command_error)?;
 Ok(serde_json::json!({"versionsDirectory":versions_directory,"moved":false,"restartRequired":false}))
}
