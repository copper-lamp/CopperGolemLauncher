//! 游戏运行库依赖探测与安装命令。
use tauri::State;
use crate::{error::CommandResult, state::KernelContext};
use super::into_command_error;
fn game_paths(kernel:&KernelContext,name:&str)->Result<(std::path::PathBuf,std::path::PathBuf),crate::error::KernelError>{let dir=crate::modules::home::meta::resolve_version_dir(&kernel.versions_root(),name)?;let exe=if crate::modules::home::compatibility::is_automatic(&dir)&&crate::modules::home::compatibility::read(&dir)?.is_none(){dir.join("Minecraft.Windows.exe.orig")}else{crate::modules::home::launch::game_exe_path(&dir)};let base=crate::modules::home::compatibility::read(&dir)?.map(|b|b.source_dir).unwrap_or(dir);Ok((base,exe))}
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
