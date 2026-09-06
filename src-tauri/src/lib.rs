// 铜内核入口：装配全部内核服务与中介，注册命令并启动 Tauri 应用。
//
// 内置模块开发阶段（开始页已接入）：部分内核公共 API（db / i18n.t /
// paths.data_dir / 模块生命周期 stop / shutdown 等）仍待后续模块（游戏下载、
// 内容下载、账户、更新）消费，暂允许 dead_code；全部内置模块落地后应移除
// 本声明并让编译器帮助收敛死代码。

#![allow(dead_code)]

mod commands;
mod error;
mod modules;
mod registry;
mod services;
mod state;

use std::sync::Arc;

use tauri::Manager;

use error::KernelError;
use registry::events::EventBus;
use registry::intents::IntentRegistry;
use registry::modules::ModuleRegistry;
use services::account::AccountService;
use services::database::{CORE_MIGRATIONS, DatabaseService};
use services::download::DownloadService;
use services::i18n::I18nService;
use services::paths::Paths;
use services::settings::{defaults as settings_defaults, SettingsService};
use services::theme::ThemeService;
use services::updater::UpdaterService;
use state::KernelContext;

/// 默认同时下载数（可从设置读取，暂未开放配置）。
const DEFAULT_CONCURRENCY: usize = 3;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .setup(|app| {
            let runtime = tauri::async_runtime::handle().inner().clone();

            // 路径体系 + 数据库（含内核自身 schema 迁移）。
            let paths = Arc::new(
                Paths::new().map_err(|e| KernelError::Config(e.to_string()))?,
            );
            paths.ensure_dirs()?;
            let db = Arc::new(DatabaseService::open(paths.db_file())?);
            db.migrate_scope("core", CORE_MIGRATIONS)?;

            // 中介：事件总线（绑定前端桥接）与意图注册表。
            let events = Arc::new(EventBus::new());
            events.bind_app(app.handle().clone());

            // 能力服务。
            let settings =
                Arc::new(SettingsService::new(db.clone(), events.clone(), settings_defaults())?);
            let i18n = Arc::new(I18nService::new(settings.clone())?);
            let theme = Arc::new(ThemeService::new(settings.clone()));
            let download = Arc::new(DownloadService::new(
                DEFAULT_CONCURRENCY,
                runtime.clone(),
                paths.clone(),
                events.clone(),
            ));
            let account =
                Arc::new(AccountService::new(db.clone(), events.clone(), runtime.clone()));
            let updater = Arc::new(UpdaterService::new(
                settings.clone(),
                download.clone(),
                paths.clone(),
                events.clone(),
            ));

            let intents = Arc::new(IntentRegistry::new());
            let modules = Arc::new(ModuleRegistry::new());

            let kernel = KernelContext::new(
                runtime,
                paths,
                db,
                settings,
                i18n,
                theme,
                download,
                account,
                updater,
                events,
                intents,
                modules,
            );

            // 装载模块。当前内置模块：开始页（home）、内容下载（content-download）。
            kernel
                .modules()
                .register(Arc::new(modules::home::HomeModule::default()));
            kernel
                .modules()
                .register(Arc::new(modules::content_download::ContentDownloadModule::default()));
            kernel.modules().boot(&kernel);

            app.manage(kernel);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::kernel::kernel_info,
            commands::kernel::paths_snapshot,
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
            commands::account::account_logout,
            commands::account::account_refresh,
            commands::account::account_credentials,
            commands::updater::updater_check,
            commands::updater::updater_apply,
            commands::updater::updater_install,
            commands::updater::updater_status,
            commands::modules::modules_list,
            commands::modules::modules_set_enabled,
            commands::intents::intents_request,
            commands::intents::intents_declared,
            // 内容下载模块（content-download）
            commands::content_download::content_download_list,
            commands::content_download::content_download_detail,
            commands::content_download::content_download_readme,
            commands::content_download::content_download_download,
            commands::content_download::content_download_lip_env,
            commands::content_download::content_download_lip_install,
            // 开始页模块（home）
            commands::home::home_versions_list,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
