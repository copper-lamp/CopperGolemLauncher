//! 同进程动态库装载后端（`libloading` 实现）。
//!
//! [`ModuleLoadBackend`] 的当前实现：从模块目录的 `backend/` 子目录定位当前平台的
//! 动态库，查找 [`ENTRY_SYMBOL`] 入口并取回 `Arc<dyn Module>`。
//!
//! # 平台适配
//!
//! 动态库扩展名按运行平台取 [`std::env::consts::DLL_EXTENSION`]
//! （Windows `dll` / Linux 与安卓 `so` / macOS `dylib`），并按**文件名主干**
//! 匹配清单声明的产物（清单的 `artifact_glob` 多写 `.dll`，跨平台构建产物扩展名
//! 不同，只有主干稳定）。故同一份模块源码在受支持的任一平台都能被装载。
//!
//! # 已知风险（诚实记录）
//!
//! 库句柄**不卸载**（保活到进程结束，见 [`DylibBackend::load`]）。同进程加载要求
//! 模块动态库与内核「同 crate 版本、同工具链、同 panic/优化配置」构建，详见
//! [`crate::registry::module_entry`] 的 ABI 约束说明。这不是稳定 ABI，
//! 跨版本二进制会被拒绝（缺符号）或崩溃；切换到受管控子进程方案可根治。

#![allow(improper_ctypes_definitions)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::error::KernelError;
use crate::registry::loader::ModuleLoadBackend;
use crate::registry::manifest::ModuleManifest;
use crate::registry::module_entry::{ENTRY_SYMBOL, ModuleEntryFn};
use crate::registry::modules::Module;

/// 同进程动态库装载后端。
///
/// 持有全部已加载库的句柄：动态库一旦被卸载，其中定义的代码（模块 `impl`）即成
/// 悬垂函数指针。当前不支持卸载（对应「安装后重启生效」的产品决策），故句柄
/// 与进程同生命周期，绝不让 `Library` 提前析构。
pub struct DylibBackend {
    libs: Mutex<Vec<libloading::Library>>,
}

impl DylibBackend {
    pub fn new() -> Self {
        Self {
            libs: Mutex::new(Vec::new()),
        }
    }
}

impl Default for DylibBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleLoadBackend for DylibBackend {
    fn name(&self) -> &'static str {
        "dylib"
    }

    fn load(
        &self,
        manifest: &ModuleManifest,
        module_dir: &Path,
    ) -> Result<Arc<dyn Module>, KernelError> {
        let artifact = resolve_artifact(module_dir, manifest)?;

        // SAFETY: 加载任意动态库会执行其初始化代码（静态构造 / DLL 入口）。这是
        // 「同进程加载第三方代码」的固有信任支出，无法在此规避——隔离需由子进程方案
        // 提供。当前产品策略是只装载官方/已审核模块（见 cgl-libs 审核红线第 8 条）。
        let lib = unsafe { libloading::Library::new(&artifact) }.map_err(|e| {
            KernelError::Module(format!("加载模块动态库失败 {}: {e}", artifact.display()))
        })?;

        let module = {
            // SAFETY: `get` 要求符号签名与真实定义一致；入口签名由内核统一约定
            // （见 module_entry::ModuleEntryFn），模块侧由 `copper_module_entry!` 宏生成。
            let entry: libloading::Symbol<ModuleEntryFn> =
                unsafe { lib.get(ENTRY_SYMBOL) }.map_err(|e| {
                    KernelError::Module(format!(
                        "模块 {} 缺少入口符号 `copper_module_entry`: {e}",
                        artifact.display()
                    ))
                })?;
            let raw = entry();
            if raw.is_null() {
                return Err(KernelError::Module(format!(
                    "模块 {} 的入口返回空指针",
                    artifact.display()
                )));
            }
            // SAFETY: `raw` 由模块侧的 `Arc::into_raw` 产生，所有权在此**转移**
            // 给内核，引用计数对称（不会双重释放）。
            unsafe { Arc::from_raw(raw) }
        };

        // 保活：库句柄不能在模块仍被引用期间析构。当前不支持卸载，故永久持有。
        self.libs.lock().push(lib);

        Ok(module)
    }
}

