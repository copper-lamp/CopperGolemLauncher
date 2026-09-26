//! 受监管 Node 会话装载后端：声明了 `runtime` 的附加模块由内核派生并监管一个
//! Node Agent 进程，经统一 NDJSON 协议与它对话。
//!
//! # 与 [`crate::registry::helper_backend`] 的关系
//!
//! 两者都实现 [`ModuleLoadBackend`]，都把「模块」适配成 [`Module`] 生命周期；差别
//! 只在被监管的进程是什么：helper 后端跑的是同源的 Rust 动态库宿主，Node 后端跑的是
//! 模块自带的 `runtime/` 下的 Agent 脚本。分流由 [`crate::registry::backend_router`]
//! 按清单决定。
//!
//! # 校验必须在派生进程之前完成
//!
//! [`NodeBackend::load`] 把「平台 / 入口 / 解释器 / 版本」四道校验全部做完才返回代理
//! 模块，任何一步失败都不产生进程。这与 helper 后端「只构造代理、进程在 `init` 时起」
//! 是同一原则：装载期失败不该留下半启动的子进程。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{json, Value};

use copper_module_abi::helper_client::CapabilityDispatcher;
use copper_module_abi::ipc::{AGENT_METHODS, METHOD_AGENT_PING};

use crate::error::KernelError;
use crate::registry::capability::KernelCapabilities;
use crate::registry::events::EventBus;
use crate::registry::intents::IntentRegistry;
use crate::registry::loader::ModuleLoadBackend;
use crate::registry::manifest::ModuleManifest;
use crate::registry::module_storage::ModuleStorage;
use crate::registry::modules::{Module, ModuleRegistry};
use crate::registry::runtime::{
    inspect_node, resolve_node_executable, resolve_runtime_entry, validate_runtime_platform,
    NodeRuntimeSession,
};
use crate::registry::sandbox::ModuleSandbox;
use crate::state::KernelContext;

/// 请求 Agent 优雅退出的等待上限。与 helper 停机保持同一量级。
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// 把底层原因包装成带模块 id 的装载错误。
///
/// 模块作者 / 用户看到的是启动日志里的一行文本，说不出「哪个模块、错在哪」就等于
/// 没报错，因此每个失败点都必须带上 id 与具体原因。
fn runtime_load_error(module_id: &str, reason: impl std::fmt::Display) -> KernelError {
    KernelError::Module(format!("模块 `{module_id}` 的 runtime 装载失败：{reason}"))
}

/// [`ModuleLoadBackend`] 的受监管 Node 会话实现。
pub struct NodeBackend {
    /// 用于构造能力派发器：Agent 反向请求需要按会话身份查注册表。
    modules: Arc<ModuleRegistry>,
    /// 意图注册表：`intent.request` 能力的落点。
    intents: Arc<IntentRegistry>,
    /// 模块私有存储。全体附加模块共用一个实例，因此命名空间与配额判定全局一致。
    storage: Arc<ModuleStorage>,
    /// 事件总线：`events.publish` 能力的落点。
    event_bus: Arc<EventBus>,
    /// 模块沙箱：需要权限的能力在此逐次授权。
    sandbox: Arc<ModuleSandbox>,
    /// 显式指定的 Node 可执行文件；`None` 表示按 `PATH` 查找。
    ///
    /// 本批生产装配传 `None`；留作后续「设置项指定 Node 路径」的接入点，因此现在就
    /// 把它作为构造参数，避免将来再改一次装配链路。
    explicit_node: Option<PathBuf>,
}

impl NodeBackend {
    pub fn new(
        modules: Arc<ModuleRegistry>,
        intents: Arc<IntentRegistry>,
        sandbox: Arc<ModuleSandbox>,
        events: Arc<EventBus>,
        data_dir: PathBuf,
        explicit_node: Option<PathBuf>,
    ) -> Self {
        Self {
            modules,
            intents,
            storage: Arc::new(ModuleStorage::new(&data_dir)),
            event_bus: events,
            sandbox,
            explicit_node,
        }
    }
}

