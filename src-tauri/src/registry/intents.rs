//! 意图注册表：模块声明"我能做什么"，其他模块发起"我要什么"，
//! 内核按意图名分发到唯一声明的处理者。用于请求 / 响应式联动。

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde_json::Value;

use crate::error::KernelError;
use crate::registry::sandbox::{ModuleSandbox, Permission};

/// 意图处理者：接收负载，返回响应（同步；重活应自行 `tokio::spawn`）。
pub type IntentHandler = Arc<dyn Fn(Value) -> Result<Value, KernelError> + Send + Sync>;

/// 意图注册表。
pub struct IntentRegistry {
    /// 意图名 -> (声明模块, 处理者)
    handlers: RwLock<HashMap<String, (String, IntentHandler)>>,
    /// 模块沙箱：绑定后对已登记的附加模块强制 `intents` 权限。
    sandbox: RwLock<Option<Arc<ModuleSandbox>>>,
}

impl IntentRegistry {
    pub fn new() -> Self {
        Self {
            handlers: RwLock::new(HashMap::new()),
            sandbox: RwLock::new(None),
        }
    }

    /// 声明能力。同一意图重复声明报错，避免静默覆盖导致行为不可预期。
    pub fn declare(
        &self,
        intent: &str,
        module_id: &str,
        handler: IntentHandler,
    ) -> Result<(), KernelError> {
        let mut map = self.handlers.write();
        if let Some((owner, _)) = map.get(intent) {
            return Err(KernelError::Intent(format!(
                "intent `{intent}` 已被模块 `{owner}` 声明，模块 `{module_id}` 重复声明"
            )));
        }
        map.insert(intent.to_string(), (module_id.to_string(), handler));
        Ok(())
    }

    /// 发起意图。未声明返回错误（调用方决定是否降级提示）。
    pub fn request(&self, intent: &str, payload: Value) -> Result<Value, KernelError> {
        let map = self.handlers.read();
        let (_, handler) = map
            .get(intent)
            .ok_or_else(|| KernelError::Intent(format!("意图 `{intent}` 无模块声明")))?;
        handler(payload)
    }

    /// 附带沙箱校验地发起意图（附加模块的调用路径）。
    ///
    /// 先确认调用方已声明 `intents` 权限，再转发；校验失败不会触及处理者，
    /// 保证未授权模块无法借他人实现间接获得能力。
    pub fn request_checked(
        &self,
        caller: &str,
        intent: &str,
        payload: Value,
    ) -> Result<Value, KernelError> {
        self.enforce_caller(caller, Permission::Intents, "intents.request", intent)?;
        self.request(intent, payload)
    }

    /// 附带沙箱校验地声明意图（附加模块的调用路径）。
    pub fn declare_checked(
        &self,
        module_id: &str,
        intent: &str,
        handler: IntentHandler,
    ) -> Result<(), KernelError> {
        self.enforce_caller(module_id, Permission::Intents, "intents.declare", intent)?;
        self.declare(intent, module_id, handler)
    }

    /// 统一的能力校验入口：内置模块不经沙箱（与内核同信任级），
    /// 附加模块必须持有对应权限。模块是否受管辖由沙箱是否已登记决定。
    fn enforce_caller(
        &self,
        module_id: &str,
        permission: Permission,
        operation: &str,
        target: &str,
    ) -> Result<(), KernelError> {
        // 先取读锁再判定：未登记授权的模块视为内核内置，直接放行。
        let guard = self.sandbox.read();
        match guard.as_ref() {
            Some(sb) if sb.knows(module_id) => {
                sb.enforce(module_id, permission, operation, target)
            }
            _ => Ok(()),
        }
    }

    /// 绑定沙箱（内核启动时调用一次）。未绑定时不做权限校验，
    /// 使注册表可独立测试，也避免内置模块路径上引入无意义的开销。
    pub fn bind_sandbox(&self, sandbox: Arc<ModuleSandbox>) {
        *self.sandbox.write() = Some(sandbox);
    }

