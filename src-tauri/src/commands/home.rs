//! 开始页模块命令：版本清单 / 设置 / 启动 / 内容管理 / 模组管理。
//!
//! 所有命令经内核上下文访问 home 模块业务逻辑，错误统一映射为 `CommandError`。

use tauri::State;

use crate::commands::into_command_error;
use crate::error::{CommandResult, KernelError};
use crate::modules::home::client;
use crate::modules::home::content;
use crate::modules::home::dashboard;
use crate::modules::home::controls;
use crate::modules::home::launch::LaunchOutcome;
use crate::modules::home::mods;
use crate::modules::home::{meta, OpenDirKind, VersionMetaUpdate, VersionView};
use crate::state::KernelContext;

#[tauri::command]
pub fn home_compat_discover()->Vec<String>{crate::modules::home::compatibility::discover()}
#[tauri::command]
pub async fn home_compat_probe(source_dir:String)->CommandResult<crate::modules::home::compatibility::CompatibilityProbe> { tauri::async_runtime::spawn_blocking(move ||crate::modules::home::compatibility::probe(std::path::Path::new(&source_dir))).await.map_err(|e|into_command_error(KernelError::Module(e.to_string()))) }
#[tauri::command]
pub async fn home_compat_bind(kernel:State<'_,KernelContext>,source_dir:String,alias:String)->CommandResult<VersionView>{
 let root=kernel.versions_root();let task_alias=alias.clone();
 tauri::async_runtime::spawn_blocking(move ||crate::modules::home::compatibility::bind(&root,std::path::Path::new(&source_dir),&task_alias)).await.map_err(|e|into_command_error(KernelError::Module(e.to_string())))?.map_err(into_command_error)?;
 kernel.events().publish("version.installed",serde_json::json!({"name":alias}));
 crate::modules::home::HomeModule::get_version(kernel.inner(),&alias).map_err(into_command_error)
}

/// 版本清单。
#[tauri::command]
pub fn home_versions_list(kernel: State<'_, KernelContext>) -> CommandResult<Vec<VersionView>> {
    Ok(crate::modules::home::HomeModule::list_versions(kernel.inner()))
}

/// 当前解析的版本根目录（供前端显示实际游戏目录）。
#[tauri::command]
pub fn home_versions_root(kernel: State<'_, KernelContext>) -> CommandResult<String> {
    Ok(kernel
        .inner()
        .versions_root()
        .to_string_lossy()
        .into_owned())
}

/// 单个版本信息。
#[tauri::command]
pub fn home_version_get(
    kernel: State<'_, KernelContext>,
    name: String,
) -> CommandResult<VersionView> {
    crate::modules::home::HomeModule::get_version(kernel.inner(), &name)
        .map_err(into_command_error)
}

/// 部分更新版本设置（渲染龙 / 世界编辑器 / 启动参数等）。
#[tauri::command]
pub fn home_version_save_meta(
    kernel: State<'_, KernelContext>,
    name: String,
    update: VersionMetaUpdate,
) -> CommandResult<VersionView> {
    crate::modules::home::HomeModule::save_meta(kernel.inner(), &name, &update)
        .map_err(into_command_error)
}

/// 重命名版本。
#[tauri::command]
pub fn home_version_rename(
    kernel: State<'_, KernelContext>,
    old_name: String,
    new_name: String,
) -> CommandResult<VersionView> {
    crate::modules::home::HomeModule::rename_version(kernel.inner(), &old_name, &new_name)
        .map_err(into_command_error)
}

/// 删除版本（成功后广播 `version.removed`）。
#[tauri::command]
pub fn home_version_delete(kernel: State<'_, KernelContext>, name: String) -> CommandResult<()> {
    crate::modules::home::HomeModule::delete_version(kernel.inner(), &name)
        .map_err(into_command_error)
}

/// 启动游戏（成功后后台确认进程并广播 `game.launched` / `game.exited`）。
#[tauri::command]
pub async fn home_launch(kernel: State<'_, KernelContext>, name: String) -> CommandResult<LaunchOutcome> {
    let ctx=crate::modules::home::launch::LaunchCtx::from_kernel(kernel.inner());
    tauri::async_runtime::spawn_blocking(move ||crate::modules::home::launch::launch_game(&ctx,&name,true)).await.map_err(|e|into_command_error(KernelError::Module(e.to_string())))?.map_err(into_command_error)
}

/// 该版本的游戏进程是否在运行（开始页「启动 / 退出」按钮形态的依据）。
#[tauri::command]
pub fn home_game_running(kernel: State<'_, KernelContext>, name: String) -> CommandResult<bool> {
    crate::modules::home::HomeModule::game_running(kernel.inner(), &name)
        .map_err(into_command_error)
}

