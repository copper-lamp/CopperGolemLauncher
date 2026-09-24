//! 模块隔离：附加模块的能力授权与越权拦截。
//!
//! 背景：附加模块（第三方）不再以内核同进程的动态库方式加载 —— 同进程共享地址
//! 空间与全部权限，`permissions` 只能靠自觉，审核红线无法真正执行。因此附加模块
//! 运行在**受管控的子进程**中，由本模块持有该进程的能力授权表并在每一次跨进程请求
//! 上强制执行权限，越权直接拒绝并留痕。
//!
//! 本模块只负责「授权 + 强制 + 留痕」三件事，不含进程拉起与 IPC 传输（见
//! `ModuleHost` 契约）。这样权限判定可以独立测试，也便于日后更换传输方式。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::KernelError;
use crate::services::paths::Paths;

/// 模块声明的最小权限集（与 `module.json` 的 `permissions` 一一对应）。
///
/// 取值固定为 9 项，新增权限必须同时更新 cgl-libs 的字段表与审核口径，
/// 因此这里用穷举枚举而非字符串，保证未知名在解析期就被拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// 读写自己模块目录之外的文件。
    FileSystem,
    /// 发起网络请求。
    Network,
    /// 读写内核数据库（限定在 `module:<id>` scope）。
    Database,
    /// 读写内核设置。
    Settings,
    /// 注册前端界面（导航项 / 页面）。
    Ui,
    /// 发布与订阅事件总线。
    Events,
    /// 声明与发起意图。
    Intents,
    /// 拉起外部进程。
    SpawnProcess,
    /// 弹出系统通知。
    Notification,
}

impl Permission {
    /// 全部权限（用于校验与文档生成）。
    pub const ALL: [Permission; 9] = [
        Permission::FileSystem,
        Permission::Network,
        Permission::Database,
        Permission::Settings,
        Permission::Ui,
        Permission::Events,
        Permission::Intents,
        Permission::SpawnProcess,
        Permission::Notification,
    ];

    /// 稳定字符串名（写入清单、日志与前端；与 `module.json` 一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::FileSystem => "file_system",
            Permission::Network => "network",
            Permission::Database => "database",
            Permission::Settings => "settings",
            Permission::Ui => "ui",
            Permission::Events => "events",
            Permission::Intents => "intents",
            Permission::SpawnProcess => "spawn_process",
            Permission::Notification => "notification",
        }
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Permission {
    type Err = KernelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Permission::ALL
            .iter()
            .copied()
            .find(|p| p.as_str() == s)
            .ok_or_else(|| KernelError::Module(format!("未知权限 `{s}`")))
    }
}

/// 一次越权尝试的留痕记录。
///
/// 留痕不是可选项：审核红线要求越权行为可追溯，且安装后的模块无法靠事后下架补救，
/// 所以每次拒绝都必须记录到本地，供设置页展示。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ViolationRecord {
    pub module_id: String,
    pub permission: Permission,
    /// 被拒绝的具体操作（如 `fs.write`、`http.get`）。
    pub operation: String,
    /// 目标（路径 / 主机 / 表名），用于人工判断是否恶意。
    pub target: String,
    /// Unix 毫秒时间戳。
    pub at_ms: u64,
}

/// 单个模块的授权档位。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Grant {
    pub module_id: String,
    /// 清单声明的权限。
    pub declared: HashSet<Permission>,
    /// 内核当前实际授予的权限（用户可在设置页收紧，但不能放宽到声明的之外）。
    pub effective: HashSet<Permission>,
    /// 模块目录（文件系统权限的边界）。
    pub module_dir: std::path::PathBuf,
    /// 是否已因越权被强制停用。
    pub suspended: bool,
}

/// 沙箱：附加模块的授权与强制执行中心。
pub struct ModuleSandbox {
    grants: parking_lot::RwLock<HashMap<String, Grant>>,
    violations: parking_lot::RwLock<Vec<ViolationRecord>>,
    /// 单模块越权上限，达到即强制停用（防止刷屏式试探与日志膨胀）。
    violation_limit: usize,
}

/// 越权次数上限：超过即视为恶意，强制停用该模块。
const DEFAULT_VIOLATION_LIMIT: usize = 32;

