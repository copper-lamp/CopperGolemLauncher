//! 意图注册表：模块声明"我能做什么"，其他模块发起"我要什么"，
//! 内核按意图名分发到唯一声明的处理者。用于请求 / 响应式联动。

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde_json::Value;

use crate::error::KernelError;

/// 意图处理者：接收负载，返回响应（同步；重活应自行 `tokio::spawn`）。
pub type IntentHandler = Arc<dyn Fn(Value) -> Result<Value, KernelError> + Send + Sync>;

/// 意图注册表。
pub struct IntentRegistry {
    /// 意图名 -> (声明模块, 处理者)
    handlers: RwLock<HashMap<String, (String, IntentHandler)>>,
}

impl IntentRegistry {
    pub fn new() -> Self {
        Self {
            handlers: RwLock::new(HashMap::new()),
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