/// 结束该版本运行中的游戏进程，返回结束的进程数。
///
/// 不在此处广播 `game.exited`：状态只由进程监控广播，前端因此永远以
/// 真实进程状态为准，不会出现「按钮显示已退出但游戏还在跑」。
#[tauri::command]
pub fn home_game_kill(kernel: State<'_, KernelContext>, name: String) -> CommandResult<usize> {
    crate::modules::home::HomeModule::kill_game(kernel.inner(), &name)
        .map_err(into_command_error)
}

/// 启动文件状态：hook DLL 是否落位、导入表是否已改写、原始备份是否存在。
#[tauri::command]
pub fn home_launch_file_state(
    kernel: State<'_, KernelContext>,
    name: String,
) -> CommandResult<crate::modules::home::inject::LaunchFileState> {
    crate::modules::home::HomeModule::launch_file_state(kernel.inner(), &name)
        .map_err(into_command_error)
}

/// 还原为未注入的原始启动文件。
#[tauri::command]
pub fn home_launch_file_restore(kernel: State<'_, KernelContext>, name: String) -> CommandResult<()> {
    crate::modules::home::HomeModule::restore_launch_file(kernel.inner(), &name)
        .map_err(into_command_error)
}

/// 删除原始启动文件备份。
#[tauri::command]
pub fn home_launch_backup_delete(kernel: State<'_, KernelContext>, name: String) -> CommandResult<()> {
    crate::modules::home::HomeModule::delete_launch_backup(kernel.inner(), &name)
        .map_err(into_command_error)
}

/// 预加载清单探测摘要（将要注入哪些原生 DLL）。
#[tauri::command]
pub fn home_preload_summary(
    kernel: State<'_, KernelContext>,
    name: String,
) -> CommandResult<crate::modules::home::preload::PreloadSummary> {
    crate::modules::home::HomeModule::preload_summary(kernel.inner(), &name)
        .map_err(into_command_error)
}

/// 保存版本图标（`data:image/png;base64,...`，前端已裁剪 256×256）。
#[tauri::command]
pub fn home_logo_set(
    kernel: State<'_, KernelContext>,
    name: String,
    data_url: String,
) -> CommandResult<()> {
    let dir = meta::resolve_version_dir(&kernel.inner().versions_root(), &name).map_err(into_command_error)?;
    meta::save_logo(&dir, &data_url).map_err(into_command_error)
}

/// 移除版本图标。
#[tauri::command]
pub fn home_logo_remove(kernel: State<'_, KernelContext>, name: String) -> CommandResult<()> {
    let dir = meta::resolve_version_dir(&kernel.inner().versions_root(), &name).map_err(into_command_error)?;
    meta::remove_logo(&dir).map_err(into_command_error)
}

/// 开始页默认模式：跨实例扫描本地世界与外部服务器。
#[tauri::command]
pub fn home_dashboard_entries(
    kernel: State<'_, KernelContext>,
) -> CommandResult<Vec<dashboard::DashboardEntry>> {
    Ok(dashboard::entries(kernel.inner()))
}

/// 版本已加入资源清单。
#[tauri::command]
pub fn home_content_list(
    kernel: State<'_, KernelContext>,
    name: String,
) -> CommandResult<Vec<content::ContentItem>> {
    content::list_content(kernel.inner(), &name).map_err(into_command_error)
}

/// 启用 / 禁用内容条目。
#[tauri::command]
pub fn home_content_set_enabled(
    kernel: State<'_, KernelContext>,
    name: String,
    item_id: String,
    enabled: bool,
) -> CommandResult<()> {
    content::set_content_enabled(kernel.inner(), &name, &item_id, enabled)
        .map_err(into_command_error)
}

/// 删除内容条目。
#[tauri::command]
pub fn home_content_remove(
    kernel: State<'_, KernelContext>,
    name: String,
    item_id: String,
) -> CommandResult<()> {
    content::remove_content(kernel.inner(), &name, &item_id).map_err(into_command_error)
}

/// 读取内容条目预览图（data URL，没有图标时为 None）。
#[tauri::command]
pub fn home_content_preview(
    kernel: State<'_, KernelContext>,
    name: String,
    item_id: String,
) -> CommandResult<Option<String>> {
    content::content_preview(kernel.inner(), &name, &item_id).map_err(into_command_error)
}

/// 在文件管理器中定位内容条目。
#[tauri::command]
pub fn home_content_open_folder(
    kernel: State<'_, KernelContext>,
    name: String,
    item_id: String,
) -> CommandResult<String> {
    content::open_content_folder(kernel.inner(), &name, &item_id).map_err(into_command_error)
}

// ---------------------------------------------------------------- 客户端库（开始页模块内部）