impl ModuleLoadBackend for NodeBackend {
    fn name(&self) -> &'static str {
        "node"
    }

    fn load(
        &self,
        manifest: &ModuleManifest,
        module_dir: &Path,
    ) -> Result<Arc<dyn Module>, KernelError> {
        let module_id = manifest.id.as_str();

        // 路由已保证到这里必有 runtime；这里仍 fail closed，避免直接调用本后端时
        // 用一个空 runtime 造出无法启动的代理。
        let Some(runtime) = manifest.runtime.as_ref() else {
            return Err(runtime_load_error(module_id, "清单未声明 runtime"));
        };

        // 顺序即安全边界：全部校验都先于任何进程派生。
        validate_runtime_platform().map_err(|error| runtime_load_error(module_id, error))?;
        let entry = resolve_runtime_entry(module_dir, runtime)
            .map_err(|error| runtime_load_error(module_id, error))?;
        let executable = resolve_node_executable(
            self.explicit_node.as_deref(),
            &std::env::var_os("PATH").unwrap_or_default(),
            cfg!(windows),
        )
        .map_err(|error| runtime_load_error(module_id, error))?;
        let node_version = inspect_node(&executable, runtime)
            .map_err(|error| runtime_load_error(module_id, error))?;

        // 能力派发器是**每模块一份**：发布上界按本模块清单注入，不能共用。
        let capabilities: Arc<dyn CapabilityDispatcher> = Arc::new(KernelCapabilities::new(
            Arc::clone(&self.modules),
            Arc::clone(&self.intents),
            Arc::clone(&self.storage),
            Arc::clone(&self.event_bus),
            Arc::clone(&self.sandbox),
            manifest.events.publish.clone(),
        ));

        // 只构造代理，不派生进程：进程在 `init` 时按注册表节奏启动。
        Ok(Arc::new(NodeAgentProxyModule::new(
            manifest,
            executable,
            entry,
            node_version,
            capabilities,
        )))
    }
}

fn not_running_error(module_id: &str) -> KernelError {
    KernelError::Module(format!(
        "模块 `{module_id}` 的受监管 Node 会话未在运行（尚未初始化或已停止）"
    ))
}

/// 受监管 Node 模块在注册表中的代理：把 [`Module`] 生命周期映射到 [`NodeRuntimeSession`]。
pub struct NodeAgentProxyModule {
    id: String,
    version: String,
    executable: PathBuf,
    entry: PathBuf,
    /// 已通过校验的 Node 版本，仅用于日志（装载期已经确认满足要求）。
    node_version: semver::Version,
    capabilities: Arc<dyn CapabilityDispatcher>,
    /// 会话只在 `init` 成功后存在；`stop` 会取走它以确保进程被回收。
    session: Mutex<Option<NodeRuntimeSession>>,
}

impl NodeAgentProxyModule {
    pub fn new(
        manifest: &ModuleManifest,
        executable: PathBuf,
        entry: PathBuf,
        node_version: semver::Version,
        capabilities: Arc<dyn CapabilityDispatcher>,
    ) -> Self {
        Self {
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            executable,
            entry,
            node_version,
            capabilities,
            session: Mutex::new(None),
        }
    }

    /// 会话当前是否在运行。
    pub fn is_running(&self) -> bool {
        self.session.lock().is_some()
    }

    fn with_session<T>(
        &self,
        action: impl FnOnce(&mut NodeRuntimeSession) -> Result<T, KernelError>,
    ) -> Result<T, KernelError> {
        let mut guard = self.session.lock();
        let Some(session) = guard.as_mut() else {
            return Err(not_running_error(&self.id));
        };
        action(session)
    }

    /// 派生会话（`Module::init` 的全部内容）。
    ///
    /// 独立于 [`Module`] 契约的无参入口：`Module::init` 拿到了 `&KernelContext`，但本
    /// 模块的全部协作对象已在装载时注入，构造整个内核上下文只为调一次生命周期会让
    /// 这条链路无法在真实子进程上被测试（与 [`crate::registry::helper_backend`] 同一
    /// 考虑）。`Module::init` 只是转调本方法。
    pub fn init_module(&self) -> Result<(), KernelError> {
        let session = NodeRuntimeSession::launch(
            &self.executable,
            &self.entry,
            Arc::clone(&self.capabilities),
            &self.id,
        )?;
        log::info!(
            "[addon/{}] 受监管 Node 会话已就绪（Node {}，入口 {}）",
            self.id,
            self.node_version,
            self.entry.display()
        );
        *self.session.lock() = Some(session);
        Ok(())
    }

