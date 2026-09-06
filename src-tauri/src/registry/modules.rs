//! 模块契约与注册表：铜内核的插拔能力载体。
//!
//! 模块 = Rust crate（后端）+ Vue 包（前端）。内置模块静态编译进内核，
//! 由本注册表装载并驱动生命周期；附加模块（动态加载）未来复用同一 trait。
//!
//! 模块间不直接依赖，一切协同经事件总线 + 意图注册表中转。

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::Serialize;

use crate::error::KernelError;
use crate::state::KernelContext;

/// 模块后端契约。
///
/// 生命周期：`init`（注册命令 / 订阅 / 声明意图 / schema 迁移）→ `start`（开始工作）→ `stop`（清理）。
pub trait Module: Send + Sync {
    /// 模块唯一标识（如 `home`、`game-download`、`content-download`）。
    fn id(&self) -> &'static str;

    /// 初始化：事件订阅、意图声明、数据库 schema（调用 `kernel.db().migrate_scope("module:<id>", …)`）。
    fn init(&self, kernel: &KernelContext) -> Result<(), KernelError>;

    /// 启动：模块开始工作。
    fn start(&self, kernel: &KernelContext) -> Result<(), KernelError>;

    /// 停止：退订、注销意图、释放资源。
    fn stop(&self, kernel: &KernelContext) -> Result<(), KernelError>;
}

/// 模块运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleState {
    Stopped,
    Running,
    Failed,
}

/// 模块信息（供设置页"模块"Tab 展示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ModuleInfo {
    pub id: String,
    pub enabled: bool,
    pub is_builtin: bool,
    pub state: ModuleState,
    pub error: Option<String>,
}

/// 模块注册表。
pub struct ModuleRegistry {
    modules: RwLock<Vec<Arc<dyn Module>>>,
    states: RwLock<HashMap<String, ModuleState>>,
    errors: RwLock<HashMap<String, String>>,
}

impl ModuleRegistry {
    pub fn new() -> Self {
        Self {
            modules: RwLock::new(Vec::new()),
            states: RwLock::new(HashMap::new()),
            errors: RwLock::new(HashMap::new()),
        }
    }

    /// 注册模块（内核启动前静态注册）。
    pub fn register(&self, module: Arc<dyn Module>) {
        self.modules.write().push(module);
    }

    /// 依次装载全部已启用模块：`init` → `start`。任一失败即中止并标记 Failed。
    pub fn boot(&self, kernel: &KernelContext) {
        let modules = self.modules.read().clone();
        for m in modules {
            if !self.is_enabled(kernel, m.id()) {
                continue;
            }
            let result = m.init(kernel).and_then(|_| m.start(kernel));
            match result {
                Ok(()) => {
                    self.states.write().insert(m.id().to_string(), ModuleState::Running);
                }
                Err(e) => {
                    self.states
                        .write()
                        .insert(m.id().to_string(), ModuleState::Failed);
                    self.errors.write().insert(m.id().to_string(), e.friendly());
                    log::error!("module {} failed to boot: {e}", m.id());
                }
            }
        }
    }

    /// 逆序停止全部模块（应用退出时调用）。
    pub fn shutdown(&self, kernel: &KernelContext) {
        let modules = self.modules.read().clone();
        for m in modules.iter().rev() {
            if let Err(e) = m.stop(kernel) {
                log::error!("module {} failed to stop: {e}", m.id());
            }
            self.states
                .write()
                .insert(m.id().to_string(), ModuleState::Stopped);
        }
    }

    /// 模块是否启用（设置 `modules.<id>.enabled`，默认启用）。
    pub fn is_enabled(&self, kernel: &KernelContext, id: &str) -> bool {
        kernel
            .settings()
            .get_or(&format!("modules.{id}.enabled"), true)
    }

    /// 切换模块启用状态（下次启动生效；正在运行的模块不受影响）。
    pub fn set_enabled(&self, kernel: &KernelContext, id: &str, enabled: bool) -> Result<(), KernelError> {
        if !self.modules.read().iter().any(|m| m.id() == id) {
            return Err(KernelError::Module(format!("模块 `{id}` 不存在")));
        }
        kernel
            .settings()
            .set(&format!("modules.{id}.enabled"), &enabled)
    }

    /// 模块信息列表（按注册顺序）。
    pub fn list(&self, kernel: &KernelContext) -> Vec<ModuleInfo> {
        let modules = self.modules.read().clone();
        modules
            .iter()
            .map(|m| {
                let id = m.id();
                ModuleInfo {
                    id: id.to_string(),
                    enabled: self.is_enabled(kernel, id),
                    is_builtin: true,
                    state: self.states.read().get(id).copied().unwrap_or(ModuleState::Stopped),
                    error: self.errors.read().get(id).cloned(),
                }
            })
            .collect()
    }
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}