/// 留痕上限，超出后丢弃最旧记录（仍保留停用计数）。
const VIOLATION_CAP: usize = 512;

impl ModuleSandbox {
    pub fn new() -> Self {
        Self {
            grants: parking_lot::RwLock::new(HashMap::new()),
            violations: parking_lot::RwLock::new(Vec::new()),
            violation_limit: DEFAULT_VIOLATION_LIMIT,
        }
    }

    /// 登记模块的授权档位。
    ///
    /// `declared` 来自模块清单，是授权的**上界**：模块无法获得未声明的权限，
    /// 也不能靠调用方传参放宽。`module_dir` 一律规范化后保存，作为文件系统
    /// 权限的比较基准。
    pub fn grant(
        &self,
        module_id: &str,
        declared: HashSet<Permission>,
        module_dir: std::path::PathBuf,
    ) {
        let mut grants = self.grants.write();
        grants.insert(
            module_id.to_string(),
            Grant {
                module_id: module_id.to_string(),
                effective: declared.clone(),
                declared,
                module_dir: normalize_path(&module_dir),
                suspended: false,
            },
        );
    }

    /// 由声明集派生模块目录：`<data_dir>/modules/<id>`。
    ///
    /// 目录不存在时创建，确保文件系统权限有确定的边界可校验。返回值已规范化，
    /// 与 `enforce_path` 的比较基准保持一致。
    pub fn module_dir(paths: &Paths, module_id: &str) -> Result<std::path::PathBuf, KernelError> {
        let dir = normalize_path(&paths.modules_dir().join(module_id));
        std::fs::create_dir_all(&dir)
            .map_err(|e| KernelError::Module(format!("创建模块目录失败: {e}")))?;
        Ok(dir)
    }

    /// 检查某权限是否已授予且未被停用。
    pub fn is_allowed(&self, module_id: &str, permission: Permission) -> bool {
        self.grants
            .read()
            .get(module_id)
            .map(|g| !g.suspended && g.effective.contains(&permission))
            .unwrap_or(false)
    }

    /// 该模块是否受沙箱管辖（即已登记授权）。
    ///
    /// 未登记的模块视为内核内置、与内核同信任级；已登记的模块（附加模块）
    /// 一律走权限校验。判定依据是沙箱自身状态，模块无法自我声明豁免。
    pub fn knows(&self, module_id: &str) -> bool {
        self.grants.read().contains_key(module_id)
    }

    /// 强制执行一次操作。未授权则留痕并返回错误。
    ///
    /// 这是所有权能路径的唯一入口：任何模块发起的跨进程请求都要经过这里，
    /// 避免出现绕过判定的旁路。
    pub fn enforce(
        &self,
        module_id: &str,
        permission: Permission,
        operation: &str,
        target: &str,
    ) -> Result<(), KernelError> {
        if self.is_allowed(module_id, permission) {
            return Ok(());
        }

        let suspended = self.record_violation(module_id, permission, operation, target);
        if suspended {
            Err(KernelError::Module(format!(
                "模块 `{module_id}` 越权调用 `{operation}`（缺少 `{permission}` 权限），已强制停用"
            )))
        } else {
            Err(KernelError::Module(format!(
                "模块 `{module_id}` 越权调用 `{operation}`：未声明 `{permission}` 权限"
            )))
        }
    }