    /// 确认会话可用（`Module::start` 的全部内容）。
    ///
    /// `init` 只保证进程起来了，不保证协议侧真的会应答；发一次 ping 才算「开始工作」。
    pub fn start_module(&self) -> Result<(), KernelError> {
        self.with_session(|session| session.request(METHOD_AGENT_PING, json!({})))?;
        Ok(())
    }

    /// 优雅停机并回收会话（`Module::stop` 的全部内容）。幂等。
    pub fn stop_module(&self) -> Result<(), KernelError> {
        // 先取走会话：无论停机是否报错，进程与 IPC 资源都必须被回收（`Drop` 兜底终止）。
        let Some(mut session) = self.session.lock().take() else {
            return Ok(());
        };
        session.shutdown(SHUTDOWN_TIMEOUT)
    }
}

impl Module for NodeAgentProxyModule {
    fn id(&self) -> &str {
        &self.id
    }

    fn version(&self) -> &str {
        &self.version
    }

    // 不覆写 `display_name`：展示名交由前端 i18n 语言包按命名空间解析。

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
        // 白名单：只转发受监管 Agent 协议里已定义的方法，不做任意方法透传。
        // 模块自带脚本无法经此通道让内核执行计划外的方法。
        if !AGENT_METHODS.iter().any(|method| *method == command) {
            return Err(KernelError::Module(format!(
                "模块 `{}` 不接受命令 `{command}`：不在受监管 Agent 方法白名单内",
                self.id
            )));
        }
        self.with_session(|session| session.request(command, args))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use copper_module_abi::helper_client::NoCapabilities;
    use serde_json::json;

    use super::*;

    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "copper-node-backend-{}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 声明受监管 Node 运行时的清单。
    fn runtime_manifest(node_requirement: &str) -> ModuleManifest {
        let raw = json!({
            "schema_version": "2",
            "id": "copper-lamp.agent",
            "i18n_namespace": "agent",
            "display_name": "AI 助手",
            "description": "受监管 Node 运行时模块",
            "author": { "name": "copper-lamp" },
            "license": "MIT",
            "version": "0.1.0",
            "platforms": ["windows-x86_64", "linux-x86_64"],
            "launcher": { "min": "0.1.0", "max": null },
            "api_version": 2,
            "backend": {
                "crate": "copper-module-agent",
                "entry": "copper_module_agent::AgentModule",
                "artifact_glob": "copper_module_agent.dll"
            },
            "frontend": { "dist": "frontend/dist", "register": "register.js" },
            "runtime": {
                "kind": "node",
                "entry": "runtime/agent.mjs",
                "engines": { "node": node_requirement },
                "wire_protocol": { "id": "copper-addon.ndjson", "version": 2 }
            }
        });
        ModuleManifest::parse_and_validate(&serde_json::to_vec(&raw).unwrap())
            .expect("夹具清单必须合法")
    }

    fn backend(data_dir: PathBuf, explicit_node: Option<PathBuf>) -> NodeBackend {
        NodeBackend::new(
            Arc::new(ModuleRegistry::new()),
            Arc::new(IntentRegistry::new()),
            Arc::new(ModuleSandbox::new()),
            Arc::new(EventBus::new()),
            data_dir,
            explicit_node,
        )
    }

    #[test]
    fn a_missing_runtime_entry_is_reported_with_the_module_id_and_no_process() {
        let temp = TempDir::new();
        // 只建目录，不写 runtime/agent.mjs：入口不存在。
        let backend = backend(temp.0.clone(), None);

        let error = backend
            .load(&runtime_manifest(">=22.19.0"), &temp.0)
            .expect_err("runtime 入口不存在必须装载失败");

        let message = error.friendly();
        assert!(message.contains("copper-lamp.agent"), "错误必须带模块 id：{message}");
        assert!(message.contains("runtime"), "错误必须指出 runtime 入口问题：{message}");
    }

    #[test]
    fn a_node_version_that_does_not_satisfy_engines_is_rejected() {
        let temp = TempDir::new();
        let runtime_dir = temp.0.join("runtime");
        fs::create_dir_all(&runtime_dir).unwrap();
        fs::write(runtime_dir.join("agent.mjs"), b"process.exit(0)").unwrap();

        // 先确认本机确实有一份可用的 Node，否则本用例退化成了「找不到 Node」。
        resolve_node_executable(
            None,
            &std::env::var_os("PATH").unwrap_or_default(),
            cfg!(windows),
        )
        .expect("本用例要求本机装有 Node");

        let backend = backend(temp.0.clone(), None);
        // 要求一个任何真实 Node 都不会满足的版本区间：必须在派生进程之前被拒。
        let error = backend
            .load(&runtime_manifest(">=999.0.0"), &temp.0)
            .expect_err("engines.node 不满足必须装载失败");

        let message = error.friendly();
        assert!(message.contains("copper-lamp.agent"), "错误必须带模块 id：{message}");
        assert!(
            message.contains("不满足") || message.contains("engines") || message.contains("版本"),
            "错误必须指出版本不满足：{message}"
        );
    }

