//! 受监管子进程装载后端：附加模块后端运行在独立 helper 进程中。
//!
//! # 为什么不再同进程加载 dylib
//!
//! 同进程方案（[`crate::registry::dylib_backend`]）要求插件与内核「同 crate 版本、
//! 同工具链、同优化/panic 配置」构建，且插件一旦崩溃会带走整个启动器。本后端把
//! 插件放进独立进程：内核只经版本化 IPC 与它对话，插件崩溃或卡死只影响自己。
//!
//! # 分层
//!
//! - [`AddonSession`]：一个 helper 子进程的会话（派生 → 握手 → 初始化 → 调用）。
//!   不依赖 [`KernelContext`]，因此可以用真实进程直接单测。
//! - [`AddonProxyModule`]：把会话适配成 [`Module`] 契约，并负责**生命周期登记**：
//!   事件订阅（见 [`crate::registry::addon_events`]）与意图处理器（见
//!   [`crate::registry::intents`]）都在 `start` 成功后落地、`stop` 时撤销。
//!   其宿主侧协作对象由 [`AddonHostServices`] 在装载时注入，因此同样不依赖
//!   `KernelContext`——整条生命周期可以在真实子进程上单测。
//!
//! # 平台
//!
//! helper 可执行文件与主程序同目录分发；开发/测试场景（可执行文件在
//! `target/<profile>/deps`）会向上一级查找。Android 是否支持该模型尚未通过门禁，
//! 见实施计划阶段 6；未通过前不得静默回退到同进程 dylib。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{json, Value};

use copper_module_abi::helper_client::{CapabilityDispatcher, HelperError, HelperProcess, HelperPushHandle};
use copper_module_abi::ipc::PLUGIN_COMMAND_INTENT_PREFIX;

use crate::error::KernelError;
use crate::registry::addon_events::AddonEventBridge;
use crate::registry::capability::KernelCapabilities;
use crate::registry::dylib_backend::resolve_artifact;
use crate::registry::intents::{IntentHandler, IntentRegistry};
use crate::registry::loader::ModuleLoadBackend;
use crate::registry::manifest::{EventDeclarations, ModuleManifest};
use crate::registry::module_storage::ModuleStorage;
use crate::registry::modules::{Module, ModuleRegistry};
use crate::registry::sandbox::{ModuleSandbox, Permission};
use crate::registry::events::EventBus;
use crate::state::KernelContext;

