//! 游戏下载模块命令：清单 / 下载 / APK 导入。
use std::path::PathBuf;
use tauri::State;
use crate::commands::into_command_error;
use crate::error::{CommandResult, KernelError};
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

/// 仅重装：整包已在本地时重跑安装流水线，不重新下载。
///
/// 安装阶段失败（商店授权、md5、解包中断）后的补救路径——此前只能
/// `game_download_enqueue` 重来，等于把数 GB 的下载重做一遍。
/// 本地整包缺失或校验不符时**明确报错**而不是悄悄改走下载。
#[tauri::command]
pub async fn game_download_install(kernel: State<'_, KernelContext>, id: String) -> CommandResult<()> {
    installer::install(&Ctx::from_kernel(&kernel), &id).map_err(into_command_error)
}

/// 下载任务 → 游戏版本的绑定关系。
///
/// 下载中心按核心下载任务展示条目，而安装按版本 id 取记录；这个映射由内核给出，
/// 前端不依据 dest / 文件名猜测，避免把安装指向另一个版本。
#[tauri::command]
pub fn game_download_task_bindings(
    kernel: State<'_, KernelContext>,
) -> CommandResult<Vec<installer::TaskBinding>> {
    installer::task_bindings(&Ctx::from_kernel(&kernel)).map_err(into_command_error)
}

/// 导入一个 APK / APKS。
///
/// `source_path` 必须是**应用私有目录内**的路径：安卓端由系统文件选择器
/// （SAF）返回的 `content://` URI 会先被复制到 `cache/inbox/`，
/// 桌面端由文件对话框直接给出绝对路径。命令不接受任意外部路径，
/// 避免把「用户选了一个文件」变成「内核可以读设备上任意文件」。
///
/// 包名与版本号由 [`apk::import_file`] 从二进制 `AndroidManifest.xml` 中
/// 解码，前端不参与、也不允许覆盖：这些值决定安卓运行时加载哪一套
/// 原生库。
#[tauri::command]
pub async fn game_download_import_apk(
    kernel: State<'_, KernelContext>,
    source_path: String,
    name: String,
) -> CommandResult<apk::ApkPackageInfo> {
    let ctx = Ctx::from_kernel(&kernel);
    let root = ctx.versions_root();
    meta::validate_version_name(&root, &name).map_err(into_command_error)?;

    let source = PathBuf::from(source_path);
    let staging = root
        .join(".import")
        .join(format!("{}-{}", name.replace(['/', '\\'], "_"), std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);

    let imported = match apk::import_file(&source, &staging) {
        Ok(info) => info,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(into_command_error(error));
        }
    };

    let instance_dir = root.join(&name);
    if instance_dir.exists() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(into_command_error(KernelError::Conflict(format!(
            "实例 `{name}` 已存在，请先删除或重命名"
        ))));
    }
    // 原子落位：先 rename 再写元数据，中断时不会留下「有目录无元数据」的
    // 半成品实例（`scan_versions` 只认带 version.json 的目录）。
    std::fs::rename(&staging, &instance_dir)
        .map_err(|e| into_command_error(KernelError::Io(e)))?;

    let version_name = if imported.version_name.is_empty() {
        // 极端情况下 manifest 缺 versionName：退回 versionCode 的十进制串，
        // 交给安卓侧按「经典四件套」加载，并在文档中标注为降级路径。
        imported.version_code.to_string()
    } else {
        imported.version_name.clone()
    };

    let meta = VersionMeta {
        name: name.clone(),
        game_version: version_name.clone(),
        version_type: "release".into(),
        enable_isolation: true,
        created_at: meta::now_rfc3339(),
        android: Some(AndroidVersionMeta {
            package_name: imported.package_name.clone(),
            version_code: imported.version_code,
            version_name,
            abi: imported.abi.clone(),
            package_dir: name.clone(),
            lib_cache_dir: format!("runtime_libs/{name}"),
        }),
        ..Default::default()
    };
    if let Err(error) = VersionMeta::write(&instance_dir, &meta) {
        // 元数据写失败即视为导入失败：没有 version.json 的目录不会被识别为实例。
        let _ = std::fs::remove_dir_all(&instance_dir);
        return Err(into_command_error(error));
    }

    kernel.events().publish(
        "version.installed",
        serde_json::json!({ "name": name, "platform": "android" }),
    );
    Ok(imported)
}

/// 取走安卓游戏退出记录（文件信箱，take 语义）。
///
/// 桌面端没有游戏宿主，恒返回 `None`；命令在所有平台都注册，
/// 前端因此无需按平台分支判断命令是否存在。
#[tauri::command]
pub async fn android_game_take_exit(
    kernel: State<'_, KernelContext>,
) -> CommandResult<Option<crate::platform::android::ExitRecord>> {
    crate::platform::android::take_exit_record(&data_root(&kernel)).map_err(into_command_error)
}

/// 取走一次 SAF 文件选择的落盘结果（take 语义）。
///
/// 返回 `None` 表示 Java 宿主尚未写回结果（仍在选择或复制中），
/// 前端按固定间隔轮询；`error` 非空表示用户在系统选择器里取消或授权失败。
#[tauri::command]
pub async fn android_apk_pick_result(
    kernel: State<'_, KernelContext>,
    request_id: String,
) -> CommandResult<Option<crate::platform::android::ApkPickResult>> {
    crate::platform::android::take_apk_pick_result(&data_root(&kernel), &request_id)
        .map_err(into_command_error)
}

/// 数据根目录：`<root>/data`（`Paths::with_root` 布局）。
fn data_root(kernel: &State<'_, KernelContext>) -> PathBuf {
    kernel
        .paths()
        .versions_dir()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| kernel.paths().versions_dir().clone())
}
