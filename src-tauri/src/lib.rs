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

use std::sync::Arc;

use tauri::Manager;

/// 最小 stdout 日志后端。
///
/// `tauri-plugin-log` 会在启动时强制创建文件日志目录，在受限环境下会因无法
/// 写入 `app_log_dir` 而直接中断应用启动。这里改为仅输出到标准输出：保留
/// 全部 `log::` 调用点的可观测性，同时不产生任何文件系统副作用。
struct StdoutLogger;

impl log::Log for StdoutLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        println!(
            "[{}][{}] {}",
            record.level(),
            record.target(),
            record.args()
        );
    }

    fn flush(&self) {}
}

/// 安装全局日志后端（幂等：重复调用只生效一次）。
fn init_logging() {
    static LOGGER: StdoutLogger = StdoutLogger;
    let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Info));
}

use registry::events::EventBus;
use registry::intents::IntentRegistry;
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

/// 默认同时下载数（可从设置读取，暂未开放配置）。
const DEFAULT_CONCURRENCY: usize = 3;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let runtime = tauri::async_runtime::handle().inner().clone();

            // 启动早期注入系统代理到环境变量，使只读 env 的下游（下载引擎等）一并走代理。
            services::http_client::inject_system_proxy_env();

            // 路径体系 + 数据库（含内核自身 schema 迁移）。
            // 根目录由宿主（Tauri）提供：桌面端为标准应用数据目录，安卓端为应用
            // 私有 `filesDir`。显式传入而非依赖 `directories` 探测，避免安卓无
            // XDG 目录导致启动中断（见 docs/平台适配.md 3.1 风险 2）。
            let paths = Arc::new(Paths::resolve(app.handle())?);
            paths.ensure_dirs()?;
            let db = Arc::new(DatabaseService::open(paths.db_file())?);
            db.migrate_scope("core", CORE_MIGRATIONS)?;

            // 平台后端：按当前平台装配（凭证存储等），供各服务注入。
            let backends = Arc::new(platform::Backends::assemble());

            // 中介：事件总线（绑定前端桥接）与意图注册表。
            let events = Arc::new(EventBus::new());
            events.bind_app(app.handle().clone());

            // 能力服务。
            let settings =
                Arc::new(SettingsService::new(db.clone(), events.clone(), settings_defaults())?);
            let i18n = Arc::new(I18nService::new(settings.clone())?);
            let theme = Arc::new(ThemeService::new(settings.clone()));
            // 提示依赖 i18n：文案取自内核语言包，随语言切换自动跟随。
            let tips = Arc::new(TipsService::new(i18n.clone()));
            let download = Arc::new(DownloadService::new(
                DEFAULT_CONCURRENCY,
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
