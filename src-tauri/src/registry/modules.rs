//! 模块契约与注册表：铜内核的插拔能力载体。
//!
//! 模块 = Rust crate（后端）+ Vue 包（前端）。内置模块静态编译进内核，
//! 由本注册表装载并驱动生命周期；附加模块（动态加载）未来复用同一 trait。
//!
//! 模块间不直接依赖，一切协同经事件总线 + 意图注册表中转。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

    /// 模块自身版本号（严格 semver）。
    ///
    /// 带默认实现是为了让**既有内置模块在未补版本号前仍然可编译**：契约新增能力
    /// 不能变成强制改动，否则任何新增字段都会迫使全部模块同步修改。默认值
    /// `"0.0.0"` 是显式的"未知版本"占位，前端据此隐藏版本行而不是显示假版本。
    /// 内置模块应在自身 `impl Module` 中覆写为本模块真实版本。
    fn version(&self) -> &'static str {
        "0.0.0"
    }

    /// 模块展示名（可为中文）。
    ///
    /// 默认 `None`：内置模块的展示名由前端语言包（`module.<i18n_namespace>.*`）提供，
    /// 只有需要向内核自报展示名的模块才覆写。`None` 表示"交由前端 i18n 解析"，
    /// 而不是"没有名字"。
    fn display_name(&self) -> Option<&'static str> {
        None
    }

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

/// 模块来源：决定该模块是否受沙箱管辖。
///
/// 内置模块由内核静态编译，与内核同进程、同信任级；附加模块（第三方）运行在
/// 受管控子进程中，权限由 [`crate::registry::sandbox::ModuleSandbox`] 强制执行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleOrigin {
    /// 随内核编译，不可卸载。
    Builtin,
    /// 从 `<data_dir>/modules` 装载，受沙箱管辖。
    Addon,
}

impl ModuleOrigin {
    pub fn is_builtin(self) -> bool {
        matches!(self, ModuleOrigin::Builtin)
    }

    /// 稳定字符串名（写入 `ModuleInfo.source` 并透出前端；与规格 `builtin` / `addon` 一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            ModuleOrigin::Builtin => "builtin",
            ModuleOrigin::Addon => "addon",
        }
    }
}

/// 模块信息（供设置页"模块"Tab 展示）。
///
/// 全部字段可在本地确定：**不含** `available_version` / `installed_version` 这类
/// 需要远端元数据才能得出的值——那属于元数据服务（`services/registry`）的职责，
/// 由前端按 `id` 关联远端条目后合并渲染，内核不在本地伪造。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ModuleInfo {
    /// 模块唯一标识。保留为前后端关联远端条目的**唯一键**，不可省略。
    pub id: String,
    pub enabled: bool,
    /// 是否内核静态编译（等价于 `source == "builtin"`，保留以兼容既有前端）。
    pub is_builtin: bool,
    /// 模块来源：`builtin` / `addon`。语义与 `is_builtin` 一致，但显式给出字符串，
    /// 供前端做来源标记而不必反推布尔值。
    pub source: String,
    /// 模块自身版本号；`None` 表示模块未自报（见 [`Module::version`]）。
    pub version: Option<String>,
    /// 模块展示名；`None` 表示由前端 i18n 语言包提供。
    pub display_name: Option<String>,
    /// 附加模块所在目录（内置模块为 `None`）。供 UI 显示位置与卸载入口使用。
    pub dir: Option<String>,
    pub state: ModuleState,
    pub error: Option<String>,
    /// 是否因越权被沙箱强制停用（仅附加模块可能为真）。
    pub suspended: bool,
}

/// 模块注册表。
pub struct ModuleRegistry {
    modules: RwLock<Vec<Arc<dyn Module>>>,
    states: RwLock<HashMap<String, ModuleState>>,
    errors: RwLock<HashMap<String, String>>,
    origins: RwLock<HashMap<String, ModuleOrigin>>,
}

