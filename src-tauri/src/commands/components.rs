//! 外部组件受控命令；来源与路径不接受前端输入。
use tauri::State;
use crate::{error::CommandResult, state::KernelContext, services::components::{ComponentId,ComponentStatus}};
use super::into_command_error;
fn game_paths(kernel:&KernelContext,name:&str)->Result<(std::path::PathBuf,std::path::PathBuf),crate::error::KernelError>{let dir=crate::modules::home::meta::resolve_version_dir(&kernel.versions_root(),name)?;let exe=crate::modules::home::launch::game_exe_path(&dir);Ok((dir,exe))}
#[tauri::command]
pub async fn game_dependencies_probe(kernel:State<'_,KernelContext>,name:String)->CommandResult<crate::services::system_runtime::SystemDependencyStatus>{
    let (dir,exe)=game_paths(&kernel,&name).map_err(into_command_error)?;
    crate::services::system_runtime::probe(&dir,&exe).await.map_err(into_command_error)
}
#[tauri::command]
pub async fn game_dependencies_install(kernel:State<'_,KernelContext>,name:String)->CommandResult<crate::services::system_runtime::SystemInstallOutcome>{
    let (dir,exe)=game_paths(&kernel,&name).map_err(into_command_error)?;
    if crate::modules::home::launch::is_process_running_at_path(&exe){return Err(into_command_error(crate::error::KernelError::InvalidArgument("game_dependency_instance_busy".into())));}
    let status=crate::services::system_runtime::probe(&dir,&exe).await.map_err(into_command_error)?;
    let cache=kernel.paths().cache_dir().join("system-installers");
    if !status.missing_vc_dlls.is_empty(){let result=crate::services::system_runtime::install_vc(&status,&cache).await.map_err(into_command_error)?;if !matches!(result.state.as_str(),"installed"|"already-installed"){return Ok(result);}}
    crate::services::system_runtime::install_packages(&status,&cache).await.map_err(into_command_error)
}
#[tauri::command]
pub fn components_list(kernel:State<'_,KernelContext>)->Vec<ComponentStatus>{kernel.components().snapshot()}
#[tauri::command]
pub async fn components_probe(kernel:State<'_,KernelContext>,id:ComponentId)->CommandResult<bool>{kernel.components().probe(id).await.map(|_|true).map_err(into_command_error)}
#[tauri::command]
pub async fn components_check(kernel:State<'_,KernelContext>,id:ComponentId)->CommandResult<ComponentStatus>{kernel.components().check(id).await.map_err(into_command_error)}
#[tauri::command]
pub async fn components_install(kernel:State<'_,KernelContext>,id:ComponentId)->CommandResult<()>{kernel.components().install(id).await.map_err(into_command_error)}
