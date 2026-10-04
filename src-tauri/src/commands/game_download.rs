//! 游戏下载模块命令：清单 / 加载器 / 实例安装 / APK 导入。
use std::path::PathBuf;
use tauri::State;
use crate::commands::into_command_error;
use crate::error::{CommandResult, KernelError};
use crate::modules::game_download::{apk, installer::{self, Ctx}};
use crate::modules::home::meta::{self, AndroidVersionMeta, VersionMeta};
use crate::modules::game_download::manifest::{self, ManifestView};
use crate::modules::content_download::loader_catalog;
use crate::state::KernelContext;

/// 版本清单（可选强制刷新源）+ 加载器可用性徽标。
///
/// 加载器目录（lipr 索引）拉取失败**不影响**清单本身：徽标退化为「不显示」，
/// 而不是让整页因为一个可选装饰拉不到数据就空掉。
#[tauri::command]
pub async fn game_download_manifest(kernel: State<'_, KernelContext>, refresh: Option<bool>) -> CommandResult<ManifestView> {
    let ctx = Ctx::from_kernel(&kernel);
    let versions = manifest::load_manifest(&ctx, refresh.unwrap_or(false)).await.map_err(into_command_error)?;
    let catalog = match loader_catalog::LoaderCatalog::load().await {
        Ok(catalog) => Some(catalog),
        Err(error) => {
            log::warn!("[game-download] 加载器目录拉取失败（加载器徽标不显示）: {error}");
            None
        }
    };
    Ok(manifest::build_view(&versions, catalog.as_ref()))
}

/// 某版本可选的加载器清单（详情页「加载器」下拉）。
#[tauri::command]
pub async fn game_download_loaders(id: String) -> CommandResult<installer::LoaderOptions> {
    installer::loader_options(&id).await.map_err(into_command_error)
}

/// 为该版本推荐一个可用实例名（安装确认弹窗的初值）。
#[tauri::command]
pub fn game_download_instance_suggest(kernel: State<'_, KernelContext>, id: String) -> CommandResult<String> {
    Ok(installer::suggest_instance_name(&Ctx::from_kernel(&kernel), &id))
}

/// 实例名可用性检查（弹窗输入即时反馈；`reason` 为原因码，文案在前端）。
#[tauri::command]
pub fn game_download_instance_check(kernel: State<'_, KernelContext>, name: String) -> CommandResult<installer::InstanceCheck> {
    Ok(installer::check_instance_name(&Ctx::from_kernel(&kernel), &name))
}

/// 以指定实例名安装某版本，返回整包下载任务 id。
///
/// 返回 0 表示整包已在本地、无需下载（安装已经开始）；前端据此决定是否提示「开始下载」。
/// 同一个版本可以用不同的实例名安装任意多次，实例之间完全隔离。
#[tauri::command]
pub async fn game_download_install(
    kernel: State<'_, KernelContext>,
    id: String,
    instance: String,
    loader: Option<String>,
) -> CommandResult<u64> {
    installer::enqueue(
        &Ctx::from_kernel(&kernel),
        &installer::install_lock(),
        &id,
        &instance,
        loader.as_deref(),
    )
    .await
    .map_err(into_command_error)
}

/// 实例级重试安装：整包在本地时**不重新下载**。
#[tauri::command]
pub fn game_download_retry(kernel: State<'_, KernelContext>, instance: String) -> CommandResult<()> {
    installer::retry_instance(&Ctx::from_kernel(&kernel), &installer::install_lock(), &instance)
        .map_err(into_command_error)
}

/// 版本级重试安装：把该版本下所有未装好的实例重新排进安装（下载中心的「安装」入口）。
#[tauri::command]
pub fn game_download_retry_version(kernel: State<'_, KernelContext>, id: String) -> CommandResult<()> {
    installer::retry_version(&Ctx::from_kernel(&kernel), &installer::install_lock(), &id)
        .map_err(into_command_error)
}

/// 取消一次实例安装（该版本再无待装实例时连整包下载一起放弃）。
#[tauri::command]
pub fn game_download_cancel(kernel: State<'_, KernelContext>, instance: String) -> CommandResult<()> {
    installer::cancel(&Ctx::from_kernel(&kernel), &instance).map_err(into_command_error)
}