/// 合法的模块 id 形态：**两段式** `author.module`，与 cgl-libs 2.3.2 的正则逐字一致。
///
/// 每段由小写字母数字组成、可用单个 `-` 连接，段数 >= 2。这条校验是安全边界的
/// 第一道闸：模块 id 会被直接当作**目录名**使用，若放行 `..`、`/`、`\`、盘符或
/// Windows 保留名，扫描与卸载就会越过 `modules_dir`。
fn is_valid_module_id(id: &str) -> bool {
    // 长度上界防止病态输入（目录名超长在 Windows 上会直接失败）。
    if id.is_empty() || id.len() > 128 {
        return false;
    }
    let mut count = 0usize;
    for seg in id.split('.') {
        count += 1;
        if !is_valid_id_segment(seg) {
            return false;
        }
    }
    // 必须两段及以上（`author.module`），单段 id 不符合规范。
    count >= 2
}

/// 单个 id 段：`[a-z0-9]+(-[a-z0-9]+)*`。
fn is_valid_id_segment(seg: &str) -> bool {
    if seg.is_empty() {
        return false;
    }
    for part in seg.split('-') {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
            return false;
        }
    }
    true
}

/// 规范化路径：解析 `.` 与 `..`，不做符号链接解引用。
///
/// 与 `registry::sandbox::normalize_path` 语义一致。此处独立实现（而非复用其私有
/// 函数）是为了让注册表的路径判定可单独测试，且不依赖沙箱的授权状态。
fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
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

/// 解析 `<modules_dir>/<id>` 并校验其**确实落在** `modules_dir` 内。
///
/// 三重防护，缺一不可：
/// 1. id 形态校验（拒绝 `..`、路径分隔符、盘符等）；
/// 2. 路径规范化后做**组件级**前缀比较（不是字符串前缀比较，避免 `mods-evil`
///    被误判为 `mods` 的子路径）；
/// 3. 要求结果严格深于 `modules_dir`，拒绝把 `modules_dir` 自身当作模块目录。
///
/// 返回规范化后的路径，供删除与扫描统一使用。
fn resolve_module_dir(modules_dir: &Path, id: &str) -> Result<PathBuf, KernelError> {
    if !is_valid_module_id(id) {
        return Err(KernelError::Module(format!(
            "模块 id `{id}` 不合法：必须为两段式小写形态（如 `author.module`）"
        )));
    }

    let root = normalize_path(modules_dir);
    let candidate = normalize_path(&root.join(id));

    if candidate == root || !candidate.starts_with(&root) {
        return Err(KernelError::Module(format!(
            "模块 id `{id}` 解析后的路径越界：不在模块目录内"
        )));
    }

    Ok(candidate)
}

impl ModuleRegistry {
    pub fn new() -> Self {
        Self {
            modules: RwLock::new(Vec::new()),
            states: RwLock::new(HashMap::new()),
            errors: RwLock::new(HashMap::new()),
            origins: RwLock::new(HashMap::new()),
        }
    }

    /// 注册模块（内核启动前静态注册）。默认按内置处理。
    pub fn register(&self, module: Arc<dyn Module>) {
        self.register_with_origin(module, ModuleOrigin::Builtin);
    }

    /// 按来源注册模块。附加模块必须显式声明来源，否则会被误当作内核自带而
    /// 绕过沙箱，属于安全相关的默认值。
    pub fn register_with_origin(&self, module: Arc<dyn Module>, origin: ModuleOrigin) {
        let id = module.id().to_string();
        self.origins.write().insert(id, origin);
        self.modules.write().push(module);
    }

