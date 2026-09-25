//! 附加模块装载器：从 `<data_dir>/modules` 扫描、校验、加载并注册附加模块。
//!
//! # 为什么要有「装载后端」抽象
//!
//! 加载附加模块的**物理方式**有两条路（见 [cgl-models](../../../docs/cgl-models.md) 阶段二）：
//! - 同进程动态库（当前实现，[`crate::registry::dylib_backend`]）：改动小、链路最短，
//!   但模块与内核同进程，权限只能靠契约；
//! - 受管控子进程 + IPC（未来）：隔离真实，但需定义跨进程协议与进程生命周期。
//!
//! 二者对上层（扫描 → 校验 → 注册 → 授权）完全同构。故把「拿到一个
//! `Arc<dyn Module>`」这一步抽成 [`ModuleLoadBackend`]，装载编排只依赖该抽象，
//! 未来换子进程后端时本文件无需改动。
//!
//! # 职责边界
//!
//! 本文件**只做装载**（启动路径）。安装/卸载（下载、解包、原子落盘）属于命令层，
//! 见 `commands/modules.rs`。装载在 `boot()` 之前完成：装载进来的模块与内置模块
//! 一并由 [`crate::registry::modules::ModuleRegistry::boot`] 驱动生命周期。

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;

use crate::error::KernelError;
use crate::registry::manifest::{EventDeclarations, ModuleManifest};
use crate::registry::modules::{Module, ModuleOrigin, resolve_addon_dir};
use crate::registry::sandbox::Permission;
use crate::state::KernelContext;

/// 装载后端：把「模块目录 + 已校验清单」变成可注册的模块实例。
///
/// 实现必须是线程安全的（装载在启动期单线程调用，但注册表与沙箱是共享状态）。
pub trait ModuleLoadBackend: Send + Sync {
    /// 加载一个附加模块。返回的模块尚未 `init` / `start`，由注册表 `boot()` 驱动。
    fn load(
        &self,
        manifest: &ModuleManifest,
        module_dir: &Path,
    ) -> Result<Arc<dyn Module>, KernelError>;

    /// 后端标识（写入日志，便于区分 dylib / 子进程两种实现）。
    fn name(&self) -> &'static str;
}

/// 单个附加模块的装载结果（供启动日志与自检展示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AddonLoadReport {
    pub id: String,
    pub version: Option<String>,
    pub loaded: bool,
    pub error: Option<String>,
}

/// 装载编排器。
pub struct ModuleLoader {
    backend: Arc<dyn ModuleLoadBackend>,
}

impl ModuleLoader {
    pub fn new(backend: Arc<dyn ModuleLoadBackend>) -> Self {
        Self { backend }
    }

    /// 后端标识。
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// 扫描并装载全部已安装的附加模块。
    ///
    /// 单个模块失败**不影响其它模块**（逐个返回报告，失败者经
    /// [`crate::registry::modules::ModuleRegistry::mark_failed`] 留痕），
    /// 因为一个坏模块不该让用户其余模块一起不可用。
    ///
    /// 返回每个「合法 id 目录」的装载报告，按 id 升序（稳定顺序，便于日志比对）。
    pub fn load_installed(&self, kernel: &KernelContext) -> Vec<AddonLoadReport> {
        let modules_dir = kernel.paths().modules_dir().clone();
        let names = match scan_module_dirs(&modules_dir) {
            Ok(names) => names,
            Err(e) => {
                log::warn!("[loader] 扫描附加模块目录失败 {}: {e}", modules_dir.display());
                return Vec::new();
            }
        };

        names
            .into_iter()
            .map(|id| self.load_one(kernel, &id, &modules_dir))
            .collect()
    }

    /// 装载单个模块，失败一律转成报告（不 panic、不阻断其它模块）。
    fn load_one(&self, kernel: &KernelContext, id: &str, modules_dir: &Path) -> AddonLoadReport {
        match self.load_one_inner(kernel, id, modules_dir) {
            Ok(version) => AddonLoadReport {
                id: id.to_string(),
                version: Some(version),
                loaded: true,
                error: None,
            },
            Err(e) => {
                let message = e.friendly();
                log::warn!("[loader] 附加模块 `{id}` 装载失败: {message}");
                // 留痕：让磁盘视图把该目录呈现为 Failed 并展示原因。
                kernel.modules().mark_failed(id, message.clone());
                AddonLoadReport {
                    id: id.to_string(),
                    version: None,
                    loaded: false,
                    error: Some(message),
                }
            }
        }
    }

