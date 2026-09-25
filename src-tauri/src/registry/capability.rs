//! 宿主能力派发：把插件的能力请求映射到内核服务，并按会话绑定的身份逐项授权。
//!
//! # 安全模型
//!
//! - **身份只来自会话**：`module_id` 由宿主在派生 helper 时下发，插件既不能声明也
//!   无法改写它。派发器读到的是宿主自己记下的那一个。
//! - **未知能力默认拒绝**：能力表里没有的名字一律返回
//!   `capability_not_supported`，不返回空结果、也不猜测调用者意图。
//! - **能力表当前只实现 `module.info`**（自省，无需权限）。其余能力在补上参数校验、
//!   权限映射与审计前必须继续拒绝——宁少不多，多放行一项就等于多一条越权路径。
//!
//! 需要权限的能力（如 `filesystem:read`）在纳入能力表时必须同时接入
//! [`crate::registry::sandbox::ModuleSandbox`] 的逐次授权校验，并对每次调用留痕。

use std::sync::Arc;

use serde_json::{json, Value};

use copper_module_abi::helper_client::{CapabilityDispatcher, CapabilityError};
use copper_module_abi::ipc::CapabilityRequest;

use crate::registry::module_storage::{ModuleStorage, StorageError};
use crate::registry::modules::ModuleRegistry;

/// 自省能力：返回调用方**自身**的模块信息。
pub const CAPABILITY_MODULE_INFO: &str = "module.info";
/// 模块私有存储：读 / 写 / 删 / 列。命名空间由会话身份决定，模块无法跨命名空间访问。
pub const CAPABILITY_STORAGE_GET: &str = "storage.get";
pub const CAPABILITY_STORAGE_SET: &str = "storage.set";
pub const CAPABILITY_STORAGE_REMOVE: &str = "storage.remove";
pub const CAPABILITY_STORAGE_LIST: &str = "storage.list";

/// 绑定内核注册表与模块存储的能力派发器。
pub struct KernelCapabilities {
    modules: Arc<ModuleRegistry>,
    storage: Arc<ModuleStorage>,
}

impl KernelCapabilities {
    pub fn new(modules: Arc<ModuleRegistry>, storage: Arc<ModuleStorage>) -> Self {
        Self { modules, storage }
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
    use crate::state::KernelContext;

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
    fn capabilities(tag: &str) -> (KernelCapabilities, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "cgl-capabilities-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(ModuleStorage::new(&root));
        (KernelCapabilities::new(registry_with_stub(), storage), root)
    }

    #[test]
    fn module_info_reports_the_session_bound_identity() {
        let (capabilities, root) = capabilities("info");

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
        let (capabilities, root) = capabilities("unknown");

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
        let (capabilities, root) = capabilities("storage");
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
        let (capabilities, root) = capabilities("storage-params");
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
}