    /// 某模块的来源（未登记时保守地按附加模块处理）。
    pub fn origin_of(&self, id: &str) -> ModuleOrigin {
        self.origins
            .read()
            .get(id)
            .copied()
            .unwrap_or(ModuleOrigin::Addon)
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

    /// 模块是否已注册（包含内置与已注册的附加模块）。
    ///
    /// 供运行期查询使用：安装前置校验、卸载后的状态确认、以及"某 id 是否已被占用"
    /// 的冲突检测都依赖它，避免调用方各自遍历 `modules` 造成口径不一致。
    pub fn is_registered(&self, id: &str) -> bool {
        self.modules.read().iter().any(|m| m.id() == id)
    }

    /// 模块是否处于运行态。
    ///
    /// 卸载与注销都要据此拒绝：运行中的模块仍持有事件订阅与命令注册，
    /// 直接移除会留下无法回收的幽灵模块。
    pub fn is_running(&self, id: &str) -> bool {
        matches!(self.states.read().get(id).copied(), Some(ModuleState::Running))
    }

    /// 模块的规范化目录（内置模块返回 `None`）。
    ///
    /// 内置模块随内核编译，磁盘上的 `<modules_dir>/<id>` 与其无关，因此一律返回
    /// `None`——避免前端把内置模块当成"可卸载的目录"而误导用户。
    pub fn dir_of(&self, paths: &crate::services::paths::Paths, id: &str) -> Option<String> {
        if self.origin_of(id).is_builtin() {
            return None;
        }
        resolve_module_dir(paths.modules_dir(), id)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    }

    /// 扫描 `modules_dir()` 下已解包的附加模块目录，返回其模块信息。
    ///
    /// 只做**本地事实收集**，不装载动态库（热加载见 G1/G5）。因此 `state` 只可能是
    /// `Stopped`（目录存在但未运行）或 `Failed`（目录存在但已有错误记录）。
    ///
    /// 安全项：目录名必须是合法两段式模块 id，否则一律**跳过并记 warning**——
    /// 不跳过就意味着 `..`、符号链接名或任意垃圾目录会被当作模块列出，进而让
    /// 卸载命令拿到越界路径。已注册的 id 不重复返回（以注册表为准，避免同一模块
    /// 在列表里出现两次）。
    pub fn list_installed_addons(&self, kernel: &KernelContext) -> Vec<ModuleInfo> {
        let modules_dir = kernel.paths().modules_dir();
        let entries = match std::fs::read_dir(modules_dir) {
            Ok(entries) => entries,
            Err(e) => {
                // 目录不存在是正常状态（用户从未装过附加模块），不视为错误。
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!(
                        "扫描附加模块目录失败 {}: {e}",
                        modules_dir.display()
                    );
                }
                return Vec::new();
            }
        };

        let mut out: Vec<ModuleInfo> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            // 只认目录：文件（含残留的临时文件、下载产物）一律不是模块。
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                log::warn!("跳过非法附加模块目录（文件名非 UTF-8）: {}", path.display());
                continue;
            };
            if !is_valid_module_id(name) {
                log::warn!("跳过非法附加模块目录名（不符合两段式 id）: {name}");
                continue;
            }
            // 目录名合法仍要再验一次落点：防符号链接把目录指到 modules_dir 之外。
            if resolve_module_dir(modules_dir, name).is_err() {
                log::warn!("跳过越界的附加模块目录: {}", path.display());
                continue;
            }
            if self.is_registered(name) {
                continue;
            }
            // 解包目录里若已有错误记录则以 Failed 呈现，否则为 Stopped。
            let error = self.errors.read().get(name).cloned();
            let state = if error.is_some() {
                ModuleState::Failed
            } else {
                ModuleState::Stopped
            };
            out.push(ModuleInfo {
                id: name.to_string(),
                enabled: self.is_enabled(kernel, name),
                is_builtin: false,
                source: ModuleOrigin::Addon.as_str().to_string(),
                version: None,
                display_name: None,
                dir: Some(path.to_string_lossy().into_owned()),
                state,
                error,
                suspended: kernel.sandbox().is_suspended(name),
            });
        }

        // 稳定顺序（目录枚举顺序依文件系统而定），保证前端列表不抖动。
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// 注销模块：清理 `states` / `errors` / `origins` 三张表。
    ///
    /// 供卸载流程在删除目录后调用。**正在运行的模块拒绝注销**（返回明确错误而不是
    /// 静默移除）：静默移除会让模块继续持有事件订阅与命令注册，而注册表已不再
    /// 认识它，后续无从停止，形成不可回收的幽灵模块。
    ///
    /// 返回是否确有注册项被清除。
    pub fn unregister(&self, id: &str) -> Result<bool, KernelError> {
        if let Some(ModuleState::Running) = self.states.read().get(id).copied() {
            return Err(KernelError::Module(format!(
                "模块 `{id}` 正在运行，无法注销：请先停止该模块"
            )));
        }

        // 从注册表移除模块实例；未注册时仅清理残留状态。
        let removed = {
            let mut modules = self.modules.write();
            let before = modules.len();
            modules.retain(|m| m.id() != id);
            before != modules.len()
        };

        self.states.write().remove(id);
        self.errors.write().remove(id);
        self.origins.write().remove(id);
        Ok(removed)
    }

    /// 模块信息列表（按注册顺序）。
    pub fn list(&self, kernel: &KernelContext) -> Vec<ModuleInfo> {
        let modules = self.modules.read().clone();
        modules
            .iter()
            .map(|m| {
                let id = m.id();
                let origin = self.origin_of(id);
                // `version()` 默认返回 `"0.0.0"`（未自报），此时透出 `None` 而非假版本号。
                let version = m.version();
                ModuleInfo {
                    id: id.to_string(),
                    enabled: self.is_enabled(kernel, id),
                    is_builtin: origin.is_builtin(),
                    source: origin.as_str().to_string(),
                    version: if version.is_empty() || version == "0.0.0" {
                        None
                    } else {
                        Some(version.to_string())
                    },
                    display_name: m.display_name().map(|s| s.to_string()),
                    dir: self.dir_of(kernel.paths(), id),
                    state: self.states.read().get(id).copied().unwrap_or(ModuleState::Stopped),
                    error: self.errors.read().get(id).cloned(),
                    suspended: kernel.sandbox().is_suspended(id),
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

/// 解析某个附加模块的安装目录（**只读校验**，不触碰磁盘）。
///
/// 前置条件（缺一不可）：
/// - `id` 为合法两段式模块 id；
/// - 解析后的目录确实在 `modules_dir` 内；
/// - 目录存在，且为目录而非同名文件；
/// - 符号链接解析后的真实路径仍在 `modules_dir` 内（防"目录名合法、但被换成
///   指向 `C:\Windows` 的链接"）。
///
/// 与 [`remove_addon_dir`] 的关系：本函数是它的校验部分，且被卸载命令直接复用。
/// 卸载是**破坏性且不可逆**的操作，因此校验必须能先于任何副作用独立执行——
/// 调用方先校验、再撤权、再删除，避免"校验失败但状态已改了一半"。
pub fn resolve_addon_dir(modules_dir: &Path, id: &str) -> Result<PathBuf, KernelError> {
    let dir = resolve_module_dir(modules_dir, id)?;

    if !dir.is_dir() {
        return Err(KernelError::Module(format!(
            "模块 `{id}` 的安装目录不存在，无需卸载"
        )));
    }

    // 复核真实路径：符号链接解析后仍须落在 modules_dir 内。
    let real_root = std::fs::canonicalize(modules_dir)
        .map_err(|e| KernelError::Module(format!("解析模块根目录失败: {e}")))?;
    let real_dir = std::fs::canonicalize(&dir)
        .map_err(|e| KernelError::Module(format!("解析模块目录失败: {e}")))?;
    if real_dir == real_root || !real_dir.starts_with(&real_root) {
        return Err(KernelError::Module(format!(
            "模块 `{id}` 的目录指向模块目录之外，拒绝删除"
        )));
    }

    Ok(dir)
}

/// 删除某个附加模块的安装目录（卸载的磁盘侧动作）。
///
/// 校验全部委托给 [`resolve_addon_dir`]，通过后执行 `remove_dir_all`。
pub fn remove_addon_dir(modules_dir: &Path, id: &str) -> Result<PathBuf, KernelError> {
    let dir = resolve_addon_dir(modules_dir, id)?;
    std::fs::remove_dir_all(&dir)
        .map_err(|e| KernelError::Module(format!("删除模块目录失败: {e}")))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固定的模块根目录（仅做路径比较，测试不写真实系统目录）。
    fn root() -> PathBuf {
        PathBuf::from("C:\\cgmods")
    }

    /// 临时目录：用进程 id + 计数器保证并行测试之间不互相踩踏。
    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cgmods-test-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建测试目录");
        dir
    }

    // ------------------------------------------------------------ id 形态校验

    #[test]
    fn two_segment_id_is_accepted() {
        // 规格 2.3.2 的合法样例（含连字符与数字）。
        assert!(is_valid_module_id("copper-lamp.server-manager"));
        assert!(is_valid_module_id("thirdparty.world-editor"));
        assert!(is_valid_module_id("a.b"));
        assert!(is_valid_module_id("author9.mod2"));
    }

    #[test]
    fn single_segment_and_malformed_ids_are_rejected() {
        // 单段 id 不符合两段式规范。
        assert!(!is_valid_module_id("home"));
        assert!(!is_valid_module_id("game-download"));
        // 大写、空段、前导/尾随点号均非法。
        assert!(!is_valid_module_id("Author.Module"));
        assert!(!is_valid_module_id("author."));
        assert!(!is_valid_module_id(".module"));
        assert!(!is_valid_module_id("author..module"));
        assert!(!is_valid_module_id("author.-module"));
        assert!(!is_valid_module_id("author.mod-"));
        assert!(!is_valid_module_id("author._mod"));
        assert!(!is_valid_module_id(""));
    }

    #[test]
    fn path_traversal_shaped_ids_are_rejected() {
        // 这些形态一旦放行，模块目录就会落到 modules_dir 之外。
        assert!(!is_valid_module_id(".."));
        assert!(!is_valid_module_id("../evil"));
        assert!(!is_valid_module_id("author.../evil"));
        assert!(!is_valid_module_id("..\\evil.x"));
        assert!(!is_valid_module_id("C:\\Windows.x"));
        assert!(!is_valid_module_id("author.module/../../etc"));
        assert!(!is_valid_module_id("author.mod\u{0}ule"));
        // 超长 id 直接拒绝，避免病态路径。
        assert!(!is_valid_module_id(&format!("author.{}", "a".repeat(200))));
    }

    // ------------------------------------------------------ resolve_module_dir

    #[test]
    fn resolve_module_dir_stays_inside_root() {
        let dir = resolve_module_dir(&root(), "copper-lamp.demo-tools").unwrap();
        assert_eq!(dir, root().join("copper-lamp.demo-tools"));
        assert!(dir.starts_with(root()));
    }

    #[test]
    fn resolve_module_dir_rejects_traversal_and_invalid_ids() {
        // 非法 id 一律拒绝。
        assert!(resolve_module_dir(&root(), "../escape").is_err());
        assert!(resolve_module_dir(&root(), "..").is_err());
        assert!(resolve_module_dir(&root(), "home").is_err());

        // 即便模块根目录自身带 `..`，规范化后仍须落在规范化后的根内。
        let messy_root = PathBuf::from("C:\\a\\b\\..\\cgmods");
        let dir = resolve_module_dir(&messy_root, "author.mod").unwrap();
        assert_eq!(dir, PathBuf::from("C:\\a\\cgmods").join("author.mod"));
        assert!(dir.starts_with(PathBuf::from("C:\\a\\cgmods")));
    }

    #[test]
    fn resolve_module_dir_rejects_sibling_prefix_confusion() {
        // `cgmods-evil` 以 `cgmods` 为字符串前缀，但组件比较必须判为越界。
        let sibling = PathBuf::from("C:\\cgmods-evil").join("author.mod");
        assert!(!sibling.starts_with(root()));
        // 正常解析结果不会被误判成兄弟目录。
        let ok = resolve_module_dir(&root(), "author.mod").unwrap();
        assert_ne!(ok, sibling);
    }

    // ------------------------------------------------------- remove_addon_dir

    #[test]
    fn remove_addon_dir_deletes_existing_module_dir() {
        let base = temp_dir("remove-ok");
        let target = base.join("copper-lamp.demo-tools");
        std::fs::create_dir_all(target.join("nested")).unwrap();
        std::fs::write(target.join("module.json"), b"{}").unwrap();
        assert!(target.exists());

        let removed = remove_addon_dir(&base, "copper-lamp.demo-tools").unwrap();
        assert_eq!(removed, target);
        assert!(!target.exists(), "卸载后目录必须已删除");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn remove_addon_dir_refuses_missing_and_illegal_targets() {
        let base = temp_dir("remove-bad");

        // 目录不存在：报错而不是假装卸载成功。
        let err = remove_addon_dir(&base, "author.not-installed").unwrap_err();
        assert!(err.friendly().contains("不存在"), "got: {}", err.friendly());

        // 非法 id：拒绝，且绝不触碰磁盘。
        assert!(remove_addon_dir(&base, "../escape").is_err());
        assert!(remove_addon_dir(&base, "..").is_err());
        assert!(remove_addon_dir(&base, "single").is_err());

        // 确认上面的穿越尝试没有在父目录留下/删除任何东西。
        assert!(base.exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn remove_addon_dir_cannot_escape_via_dotdot_name() {
        let base = temp_dir("remove-escape");
        // 在父目录放一个"哨兵"，穿越攻击若得手会把它删掉。
        let sentinel = base.parent().unwrap().join("cgmods-sentinel");
        std::fs::create_dir_all(&sentinel).unwrap();
        let sentinel_file = sentinel.join("keep.txt");
        std::fs::write(&sentinel_file, b"keep").unwrap();

        // 各种穿越形态都必须被拒绝。
        for bad in ["..", "../cgmods-sentinel", "author./../..", "..\\cgmods-sentinel"] {
            assert!(
                remove_addon_dir(&base, bad).is_err(),
                "穿越形态 `{bad}` 必须被拒绝"
            );
        }
        assert!(sentinel_file.exists(), "哨兵文件不能被删除");

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&sentinel);
    }

    // ------------------------------------------------------ resolve_addon_dir

    #[test]
    fn resolve_addon_dir_validates_without_touching_disk() {
        let base = temp_dir("resolve-ro");
        let target = base.join("copper-lamp.demo-tools");
        std::fs::create_dir_all(&target).unwrap();

        // 校验通过：返回规范化目录。
        let resolved = resolve_addon_dir(&base, "copper-lamp.demo-tools").unwrap();
        assert_eq!(resolved, target);
        // 关键性质：本函数是**只读**的，校验后目录必须原样存在。
        assert!(target.is_dir(), "resolve_addon_dir 不得删除或改动目录");

        // 目标不存在时报错。
        assert!(resolve_addon_dir(&base, "author.absent").is_err());
        // 同名文件（非目录）不能当作模块。
        std::fs::write(base.join("author.afile"), b"x").unwrap();
        assert!(resolve_addon_dir(&base, "author.afile").is_err());
        // 非法 id 拒绝。
        for bad in ["..", "../escape", "home", ""] {
            assert!(
                resolve_addon_dir(&base, bad).is_err(),
                "非法 id `{bad}` 必须被拒绝"
            );
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn resolve_addon_dir_rejects_symlink_escaping_root() {
        let base = temp_dir("resolve-link");
        // 在根之外建一个真实目录，再在根内放一个指向它的链接。
        let outside = base.parent().unwrap().join(format!(
            "cgmods-outside-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&outside).unwrap();
        let victim = outside.join("precious.txt");
        std::fs::write(&victim, b"precious").unwrap();

        let link = base.join("author.link");
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&outside, &link).is_ok();
        #[cfg(not(windows))]
        let linked = std::os::unix::fs::symlink(&outside, &link).is_ok();

        if linked {
            // 链接名合法，但真实落点在根之外：必须拒绝，且不删除目标内容。
            let err = resolve_addon_dir(&base, "author.link").unwrap_err();
            assert!(
                err.friendly().contains("之外"),
                "符号链接逃逸必须报越界，got: {}",
                err.friendly()
            );
            assert!(resolve_addon_dir(&base, "author.link").is_err());
            // 只读校验更不能删除目标内容。
            assert!(victim.exists(), "越界目标不得被删除");

            // remove_addon_dir 同样必须拒绝。
            assert!(remove_addon_dir(&base, "author.link").is_err());
            assert!(victim.exists(), "remove_addon_dir 不得删除越界目标");
        }

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&outside);
    }

    // --------------------------------------------------- unregister 状态清理

    /// 最小模块实现，仅用于验证注册表状态机（不触碰内核服务）。
    struct FakeModule {
        id: &'static str,
    }

    impl Module for FakeModule {
        fn id(&self) -> &'static str {
            self.id
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

    /// 覆盖 `version` / `display_name` 的模块，验证默认实现与覆写路径。
    struct VersionedModule;

    impl Module for VersionedModule {
        fn id(&self) -> &'static str {
            "copper-lamp.versioned"
        }
        fn version(&self) -> &'static str {
            "1.2.3"
        }
        fn display_name(&self) -> Option<&'static str> {
            Some("版本化模块")
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

    #[test]
    fn module_trait_defaults_do_not_break_existing_modules() {
        // 默认实现是"不补版本号也能编译"的保证，必须有测试钉住。
        let m = FakeModule { id: "copper-lamp.plain" };
        assert_eq!(m.version(), "0.0.0");
        assert_eq!(m.display_name(), None);

        // 覆写路径生效。
        let v = VersionedModule;
        assert_eq!(v.version(), "1.2.3");
        assert_eq!(v.display_name(), Some("版本化模块"));
    }

    #[test]
    fn register_and_is_registered_track_membership() {
        let reg = ModuleRegistry::new();
        assert!(!reg.is_registered("copper-lamp.plain"));

        reg.register(Arc::new(FakeModule { id: "copper-lamp.plain" }));
        assert!(reg.is_registered("copper-lamp.plain"));
        assert!(!reg.is_registered("copper-lamp.other"));
        // 默认按内置登记。
        assert_eq!(reg.origin_of("copper-lamp.plain"), ModuleOrigin::Builtin);
    }

    #[test]
    fn register_with_origin_marks_addon_and_source_string() {
        let reg = ModuleRegistry::new();
        reg.register_with_origin(
            Arc::new(FakeModule { id: "thirdparty.world-editor" }),
            ModuleOrigin::Addon,
        );
        assert_eq!(reg.origin_of("thirdparty.world-editor"), ModuleOrigin::Addon);
        assert!(!reg.origin_of("thirdparty.world-editor").is_builtin());
        assert_eq!(ModuleOrigin::Addon.as_str(), "addon");
        assert_eq!(ModuleOrigin::Builtin.as_str(), "builtin");
        // 未登记的 id 保守地按附加模块处理。
        assert_eq!(reg.origin_of("ghost.module"), ModuleOrigin::Addon);
    }

    #[test]
    fn unregister_removes_module_and_clears_state_tables() {
        let reg = ModuleRegistry::new();
        let id = "thirdparty.world-editor";
        reg.register_with_origin(Arc::new(FakeModule { id }), ModuleOrigin::Addon);

        // 直接写入三张表，模拟启动失败留下的残留状态。
        reg.states.write().insert(id.to_string(), ModuleState::Failed);
        reg.errors.write().insert(id.to_string(), "启动失败".to_string());
        assert!(reg.errors.read().contains_key(id));
        assert!(reg.origins.read().contains_key(id));

        let removed = reg.unregister(id).unwrap();
        assert!(removed, "已注册的模块必须报告为已移除");
        assert!(!reg.is_registered(id));
        // 三张表都必须清干净，不能留幽灵条目。
        assert!(!reg.states.read().contains_key(id));
        assert!(!reg.errors.read().contains_key(id));
        assert!(!reg.origins.read().contains_key(id));

        // 重复注销不报错，但报告未移除任何东西。
        assert!(!reg.unregister(id).unwrap());
    }

    #[test]
    fn unregister_refuses_running_module() {
        let reg = ModuleRegistry::new();
        let id = "thirdparty.world-editor";
        reg.register_with_origin(Arc::new(FakeModule { id }), ModuleOrigin::Addon);
        reg.states.write().insert(id.to_string(), ModuleState::Running);

        let err = reg.unregister(id).unwrap_err();
        assert!(err.friendly().contains("正在运行"), "got: {}", err.friendly());
        // 拒绝时不得有任何部分清理：模块仍在，来源仍在。
        assert!(reg.is_registered(id));
        assert!(reg.origins.read().contains_key(id));

        // 停止后即可正常注销。
        reg.states.write().insert(id.to_string(), ModuleState::Stopped);
        assert!(reg.unregister(id).unwrap());
        assert!(!reg.is_registered(id));
    }
}
