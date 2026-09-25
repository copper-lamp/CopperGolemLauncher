// 铜内核入口：装配全部内核服务与中介，注册命令并启动 Tauri 应用。
//
// 内置模块开发阶段（开始页已接入）：部分内核公共 API（db / i18n.t /
// paths.data_dir / 模块生命周期 stop / shutdown 等）仍待后续模块（游戏下载、
// 内容下载、账户、更新）消费，暂允许 dead_code；全部内置模块落地后应移除
// 本声明并让编译器帮助收敛死代码。

#![allow(dead_code)]

mod commands;
pub mod error;
mod modules;
pub mod platform;
pub mod registry;
pub mod services;
pub mod state;

use std::path::Path;
use std::sync::Arc;

use tauri::Manager;

/// 内核日志后端：标准输出 +（可选）文件落盘。
///
/// 不用 `tauri-plugin-log`：它在启动时必须先创建 `app_log_dir`，受限环境下会
/// 因该目录不可写而直接中断应用启动。可观测性不该成为启动失败的原因。
///
/// 这里自己做两层：
/// - stdout：`tauri dev` 下即时可见；
/// - 文件：让"事后复盘"成为可能。此前只有 stdout，终端一关现场就没了，
///   排查只能靠反复重编重跑。
///
/// 文件句柄在路径体系就绪后才挂上（[`attach_file_sink`]）：日志目录的解析规则
/// 由 `Paths` 统一持有，此处另起一套平台探测只会制造第二套真相。挂载失败只
/// 降级为"仅 stdout"并留一条警告，不中断启动。
struct KernelLogger {
    /// 落盘句柄；`None` 表示仅输出到 stdout（尚未挂载或挂载失败）。
    sink: parking_lot::Mutex<Option<std::fs::File>>,
    /// 进程启动时刻，用于给日志行标注相对时间。
    ///
    /// 不自造日期格式化（那要额外引入时间库）：绝对时间可由日志文件的 mtime
    /// 还原，行内只有相对偏移，足以对齐同一次启动内事件的先后顺序。
    started: std::time::Instant,
}

/// 单次运行的日志文件大小上限；超过即轮转为 `kernel.log.prev`，避免无限增长。
const LOG_ROTATE_BYTES: u64 = 8 * 1024 * 1024;

/// 全局日志器实例（`log::set_logger` 要求 `'static` 引用）。
static LOGGER: std::sync::OnceLock<KernelLogger> = std::sync::OnceLock::new();

impl log::Log for KernelLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        let line = format!(
            "[+{:>8.3}s][{}][{}] {}",
            self.started.elapsed().as_secs_f32(),
            record.level(),
            record.target(),
            record.args()
        );
        println!("{line}");
        if let Some(file) = self.sink.lock().as_mut() {
            use std::io::Write;
            // 不做缓冲：崩溃现场正是最需要日志的时刻，缓冲会把它一起丢掉。
            let _ = writeln!(file, "{line}");
        }
    }

    fn flush(&self) {
        use std::io::Write;
        if let Some(file) = self.sink.lock().as_mut() {
            let _ = file.flush();
        }
    }
}

/// 安装全局日志后端（幂等：重复调用只生效一次）。
///
/// 此处只装 stdout 部分；文件部分由 [`attach_file_sink`] 在路径就绪后补挂。
fn init_logging() {
    let logger = LOGGER.get_or_init(|| KernelLogger {
        sink: parking_lot::Mutex::new(None),
        started: std::time::Instant::now(),
    });
    let _ = log::set_logger(logger).map(|()| log::set_max_level(log::LevelFilter::Info));
}

/// 挂载文件日志（须在目录就绪后调用，即 `Paths::prepare` 之后）。
///
/// 失败只降级为"仅 stdout"，绝不中断启动。
fn attach_file_sink(logs_dir: &Path) {
    let Some(logger) = LOGGER.get() else {
        log::warn!("[kernel] 日志后端尚未初始化，跳过文件落盘");
        return;
    };
    match open_log_file(logs_dir) {
        Ok(file) => *logger.sink.lock() = Some(file),
        Err(e) => log::warn!(
            "[kernel] 日志无法落盘，本次运行仅输出到 stdout ({}): {e}",
            logs_dir.display()
        ),
    }
}

/// 打开（必要时创建）日志文件；超过上限先把旧文件轮转为 `kernel.log.prev`。
fn open_log_file(logs_dir: &Path) -> std::io::Result<std::fs::File> {
    std::fs::create_dir_all(logs_dir)?;
    let path = logs_dir.join("kernel.log");
    let oversized = std::fs::metadata(&path)
        .map(|m| m.len() > LOG_ROTATE_BYTES)
        .unwrap_or(false);
    if oversized {
        std::fs::rename(&path, logs_dir.join("kernel.log.prev"))?;
    }
    std::fs::OpenOptions::new().create(true).append(true).open(&path)
}