/// 显式指定 helper 可执行文件的路径（调试与集成测试用）。
pub const HELPER_PATH_ENV: &str = "COPPER_MODULE_HELPER";
/// helper 可执行文件主干名（按平台补扩展名）。
const HELPER_FILE_STEM: &str = "copper-module-helper";

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const INIT_TIMEOUT: Duration = Duration::from_secs(20);
const LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(10);
/// 插件命令可能是重活（解包、下载校验），给足时间但仍必须有上限。
const INVOKE_TIMEOUT: Duration = Duration::from_secs(60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
/// 把一次意图请求转发给插件的等待上限。
///
/// **必须有上限**：目标模块可能正持会话锁跑一次长命令，而它的插件此时又在等宿主
/// 的能力响应——两边互等就是闭环死锁。超时后如实报错，让调用方看到"模块忙 / 疑似
/// 循环调用"，而不是无声挂起。
const INTENT_FORWARD_TIMEOUT: Duration = Duration::from_secs(10);

/// 定位 helper 可执行文件。
///
/// 顺序：环境变量显式指定 → 与主程序同目录 → 主程序目录的上一级（开发/测试时
/// 主程序位于 `target/<profile>/deps`）。找不到就报错，不做静默降级。
pub fn locate_helper_program() -> Result<PathBuf, KernelError> {
    if let Some(explicit) = std::env::var_os(HELPER_PATH_ENV) {
        let path = PathBuf::from(explicit);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(KernelError::Module(format!(
                "环境变量 {HELPER_PATH_ENV} 指向的 helper 不存在：{}",
                path.display()
            )))
        };
    }

    let exe = std::env::current_exe().map_err(|e| {
        KernelError::Module(format!("无法定位当前可执行文件以查找 helper：{e}"))
    })?;
    let name = helper_file_name();

    let mut search_dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = exe.parent() {
        search_dirs.push(dir.to_path_buf());
        if let Some(parent) = dir.parent() {
            search_dirs.push(parent.to_path_buf());
        }
    }

    for dir in &search_dirs {
        let candidate = dir.join(&name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    let tried = search_dirs
        .iter()
        .map(|dir| dir.display().to_string())
        .collect::<Vec<_>>()
        .join("、");
    Err(KernelError::Module(format!(
        "找不到附加模块 helper 可执行文件 `{name}`（已查找：{tried}）；\
         可用环境变量 {HELPER_PATH_ENV} 显式指定路径"
    )))
}

fn helper_file_name() -> String {
    format!("{HELPER_FILE_STEM}{}", std::env::consts::EXE_SUFFIX)
}

/// 一个附加模块的受监管子进程会话。
pub struct AddonSession {
    process: HelperProcess,
    module_id: String,
}

impl AddonSession {
    /// 派生 helper 并完成握手与插件初始化。
    ///
    /// 任一步失败都会返回错误，且进程由 [`HelperProcess`] 的析构保证被回收——
    /// 不会留下半初始化的孤儿进程。
    pub fn launch(
        helper_program: &Path,
        module_id: &str,
        plugin_path: &Path,
        config: &Value,
        dispatcher: Arc<dyn CapabilityDispatcher>,
    ) -> Result<Self, HelperError> {
        let mut process =
            HelperProcess::spawn_with_dispatcher(helper_program, module_id, plugin_path, dispatcher)?;
        process.handshake(HANDSHAKE_TIMEOUT)?;
        process.initialize(config, INIT_TIMEOUT)?;
        Ok(Self {
            process,
            module_id: module_id.to_owned(),
        })
    }

    pub fn module_id(&self) -> &str {
        &self.module_id
    }

    pub fn start(&mut self) -> Result<(), HelperError> {
        self.process.start(LIFECYCLE_TIMEOUT)
    }

    pub fn stop(&mut self) -> Result<(), HelperError> {
        self.process.stop(LIFECYCLE_TIMEOUT)
    }

    /// 转发一条命令给插件。
    pub fn invoke(&mut self, command: &str, args: &Value) -> Result<Value, HelperError> {
        self.process.invoke(command, args, INVOKE_TIMEOUT)
    }

    /// 分出一个只写推送句柄给事件桥使用（见 [`crate::registry::addon_events`]）。
    ///
    /// 事件发生在任意内核线程上，而会话被 [`AddonProxyModule`] 独占；推送句柄把
    /// 「写单向通知」这条路径独立出来，因此两者可以并存而不互相阻塞。
    pub fn push_handle(&self) -> HelperPushHandle {
        self.process.push_handle()
    }

    /// 优雅停机；返回进程是否在超时前自行退出。
    pub fn shutdown(&mut self) -> Result<bool, HelperError> {
        self.process.shutdown(SHUTDOWN_TIMEOUT)
    }

    /// 子进程 stderr 尾部（限长），仅在诊断失败原因时使用。
    pub fn stderr(&self) -> String {
        self.process.stderr()
    }
}

/// 附加模块的宿主侧协作对象。
///
/// 用结构体一次性注入，而不是在生命周期里读 [`KernelContext`]：`KernelContext` 需要
/// 整个内核装配完成才能构造，而这四件东西各自都能独立构造。分开之后，附加模块的
/// 「订阅上界 → 登记 → 撤销」整条链路可以在**真实子进程**上单测，不必先起一个内核。
pub struct AddonHostServices {
    /// 插件能力请求的宿主侧授权与执行入口。
    pub capabilities: Arc<dyn CapabilityDispatcher>,
    /// 事件推送桥（全体附加模块共用一个 `*` 订阅）。
    pub bridge: Arc<AddonEventBridge>,
    /// 意图注册表：登记转发处理器，并在停止时注销。
    pub intents: Arc<IntentRegistry>,
    /// 模块沙箱：订阅事件前的逐次授权。
    pub sandbox: Arc<ModuleSandbox>,
}

/// 附加模块在注册表中的代理：把 [`Module`] 生命周期映射到 helper 子进程。
pub struct AddonProxyModule {
    id: String,
    version: String,
    module_dir: PathBuf,
    helper_program: PathBuf,
    plugin_path: PathBuf,
    /// 清单声明的事件上界（订阅 + 发布）。两侧都空表示既不收也不发。
    events: EventDeclarations,
    /// 清单声明的意图处理器上界。空表示不处理任何意图。
    intents: Vec<String>,
    services: AddonHostServices,
    /// 会话只在 `init` 成功后存在；`stop` 会取走它以确保进程被回收。
    ///
    /// 用 `Arc<Mutex<..>>` 而不是裸 `Mutex`：意图转发处理器需要克隆一份共享句柄，
    /// 这样它就不必持有整个模块（那会形成自引用）。
    session: Arc<Mutex<Option<AddonSession>>>,
}

impl AddonProxyModule {
    pub fn new(
        manifest: &ModuleManifest,
        module_dir: PathBuf,
        helper_program: PathBuf,
        plugin_path: PathBuf,
        services: AddonHostServices,
    ) -> Self {
        Self {
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            module_dir,
            helper_program,
            plugin_path,
            events: manifest.events.clone(),
            intents: manifest.intents.clone(),
            services,
            session: Arc::new(Mutex::new(None)),
        }
    }

    /// 会话当前是否在运行。
    pub fn is_running(&self) -> bool {
        self.session.lock().is_some()
    }

    /// 最近一次子进程 stderr 尾部，用于把插件故障原因带回内核日志。
    pub fn stderr_tail(&self) -> Option<String> {
        self.session.lock().as_ref().map(AddonSession::stderr)
    }

    /// 把 helper 故障转成内核错误，并附上子进程 stderr 尾部（诊断的主要线索）。
    ///
    /// **不得在持有会话锁时调用**（它会去读会话）。持锁路径请用
    /// [`AddonProxyModule::wrap_with_stderr`] 就地取 stderr。
    fn wrap(&self, error: HelperError) -> KernelError {
        self.wrap_with_stderr(error, self.stderr_tail().unwrap_or_default())
    }

    fn wrap_with_stderr(&self, error: HelperError, stderr: String) -> KernelError {
        let detail = if stderr.trim().is_empty() {
            error.to_string()
        } else {
            format!("{error}（helper stderr：{stderr}）")
        };
        KernelError::Module(format!("附加模块 `{}` 的受监管进程失败：{detail}", self.id))
    }

    fn with_session<T>(
        &self,
        action: impl FnOnce(&mut AddonSession) -> Result<T, HelperError>,
    ) -> Result<T, KernelError> {
        let mut guard = self.session.lock();
        let Some(session) = guard.as_mut() else {
            return Err(self.not_running());
        };
        match action(session) {
            Ok(value) => Ok(value),
            // 就地取 stderr：**不能**再走 `self.stderr_tail()`，那会二次加锁同一把
            // 会话锁，在错误路径上静默自锁（栈里谁都等不到的那类缺陷）。
            Err(error) => Err(self.wrap_with_stderr(error, session.stderr())),
        }
    }

    fn not_running(&self) -> KernelError {
        not_running_error(&self.id)
    }

    /// 加载并初始化插件（`Module::init` 的全部内容）。
    ///
    /// 独立于 [`Module`] 契约的无参入口：装配整个内核上下文只为调一次生命周期，
    /// 会让这条链路无法在真实子进程上被测试。
    pub fn init_module(&self) -> Result<(), KernelError> {
        // 插件初始化的配置由宿主下发；插件无法自行声明身份或扩大输入范围。
        let config = json!({
            "module_dir": self.module_dir.to_string_lossy(),
            "module_version": self.version,
        });

        let session = AddonSession::launch(
            &self.helper_program,
            &self.id,
            &self.plugin_path,
            &config,
            Arc::clone(&self.services.capabilities),
        )
        .map_err(|error| self.wrap(error))?;

        *self.session.lock() = Some(session);
        log::info!(
            "[addon/{}] helper 已就绪（插件 {}）",
            self.id,
            self.plugin_path.display()
        );
        Ok(())
    }

    /// 启动插件，并在成功后登记订阅与意图处理器（`Module::start` 的全部内容）。
    pub fn start_module(&self) -> Result<(), KernelError> {
        // 顺序即语义：先让插件真的跑起来，再登记——反过来会让宿主把事件 / 意图
        // 推给一个还没开始工作的插件。
        self.with_session(AddonSession::start)?;

        // 登记可能只完成一半（事件成功、意图失败）。失败时整体回滚并回收进程：
        // 否则会留下「注册表判为 Failed、订阅却还在收事件」的半启动模块。
        if let Err(error) = self
            .register_events()
            .and_then(|()| self.register_intents())
        {
            self.withdraw_registrations();
            if let Some(mut session) = self.session.lock().take() {
                let _ = session.shutdown();
            }
            return Err(error);
        }
        Ok(())
    }

    /// 撤销全部登记并停机（`Module::stop` 的全部内容）。
    pub fn stop_module(&self) -> Result<(), KernelError> {
        // 先撤登记再停机：反过来会让事件 / 意图在被撤销前推给正在退出的插件。
        self.withdraw_registrations();

        // 先取走会话：无论 stop 是否报错，进程与 IPC 资源都必须被回收。
        let Some(mut session) = self.session.lock().take() else {
            return Ok(());
        };
        let stop_result = session.stop();
        match session.shutdown() {
            Ok(exited) => {
                if !exited {
                    log::warn!("[addon/{}] helper 未在超时内自行退出，已强制终止", self.id);
                }
            }
            Err(error) => {
                log::warn!("[addon/{}] helper 停机失败：{error}", self.id);
            }
        }
        stop_result.map_err(|error| self.wrap(error))
    }

    /// 按清单声明的模式登记事件推送。
    fn register_events(&self) -> Result<(), KernelError> {
        if self.events.subscribe.is_empty() {
            // 空声明 = 什么都不收。不登记，也不申请权限（发布方向与本函数无关）。
            return Ok(());
        }

        // 装载时已按「`events` 非空即授予 Events」落了授权，用户仍可在设置页收紧；
        // 此处再判一次，让"收紧"真的生效，而不是一份装饰性声明。
        self.services.sandbox.enforce(
            &self.id,
            Permission::Events,
            "events.subscribe",
            &self.events.subscribe.join(","),
        )?;

        let push = self.with_session(|session| Ok(session.push_handle()))?;
        self.services
            .bridge
            .register(&self.id, self.events.subscribe.clone(), Arc::new(push));
        Ok(())
    }

    /// 按清单声明的意图名登记转发处理器。
    fn register_intents(&self) -> Result<(), KernelError> {
        for intent in &self.intents {
            let session = Arc::clone(&self.session);
            let module_id = self.id.clone();
            let declared = intent.clone();
            let handler: IntentHandler = Arc::new(move |payload: Value| {
                forward_intent(&session, &module_id, &declared, payload)
            });
            // 用 `declare_checked`：附加模块必须持有 `intents` 权限才能声明处理器，
            // 与"发起意图"同一入口、同一口径。
            self.services
                .intents
                .declare_checked(&self.id, intent, handler)?;
        }
        Ok(())
    }

    /// 撤销本模块的全部登记（事件订阅 + 意图处理器）。幂等。
    ///
    /// `stop` 与 `start` 的失败回滚共用它——两处需要的动作完全相同，分开写迟早
    /// 漏掉一边（典型的"崩溃后才想起来没退订"）。
    fn withdraw_registrations(&self) {
        self.services.bridge.unregister(&self.id);
        self.services.intents.withdraw(&self.id);
    }
}

/// 会话未运行时的统一错误（`with_session` 与意图转发共用同一措辞）。
fn not_running_error(module_id: &str) -> KernelError {
    KernelError::Module(format!(
        "附加模块 `{module_id}` 的后端进程未在运行（尚未初始化或已停止）"
    ))
}

/// 把一次意图请求转发给插件的 helper，并取回其结果。
///
/// 不持有 `Arc<AddonProxyModule>`：处理者只需要「会话 + 身份」两样东西，把整个模块
/// 塞进闭包会形成自引用（模块 → 意图注册表 → 闭包 → 模块）。
fn forward_intent(
    session: &Mutex<Option<AddonSession>>,
    module_id: &str,
    intent: &str,
    payload: Value,
) -> Result<Value, KernelError> {
    // 带超时地拿会话锁：目标模块可能正持锁跑一次长命令，而它的插件此时又在等宿主的
    // 能力响应——无限等待就是闭环死锁。自调用（模块请求自己声明的意图）必然命中
    // 这条路径，因此它必须是"显式拒绝"而不是"hang 住"。
    let mut guard = session.try_lock_for(INTENT_FORWARD_TIMEOUT).ok_or_else(|| {
        KernelError::Intent(format!(
            "意图 `{intent}` 转发到模块 `{module_id}` 超时（{} 秒）：其会话正被占用，\
             疑似循环调用或存在长时间未返回的命令",
            INTENT_FORWARD_TIMEOUT.as_secs()
        ))
    })?;

    let Some(session) = guard.as_mut() else {
        return Err(not_running_error(module_id));
    };

    let command = format!("{PLUGIN_COMMAND_INTENT_PREFIX}{intent}");
    match session.invoke(&command, &payload) {
        Ok(result) => Ok(result),
        Err(error) => {
            // 就地取 stderr，理由同 `with_session`：持锁路径不得二次加锁。
            let stderr = session.stderr();
            let detail = if stderr.trim().is_empty() {
                error.to_string()
            } else {
                format!("{error}（helper stderr：{stderr}）")
            };
            Err(KernelError::Module(format!(
                "附加模块 `{module_id}` 的受监管进程失败：{detail}"
            )))
        }
    }
}

impl Module for AddonProxyModule {
    fn id(&self) -> &str {
        &self.id
    }

    fn version(&self) -> &str {
        &self.version
    }

    // 下面三个生命周期方法只做转发：真正的实现在无参入口里，理由见
    // [`AddonProxyModule::init_module`]。全部协作对象已在装载时注入
    // （[`AddonHostServices`]），因此这里不需要读 `KernelContext`。

    fn init(&self, _kernel: &KernelContext) -> Result<(), KernelError> {
        self.init_module()
    }

    fn start(&self, _kernel: &KernelContext) -> Result<(), KernelError> {
        self.start_module()
    }

    fn stop(&self, _kernel: &KernelContext) -> Result<(), KernelError> {
        self.stop_module()
    }

    fn invoke(&self, command: &str, args: Value) -> Result<Value, KernelError> {
        self.with_session(|session| session.invoke(command, &args))
    }
}

/// [`ModuleLoadBackend`] 的受监管子进程实现。
pub struct HelperBackend {
    /// 用于构造能力派发器：插件的能力请求需要按会话身份查注册表。
    modules: Arc<ModuleRegistry>,
    /// 意图注册表：既供能力派发（插件发起意图），也供外部意图转发（插件声明处理器）。
    intents: Arc<IntentRegistry>,
    /// 模块沙箱：附加模块订阅事件前的逐次授权。
    sandbox: Arc<ModuleSandbox>,
    /// 事件推送桥：全体附加模块共用一份（一次 `*` 订阅）。
    bridge: Arc<AddonEventBridge>,
    /// 事件总线：`events.publish` 能力的落点（与桥共用同一条总线）。
    event_bus: Arc<EventBus>,
    /// 模块私有存储。全体附加模块共用一个实例，因此缓存与配额判定是全局一致的。
    storage: Arc<ModuleStorage>,
}

impl HelperBackend {
    pub fn new(
        modules: Arc<ModuleRegistry>,
        intents: Arc<IntentRegistry>,
        sandbox: Arc<ModuleSandbox>,
        events: Arc<EventBus>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            modules,
            intents,
            sandbox,
            bridge: Arc::new(AddonEventBridge::new(Arc::clone(&events))),
            event_bus: events,
            storage: Arc::new(ModuleStorage::new(&data_dir)),
        }
    }
}

impl ModuleLoadBackend for HelperBackend {
    fn name(&self) -> &'static str {
        "helper"
    }

    fn load(
        &self,
        manifest: &ModuleManifest,
        module_dir: &Path,
    ) -> Result<Arc<dyn Module>, KernelError> {
        let plugin_path = resolve_artifact(module_dir, manifest)?;
        let helper_program = locate_helper_program()?;
        let capabilities = Arc::new(KernelCapabilities::new(
            Arc::clone(&self.modules),
            Arc::clone(&self.intents),
            Arc::clone(&self.storage),
            Arc::clone(&self.event_bus),
            Arc::clone(&self.sandbox),
            // 发布上界按**本模块**的清单注入：能力派发器是每模块一份，不能共用。
            manifest.events.publish.clone(),
        ));

        // 此处只构造代理，不派生进程：进程在 `init` 时按注册表节奏启动，
        // 这样「扫描/校验失败」不会留下半启动的子进程。
        Ok(Arc::new(AddonProxyModule::new(
            manifest,
            module_dir.to_path_buf(),
            helper_program,
            plugin_path,
            AddonHostServices {
                capabilities,
                bridge: Arc::clone(&self.bridge),
                intents: Arc::clone(&self.intents),
                sandbox: Arc::clone(&self.sandbox),
            },
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_module_abi::helper_client::NoCapabilities;
    use std::collections::HashSet;
    use std::sync::OnceLock;
    use std::time::Instant;

    /// 与夹具插件清单一致的模块 id；不一致会让插件拒绝初始化。
    const FIXTURE_MODULE_ID: &str = "copper-lamp.demo-tools";

    /// 集成测试无法用 `CARGO_BIN_EXE_*` 拿到其它 crate 的产物，因此显式构建一次，
    /// 再在 target 目录里定位。`OnceLock` 避免并行测试重复构建。
    fn helper_program() -> PathBuf {
        static HELPER: OnceLock<PathBuf> = OnceLock::new();
        HELPER
            .get_or_init(|| {
                let status = std::process::Command::new(env!("CARGO"))
                    .args([
                        "build",
                        "-p",
                        "copper-module-abi",
                        "--bin",
                        "copper-module-helper",
                    ])
                    .status()
                    .expect("cargo must be available to build the helper");
                assert!(status.success(), "failed to build the helper binary");

                let candidate = target_debug_dir().join(helper_file_name());
                assert!(
                    candidate.is_file(),
                    "helper binary not found at {}",
                    candidate.display()
                );
                candidate
            })
            .clone()
    }

    fn fixture_plugin() -> PathBuf {
        static PLUGIN: OnceLock<PathBuf> = OnceLock::new();
        PLUGIN
            .get_or_init(|| {
                let status = std::process::Command::new(env!("CARGO"))
                    .args(["build", "-p", "abi-fixture-echo"])
                    .status()
                    .expect("cargo must be available to build the fixture plugin");
                assert!(status.success(), "failed to build the fixture plugin");

                let debug_dir = target_debug_dir();
                find_dylib(&debug_dir).unwrap_or_else(|| {
                    panic!(
                        "fixture plugin artifact not found under {}",
                        debug_dir.display()
                    )
                })
            })
            .clone()
    }

    fn target_debug_dir() -> PathBuf {
        let exe = std::env::current_exe().expect("test executable path");
        exe.parent()
            .and_then(Path::parent)
            .expect("test executable should live under target/<profile>/deps")
            .to_path_buf()
    }

    fn find_dylib(dir: &Path) -> Option<PathBuf> {
        let entries = std::fs::read_dir(dir).ok()?;
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    return false;
                };
                name.contains("abi_fixture_echo")
                    && (name.ends_with(".dll") || name.ends_with(".so") || name.ends_with(".dylib"))
            })
            .collect();
        candidates.sort();
        candidates.into_iter().next()
    }

    /// 与夹具插件同形的清单；`events` / `intents` 按用例给出。
    fn fixture_manifest(events: &[&str], intents: &[&str]) -> ModuleManifest {
        let raw = json!({
            "schema_version": "1",
            "id": FIXTURE_MODULE_ID,
            "i18n_namespace": "demo-tools",
            "display_name": "示例工具",
            "description": "示例模块",
            "author": { "name": "copper-lamp" },
            "license": "MIT",
            "version": "0.1.0",
            "platforms": ["windows-x86_64", "android-arm64", "linux-x86_64"],
            "launcher": { "min": "0.1.0", "max": null },
            "api_version": 1,
            "backend": {
                "crate": "abi-fixture-echo",
                "entry": "fixture",
                "artifact_glob": "abi_fixture_echo.dll"
            },
            "frontend": { "dist": "frontend/dist", "register": "register.js" },
            "permissions": [],
            "events": { "subscribe": events, "publish": [] },
            "intents": intents
        });
        ModuleManifest::parse_and_validate(&serde_json::to_vec(&raw).unwrap())
            .expect("夹具清单必须合法")
    }

    /// 组装一套可独立构造的宿主协作对象（**不需要**装配整个内核上下文）。
    fn host_services(
        capabilities: Arc<dyn CapabilityDispatcher>,
        granted: &[Permission],
    ) -> (
        AddonHostServices,
        Arc<EventBus>,
        Arc<IntentRegistry>,
        Arc<AddonEventBridge>,
    ) {
        let events = Arc::new(EventBus::new());
        let intents = Arc::new(IntentRegistry::new());
        let sandbox = Arc::new(ModuleSandbox::new());
        let granted: HashSet<Permission> = granted.iter().copied().collect();
        sandbox.grant(FIXTURE_MODULE_ID, granted, PathBuf::from("C:\\tmp\\module"));
        let bridge = Arc::new(AddonEventBridge::new(Arc::clone(&events)));

        (
            AddonHostServices {
                capabilities,
                bridge: Arc::clone(&bridge),
                intents: Arc::clone(&intents),
                sandbox,
            },
            events,
            intents,
            bridge,
        )
    }

    /// 构造一个接了真实夹具插件的代理模块。
    fn fixture_proxy(
        manifest: &ModuleManifest,
        granted: &[Permission],
    ) -> (
        Arc<AddonProxyModule>,
        Arc<EventBus>,
        Arc<IntentRegistry>,
        Arc<AddonEventBridge>,
    ) {
        let (services, events, intents, bridge) =
            host_services(Arc::new(NoCapabilities), granted);
        let proxy = Arc::new(AddonProxyModule::new(
            manifest,
            PathBuf::from("."),
            helper_program(),
            fixture_plugin(),
            services,
        ));
        (proxy, events, intents, bridge)
    }

    /// 轮询插件状态直到它收到至少 `expected` 条派发。
    ///
    /// 推送是异步的（专属线程 + 子进程），断言前必须等它真的落地；用轮询而不是
    /// `sleep(固定值)`，避免慢机器上出现假失败。
    fn wait_for_dispatches(proxy: &AddonProxyModule, expected: usize) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(state) = proxy.invoke("demo.state", json!({})) {
                let count = state["received"].as_array().map(Vec::len).unwrap_or(0);
                if count >= expected {
                    return true;
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn session_launches_a_real_plugin_and_round_trips_invoke() {
        let mut session = AddonSession::launch(
            &helper_program(),
            FIXTURE_MODULE_ID,
            &fixture_plugin(),
            &json!({ "greeting": "hi" }),
            Arc::new(NoCapabilities),
        )
        .expect("a real plugin artifact should launch");

        assert_eq!(session.module_id(), FIXTURE_MODULE_ID);
        session.start().unwrap();

        let echoed = session
            .invoke("demo.echo", &json!({ "value": 7 }))
            .unwrap();
        assert_eq!(echoed["echo"]["value"], json!(7));
        assert_eq!(echoed["started"], json!(true));
        assert!(
            echoed["config"]
                .as_str()
                .is_some_and(|config| config.contains("greeting")),
            "plugin should receive the host config, got {}",
            echoed["config"]
        );

        session.stop().unwrap();
        assert!(session.shutdown().unwrap());
    }

    #[test]
    fn session_rejects_a_module_id_mismatch_between_host_and_plugin() {
        let error = match AddonSession::launch(
            &helper_program(),
            "copper-lamp.other",
            &fixture_plugin(),
            &json!({}),
            Arc::new(NoCapabilities),
        ) {
            Ok(_) => panic!("a plugin whose declared id differs from the host's must be refused"),
            Err(error) => error,
        };

        match error {
            HelperError::Remote { code, .. } => assert_eq!(code, "plugin_init_failed"),
            other => panic!("expected a remote init failure, got {other}"),
        }
    }

    #[test]
    fn proxy_module_refuses_commands_before_init() {
        let manifest = fixture_manifest(&[], &[]);
        let (module, _events, _intents, _bridge) = fixture_proxy(&manifest, &[]);

        assert_eq!(module.id(), FIXTURE_MODULE_ID);
        assert_eq!(module.version(), "0.1.0");
        assert!(!module.is_running());
        assert!(module.stderr_tail().is_none());

        let error = module
            .invoke("demo.echo", json!({}))
            .expect_err("invoking before init must fail rather than pretend to work");
        assert!(matches!(error, KernelError::Module(_)));
    }

    #[test]
    fn declared_events_are_pushed_to_the_plugin() {
        let manifest = fixture_manifest(&["demo.*"], &[]);
        let (proxy, events, _intents, bridge) = fixture_proxy(&manifest, &[Permission::Events]);

        proxy.init_module().expect("夹具插件应能被初始化");
        proxy.start_module().expect("启动应成功");
        assert_eq!(
            bridge.subscribed(),
            vec![(FIXTURE_MODULE_ID.to_owned(), vec!["demo.*".to_owned()])],
            "start 成功后必须按声明登记订阅"
        );

        events.publish("demo.activity", json!({ "n": 7 }));
        assert!(
            wait_for_dispatches(&proxy, 1),
            "事件必须真的到达插件，而不是只写进了管道"
        );

        let state = proxy.invoke("demo.state", json!({})).unwrap();
        assert_eq!(state["received"][0]["command"], json!("event.demo.activity"));
        assert_eq!(state["received"][0]["payload"]["n"], json!(7));

        proxy.stop_module().unwrap();
        assert!(bridge.subscribed().is_empty(), "stop 必须撤销订阅");
        assert!(!proxy.is_running());
    }

    #[test]
    fn events_outside_the_declared_patterns_are_not_pushed() {
        let manifest = fixture_manifest(&["download.*"], &[]);
        let (proxy, events, _intents, _bridge) = fixture_proxy(&manifest, &[Permission::Events]);

        proxy.init_module().unwrap();
        proxy.start_module().unwrap();

        events.publish("settings.changed", json!({ "key": "theme" }));
        events.publish("download.created", json!({ "slug": "1.21.0" }));
        assert!(wait_for_dispatches(&proxy, 1));

        let state = proxy.invoke("demo.state", json!({})).unwrap();
        let received = state["received"].as_array().cloned().unwrap_or_default();
        assert_eq!(received.len(), 1, "只应收到声明范围内的事件：{received:?}");
        assert_eq!(received[0]["command"], json!("event.download.created"));

        proxy.stop_module().unwrap();
    }

    #[test]
    fn a_module_without_the_events_permission_does_not_get_subscribed() {
        let manifest = fixture_manifest(&["demo.*"], &[]);
        // 沙箱登记了该模块，但没授予 `Events`：订阅必须被拦下。
        let (proxy, events, _intents, bridge) = fixture_proxy(&manifest, &[]);

        proxy.init_module().unwrap();
        let error = proxy
            .start_module()
            .expect_err("越权订阅必须让启动失败，而不是放行");
        assert!(error.friendly().contains("events"), "got: {}", error.friendly());

        assert!(bridge.subscribed().is_empty(), "越权不得留下订阅");
        assert!(!proxy.is_running(), "回滚必须把子进程一并回收");

        // 订阅未建立，事件不应让任何东西崩溃。
        events.publish("demo.activity", json!({}));
    }

    #[test]
    fn declared_intents_are_forwarded_and_withdrawn() {
        let manifest = fixture_manifest(&[], &["demo.ping"]);
        let (proxy, _events, intents, _bridge) = fixture_proxy(&manifest, &[Permission::Intents]);

        proxy.init_module().unwrap();
        proxy.start_module().unwrap();

        let result = intents
            .request("demo.ping", json!({ "n": 1 }))
            .expect("start 成功后声明的意图必须可用");
        assert_eq!(result["handled"], json!("intent.demo.ping"));

        proxy.stop_module().unwrap();
        // 停止后必须注销：否则意图请求会打到已停的模块上。
        let error = intents
            .request("demo.ping", json!({}))
            .expect_err("停止后意图必须已注销");
        assert!(error.friendly().contains("无模块声明"), "got: {}", error.friendly());
    }

    #[test]
    fn a_module_declaring_nothing_still_starts_and_stays_silent() {
        let manifest = fixture_manifest(&[], &[]);
        let (proxy, events, intents, bridge) = fixture_proxy(&manifest, &[]);

        proxy.init_module().unwrap();
        proxy.start_module().expect("空声明不是错误，只是什么都不接");

        assert!(bridge.subscribed().is_empty());
        assert!(intents.declared().is_empty());
        events.publish("demo.activity", json!({}));
        std::thread::sleep(Duration::from_millis(50));

        // 收不到任何事件：插件侧记账仍为空。
        let state = proxy.invoke("demo.state", json!({})).unwrap();
        assert_eq!(state["received"].as_array().map(Vec::len), Some(0));

        proxy.stop_module().unwrap();
    }

    #[test]
    fn a_failed_invoke_reports_the_error_without_deadlocking_on_its_own_session_lock() {
        let manifest = fixture_manifest(&[], &[]);
        let (proxy, _events, _intents, _bridge) = fixture_proxy(&manifest, &[]);
        proxy.init_module().unwrap();
        proxy.start_module().unwrap();

        // 超过 IPC 帧上限的参数会让写帧就地失败（不涉及超时，因此本用例很快）。
        // 该失败路径需要读会话取 stderr——**在持有会话锁的前提下**再取一次就会自锁，
        // 症状是整个内核线程永久卡住。用线程 + 超时把这种回归变成"失败"而不是"挂住"。
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = {
            let proxy = Arc::clone(&proxy);
            std::thread::spawn(move || {
                let big = "x".repeat(9 * 1024 * 1024);
                let result = proxy.invoke("demo.echo", json!({ "big": big }));
                let _ = sender.send(result.is_err());
            })
        };

        let reported = receiver.recv_timeout(Duration::from_secs(10));
        let _ = worker.join();

        assert!(
            reported.unwrap_or(false),
            "写帧失败必须如实报错，且不得因二次加锁会话而死锁"
        );

        // 出错之后会话仍然可用（错误路径不该破坏会话状态）。
        let state = proxy.invoke("demo.state", json!({})).unwrap();
        assert_eq!(state["started"], json!(true));

        proxy.stop_module().unwrap();
    }
}
