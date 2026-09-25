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
//! - [`AddonProxyModule`]：把会话适配成 [`Module`] 契约，注册表因此不必知道
//!   模块究竟在进程内还是进程外。
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

use copper_module_abi::helper_client::{HelperError, HelperProcess};

use crate::error::KernelError;
use crate::registry::dylib_backend::resolve_artifact;
use crate::registry::loader::ModuleLoadBackend;
use crate::registry::manifest::ModuleManifest;
use crate::registry::modules::Module;
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
    ) -> Result<Self, HelperError> {
        let mut process = HelperProcess::spawn(helper_program, module_id, plugin_path)?;
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

    /// 优雅停机；返回进程是否在超时前自行退出。
    pub fn shutdown(&mut self) -> Result<bool, HelperError> {
        self.process.shutdown(SHUTDOWN_TIMEOUT)
    }

    /// 子进程 stderr 尾部（限长），仅在诊断失败原因时使用。
    pub fn stderr(&self) -> String {
        self.process.stderr()
    }
}

/// 附加模块在注册表中的代理：把 [`Module`] 生命周期映射到 helper 子进程。
pub struct AddonProxyModule {
    id: String,
    version: String,
    module_dir: PathBuf,
    helper_program: PathBuf,
    plugin_path: PathBuf,
    /// 会话只在 `init` 成功后存在；`stop` 会取走它以确保进程被回收。
    session: Mutex<Option<AddonSession>>,
}

impl AddonProxyModule {
    pub fn new(
        id: impl Into<String>,
        version: impl Into<String>,
        module_dir: PathBuf,
        helper_program: PathBuf,
        plugin_path: PathBuf,
    ) -> Self {
        Self {
            id: id.into(),
            version: version.into(),
            module_dir,
            helper_program,
            plugin_path,
            session: Mutex::new(None),
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

    fn wrap(&self, error: HelperError) -> KernelError {
        let detail = match self.stderr_tail() {
            Some(stderr) if !stderr.trim().is_empty() => format!("{error}（helper stderr：{stderr}）"),
            _ => error.to_string(),
        };
        KernelError::Module(format!("附加模块 `{}` 的受监管进程失败：{detail}", self.id))
    }

    fn with_session<T>(
        &self,
        action: impl FnOnce(&mut AddonSession) -> Result<T, HelperError>,
    ) -> Result<T, KernelError> {
        let mut guard = self.session.lock();
        let session = guard.as_mut().ok_or_else(|| self.not_running())?;
        action(session).map_err(|error| self.wrap(error))
    }

    fn not_running(&self) -> KernelError {
        KernelError::Module(format!(
            "附加模块 `{}` 的后端进程未在运行（尚未初始化或已停止）",
            self.id
        ))
    }
}

impl Module for AddonProxyModule {
    fn id(&self) -> &str {
        &self.id
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn init(&self, _kernel: &KernelContext) -> Result<(), KernelError> {
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

    fn start(&self, _kernel: &KernelContext) -> Result<(), KernelError> {
        self.with_session(AddonSession::start)
    }

    fn stop(&self, _kernel: &KernelContext) -> Result<(), KernelError> {
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

    fn invoke(&self, command: &str, args: Value) -> Result<Value, KernelError> {
        self.with_session(|session| session.invoke(command, &args))
    }
}

/// [`ModuleLoadBackend`] 的受监管子进程实现。
pub struct HelperBackend;

impl HelperBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HelperBackend {
    fn default() -> Self {
        Self::new()
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

        // 此处只构造代理，不派生进程：进程在 `init` 时按注册表节奏启动，
        // 这样「扫描/校验失败」不会留下半启动的子进程。
        Ok(Arc::new(AddonProxyModule::new(
            manifest.id.clone(),
            manifest.version.clone(),
            module_dir.to_path_buf(),
            helper_program,
            plugin_path,
        )))
    }
}
