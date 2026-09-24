//! 内核上下文：内核全部能力（服务 + 中介）的统一持有者。
//!
//! 模块在 `init / start / stop` 阶段获得 `&KernelContext`，经访问器按需取用能力；
//! 命令层通过 Tauri 管理的同一个上下文向前端暴露能力。上下文不持有业务状态，
//! 仅做聚合，因此可安全共享。

use std::sync::Arc;

use crate::platform::Backends;
use crate::registry::events::EventBus;
use crate::registry::intents::IntentRegistry;
use crate::registry::modules::ModuleRegistry;
use crate::registry::sandbox::ModuleSandbox;
use crate::services::account::AccountService;
use crate::services::database::DatabaseService;
use crate::services::download::DownloadService;
use crate::services::i18n::I18nService;
use crate::services::paths::Paths;
use crate::services::registry::RegistryService;
use crate::services::settings::SettingsService;
use crate::services::theme::ThemeService;
use crate::services::tips::TipsService;
use crate::services::updater::UpdaterService;

/// 内核上下文。
pub struct KernelContext {
    runtime: tokio::runtime::Handle,
    paths: Arc<Paths>,
    db: Arc<DatabaseService>,
    /// 平台后端（凭证存储等，随平台装配）。
    backends: Arc<Backends>,
    settings: Arc<SettingsService>,
    i18n: Arc<I18nService>,
    theme: Arc<ThemeService>,
    tips: Arc<TipsService>,
    download: Arc<DownloadService>,
    account: Arc<AccountService>,
    updater: Arc<UpdaterService>,
    events: Arc<EventBus>,
    intents: Arc<IntentRegistry>,
    modules: Arc<ModuleRegistry>,
    sandbox: Arc<ModuleSandbox>,
    /// 元数据客户端（`cgl-libs` 索引 / 分片）。命令层就绪前可能为 None。
    registry: Option<Arc<RegistryService>>,
}

impl KernelContext {
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::type_complexity)]
    pub fn new(
        runtime: tokio::runtime::Handle,
        paths: Arc<Paths>,
        db: Arc<DatabaseService>,
        backends: Arc<Backends>,
        settings: Arc<SettingsService>,
        i18n: Arc<I18nService>,
        theme: Arc<ThemeService>,
        tips: Arc<TipsService>,
        download: Arc<DownloadService>,
        account: Arc<AccountService>,
        updater: Arc<UpdaterService>,
        events: Arc<EventBus>,
        intents: Arc<IntentRegistry>,
        modules: Arc<ModuleRegistry>,
        sandbox: Arc<ModuleSandbox>,
        registry: Option<Arc<RegistryService>>,
    ) -> Self {
        Self {
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
            registry,
        }
    }

    /// tokio 运行时句柄（模块派生异步任务用）。
    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.runtime
    }

    pub fn paths(&self) -> &Arc<Paths> {
        &self.paths
    }

    /// 当前游戏（版本）根目录（按设置动态解析）。
    pub fn versions_root(&self) -> std::path::PathBuf {
        self.paths.versions_root(&self.settings)
    }

    pub fn db(&self) -> &Arc<DatabaseService> {
        &self.db
    }

    /// 平台后端（凭证存储等，随平台装配）。
    pub fn backends(&self) -> &Arc<Backends> {
        &self.backends
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

    /// 加载提示（内核通用能力，模块经命令层取用）。
    pub fn tips(&self) -> &Arc<TipsService> {
        &self.tips
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

    /// 模块沙箱：附加模块的能力授权与越权拦截（内置模块不经此路径）。
    pub fn sandbox(&self) -> &Arc<ModuleSandbox> {
        &self.sandbox
    }

    /// 元数据客户端（`cgl-libs`：索引 / 分片 / 三级校验 / 防降级锚点）。
    pub fn registry(&self) -> Option<&Arc<RegistryService>> {
        self.registry.as_ref()
    }
}