/// 本地客户端列表（开始页模块 · 实例「管理客户端」页使用）。
#[tauri::command]
pub fn home_clients_list(
    kernel: State<'_, KernelContext>,
) -> CommandResult<Vec<client::ClientView>> {
    client::ensure_supported().map_err(into_command_error)?;
    client::list(kernel.paths(), kernel.db()).map_err(into_command_error)
}

/// 导入客户端 DLL（落启动器数据目录 + 写库）。
#[tauri::command]
pub fn home_client_import(
    kernel: State<'_, KernelContext>,
    source_path: String,
    name: String,
) -> CommandResult<client::ClientView> {
    client::ensure_supported().map_err(into_command_error)?;
    client::import(
        kernel.paths(),
        kernel.db(),
        kernel.events(),
        &source_path,
        &name,
    )
    .map_err(into_command_error)
}

/// 删除客户端（记录 + 文件），并清掉实例里对它的引用。
#[tauri::command]
pub fn home_client_delete(kernel: State<'_, KernelContext>, id: String) -> CommandResult<()> {
    client::ensure_supported().map_err(into_command_error)?;
    client::remove(kernel.paths(), kernel.db(), kernel.events(), &id)
        .map_err(into_command_error)?;
    clear_client_references(&kernel.versions_root(), &id);
    Ok(())
}

/// 调整某个客户端的注入延迟（毫秒），返回更新后的记录。
///
/// 延迟按客户端保存：同一个 DLL 在不同实例里的初始化节奏一致，按实例存只会
/// 让用户以为「换个实例设置就丢了」。
#[tauri::command]
pub fn home_client_set_inject_delay(
    kernel: State<'_, KernelContext>,
    id: String,
    delay_ms: u64,
) -> CommandResult<client::ClientView> {
    client::ensure_supported().map_err(into_command_error)?;
    client::set_inject_delay(kernel.paths(), kernel.db(), kernel.events(), &id, delay_ms)
        .map_err(into_command_error)
}

/// 客户端删除后，把各实例里指向它的 client_id 清掉，避免留下悬空引用。
fn clear_client_references(versions_root: &std::path::Path, client_id: &str) {
    for mut version in meta::scan_versions(versions_root) {
        if version.client_id.as_deref() != Some(client_id) {
            continue;
        }
        version.client_id = None;
        version.client_dll = None;
        let dir = versions_root.join(&version.name);
        if let Err(error) = meta::VersionMeta::write(&dir, &version) {
            log::warn!(
                "[home/client] 清除实例 '{}' 的客户端引用失败: {error}",
                version.name
            );
        }
    }
}

/// 重新注入当前实例的客户端（仅客户端模式 + 游戏运行期间可用）。
///
/// 命令只投递任务，结果经 client.inject_result 事件回传：远程线程最长等 60s，
/// 不能让按钮卡在那里。
#[tauri::command]
pub fn home_client_reinject(kernel: State<'_, KernelContext>, name: String) -> CommandResult<()> {
    client::ensure_supported().map_err(into_command_error)?;
    let versions_root = kernel.versions_root();
    let dir = meta::resolve_version_dir(&versions_root, &name).map_err(into_command_error)?;
    let version = meta::VersionMeta::read(&dir)
        .ok_or_else(|| KernelError::InvalidArgument(format!("版本「{name}」不存在")))
        .map_err(into_command_error)?;
    client::reinject(
        kernel.paths(),
        kernel.db(),
        kernel.events().clone(),
        &name,
        &versions_root,
        version.mode.as_deref(),
        version.client_id.as_deref(),
    )
    .map_err(into_command_error)
}

// ---------------------------------------------------------------- 触控层布局

/// 读取某实例的屏幕触控布局（缺文件返回默认布局）。
#[tauri::command]
pub fn home_controls_get(
    kernel: State<'_, KernelContext>,
    name: String,
) -> CommandResult<controls::ControlLayout> {
    controls::load(kernel.inner(), &name).map_err(into_command_error)
}

/// 保存某实例的屏幕触控布局（校验 + 原子写，返回规整后的布局）。
#[tauri::command]
pub fn home_controls_save(
    kernel: State<'_, KernelContext>,
    name: String,
    layout: controls::ControlLayout,
) -> CommandResult<controls::ControlLayout> {
    controls::save(kernel.inner(), &name, &layout).map_err(into_command_error)
}

/// 可更新的模组：只有来自内容目录、且目录里有更新版本的模组会出现在结果里。
///
/// 手动导入的 ZIP / DLL 没有来源信息，永远不会被判定为"可更新"。
#[tauri::command]
pub async fn home_mods_updates(
    kernel: State<'_, KernelContext>,
    name: String,
) -> CommandResult<Vec<mods::ModUpdateView>> {
    mods::check_updates(kernel.inner(), &name)
        .await
        .map_err(into_command_error)
}

// ---------------------------------------------------------------- 模组管理

