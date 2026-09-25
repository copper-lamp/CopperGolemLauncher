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
use crate::registry::events::{pattern_matches, EventBus};
use crate::registry::intents::IntentRegistry;
use crate::registry::module_storage::{ModuleStorage, StorageError};
use crate::registry::modules::ModuleRegistry;
use crate::registry::sandbox::{ModuleSandbox, Permission};

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
/// 插件向外发布一条事件（需要 `events` 权限，且事件名必须落在清单 `events.publish` 声明的上界内）。
///
/// 与订阅分开授权：能收某事件不代表能以它的名义发布——后者等于允许模块冒充内核事件源，
/// 而内核事件会被桥接到前端监听器，冒充会直接污染界面状态。
pub const CAPABILITY_EVENTS_PUBLISH: &str = "events.publish";

/// 绑定内核注册表、意图注册表、模块存储、事件总线与沙箱的能力派发器。
pub struct KernelCapabilities {
    modules: Arc<ModuleRegistry>,
    intents: Arc<IntentRegistry>,
    storage: Arc<ModuleStorage>,
    /// 事件总线：`events.publish` 的落点。
    events: Arc<EventBus>,
    /// 模块沙箱：事件发布等需要权限的能力在这里逐次判定。
    sandbox: Arc<ModuleSandbox>,
    /// 本会话清单声明的**发布上界**（由装载后端按模块注入，不同模块不同）。
    publish_patterns: Vec<String>,
}