use error::KernelError;
use registry::helper_backend::HelperBackend;
use registry::events::EventBus;
use registry::intents::IntentRegistry;
use registry::loader::ModuleLoader;
use registry::modules::ModuleRegistry;
use registry::sandbox::ModuleSandbox;
use services::account::AccountService;
use services::database::{CORE_MIGRATIONS, DatabaseService};
use services::download::DownloadService;
use services::i18n::I18nService;
use services::paths::Paths;
use services::registry::RegistryService;
use services::settings::{defaults as settings_defaults, SettingsService};
use services::theme::ThemeService;
use services::tips::TipsService;
use services::updater::UpdaterService;
use state::KernelContext;

/// 默认同时下载数；实际取值以设置 `download.concurrency` 为准（1~5）。
const DEFAULT_CONCURRENCY: usize = 3;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        // 附加模块前端产物经自定义协议暴露（Windows/安卓映射为 http://cglmod.localhost）。
        .register_uri_scheme_protocol(registry::frontend::SCHEME, |app, request| {
            registry::frontend::serve_asset(&app, request)
        })
        .setup(|app| {
            let runtime = tauri::async_runtime::handle().inner().clone();

            // 启动早期注入系统代理到环境变量，使只读 env 的下游（下载引擎等）一并走代理。
            services::http_client::inject_system_proxy_env();

            // 路径体系 + 数据库（含内核自身 schema 迁移）。
            // 根目录由宿主（Tauri）提供：桌面端为标准应用数据目录，安卓端为应用
            // 私有 `filesDir`。显式传入而非依赖 `directories` 探测，避免安卓无
            // XDG 目录导致启动中断（见 docs/平台适配.md 3.1 风险 2）。
            //
            // `prepare` 会建目录并做写探针：目录"存在但不可写"是此前最难定位的
            // 一类启动失败（裸 `os error 5`，不带路径），现在会带着目录名报出。
            let mut resolved = Paths::resolve(app.handle())
                .map_err(|e| KernelError::startup("解析数据目录", e))?;
            resolved
                .prepare()
                .map_err(|e| KernelError::startup("准备数据目录", e))?;
            // 目录就绪后立刻挂文件日志，让后续每一步初始化都有落盘现场。
            attach_file_sink(resolved.logs_dir());
            let paths = Arc::new(resolved);
            // 装载后端需要数据目录来落模块私有存储；KernelContext 会另行接管一份引用。
            let paths_for_addons = Arc::clone(&paths);

            let db = Arc::new(DatabaseService::open(paths.db_file()).map_err(|e| {
                KernelError::startup(format!("打开数据库 {}", paths.db_file().display()), e)
            })?);
            db.migrate_scope("core", CORE_MIGRATIONS)
                .map_err(|e| KernelError::startup("执行内核 schema 迁移", e))?;

            // 平台后端：按当前平台装配（凭证存储等），供各服务注入。
            let backends = Arc::new(platform::Backends::assemble());

            // 中介：事件总线（绑定前端桥接）与意图注册表。
            let events = Arc::new(EventBus::new());
            events.bind_app(app.handle().clone());

            // 能力服务。
            let settings = Arc::new(
                SettingsService::new(db.clone(), events.clone(), settings_defaults())
                    .map_err(|e| KernelError::startup("装载设置服务", e))?,
            );
            let i18n = Arc::new(
                I18nService::new(settings.clone())
                    .map_err(|e| KernelError::startup("装载 i18n 服务", e))?,
            );
            let theme = Arc::new(ThemeService::new(settings.clone()));
            // 提示依赖 i18n：文案取自内核语言包，随语言切换自动跟随。
            let tips = Arc::new(TipsService::new(i18n.clone()));
            let download = Arc::new(DownloadService::new(
                settings.get_or("download.concurrency", DEFAULT_CONCURRENCY),
                runtime.clone(),
                paths.clone(),
                events.clone(),
            ));
            let account =
                Arc::new(AccountService::new(
                    db.clone(),
                    events.clone(),
                    platform::as_secret(&backends),
                    runtime.clone(),
                ));
            let updater = Arc::new(UpdaterService::new(
                settings.clone(),
                download.clone(),
                paths.clone(),
                events.clone(),
            ));

            let intents = Arc::new(IntentRegistry::new());
            let modules = Arc::new(ModuleRegistry::new());
            // 装载后端要用注册表构造能力派发器；KernelContext 会另行接管一份引用。
            let modules_for_addons = Arc::clone(&modules);
            // 附加模块沙箱：权限判定与越权留痕（内置模块不经此路径）。
            let sandbox = Arc::new(ModuleSandbox::new());
            // 意图注册表接入沙箱：附加模块声明 / 发起意图需持有 `intents` 权限。
            intents.bind_sandbox(sandbox.clone());
            // 元数据客户端：cgl-libs 索引 / 分片的拉取、三级 sha256 校验与防降级。
            let registry = Arc::new(RegistryService::new(settings.clone(), &paths));

            let kernel = KernelContext::new(
                runtime,
                paths,
                db,
                backends,
                settings,
                i18n,
                theme,
                tips,
                download,
                account,
                updater,
                events,
                intents,
                modules,
                sandbox,
                Some(registry),
            );

            // 装载模块。当前内置模块：开始页（home）、内容下载（content-download）、游戏下载（game-download）。
            kernel
                .modules()
                .register(Arc::new(modules::home::HomeModule::default()));
            kernel.modules().register(Arc::new(modules::content_download::ContentDownloadModule::default()));
            kernel
                .modules()
                .register(Arc::new(modules::game_download::GameDownloadModule::default()));

            // 附加模块：扫描 `<data_dir>/modules`，逐个校验清单并同进程装载动态库。
            // 必须在 `boot()` **之前**完成：装载进来的模块与内置模块一并由 boot 驱动
            // 生命周期；单个失败不影响其它模块（见 loader::ModuleLoader::load_installed）。
            let loader = ModuleLoader::new(Arc::new(HelperBackend::new(
                modules_for_addons,
                paths_for_addons.data_dir().clone(),
            )));
            let reports = loader.load_installed(&kernel);
            let loaded = reports.iter().filter(|r| r.loaded).count();
            if !reports.is_empty() {
                log::info!(
                    "[kernel] 附加模块装载完成（后端 {}）：成功 {loaded} / 共 {}",
                    loader.backend_name(),
                    reports.len()
                );
            }

            kernel.modules().boot(&kernel);

            app.manage(kernel);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::kernel::kernel_info,
            commands::kernel::paths_snapshot,
            commands::kernel::debug_log,
            commands::settings::settings_all,
            commands::settings::settings_get,
            commands::settings::settings_set,
            commands::settings::settings_set_many,
            commands::i18n::i18n_catalog,
            commands::i18n::i18n_supported_locales,
            commands::i18n::i18n_current_locale,
            commands::i18n::i18n_set_locale,
            commands::theme::theme_snapshot,
            commands::theme::theme_set_mode,
            commands::theme::theme_set_accent,
            commands::tips::tips_next,
            commands::tips::tips_keys,
            commands::download::download_enqueue,
            commands::download::download_tasks,
            commands::download::download_task,
            commands::download::download_pause,
            commands::download::download_resume,
            commands::download::download_cancel,
            commands::download::download_retry,
            commands::download::download_remove,
            commands::download::download_pause_all,
            commands::download::download_resume_all,
            commands::download::download_concurrency,
            commands::download::download_set_concurrency,
            commands::account::account_current,
            commands::account::account_begin_login,
            commands::account::account_begin_xal_login,
            commands::account::account_logout,
            commands::account::account_refresh,
            commands::account::account_credentials,
            commands::updater::updater_check,
            commands::updater::updater_apply,
            commands::updater::updater_install,
            commands::updater::updater_status,
            commands::modules::modules_list,
            commands::modules::modules_set_enabled,
            // 模块隔离：权限授权与越权留痕
            commands::modules::sandbox_permissions,
            commands::modules::sandbox_grants,
            commands::modules::sandbox_violations,
            commands::modules::sandbox_grant,
            commands::modules::sandbox_revoke_permission,
            commands::modules::sandbox_revoke,
            // 附加模块：已解包目录扫描与卸载（安装后仍需重启装载，见 cgl-libs.md 3.5 G5）
            commands::modules::modules_installed_addons,
            commands::modules::modules_uninstall,
            // 附加模块：安装链路 + 前端入口清单 + 后端命令分发
            commands::modules::modules_install,
            commands::modules::modules_frontends,
            commands::modules::module_invoke,
            commands::intents::intents_request,
            commands::intents::intents_declared,
            // 元数据（cgl-libs）：索引状态 / 刷新 / 远端模块列表
            commands::registry::registry_status,
            commands::registry::registry_refresh,
            commands::registry::registry_modules,
            // 内容下载模块（content-download）
            commands::content_download::content_download_list,
            commands::content_download::content_download_detail,
            commands::content_download::content_download_readme,
            commands::content_download::content_download_game_versions,
            commands::content_download::content_download_download,
            commands::content_download::content_download_lip_env,
            commands::content_download::content_download_lip_install,
            // 开始页模块（home）
            commands::home::home_versions_list,
            commands::home::home_versions_root,
            commands::home::home_version_get,
            commands::home::home_version_save_meta,
            commands::home::home_version_rename,
            commands::home::home_version_delete,
            commands::home::home_launch,
            commands::home::home_logo_set,
            commands::home::home_logo_remove,
            commands::home::home_content_list,
            commands::home::home_content_set_enabled,
            commands::home::home_content_remove,
            commands::home::home_mods_list,
            commands::home::home_mods_import_zip,
            commands::home::home_mods_import_dll,
            commands::home::home_mods_set_enabled,
            commands::home::home_mods_remove,
            commands::home::home_mods_save_manifest,
            commands::home::home_mods_open_folder,
            commands::home::home_version_open_dir,
            // 游戏下载模块（game-download）
            commands::game_download::game_download_manifest,
            commands::game_download::game_download_detail,
            commands::game_download::game_download_enqueue,
            commands::game_download::game_download_refresh_source,
            commands::game_download::game_download_cancel,
            commands::game_download::game_download_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