    #[test]
    fn a_command_outside_the_agent_whitelist_is_refused() {
        let manifest = runtime_manifest(">=22.19.0");
        let module = NodeAgentProxyModule::new(
            &manifest,
            PathBuf::from("node"),
            PathBuf::from("runtime/agent.mjs"),
            semver::Version::new(22, 19, 0),
            Arc::new(NoCapabilities),
        );

        let error = module
            .invoke("agent.evil", json!({}))
            .expect_err("白名单外的命令必须被拒，而不是透传");
        assert!(
            error.friendly().contains("不接受命令"),
            "got: {}",
            error.friendly()
        );
        assert!(
            error.friendly().contains("agent.evil"),
            "错误应指出被拒的命令名：{}",
            error.friendly()
        );
    }

    #[test]
    fn proxy_lifecycle_launches_pings_and_reaps_the_real_agent_process() {
        let temp = TempDir::new();
        let runtime_dir = temp.0.join("runtime");
        fs::create_dir_all(&runtime_dir).unwrap();
        let entry = runtime_dir.join("agent.mjs");
        // 假 Agent：实现握手、ping、shutdown；用标记文件让宿主能验证进程确实退出。
        fs::write(
            &entry,
            r#"import readline from 'node:readline';
import fs from 'node:fs';
import path from 'node:path';
const marker = path.join(path.dirname(process.argv[1]), 'alive.marker');
fs.writeFileSync(marker, String(process.pid));
process.on('exit', () => { try { fs.unlinkSync(marker); } catch {} });
const input = readline.createInterface({ input: process.stdin });
input.on('line', (line) => {
  const frame = JSON.parse(line);
  if (frame.kind === 'hello') {
    process.stdout.write(JSON.stringify({ kind: 'hello', version: 2, protocol: 'copper-addon.ndjson', supported_versions: [2], runtime: 'test-agent', capabilities: [] }) + '\n');
  } else if (frame.method === 'agent.ping') {
    process.stdout.write(JSON.stringify({ kind: 'response', version: 2, id: frame.id, result: { pong: true } }) + '\n');
  } else if (frame.method === 'agent.shutdown') {
    process.stdout.write(JSON.stringify({ kind: 'response', version: 2, id: frame.id, result: {} }) + '\n');
    process.exit(0);
  }
});"#,
        )
        .unwrap();

        let executable = resolve_node_executable(
            None,
            &std::env::var_os("PATH").unwrap_or_default(),
            cfg!(windows),
        )
        .expect("本用例要求本机装有 Node");

        let manifest = runtime_manifest(">=22.19.0");
        let module = NodeAgentProxyModule::new(
            &manifest,
            executable,
            entry.clone(),
            semver::Version::new(22, 19, 0),
            Arc::new(NoCapabilities),
        );

        let marker = runtime_dir.join("alive.marker");
        module.init_module().expect("会话应能派生并完成握手");
        assert!(module.is_running());
        module.start_module().expect("ping 应确认会话可用");
        let pong = module
            .invoke(METHOD_AGENT_PING, json!({}))
            .expect("白名单内的 agent.ping 应被转发");
        assert_eq!(pong["pong"], json!(true));

        module.stop_module().expect("停机应成功");
        assert!(!module.is_running(), "stop 后会话必须已被取走");

        // 进程真正退出后，假 Agent 的退出钩子会删掉标记文件。
        let deadline = Instant::now() + Duration::from_secs(5);
        while marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!marker.exists(), "stop 后 Agent 进程必须已退出");

        // 重复 stop 幂等。
        module.stop_module().expect("重复 stop 必须幂等成功");
        // 会话已停：再调用命令应报「未在运行」，而不是空转。
        assert!(module.invoke(METHOD_AGENT_PING, json!({})).is_err());
    }
}