impl KernelCapabilities {
    pub fn new(
        modules: Arc<ModuleRegistry>,
        intents: Arc<IntentRegistry>,
        storage: Arc<ModuleStorage>,
        events: Arc<EventBus>,
        sandbox: Arc<ModuleSandbox>,
        publish_patterns: Vec<String>,
    ) -> Self {
        Self {
            modules,
            intents,
            storage,
            events,
            sandbox,
            publish_patterns,
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

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EventPublishParams {
    name: String,
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

/// 沙箱判定失败到能力错误的映射：越权必须如实上报，绝不能放行。
fn sandbox_error(error: KernelError) -> CapabilityError {
    CapabilityError {
        code: "capability_denied",
        message: error.friendly(),
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
            CAPABILITY_EVENTS_PUBLISH => {
                let params: EventPublishParams = parse_params(request.params)?;

                // 两道闸缺一不可：先判权限（用户可在设置页收紧 `events`），再判清单上界。
                self.sandbox
                    .enforce(
                        module_id,
                        Permission::Events,
                        "events.publish",
                        &params.name,
                    )
                    .map_err(sandbox_error)?;

                if !self
                    .publish_patterns
                    .iter()
                    .any(|pattern| pattern_matches(pattern, &params.name))
                {
                    return Err(CapabilityError {
                        code: "event_not_declared",
                        message: format!(
                            "事件 `{}` 不在本模块 events.publish 声明的上界内（已声明：{}）",
                            params.name,
                            if self.publish_patterns.is_empty() {
                                "无".to_owned()
                            } else {
                                self.publish_patterns.join(", ")
                            }
                        ),
                    });
                }

                self.events.publish(&params.name, params.payload);
                Ok(json!({ "published": true }))
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
        // 默认授予 `Intents`：意图路径的授权判定发生在 `IntentRegistry` 自己的沙箱上，
        // 这里给不给都不改变其行为；给上是为了让"能力层自身的沙箱"在默认用例里不设障。
        let (capabilities, intents, _events, root) =
            capabilities_with(tag, Vec::new(), &[Permission::Intents]);
        (capabilities, intents, root)
    }

    /// 可指定**发布上界**与沙箱授予集的能力派发器。
    ///
    /// 额外返回事件总线：发布路径的断言必须落在"订阅者真的收到了"，否则只能证明
    /// 能力被受理，不能证明它到了总线。
    #[allow(clippy::type_complexity)]
    fn capabilities_with(
        tag: &str,
        publish_patterns: Vec<String>,
        granted: &[Permission],
    ) -> (
        KernelCapabilities,
        Arc<IntentRegistry>,
        Arc<EventBus>,
        std::path::PathBuf,
    ) {
        let root = std::env::temp_dir().join(format!(
            "cgl-capabilities-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(ModuleStorage::new(&root));
        let intents = Arc::new(IntentRegistry::new());
        let events = Arc::new(EventBus::new());
        let sandbox = Arc::new(ModuleSandbox::new());
        sandbox.grant(
            "copper-lamp.demo-tools",
            granted.iter().copied().collect(),
            std::path::PathBuf::from("C:\\tmp\\module"),
        );
        (
            KernelCapabilities::new(
                registry_with_stub(),
                Arc::clone(&intents),
                storage,
                Arc::clone(&events),
                sandbox,
                publish_patterns,
            ),
            intents,
            events,
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
            Arc::new(EventBus::new()),
            Arc::new(ModuleSandbox::new()),
            Vec::new(),
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

    #[test]
    fn events_publish_reaches_the_bus_for_a_declared_name() {
        let (capabilities, _intents, events, root) = capabilities_with(
            "publish-ok",
            vec!["demo-tools.activity".to_owned()],
            &[Permission::Events],
        );

        // 订阅者收得到才叫"发布了"：只看能力回执只能证明调用被受理。
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let _subscription = events.subscribe("demo-tools.*", move |name, payload| {
            sink.lock()
                .unwrap()
                .push((name.to_owned(), payload.clone()));
        });

        let result = capabilities
            .dispatch(
                "copper-lamp.demo-tools",
                request_with(
                    CAPABILITY_EVENTS_PUBLISH,
                    json!({ "name": "demo-tools.activity", "payload": { "kind": "ping" } }),
                ),
            )
            .unwrap();

        assert_eq!(result["published"], json!(true));
        let received = seen.lock().unwrap().clone();
        assert_eq!(received.len(), 1, "订阅者必须收到这条事件：{received:?}");
        assert_eq!(received[0].0, "demo-tools.activity");
        assert_eq!(received[0].1["kind"], json!("ping"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn events_publish_refuses_a_name_outside_the_declared_bound() {
        let (capabilities, _intents, _events, root) = capabilities_with(
            "publish-bound",
            vec!["demo-tools.*".to_owned()],
            &[Permission::Events],
        );

        // 声明了自己的域前缀，就不能以别人的名义（尤其是内核事件名）发布。
        let error = capabilities
            .dispatch(
                "copper-lamp.demo-tools",
                request_with(
                    CAPABILITY_EVENTS_PUBLISH,
                    json!({ "name": "settings.changed", "payload": {} }),
                ),
            )
            .unwrap_err();
        assert_eq!(error.code, "event_not_declared", "未声明的事件名必须拒绝");

        // 声明过的域内事件照常放行。
        capabilities
            .dispatch(
                "copper-lamp.demo-tools",
                request_with(
                    CAPABILITY_EVENTS_PUBLISH,
                    json!({ "name": "demo-tools.activity", "payload": {} }),
                ),
            )
            .expect("声明过的事件必须可发布");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn events_publish_is_refused_without_the_events_permission() {
        // 沙箱登记了该模块但不授予 `Events`：即使用户在设置页收紧了权限，也必须拦住。
        let (capabilities, _intents, _events, root) = capabilities_with(
            "publish-denied",
            vec!["demo-tools.activity".to_owned()],
            &[],
        );

        let error = capabilities
            .dispatch(
                "copper-lamp.demo-tools",
                request_with(
                    CAPABILITY_EVENTS_PUBLISH,
                    json!({ "name": "demo-tools.activity", "payload": {} }),
                ),
            )
            .unwrap_err();

        assert_eq!(error.code, "capability_denied", "越权必须如实上报，不得放行");
        assert!(error.message.contains("events"), "got: {}", error.message);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn events_publish_rejects_malformed_params() {
        let (capabilities, _intents, _events, root) = capabilities_with(
            "publish-params",
            vec!["demo-tools.activity".to_owned()],
            &[Permission::Events],
        );

        let missing = capabilities
            .dispatch(
                "copper-lamp.demo-tools",
                request_with(CAPABILITY_EVENTS_PUBLISH, json!({ "payload": {} })),
            )
            .unwrap_err();
        assert_eq!(missing.code, "invalid_capability_params", "缺 name 必须拒绝");

        let _ = std::fs::remove_dir_all(&root);
    }
}
