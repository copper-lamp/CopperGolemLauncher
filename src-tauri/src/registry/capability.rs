//! 宿主能力派发：把插件的能力请求映射到内核服务，并按会话绑定的身份逐项授权。
//!
//! # 安全模型
//!
//! - **身份只来自会话**：`module_id` 由宿主在派生 helper 时下发，插件既不能声明也
//!   无法改写它。派发器读到的是宿主自己记下的那一个。
//! - **未知能力默认拒绝**：能力表里没有的名字一律返回
//!   `capability_not_supported`，不返回空结果、也不猜测调用者意图。
//! - **逐项授权**：需要权限的能力（如 `intent.request`）在派发时必须经
//!   [`crate::registry::sandbox::ModuleSandbox`] 的逐次校验，绝不因为"模块装了"就放行。
//!
//! 需要权限的能力（如 `filesystem:read`）在纳入能力表时必须同时接入沙箱的逐次
//! 授权校验，并对每次调用留痕。

use std::sync::Arc;

use serde_json::{json, Value};

use copper_module_abi::helper_client::{CapabilityDispatcher, CapabilityError};
use copper_module_abi::ipc::CapabilityRequest;

use crate::error::KernelError;
use crate::registry::intents::IntentRegistry;
use crate::registry::module_storage::{ModuleStorage, StorageError};
use crate::registry::modules::ModuleRegistry;

/// 自省能力：返回调用方**自身**的模块信息。
pub const CAPABILITY_MODULE_INFO: &str = "module.info";
/// 模块私有存储：读 / 写 / 删 / 列。命名空间由会话身份决定，模块无法跨命名空间访问。
pub const CAPABILITY_STORAGE_GET: &str = "storage.get";
pub const CAPABILITY_STORAGE_SET: &str = "storage.set";
pub const CAPABILITY_STORAGE_REMOVE: &str = "storage.remove";
pub const CAPABILITY_STORAGE_LIST: &str = "storage.list";
/// 插件发起意图请求（需要 `intents:request` 权限）。
///
/// 复用既有能力 RPC 通道，因此**不需要新增 IPC 方法与 ABI 字段**——"发起意图"
/// 对 helper 而言就是一次普通的能力请求。
pub const CAPABILITY_INTENT_REQUEST: &str = "intent.request";

/// 绑定内核注册表、意图注册表与模块存储的能力派发器。
pub struct KernelCapabilities {
    modules: Arc<ModuleRegistry>,
    intents: Arc<IntentRegistry>,
    storage: Arc<ModuleStorage>,
}