/// 下载任务 → 游戏版本的绑定关系。
///
/// 下载中心按核心下载任务展示条目，而安装按版本取待装实例；这个映射由内核给出，
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
/// `name` 会被 [`meta::sanitize_instance_name`] 规整成**唯一权威**的实例名，
/// 并以 `instance_name` 原样回传：安卓宿主按同一个名字定位
/// `data/versions/<name>`，前端不许再自己推导一遍（两侧推导不一致就会出现
/// 「列表里看得到、点启动却说实例不存在」）。
///
/// 包名与版本号由 [`apk::import_file`] 从二进制 `AndroidManifest.xml` 中
/// 解码，前端不参与、也不允许覆盖：这些值决定安卓运行时加载哪一套
/// 原生库。
#[tauri::command]
pub async fn game_download_import_apk(
    kernel: State<'_, KernelContext>,
    source_path: String,
    name: String,
) -> CommandResult<apk::ApkImportResult> {
    let ctx = Ctx::from_kernel(&kernel);
    let root = ctx.versions_root();
    // 先规整再校验：调用方给的名字带空格 / 中文时在这里统一收敛，而不是
    // 让安卓宿主在定位目录时再改一次名字。
    let name = meta::sanitize_instance_name(&name);
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

    // 导入后自检：目录名 / 元数据 / 安卓宿主定位规则三者必须一致。
    // 这一步失败必须回滚——留着它只会让实例在列表里可见却永远起不来。
    if let Err(error) = verify_imported_instance(&instance_dir, &name) {
        let _ = std::fs::remove_dir_all(&instance_dir);
        return Err(into_command_error(error));
    }

    kernel.events().publish(
        "version.installed",
        serde_json::json!({ "name": name, "platform": "android" }),
    );
    Ok(apk::ApkImportResult {
        instance_name: name,
        package_info: imported,
    })
}

/// 导入落位后的自检：目录名与 `version.json` 必须互相印证。
///
/// 覆盖三类真实故障：
/// - 元数据里的 `name` / `packageDir` 与目录名不一致：安卓宿主按目录名
///   定位、内核按元数据展示，两者一旦分叉就会指向不同实例；
/// - 原生库缓存路径不按 `runtime_libs/<实例名>` 约定：宿主下一次启动
///   会另建一份缓存目录，等于白解压一遍；
/// - 缺 `android.versionName` / `packageName`：安卓运行时靠版本号决定
///   原生库加载顺序、靠包名校验游戏包身份，缺了只能静默降级。
fn verify_imported_instance(instance_dir: &std::path::Path, name: &str) -> Result<(), KernelError> {
    let meta = VersionMeta::read(instance_dir).ok_or_else(|| {
        KernelError::InvalidArgument(format!("实例 `{name}` 的 version.json 无法读取"))
    })?;
    if meta.name != name {
        return Err(KernelError::InvalidArgument(format!(
            "实例元数据名 `{}` 与目录名 `{name}` 不一致",
            meta.name
        )));
    }
    let android = meta.android.ok_or_else(|| {
        KernelError::InvalidArgument(format!("实例 `{name}` 缺少安卓元数据，无法作为安卓实例启动"))
    })?;
    if android.package_dir != name {
        return Err(KernelError::InvalidArgument(format!(
            "实例 `{name}` 的 packageDir `{}` 与目录名不一致",
            android.package_dir
        )));
    }
    if android.lib_cache_dir != format!("runtime_libs/{name}") {
        return Err(KernelError::InvalidArgument(format!(
            "实例 `{name}` 的原生库缓存路径 `{}` 不符合 runtime_libs/<实例名> 约定",
            android.lib_cache_dir
        )));
    }
    if android.version_name.trim().is_empty() {
        return Err(KernelError::InvalidArgument(format!(
            "实例 `{name}` 缺少版本号，无法确定原生库加载顺序"
        )));
    }
    if android.package_name.trim().is_empty() {
        return Err(KernelError::InvalidArgument(format!(
            "实例 `{name}` 缺少包名，无法校验游戏包身份"
        )));
    }
    Ok(())
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