    /// 真正的装载步骤（顺序即安全边界，任一步失败即终止）。
    fn load_one_inner(
        &self,
        kernel: &KernelContext,
        id: &str,
        modules_dir: &Path,
    ) -> Result<String, KernelError> {
        // 1. id 与内置模块冲突：拒绝。同名附加模块企图覆盖内置模块是劫持行为，
        //    必须显式失败而不是静默覆盖（注册表 register 不做冲突检查）。
        if kernel.modules().is_registered(id) {
            return Err(KernelError::Module(format!(
                "模块 `{id}` 与已注册模块（含内置模块）重名，拒绝装载附加模块"
            )));
        }

        // 2. 目录落点校验（id 形态 + 规范化 + 组件级前缀 + 符号链接复核）。
        let dir = resolve_addon_dir(modules_dir, id)?;

        // 3. 清单：解析 + 语义校验，任一不过即拒绝。
        let manifest = read_manifest(&dir)?;

        // 4. 平台 / API / 内核版本三重门禁。
        if !manifest.supports_current_platform() {
            return Err(KernelError::Module(format!(
                "模块 `{id}` 未声明支持当前平台（支持：{}）",
                manifest.platforms.join(" / ")
            )));
        }
        let launcher = env!("CARGO_PKG_VERSION");
        if !manifest.accepts_launcher(launcher) {
            return Err(KernelError::Module(format!(
                "模块 `{id}` 声明的内核兼容区间 [{}, {}] 不覆盖当前内核 {launcher}",
                manifest.launcher.min,
                manifest.launcher.max.as_deref().unwrap_or("+")
            )));
        }

        // 5. 交给后端加载（dylib 同进程实现见 dylib_backend）。
        let module = self.backend.load(&manifest, &dir)?;

        // 6. 一致性复核：模块自报 id 必须等于目录/清单 id。
        //    这是防「元数据说 A、二进制是 B」的最后一道闸。
        if module.id() != manifest.id {
            return Err(KernelError::Module(format!(
                "模块二进制自报 id `{}` 与清单 id `{}` 不一致，拒绝装载",
                module.id(),
                manifest.id
            )));
        }

        // 7. 注册（显式声明来源为 Addon，使其受沙箱管辖）。
        let module_id = manifest.id.clone();
        kernel
            .modules()
            .register_with_origin(module, ModuleOrigin::Addon);

        // 8. 沙箱授权：清单声明的权限是**授权上界**，装载时按映射落为生效集。
        //
        // 即便声明为空也必须登记：`ModuleSandbox::knows` 以「是否登记」区分
        // 附加模块与内置模块，未登记的模块会被当作内核内置而豁免校验。空权限
        // 也要登记，才能让「声明为空 = 什么都不许」成立，而不是「声明为空 = 不受管」。
        kernel.sandbox().grant(
            &module_id,
            declared_permissions(&manifest.permissions, &manifest.events),
            dir,
        );

        log::info!(
            "[loader/{}] 已装载附加模块 `{module_id}`@{}（声明权限 {} 项，订阅 {} 项，发布 {} 项，意图 {} 项）",
            self.backend.name(),
            manifest.version,
            manifest.permissions.len(),
            manifest.events.subscribe.len(),
            manifest.events.publish.len(),
            manifest.intents.len()
        );

        Ok(manifest.version)
    }
}

/// 扫描 `<modules_dir>` 下所有**合法两段式 id** 的目录名（升序）。
///
/// 与 [`crate::registry::modules::ModuleRegistry::list_installed_addons`] 的口径一致：
/// 非法目录名一律跳过并留 warning，绝不能放进后续装载步骤（它会被当路径用）。
fn scan_module_dirs(modules_dir: &Path) -> std::io::Result<Vec<String>> {
    let mut names = Vec::new();
    let entries = match std::fs::read_dir(modules_dir) {
        Ok(entries) => entries,
        // 目录不存在是正常状态（用户从未装过附加模块）。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(names),
        Err(e) => return Err(e),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !crate::registry::modules::is_valid_module_id(name) {
            log::warn!("[loader] 跳过非法附加模块目录名（不符合两段式 id）: {name}");
            continue;
        }
        names.push(name.to_string());
    }

    names.sort();
    Ok(names)
}

/// 读取并校验模块目录下的 `module.json`。
fn read_manifest(dir: &Path) -> Result<ModuleManifest, KernelError> {
    let path = dir.join("module.json");
    let raw = std::fs::read(&path).map_err(|e| {
        KernelError::Module(format!("读取模块清单失败 {}: {e}", path.display()))
    })?;
    ModuleManifest::parse_and_validate(&raw)
}