impl KernelCapabilities {
    pub fn new(
        modules: Arc<ModuleRegistry>,
        intents: Arc<IntentRegistry>,
        storage: Arc<ModuleStorage>,
    ) -> Self {
        Self {
            modules,
            intents,
            storage,
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StorageKeyParams {
    key: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StorageSetParams {
    key: String,
    value: Value,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StorageListParams {
    #[serde(default)]
    prefix: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentRequestParams {
    intent: String,
    #[serde(default)]
    payload: Value,
}

/// 解析能力参数。`deny_unknown_fields` + 严格类型：多余字段直接拒绝，
/// 避免"传错字段名却静默成功"这类难查的集成问题。
fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, CapabilityError> {
    serde_json::from_value(params).map_err(|error| CapabilityError {
        code: "invalid_capability_params",
        message: error.to_string(),
    })
}

fn storage_error(error: StorageError) -> CapabilityError {
    CapabilityError {
        code: error.code(),
        message: error.to_string(),
    }
}

/// 意图失败到能力错误的映射。
///
/// 只区分"插件可以降级"与"插件不能降级"两类：
/// - `intent_unavailable`（没有模块声明该意图 / 转发超时）：生态里本来就可能没有
///   处理器，插件据此降级是合理的；
/// - `intent_failed`（其余，含权限拒绝与目标模块未运行）：不该被当作"稍后重试即可"。
///
/// 不细分成更多 code：插件能采取的行动只有这两种，多出来的分类只会变成无人处理的枚举。
fn intent_error(error: KernelError) -> CapabilityError {
    match error {
        KernelError::Intent(message) => CapabilityError {
            code: "intent_unavailable",
            message,
        },
        other => CapabilityError {
            code: "intent_failed",
            message: other.friendly(),
        },
    }
}

impl CapabilityDispatcher for KernelCapabilities {
    fn dispatch(
        &self,
        module_id: &str,
        request: CapabilityRequest,
    ) -> Result<Value, CapabilityError> {
        match request.capability.as_str() {
            CAPABILITY_MODULE_INFO => {
                if !self.modules.is_registered(module_id) {
                    return Err(CapabilityError {
                        code: "unknown_module",
                        message: format!("module `{module_id}` is not registered"),
                    });
                }
                // 注意返回的是 `module_id`（会话绑定）而不是请求里可能夹带的任何
                // 身份字段——后者在协议层已被拒绝。
                Ok(json!({
                    "id": module_id,
                    "origin": self.modules.origin_of(module_id).as_str(),
                    "running": self.modules.is_running(module_id),
                }))
            }
            CAPABILITY_STORAGE_GET => {
                let params: StorageKeyParams = parse_params(request.params)?;
                let value = self
                    .storage
                    .get(module_id, &params.key)
                    .map_err(storage_error)?;
                Ok(json!({ "value": value }))
            }
            CAPABILITY_STORAGE_SET => {
                let params: StorageSetParams = parse_params(request.params)?;
                self.storage
                    .set(module_id, &params.key, params.value)
                    .map_err(storage_error)?;
                Ok(json!({ "stored": true }))
            }
            CAPABILITY_STORAGE_REMOVE => {
                let params: StorageKeyParams = parse_params(request.params)?;
                let removed = self
                    .storage
                    .remove(module_id, &params.key)
                    .map_err(storage_error)?;
                Ok(json!({ "removed": removed }))
            }
            CAPABILITY_STORAGE_LIST => {
                let params: StorageListParams = parse_params(request.params)?;
                let keys = self
                    .storage
                    .list(module_id, params.prefix.as_deref().unwrap_or(""))
                    .map_err(storage_error)?;
                Ok(json!({ "keys": keys }))
            }
            CAPABILITY_INTENT_REQUEST => {
                let params: IntentRequestParams = parse_params(request.params)?;
                // `request_checked` 强制调用方持有 `Permission::Intents`，并在"无声明"
                // 时如实报错。插件拿到的永远不是"空结果当作成功"。
                let result = self
                    .intents
                    .request_checked(module_id, &params.intent, params.payload)
                    .map_err(intent_error)?;
                Ok(json!({ "result": result }))
            }
            other => Err(CapabilityError {
                code: "capability_not_supported",
                message: format!(
                    "capability `{other}` is not implemented by this launcher build"
                ),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::KernelError;
    use crate::registry::modules::{Module, ModuleOrigin};
    use crate::registry::sandbox::ModuleSandbox;
    use crate::state::KernelContext;
    use std::collections::HashSet;

    struct StubModule;

    impl Module for StubModule {
        fn id(&self) -> &str {
            "copper-lamp.demo-tools"
        }

        fn init(&self, _kernel: &KernelContext) -> Result<(), KernelError> {
            Ok(())
        }

        fn start(&self, _kernel: &KernelContext) -> Result<(), KernelError> {
            Ok(())
        }

        fn stop(&self, _kernel: &KernelContext) -> Result<(), KernelError> {
            Ok(())
        }
    }

    fn registry_with_stub() -> Arc<ModuleRegistry> {
        let registry = Arc::new(ModuleRegistry::new());
        registry.register_with_origin(Arc::new(StubModule), ModuleOrigin::Addon);
        registry
    }

    fn request(capability: &str) -> CapabilityRequest {
        request_with(capability, json!({}))
    }

    fn request_with(capability: &str, params: Value) -> CapabilityRequest {
        CapabilityRequest {
            capability: capability.to_owned(),
            params,
        }
    }

    /// 每个测试用独立数据目录：测试并行执行，共享目录会互相污染。
    fn capabilities(tag: &str) -> (KernelCapabilities, Arc<IntentRegistry>, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "cgl-capabilities-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(ModuleStorage::new(&root));
        let intents = Arc::new(IntentRegistry::new());
        (
            KernelCapabilities::new(registry_with_stub(), Arc::clone(&intents), storage),
            intents,
            root,
        )
    }

    #[test]
    fn module_info_reports_the_session_bound_identity() {
        let (capabilities, _intents, root) = capabilities("info");

        let info = capabilities
            .dispatch("copper-lamp.demo-tools", request(CAPABILITY_MODULE_INFO))
            .unwrap();

        assert_eq!(info["id"], json!("copper-lamp.demo-tools"));
        assert_eq!(info["origin"], json!("addon"));
        assert_eq!(info["running"], json!(false));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn module_info_refuses_an_unregistered_session_identity() {
        let root = std::env::temp_dir().join(format!(
            "cgl-capabilities-unregistered-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let capabilities = KernelCapabilities::new(
            Arc::new(ModuleRegistry::new()),
            Arc::new(IntentRegistry::new()),
            Arc::new(ModuleStorage::new(&root)),
        );

        let error = capabilities
            .dispatch("copper-lamp.ghost", request(CAPABILITY_MODULE_INFO))
            .unwrap_err();

        assert_eq!(error.code, "unknown_module");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_capabilities_fail_closed() {
        let (capabilities, _intents, root) = capabilities("unknown");

        let error = capabilities
            .dispatch("copper-lamp.demo-tools", request("filesystem:read"))
            .unwrap_err();

        assert_eq!(
            error.code, "capability_not_supported",
            "an unimplemented capability must be refused, never answered with a stub"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn storage_round_trips_inside_the_session_namespace() {
        let (capabilities, _intents, root) = capabilities("storage");
        let id = "copper-lamp.demo-tools";

        let stored = capabilities
            .dispatch(
                id,
                request_with(
                    CAPABILITY_STORAGE_SET,
                    json!({ "key": "note", "value": { "title": "hi" } }),
                ),
            )
            .unwrap();
        assert_eq!(stored["stored"], json!(true));

        let read = capabilities
            .dispatch(id, request_with(CAPABILITY_STORAGE_GET, json!({ "key": "note" })))
            .unwrap();
        assert_eq!(read["value"]["title"], json!("hi"));

        let listed = capabilities
            .dispatch(
                id,
                request_with(CAPABILITY_STORAGE_LIST, json!({ "prefix": "no" })),
            )
            .unwrap();
        assert_eq!(listed["keys"], json!(["note"]));

        let removed = capabilities
            .dispatch(
                id,
                request_with(CAPABILITY_STORAGE_REMOVE, json!({ "key": "note" })),
            )
            .unwrap();
        assert_eq!(removed["removed"], json!(true));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn storage_rejects_malformed_params() {
        let (capabilities, _intents, root) = capabilities("storage-params");
        let id = "copper-lamp.demo-tools";

        // 缺字段。
        let missing = capabilities
            .dispatch(id, request_with(CAPABILITY_STORAGE_GET, json!({ "k": "note" })))
            .unwrap_err();
        assert_eq!(missing.code, "invalid_capability_params");

        // 多余字段：不能静默忽略，否则拼错字段名会变成难查的集成问题。
        let unknown = capabilities
            .dispatch(
                id,
                request_with(CAPABILITY_STORAGE_GET, json!({ "key": "a", "extra": 1 })),
            )
            .unwrap_err();
        assert_eq!(unknown.code, "invalid_capability_params");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn intent_request_is_forwarded_through_the_registry() {
        let (capabilities, intents, root) = capabilities("intent-ok");
        intents
            .declare(
                "game.list",
                "home",
                Arc::new(|payload| Ok(json!({ "asked": payload }))),
            )
            .unwrap();

        let result = capabilities
            .dispatch(
                "copper-lamp.demo-tools",
                request_with(
                    CAPABILITY_INTENT_REQUEST,
                    json!({ "intent": "game.list", "payload": { "n": 1 } }),
                ),
            )
            .unwrap();

        // 结果必须真的经能力响应回到插件，而不只是"调用没报错"。
        assert_eq!(result["result"]["asked"]["n"], json!(1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn intent_request_without_a_declaration_is_reported_as_unavailable() {
        let (capabilities, _intents, root) = capabilities("intent-missing");

        let error = capabilities
            .dispatch(
                "copper-lamp.demo-tools",
                request_with(CAPABILITY_INTENT_REQUEST, json!({ "intent": "nobody.declares" })),
            )
            .unwrap_err();

        // 与"授权失败"区分开：插件据此降级是合理的。
        assert_eq!(error.code, "intent_unavailable");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn intent_request_is_refused_without_the_intents_permission() {
        let (capabilities, intents, root) = capabilities("intent-denied");

        // 登记为附加模块但不给 intents 权限：沙箱绑定后必须拦住它。
        let sandbox = Arc::new(ModuleSandbox::new());
        sandbox.grant(
            "copper-lamp.demo-tools",
            HashSet::new(),
            std::path::PathBuf::from("C:\\tmp\\module"),
        );
        intents.bind_sandbox(sandbox);
        intents
            .declare("game.list", "home", Arc::new(|payload| Ok(payload)))
            .unwrap();

        let error = capabilities
            .dispatch(
                "copper-lamp.demo-tools",
                request_with(
                    CAPABILITY_INTENT_REQUEST,
                    json!({ "intent": "game.list", "payload": {} }),
                ),
            )
            .unwrap_err();

        assert_eq!(error.code, "intent_failed", "越权必须如实上报，不得放行");
        assert!(error.message.contains("intents"), "got: {}", error.message);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn intent_request_rejects_malformed_params() {
        let (capabilities, _intents, root) = capabilities("intent-params");
        let id = "copper-lamp.demo-tools";

        let missing = capabilities
            .dispatch(
                id,
                request_with(CAPABILITY_INTENT_REQUEST, json!({ "payload": {} })),
            )
            .unwrap_err();
        assert_eq!(missing.code, "invalid_capability_params", "缺 intent 必须拒绝");

        let unknown = capabilities
            .dispatch(
                id,
                request_with(
                    CAPABILITY_INTENT_REQUEST,
                    json!({ "intent": "game.list", "payload": {}, "module_id": "forged" }),
                ),
            )
            .unwrap_err();
        assert_eq!(
            unknown.code, "invalid_capability_params",
            "拼错字段名或夹带身份字段都必须拒绝"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