    /// 校验文件路径是否落在模块目录内。
    ///
    /// 路径先规范化再比较，防止 `..` 穿越；未声明 `file_system` 时一律拒绝，
    /// 声明了也只允许模块目录内部（外部访问走 `enforce` 的 `file_system` 放行点）。
    pub fn enforce_path(
        &self,
        module_id: &str,
        path: &std::path::Path,
        operation: &str,
    ) -> Result<(), KernelError> {
        let grant = self.grants.read();
        let Some(g) = grant.get(module_id) else {
            drop(grant);
            return self.enforce(module_id, Permission::FileSystem, operation, &path.display().to_string());
        };
        let root = g.module_dir.clone();
        let suspended = g.suspended;
        drop(grant);

        if suspended {
            return Err(KernelError::Module(format!("模块 `{module_id}` 已被强制停用")));
        }

        // 相对路径无法可靠判断归属（其结果取决于进程 cwd），一律拒绝，
        // 避免规范化后意外落在模块目录内而被放行。
        if !path.is_absolute() {
            let _ = self.record_violation(
                module_id,
                Permission::FileSystem,
                operation,
                &path.display().to_string(),
            );
            return Err(KernelError::Module(format!(
                "模块 `{module_id}` 拒绝相对路径 `{}`：文件访问必须使用绝对路径",
                path.display()
            )));
        }

        // 规范化：消除 `.` 与 `..`，得到可用于前缀比较的绝对路径。
        let normalized = normalize_path(path);
        if !normalized.starts_with(&root) {
            let _ = self.record_violation(
                module_id,
                Permission::FileSystem,
                operation,
                &path.display().to_string(),
            );
            return Err(KernelError::Module(format!(
                "模块 `{module_id}` 越界访问 `{}`：超出模块目录",
                path.display()
            )));
        }

        // 目录内访问仍需 `file_system` 声明，保持「声明即能力」的一致语义。
        self.enforce(module_id, Permission::FileSystem, operation, &path.display().to_string())
    }

    /// 记录一次越权。返回是否因此触发强制停用。
    fn record_violation(
        &self,
        module_id: &str,
        permission: Permission,
        operation: &str,
        target: &str,
    ) -> bool {
        let record = ViolationRecord {
            module_id: module_id.to_string(),
            permission,
            operation: operation.to_string(),
            target: target.to_string(),
            at_ms: now_ms(),
        };

        {
            let mut log = self.violations.write();
            log.push(record);
            if log.len() > VIOLATION_CAP {
                let overflow = log.len() - VIOLATION_CAP;
                log.drain(0..overflow);
            }
        }

        let mut grants = self.grants.write();
        let Some(g) = grants.get_mut(module_id) else {
            return false;
        };
        // 未登记的模块没有档位可停用；已登记的按累计次数判定。
        let count = self
            .violations
            .read()
            .iter()
            .filter(|v| v.module_id == module_id)
            .count();
        if count >= self.violation_limit && !g.suspended {
            g.suspended = true;
            log::error!(
                "module `{module_id}` suspended after {count} permission violations"
            );
            return true;
        }
        false
    }

    /// 某模块是否已被强制停用。
    pub fn is_suspended(&self, module_id: &str) -> bool {
        self.grants
            .read()
            .get(module_id)
            .map(|g| g.suspended)
            .unwrap_or(false)
    }

    /// 当前授权概览（供设置页"模块"Tab 展示权限）。
    pub fn grants(&self) -> BTreeMap<String, Grant> {
        self.grants
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// 越权留痕（供设置页展示风险）。
    pub fn violations(&self) -> Vec<ViolationRecord> {
        self.violations.read().clone()
    }

    /// 注销模块（卸载时调用）。
    pub fn revoke(&self, module_id: &str) {
        self.grants.write().remove(module_id);
        self.violations
            .write()
            .retain(|v| v.module_id != module_id);
    }

    /// 由用户收紧某模块的实际权限。
    ///
    /// 只能收紧、不能放宽：若目标权限不在清单声明内，说明调用方在尝试提权，
    /// 直接拒绝而不是静默加进 `effective`。
    pub fn revoke_permission(
        &self,
        module_id: &str,
        permission: Permission,
    ) -> Result<(), KernelError> {
        let mut grants = self.grants.write();
        let g = grants
            .get_mut(module_id)
            .ok_or_else(|| KernelError::Module(format!("模块 `{module_id}` 未登记授权")))?;
        if !g.declared.contains(&permission) {
            return Err(KernelError::Module(format!(
                "模块 `{module_id}` 未声明 `{permission}` 权限，无法操作"
            )));
        }
        g.effective.remove(&permission);
        Ok(())
    }

    /// 恢复某模块到清单声明的完整权限（用户改主意时调用）。
    pub fn restore_permission(
        &self,
        module_id: &str,
        permission: Permission,
    ) -> Result<(), KernelError> {
        let mut grants = self.grants.write();
        let g = grants
            .get_mut(module_id)
            .ok_or_else(|| KernelError::Module(format!("模块 `{module_id}` 未登记授权")))?;
        if !g.declared.contains(&permission) {
            return Err(KernelError::Module(format!(
                "模块 `{module_id}` 未声明 `{permission}` 权限，无法授予"
            )));
        }
        g.effective.insert(permission);
        Ok(())
    }
}

impl Default for ModuleSandbox {
    fn default() -> Self {
        Self::new()
    }
}

/// 规范化路径：解析 `.` 与 `..`，不做符号链接解引用（符号链接逃逸由调用方在
/// 真实路径上再校验一次）。
fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;