/// 模组清单（附带缺少清单被跳过的目录数）。
#[tauri::command]
pub fn home_mods_list(
    kernel: State<'_, KernelContext>,
    name: String,
) -> CommandResult<mods::ModListResult> {
    mods::list_mods(kernel.inner(), &name).map_err(into_command_error)
}

/// 从 ZIP 导入模组（重名且 `overwrite` 为假时返回 `conflict`）。
#[tauri::command]
pub fn home_mods_import_zip(
    kernel: State<'_, KernelContext>,
    name: String,
    source_path: String,
    overwrite: bool,
) -> CommandResult<mods::ModView> {
    mods::import_zip(kernel.inner(), &name, &source_path, overwrite).map_err(into_command_error)
}

/// 从单个 DLL 导入模组（自动生成清单）。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn home_mods_import_dll(
    kernel: State<'_, KernelContext>,
    name: String,
    source_path: String,
    mod_name: String,
    mod_type: String,
    version: String,
    overwrite: bool,
) -> CommandResult<mods::ModView> {
    mods::import_dll(
        kernel.inner(),
        &name,
        &source_path,
        &mod_name,
        &mod_type,
        &version,
        overwrite,
    )
    .map_err(into_command_error)
}

/// 启用 / 停用模组。
#[tauri::command]
pub fn home_mods_set_enabled(
    kernel: State<'_, KernelContext>,
    name: String,
    folder: String,
    enabled: bool,
) -> CommandResult<()> {
    mods::set_mod_enabled(kernel.inner(), &name, &folder, enabled).map_err(into_command_error)
}

/// 删除模组。
#[tauri::command]
pub fn home_mods_remove(
    kernel: State<'_, KernelContext>,
    name: String,
    folder: String,
) -> CommandResult<()> {
    mods::remove_mod(kernel.inner(), &name, &folder).map_err(into_command_error)
}

/// 批量启用 / 停用模组（多选面板用），返回实际处理的数量。
#[tauri::command]
pub fn home_mods_set_enabled_many(
    kernel: State<'_, KernelContext>,
    name: String,
    folders: Vec<String>,
    enabled: bool,
) -> CommandResult<usize> {
    mods::set_mods_enabled(kernel.inner(), &name, &folders, enabled).map_err(into_command_error)
}

/// 批量删除模组（多选面板用），返回实际删除的数量。
#[tauri::command]
pub fn home_mods_remove_many(
    kernel: State<'_, KernelContext>,
    name: String,
    folders: Vec<String>,
) -> CommandResult<usize> {
    mods::remove_mods(kernel.inner(), &name, &folders).map_err(into_command_error)
}

/// 应用更新：把选中的模组按目录最新版本重新投递到下载队列，返回投递任务数。
#[tauri::command]
pub async fn home_mods_apply_updates(
    kernel: State<'_, KernelContext>,
    name: String,
    folders: Vec<String>,
) -> CommandResult<usize> {
    mods::apply_updates(kernel.inner(), &name, &folders)
        .await
        .map_err(into_command_error)
}

/// 编辑模组清单。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn home_mods_save_manifest(
    kernel: State<'_, KernelContext>,
    name: String,
    folder: String,
    mod_name: String,
    entry: String,
    version: String,
    mod_type: String,
    author: String,
) -> CommandResult<mods::ModView> {
    mods::save_mod_manifest(
        kernel.inner(),
        &name,
        &folder,
        &mod_name,
        &entry,
        &version,
        &mod_type,
        &author,
    )
    .map_err(into_command_error)
}

/// 在系统文件管理器中打开模组目录（目录不存在则创建）。
#[tauri::command]
pub fn home_mods_open_folder(
    kernel: State<'_, KernelContext>,
    name: String,
) -> CommandResult<String> {
    let dir = mods::mods_dir(kernel.inner(), &name, true).map_err(into_command_error)?;
    tauri_plugin_opener::open_path(&dir, None::<&str>)
        .map_err(|e| KernelError::InvalidArgument(format!("打开目录失败: {e}")))
        .map_err(into_command_error)?;
    Ok(dir.to_string_lossy().into_owned())
}

/// 在系统文件管理器中打开版本相关目录（版本目录 / 模组目录 / 存档目录），
/// 返回目录绝对路径。目录不存在则创建。
#[tauri::command]
pub fn home_version_open_dir(
    kernel: State<'_, KernelContext>,
    name: String,
    kind: OpenDirKind,
) -> CommandResult<String> {
    let dir = crate::modules::home::resolve_open_dir(kernel.inner(), &name, kind, true)
        .map_err(into_command_error)?;
    tauri_plugin_opener::open_path(&dir, None::<&str>)
        .map_err(|e| KernelError::InvalidArgument(format!("打开目录失败: {e}")))
        .map_err(into_command_error)?;
    Ok(dir.to_string_lossy().into_owned())
}