/// 把清单权限字符串映射为沙箱 [`Permission`] 集合。
///
/// # 事件订阅的权限来源
///
/// `events` 列表非空即视为申请 [`Permission::Events`]（订阅与发布任一方向非空都算）。
/// 清单的 `permissions` 枚举里没有对应项，而 `events` 声明本身既表达了"要不要收发"
/// 又表达了"能收发哪些"——再加一项 `permissions` 条目就是同一语义的第二份真相，
/// 两份真相迟早漂移（权限枚举已经因为粗细粒度分歧吃过这个亏，见下）。
///
/// # 契约分歧（已知，需收口）
///
/// 沙箱枚举（[`Permission`]）为 9 项**粗粒度**取值（`file_system` / `settings` …），
/// 而清单权威枚举是 9 项**细粒度**取值（`filesystem:read` / `settings:write` …，
/// 见 cgl-libs 2.3.2 与模板 `module.schema.json`）。两者并非一一对应。
///
/// 本函数做**保守映射**：只有能无歧义对应到粗粒度项的才授予，其余（如
/// `account:read` / `download:enqueue`，沙箱尚无对应能力）**不授予并留 warning**——
/// 宁少不多，因为多授予一项就等于放开一条越权路径。
///
/// 该分歧的收口（细粒度化沙箱枚举或补齐映射表）属后续任务，记录在
/// `docs/铜核心/设计.md`。
fn declared_permissions(raw: &[String], events: &EventDeclarations) -> HashSet<Permission> {
    let mut set = HashSet::new();
    for name in raw {
        match name.as_str() {
            // 三类文件系统权限统一收敛到沙箱的 FileSystem（沙箱当前不区分读写与游戏目录）。
            "filesystem:read" | "filesystem:write" | "filesystem:game-dir" => {
                set.insert(Permission::FileSystem);
            }
            "network" => {
                set.insert(Permission::Network);
            }
            "process:spawn" => {
                set.insert(Permission::SpawnProcess);
            }
            "settings:write" => {
                set.insert(Permission::Settings);
            }
            "intents:request" => {
                set.insert(Permission::Intents);
            }
            // 沙箱暂无对应能力项：如实不授予，避免「清单声明了却拿到更宽权限」。
            "account:read" | "download:enqueue" => {
                log::warn!("[loader] 清单权限 `{name}` 在内核沙箱中暂无对应能力，未授予");
            }
            other => {
                log::warn!("[loader] 未知清单权限 `{other}`，未授予");
            }
        }
    }

    // 订阅上界非空 ⇒ 授予 `Events`。空列表不授予，使"空 = 什么都不收"在权限层也成立。
    if !events.is_empty() {
        set.insert(Permission::Events);
    }

    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_permissions_maps_known_and_drops_unknown() {
        let raw: Vec<String> = [
            "filesystem:read",
            "filesystem:write",
            "filesystem:game-dir",
            "network",
            "process:spawn",
            "settings:write",
            "intents:request",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let set = declared_permissions(&raw, &EventDeclarations::default());
        assert!(set.contains(&Permission::FileSystem));
        assert!(set.contains(&Permission::Network));
        assert!(set.contains(&Permission::SpawnProcess));
        assert!(set.contains(&Permission::Settings));
        assert!(set.contains(&Permission::Intents));
        // 三类文件系统权限收敛为一项，不应膨胀成三个。
        assert_eq!(set.len(), 5);
        // 事件声明为空时不得凭空获得 Events 能力。
        assert!(!set.contains(&Permission::Events));
    }

    #[test]
    fn declared_permissions_grants_events_only_for_a_non_empty_declaration() {
        // 空 = 什么都不收也不发：权限层也必须一致。
        assert!(!declared_permissions(&[], &EventDeclarations::default())
            .contains(&Permission::Events));

        let subscribing = EventDeclarations {
            subscribe: vec!["download.*".to_string()],
            publish: Vec::new(),
        };
        assert!(declared_permissions(&[], &subscribing).contains(&Permission::Events));

        // 只声明发布（不订阅）同样要授予：两个方向共用同一项能力。
        let publishing = EventDeclarations {
            subscribe: Vec::new(),
            publish: vec!["demo-tools.activity".to_string()],
        };
        let set = declared_permissions(&[], &publishing);
        assert!(set.contains(&Permission::Events));
        assert_eq!(set.len(), 1, "events 只映射到一项能力");
    }

    #[test]
    fn declared_permissions_is_fail_closed_for_unknown() {
        // 尚未有对应能力的清单权限不得被授予。
        let raw = vec!["account:read".to_string(), "download:enqueue".to_string()];
        assert!(declared_permissions(&raw, &EventDeclarations::default()).is_empty());
        assert!(declared_permissions(&["totally-made-up".to_string()], &EventDeclarations::default())
            .is_empty());
        assert!(declared_permissions(&[], &EventDeclarations::default()).is_empty());
    }

    #[test]
    fn scan_skips_illegal_dir_names() {
        let dir = std::env::temp_dir().join(format!("cgl-loader-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("copper-lamp.demo-tools")).unwrap();
        std::fs::create_dir_all(dir.join("NotValid")).unwrap();
        std::fs::create_dir_all(dir.join("..")).ok();
        std::fs::write(dir.join("stray-file"), b"x").unwrap();

        let names = scan_module_dirs(&dir).unwrap();
        assert_eq!(names, vec!["copper-lamp.demo-tools".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_missing_dir_is_empty_not_error() {
        let dir = std::env::temp_dir().join(format!("cgl-loader-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(scan_module_dirs(&dir).unwrap().is_empty());
    }
}
