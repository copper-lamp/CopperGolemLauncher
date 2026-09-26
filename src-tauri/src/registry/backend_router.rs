//! 附加模块装载后端路由：按清单是否声明 `runtime`，把模块分流到受监管 Node 会话
//! 后端或 helper 子进程后端。
//!
//! # 为什么是显式分流而不是容错回退
//!
//! 分流**只依据清单**：声明了 `runtime` 的模块走受监管 Node 后端，未声明的走 helper
//! 后端。之所以强调这一点，是因为两种后端的隔离与授权模型并不等价——若某个后端
//! 启动 / 握手失败时改投另一个后端，就会把「这台机器的 Node 不满足要求」悄悄变成
//! 「换一种方式跑起来」，而用户与模块作者都无从得知实际用了哪种执行模型。因此这里
//! 失败即失败，不做任何回退。

use std::path::Path;
use std::sync::Arc;

use crate::error::KernelError;
use crate::registry::loader::ModuleLoadBackend;
use crate::registry::manifest::ModuleManifest;
use crate::registry::modules::Module;

/// 按清单分流两条装载路径的后端。
pub struct AddonBackendRouter {
    /// 未声明 `runtime` 的模块（现有 helper 路径）。
    plugin: Arc<dyn ModuleLoadBackend>,
    /// 声明了 `runtime` 的模块（受监管 Node 会话路径）。
    node: Arc<dyn ModuleLoadBackend>,
}

impl AddonBackendRouter {
    pub fn new(plugin: Arc<dyn ModuleLoadBackend>, node: Arc<dyn ModuleLoadBackend>) -> Self {
        Self { plugin, node }
    }
}

impl ModuleLoadBackend for AddonBackendRouter {
    fn name(&self) -> &'static str {
        "routed"
    }

    fn load(
        &self,
        manifest: &ModuleManifest,
        module_dir: &Path,
    ) -> Result<Arc<dyn Module>, KernelError> {
        // 分流判据只有一项：清单有没有声明 runtime。此处不看后端是否可用、也不因
        // 某个后端报错而改投另一个（见文件顶部说明）。
        if manifest.runtime.is_some() {
            self.node.load(manifest, module_dir)
        } else {
            self.plugin.load(manifest, module_dir)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use serde_json::json;

    use crate::state::KernelContext;

    /// 路由只关心「拿到一个模块」，因此假模块不必真的有行为。
    struct StubModule {
        id: String,
    }

    impl Module for StubModule {
        fn id(&self) -> &str {
            &self.id
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

    /// 记录型假后端：断言「哪个后端被调用了」，并可令其失败以验证不回退。
    struct RecordingBackend {
        label: &'static str,
        calls: Arc<Mutex<Vec<String>>>,
        fail: bool,
    }

    impl RecordingBackend {
        fn new(label: &'static str, fail: bool) -> (Arc<Self>, Arc<Mutex<Vec<String>>>) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let backend = Arc::new(Self {
                label,
                calls: Arc::clone(&calls),
                fail,
            });
            (backend, calls)
        }
    }

    impl ModuleLoadBackend for RecordingBackend {
        fn name(&self) -> &'static str {
            self.label
        }

        fn load(
            &self,
            manifest: &ModuleManifest,
            _module_dir: &Path,
        ) -> Result<Arc<dyn Module>, KernelError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{}:{}", self.label, manifest.id));
            if self.fail {
                return Err(KernelError::Module(format!("{} 后端故意失败", self.label)));
            }
            Ok(Arc::new(StubModule {
                id: manifest.id.clone(),
            }))
        }
    }

    /// 构造可用于路由判定的清单；`with_runtime` 决定是否声明受监管运行时。
    ///
    /// 路由只看 `runtime` 字段，因此这里用 `parse`（不做语义校验）即可，避免把
    /// 用例与清单校验规则耦合在一起。
    fn manifest(id: &str, with_runtime: bool) -> ModuleManifest {
        let mut raw = json!({
            "schema_version": "2",
            "id": id,
            "i18n_namespace": "demo",
            "display_name": "示例",
            "description": "示例",
            "author": { "name": "copper-lamp" },
            "license": "MIT",
            "version": "0.1.0",
            "platforms": ["windows-x86_64", "linux-x86_64"],
            "launcher": { "min": "0.1.0", "max": null },
            "api_version": 2,
            "backend": { "crate": "demo", "entry": "demo::Demo", "artifact_glob": "demo.dll" },
            "frontend": { "dist": "frontend/dist", "register": "register.js" }
        });
        if with_runtime {
            raw["runtime"] = json!({
                "kind": "node",
                "entry": "runtime/agent.mjs",
                "engines": { "node": ">=22.19.0" },
                "wire_protocol": { "id": "copper-addon.ndjson", "version": 2 }
            });
        }
        ModuleManifest::parse(&serde_json::to_vec(&raw).unwrap()).expect("夹具清单必须可解析")
    }

    #[test]
    fn a_manifest_with_runtime_routes_to_the_node_backend() {
        let (plugin, plugin_calls) = RecordingBackend::new("plugin", false);
        let (node, node_calls) = RecordingBackend::new("node", false);
        let router = AddonBackendRouter::new(plugin, node);

        router
            .load(&manifest("copper-lamp.agent", true), Path::new("."))
            .expect("声明 runtime 的模块应能装载");

        assert_eq!(node_calls.lock().unwrap().len(), 1);
        assert!(
            plugin_calls.lock().unwrap().is_empty(),
            "声明 runtime 的模块绝不能落到 helper 后端"
        );
    }

    #[test]
    fn a_manifest_without_runtime_routes_to_the_plugin_backend() {
        let (plugin, plugin_calls) = RecordingBackend::new("plugin", false);
        let (node, node_calls) = RecordingBackend::new("node", false);
        let router = AddonBackendRouter::new(plugin, node);

        router
            .load(&manifest("copper-lamp.demo-tools", false), Path::new("."))
            .expect("未声明 runtime 的模块应能装载");

        assert_eq!(plugin_calls.lock().unwrap().len(), 1);
        assert!(
            node_calls.lock().unwrap().is_empty(),
            "未声明 runtime 的模块绝不能落到 Node 后端"
        );
    }

    #[test]
    fn a_failing_backend_does_not_fall_back_to_the_other_one() {
        let (plugin, plugin_calls) = RecordingBackend::new("plugin", false);
        let (node, node_calls) = RecordingBackend::new("node", true);
        let router = AddonBackendRouter::new(plugin, node);

        let error = router
            .load(&manifest("copper-lamp.agent", true), Path::new("."))
            .expect_err("node 后端失败时必须如实上报，而不是改投 helper 后端");

        assert!(error.friendly().contains("node"), "got: {}", error.friendly());
        assert_eq!(node_calls.lock().unwrap().len(), 1);
        assert!(
            plugin_calls.lock().unwrap().is_empty(),
            "分流失败不得回退：helper 后端一次都不能被调用"
        );
    }
}
