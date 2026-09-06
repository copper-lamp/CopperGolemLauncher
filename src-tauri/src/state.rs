//! 内核上下文：内核全部能力（服务 + 中介）的统一持有者。
//!
//! 模块在 `init / start / stop` 阶段获得 `&KernelContext`，经访问器按需取用能力；
//! 命令层通过 Tauri 管理的同一个上下文向前端暴露能力。上下文不持有业务状态，
//! 仅做聚合，因此可安全共享。

use std::sync::Arc;

use crate::registry::events::EventBus;
use crate::registry::intents::IntentRegistry;
use crate::registry::modules::ModuleRegistry;
use crate::services::account::AccountService;
use crate::services::database::DatabaseService;
use crate::services::download::DownloadService;
use crate::services::i18n::I18nService;
use crate::services::paths::Paths;
use crate::services::settings::SettingsService;
use crate::services::theme::ThemeService;
use crate::services::updater::UpdaterService;

/// 内核上下文。
pub struct KernelContext {
    runtime: tokio::runtime::Handle,
    paths: Arc<Paths>,
    db: Arc<DatabaseService>,
    settings: Arc<SettingsService>,
    i18n: Arc<I18nService>,
    theme: Arc<ThemeService>,
    download: Arc<DownloadService>,
    account: Arc<AccountService>,
    updater: Arc<UpdaterService>,
    events: Arc<EventBus>,
    intents: Arc<IntentRegistry>,
    modules: Arc<ModuleRegistry>,
}

impl KernelContext {
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::type_complexity)]
    pub fn new(
        runtime: tokio::runtime::Handle,
        paths: Arc<Paths>,
        db: Arc<DatabaseService>,
        settings: Arc<SettingsService>,
        i18n: Arc<I18nService>,
        theme: Arc<ThemeService>,
        download: Arc<DownloadService>,
        account: Arc<AccountService>,
        updater: Arc<UpdaterService>,
        events: Arc<EventBus>,
        intents: Arc<IntentRegistry>,
        modules: Arc<ModuleRegistry>,
    ) -> Self {
        Self {
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
        }
    }

    /// tokio 运行时句柄（模块派生异步任务用）。
    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.runtime
    }

    pub fn paths(&self) -> &Arc<Paths> {
        &self.paths
    }

    pub fn db(&self) -> &Arc<DatabaseService> {
        &self.db
    }

    pub fn settings(&self) -> &Arc<SettingsService> {
        &self.settings
    }

    pub fn i18n(&self) -> &Arc<I18nService> {
        &self.i18n
    }

    pub fn theme(&self) -> &Arc<ThemeService> {
        &self.theme
    }

    pub fn download(&self) -> &Arc<DownloadService> {
        &self.download
    }

    pub fn account(&self) -> &Arc<AccountService> {
        &self.account
    }

    pub fn updater(&self) -> &Arc<UpdaterService> {
        &self.updater
    }

    pub fn events(&self) -> &Arc<EventBus> {
        &self.events
    }

    pub fn intents(&self) -> &Arc<IntentRegistry> {
        &self.intents
    }

    pub fn modules(&self) -> &Arc<ModuleRegistry> {
        &self.modules
    }
}