    let mut out = std::path::PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn perms(items: &[Permission]) -> HashSet<Permission> {
        items.iter().copied().collect()
    }

    /// 模块目录基址：仅做路径比较，无需真实存在，因此测试不触碰文件系统，
    /// 可在受限环境下稳定运行。
    fn module_root(module_id: &str) -> std::path::PathBuf {
        std::path::Path::new("C:\\cgsbx").join(module_id)
    }

    fn sandbox_with(module_id: &str, declared: &[Permission]) -> (ModuleSandbox, std::path::PathBuf) {
        let sb = ModuleSandbox::new();
        let dir = module_root(module_id);
        sb.grant(module_id, perms(declared), dir.clone());
        (sb, dir)
    }

    #[test]
    fn permission_roundtrips_through_str() {
        for p in Permission::ALL {
            assert_eq!(Permission::from_str(p.as_str()).unwrap(), p);
        }
        assert!(Permission::from_str("root").is_err());
        assert!(Permission::from_str("").is_err());
    }

    #[test]
    fn granted_permission_is_allowed() {
        let (sb, _) = sandbox_with("demo", &[Permission::Network]);
        assert!(sb.is_allowed("demo", Permission::Network));
        assert!(!sb.is_allowed("demo", Permission::FileSystem));
    }

    #[test]
    fn undeclared_permission_is_rejected_and_recorded() {
        let (sb, _) = sandbox_with("demo", &[Permission::Network]);

        let err = sb
            .enforce("demo", Permission::SpawnProcess, "process.spawn", "cmd.exe")
            .unwrap_err();
        assert!(err.friendly().contains("spawn_process"));
        assert!(err.friendly().contains("demo"));

        let log = sb.violations();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].module_id, "demo");
        assert_eq!(log[0].permission, Permission::SpawnProcess);
        assert_eq!(log[0].target, "cmd.exe");
        assert!(!sb.is_suspended("demo"));
    }

    #[test]
    fn declared_permission_passes_enforcement() {
        let (sb, _) = sandbox_with("demo", &[Permission::Network, Permission::Database]);
        assert!(sb.enforce("demo", Permission::Network, "http.get", "example.com").is_ok());
        assert!(sb.enforce("demo", Permission::Database, "db.query", "kv").is_ok());
        assert!(sb.violations().is_empty());
    }

    #[test]
    fn unknown_module_is_denied_by_default() {
        let sb = ModuleSandbox::new();
        assert!(!sb.is_allowed("ghost", Permission::Network));
        assert!(sb.enforce("ghost", Permission::Network, "http.get", "x").is_err());
    }

    #[test]
    fn path_inside_module_dir_is_allowed() {
        let (sb, dir) = sandbox_with("demo", &[Permission::FileSystem]);
        let inside = dir.join("cache").join("a.bin");
        assert!(sb.enforce_path("demo", &inside, "fs.write").is_ok());
        assert!(sb.violations().is_empty());
    }

    #[test]
    fn dotdot_escape_is_blocked() {
        let (sb, dir) = sandbox_with("demo", &[Permission::FileSystem]);
        // 构造 `..` 穿越：模块目录的上层就是 data_dir。
        let escape = dir.join("..").join("..").join("core.db");
        let err = sb.enforce_path("demo", &escape, "fs.write").unwrap_err();
        assert!(err.friendly().contains("越界"), "got: {}", err.friendly());
        assert_eq!(sb.violations().len(), 1);
    }

    #[test]
    fn dotdot_that_stays_inside_is_allowed() {
        let (sb, dir) = sandbox_with("demo", &[Permission::FileSystem]);
        // `sub/../file` 规范化后仍在模块目录内，不应误判。
        let ok = dir.join("sub").join("..").join("file.txt");
        assert!(sb.enforce_path("demo", &ok, "fs.write").is_ok());
    }

    #[test]
    fn path_check_requires_file_system_declaration() {
        let (sb, dir) = sandbox_with("demo", &[Permission::Network]);
        let inside = dir.join("a.bin");
        let err = sb.enforce_path("demo", &inside, "fs.write").unwrap_err();
        assert!(err.friendly().contains("file_system"));
    }

    #[test]
    fn relative_path_is_rejected() {
        let (sb, _) = sandbox_with("demo", &[Permission::FileSystem]);
        let err = sb
            .enforce_path("demo", std::path::Path::new("cache/a.bin"), "fs.write")
            .unwrap_err();
        assert!(err.friendly().contains("绝对路径"), "got: {}", err.friendly());
        assert_eq!(sb.violations().len(), 1);
    }

    #[test]
    fn sibling_module_dir_is_not_confused_with_prefix() {
        // `cgsbx-demo2` 以 `cgsbx-demo` 为字符串前缀，但组件比较必须判为越界。
        let sb = ModuleSandbox::new();
        let dir = module_root("cgsbx-demo");
        let sibling = module_root("cgsbx-demo2").join("steal.bin");
        sb.grant("cgsbx-demo", perms(&[Permission::FileSystem]), dir);

        let err = sb
            .enforce_path("cgsbx-demo", &sibling, "fs.write")
            .unwrap_err();
        assert!(err.friendly().contains("越界"), "got: {}", err.friendly());
    }

    #[test]
    fn repeated_violations_suspend_the_module() {
        let (sb, _) = sandbox_with("demo", &[Permission::Network]);
        for _ in 0..DEFAULT_VIOLATION_LIMIT {
            let _ = sb.enforce("demo", Permission::SpawnProcess, "process.spawn", "cmd.exe");
        }
        assert!(sb.is_suspended("demo"));
        // 停用后即使原本已声明的权限也被拒绝。
        let err = sb.enforce("demo", Permission::Network, "http.get", "x").unwrap_err();
        assert!(err.friendly().contains("停用"));
    }

    #[test]
    fn violation_log_is_bounded() {
        let sb = ModuleSandbox::new();
        for i in 0..(VIOLATION_CAP + 50) {
            let _ = sb.enforce("ghost", Permission::Network, "http.get", &format!("h{i}"));
        }
        assert!(sb.violations().len() <= VIOLATION_CAP);
    }

    #[test]
    fn revoke_clears_grant_and_history() {
        let (sb, _) = sandbox_with("demo", &[Permission::Network]);
        let _ = sb.enforce("demo", Permission::Ui, "ui.register", "nav");
        assert_eq!(sb.violations().len(), 1);

        sb.revoke("demo");
        assert!(sb.violations().is_empty());
        assert!(!sb.is_allowed("demo", Permission::Network));
        assert!(sb.grants().is_empty());
    }

    #[test]
    fn user_can_tighten_but_not_widen_permissions() {
        let (sb, _) = sandbox_with("demo", &[Permission::Network, Permission::Database]);

        // 收紧：声明过的权限可以撤销。
        sb.revoke_permission("demo", Permission::Network).unwrap();
        assert!(!sb.is_allowed("demo", Permission::Network));
        assert!(sb.is_allowed("demo", Permission::Database));

        // 恢复：回到清单声明的范围。
        sb.restore_permission("demo", Permission::Network).unwrap();
        assert!(sb.is_allowed("demo", Permission::Network));

        // 提权：未声明的权限不能授予。
        let err = sb.restore_permission("demo", Permission::SpawnProcess).unwrap_err();
        assert!(err.friendly().contains("未声明"), "got: {}", err.friendly());
        assert!(!sb.is_allowed("demo", Permission::SpawnProcess));
    }

    #[test]
    fn revoke_permission_rejects_unknown_module() {
        let sb = ModuleSandbox::new();
        assert!(sb.revoke_permission("ghost", Permission::Network).is_err());
        assert!(sb.restore_permission("ghost", Permission::Network).is_err());
    }
}