    /// 注销某模块声明的全部意图（模块停止时调用）。
    pub fn withdraw(&self, module_id: &str) {
        let mut map = self.handlers.write();
        map.retain(|_, (owner, _)| owner != module_id);
    }

    /// 已声明意图清单（模块名 -> 意图列表）。
    pub fn declared(&self) -> Vec<(String, String)> {
        self.handlers
            .read()
            .iter()
            .map(|(intent, (owner, _))| (owner.clone(), intent.clone()))
            .collect()
    }
}

impl Default for IntentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::sandbox::Permission;
    use serde_json::json;
    use std::collections::HashSet;

    fn handler(v: Value) -> Result<Value, KernelError> {
        Ok(v)
    }

    fn arc_handler() -> IntentHandler {
        Arc::new(handler)
    }

    fn sandbox_with(module_id: &str, perms: &[Permission]) -> Arc<ModuleSandbox> {
        let sb = Arc::new(ModuleSandbox::new());
        let set: HashSet<Permission> = perms.iter().copied().collect();
        sb.grant(module_id, set, std::path::PathBuf::from("C:\\tmp\\module"));
        sb
    }

    #[test]
    fn request_and_declare_work_without_sandbox() {
        let reg = IntentRegistry::new();
        reg.declare("game.list", "home", arc_handler()).unwrap();
        let out = reg.request("game.list", json!({"a": 1})).unwrap();
        assert_eq!(out["a"], 1);
    }

    #[test]
    fn duplicate_declaration_is_rejected() {
        let reg = IntentRegistry::new();
        reg.declare("x.y", "home", arc_handler()).unwrap();
        let err = reg.declare("x.y", "other", arc_handler()).unwrap_err();
        assert!(err.friendly().contains("重复声明"));
    }

    #[test]
    fn unchecked_request_errors_when_no_declaration() {
        let reg = IntentRegistry::new();
        let err = reg.request("missing.intent", json!({})).unwrap_err();
        assert!(err.friendly().contains("无模块声明"));
    }

    #[test]
    fn addon_without_intents_permission_is_blocked() {
        let reg = IntentRegistry::new();
        reg.bind_sandbox(sandbox_with("addon", &[Permission::Network]));

        // 处理者的存在不应让未授权调用方绕过校验。
        reg.declare("game.list", "home", arc_handler()).unwrap();
        let err = reg
            .request_checked("addon", "game.list", json!({}))
            .unwrap_err();
        assert!(err.friendly().contains("intents"), "got: {}", err.friendly());
    }

    #[test]
    fn addon_with_intents_permission_passes() {
        let reg = IntentRegistry::new();
        reg.bind_sandbox(sandbox_with("addon", &[Permission::Intents]));
        reg.declare("game.list", "home", arc_handler()).unwrap();

        let out = reg.request_checked("addon", "game.list", json!({"ok": true})).unwrap();
        assert_eq!(out["ok"], true);
    }

    #[test]
    fn builtin_module_is_not_policed_by_sandbox() {
        let reg = IntentRegistry::new();
        // 沙箱只登记了 addon，内置模块 `home` 未登记 -> 放行。
        reg.bind_sandbox(sandbox_with("addon", &[Permission::Intents]));
        reg.declare("game.list", "home", arc_handler()).unwrap();

        let out = reg.request_checked("home", "game.list", json!({"n": 2})).unwrap();
        assert_eq!(out["n"], 2);
    }

    #[test]
    fn declare_checked_requires_permission() {
        let reg = IntentRegistry::new();
        reg.bind_sandbox(sandbox_with("addon", &[Permission::Network]));
        let err = reg
            .declare_checked("addon", "x.y", arc_handler())
            .unwrap_err();
        assert!(err.friendly().contains("intents"), "got: {}", err.friendly());
    }

    #[test]
    fn withdraw_removes_all_intents_of_module() {
        let reg = IntentRegistry::new();
        reg.declare("a.b", "addon", arc_handler()).unwrap();
        reg.declare("c.d", "addon", arc_handler()).unwrap();
        assert_eq!(reg.declared().len(), 2);

        reg.withdraw("addon");
        assert!(reg.declared().is_empty());
    }
}