/// 在模块目录的 `backend/` 子目录定位当前平台的动态库。
///
/// 匹配策略：优先命中清单声明的产物主干（[`BackendSpec::artifact_stem`]），
/// 否则回退到该目录下第一个扩展名匹配的文件。找不到则报错，不做猜测。
fn resolve_artifact(module_dir: &Path, manifest: &ModuleManifest) -> Result<PathBuf, KernelError> {
    let backend_dir = module_dir.join("backend");
    let ext = std::env::consts::DLL_EXTENSION;

    let entries = std::fs::read_dir(&backend_dir).map_err(|e| {
        KernelError::Module(format!("模块后端目录不可读 {}: {e}", backend_dir.display()))
    })?;

    let mut candidates: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|s| s.to_str()) == Some(ext))
        .collect();

    if candidates.is_empty() {
        return Err(KernelError::Module(format!(
            "模块后端目录中找不到 .{ext} 动态库: {}",
            backend_dir.display()
        )));
    }
    candidates.sort();

    // 优先用清单声明的主干定位产物（跨平台构建产物扩展名不同，主干稳定）。
    if let Some(stem) = manifest.backend.artifact_stem() {
        if let Some(hit) = candidates.iter().find(|p| {
            p.file_stem().and_then(|s| s.to_str()) == Some(stem.as_str())
        }) {
            return Ok(hit.clone());
        }
    }

    Ok(candidates.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::manifest::ModuleManifest;

    const SAMPLE: &str = r#"{
      "schema_version": "1",
      "id": "copper-lamp.demo-tools",
      "i18n_namespace": "demo-tools",
      "display_name": "示例工具",
      "description": "示例模块",
      "author": { "name": "copper-lamp", "url": "https://github.com/copper-lamp" },
      "license": "MIT",
      "version": "0.1.0",
      "platforms": ["windows-x86_64", "android-arm64", "linux-x86_64", "windows-aarch64"],
      "launcher": { "min": "0.1.0", "max": null },
      "api_version": 1,
      "backend": {
        "crate": "copper-module-demo",
        "entry": "copper_module_demo::DemoModule",
        "artifact_glob": "target/release/copper_module_demo.dll"
      },
      "frontend": { "dist": "frontend/dist", "register": "register.js" },
      "permissions": [],
      "icon": "assets/icon.svg",
      "category": "utility"
    }"#;

    fn temp_module_dir(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cgl-dylib-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("backend")).unwrap();
        dir
    }

    fn manifest() -> ModuleManifest {
        ModuleManifest::parse(SAMPLE.as_bytes()).unwrap()
    }

    #[test]
    fn resolve_artifact_prefers_declared_stem() {
        let dir = temp_module_dir("stem");
        let ext = std::env::consts::DLL_EXTENSION;
        // 放一个"干扰"产物与一个清单声明的主干产物。
        std::fs::write(dir.join("backend").join(format!("other.{ext}")), b"x").unwrap();
        let declared = dir
            .join("backend")
            .join(format!("copper_module_demo.{ext}"));
        std::fs::write(&declared, b"y").unwrap();

        let found = resolve_artifact(&dir, &manifest()).unwrap();
        assert_eq!(found, declared, "必须命中清单声明的主干");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_artifact_falls_back_and_errors_cleanly() {
        let dir = temp_module_dir("fallback");
        let ext = std::env::consts::DLL_EXTENSION;

        // 未命中声明主干时回退到唯一候选。
        let only = dir.join("backend").join(format!("whatever.{ext}"));
        std::fs::write(&only, b"z").unwrap();
        assert_eq!(resolve_artifact(&dir, &manifest()).unwrap(), only);

        // 无任何候选时明确报错。
        let empty = temp_module_dir("empty");
        assert!(resolve_artifact(&empty, &manifest()).is_err());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }
}
