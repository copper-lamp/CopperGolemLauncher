//! 模块命令：模块清单、启用 / 禁用、权限授权与越权留痕。

use std::collections::BTreeMap;
use std::str::FromStr;

use tauri::State;

use crate::commands::into_command_error;
use crate::error::{CommandResult, KernelError};
use crate::registry::modules::ModuleInfo;
use crate::registry::sandbox::{Grant, ModuleSandbox, Permission, ViolationRecord};
use crate::state::KernelContext;

/// 模块信息列表（供设置页"模块"Tab 展示）。
#[tauri::command]
pub fn modules_list(kernel: State<'_, KernelContext>) -> CommandResult<Vec<ModuleInfo>> {
    Ok(kernel.inner().modules().list(kernel.inner()))
}

/// 切换模块启用状态（下次启动生效）。
#[tauri::command]
pub fn modules_set_enabled(
    kernel: State<'_, KernelContext>,
    id: String,
    enabled: bool,
) -> CommandResult<()> {
    kernel
        .inner()
        .modules()
        .set_enabled(kernel.inner(), &id, enabled)
        .map_err(into_command_error)
}

/// 全部已知权限项（供前端渲染权限勾选列表，避免前端硬编码枚举）。
#[tauri::command]
pub fn sandbox_permissions() -> CommandResult<Vec<String>> {
    Ok(Permission::ALL.iter().map(|p| p.as_str().to_string()).collect())
}

/// 附加模块授权概览（模块 -> 声明 / 实际生效 / 是否停用）。
#[tauri::command]
pub fn sandbox_grants(kernel: State<'_, KernelContext>) -> CommandResult<BTreeMap<String, Grant>> {
    Ok(kernel.inner().sandbox().grants())
}

/// 越权留痕（按发生顺序；前端据此提示风险）。
#[tauri::command]
pub fn sandbox_violations(kernel: State<'_, KernelContext>) -> CommandResult<Vec<ViolationRecord>> {
    Ok(kernel.inner().sandbox().violations())
}

/// 为附加模块登记授权（安装时调用）。
///
/// `declared` 为清单声明的权限；未知权限名直接报错，避免静默降级成"少授权"。
#[tauri::command]
pub fn sandbox_grant(
    kernel: State<'_, KernelContext>,
    id: String,
    declared: Vec<String>,
) -> CommandResult<()> {
    let mut set = std::collections::HashSet::new();
    for name in &declared {
        set.insert(Permission::from_str(name).map_err(into_command_error)?);
    }
    let dir = ModuleSandbox::module_dir(kernel.inner().paths(), &id).map_err(into_command_error)?;
    kernel.inner().sandbox().grant(&id, set, dir);
    log::info!("module `{id}` granted {} permissions", declared.len());
    Ok(())
}

/// 由用户收紧某模块的实际权限（不能超出清单声明）。
#[tauri::command]
pub fn sandbox_revoke_permission(
    kernel: State<'_, KernelContext>,
    id: String,
    permission: String,
) -> CommandResult<()> {
    let perm = Permission::from_str(&permission).map_err(into_command_error)?;
    kernel
        .inner()
        .sandbox()
        .revoke_permission(&id, perm)
        .map_err(into_command_error)
}

/// 注销模块（卸载时调用）：清除授权与留痕。
#[tauri::command]
pub fn sandbox_revoke(kernel: State<'_, KernelContext>, id: String) -> CommandResult<()> {
    if id.trim().is_empty() {
        return Err(into_command_error(KernelError::InvalidArgument(
            "模块 id 不能为空".into(),
        )));
    }
    kernel.inner().sandbox().revoke(&id);
    Ok(())
}

/// 已解包的附加模块列表（`modules_dir()` 下扫描结果）。
///
/// 与 `modules_list` 的分工：后者是**注册表**视角（已装载的模块，含内置），
/// 本命令是**磁盘**视角（已安装但未必已装载的附加模块）。前端把两者按 `id`
/// 左外连接，即可区分"已安装未启用/未装载"与"已装载"。
#[tauri::command]
pub fn modules_installed_addons(kernel: State<'_, KernelContext>) -> CommandResult<Vec<ModuleInfo>> {
    Ok(kernel.inner().modules().list_installed_addons(kernel.inner()))
}

/// 卸载附加模块：校验 → 清沙箱授权 → 删目录 → 注销注册表状态。
///
/// 顺序是刻意的，两点都必须成立：
/// 1. **全部前置校验先做完、再动任何状态**。卸载是破坏性且不可逆的操作，
///    若先撤权再发现"目录不存在"，就留下了一致性被破坏、却没有真正卸载的中间态。
/// 2. **撤权在删文件之前**。反过来的话，删除与撤权之间会留下一个短暂窗口，
///    模块仍持有文件系统授权而目录已经消失（或被替换），可能被利用。
///
/// 拒绝的三种情况，均为明确错误而不是静默成功：
/// 1. `id` 非法（含路径穿越形态）；
/// 2. 内置模块（`origin == Builtin`）—— 随内核编译，物理上就不存在可删的目录；
/// 3. 模块正在运行 —— 运行中的模块必须先停止，否则会留下无法回收的幽灵模块。
#[tauri::command]
pub fn modules_uninstall(kernel: State<'_, KernelContext>, id: String) -> CommandResult<()> {
    let kernel = kernel.inner();
    let id = id.trim().to_string();

    if id.is_empty() {
        return Err(into_command_error(KernelError::InvalidArgument(
            "模块 id 不能为空".into(),
        )));
    }

    // 内置模块拒绝卸载：这是不可逆操作的第一道闸。
    //
    // 判定必须同时看"来源为内置"与"确实已注册"：`origin_of` 对**未登记**的 id
    // 保守地返回 `Addon`，若只看来源，一个拼错的 id 会被当作附加模块继续往下走。
    if kernel.modules().is_registered(&id) && kernel.modules().origin_of(&id).is_builtin() {
        return Err(into_command_error(KernelError::Module(format!(
            "模块 `{id}` 为内核内置模块，无法卸载"
        ))));
    }

    // 运行中的模块拒绝卸载（与 unregister 的口径一致，提前给出更准确的文案）。
    if kernel.modules().is_running(&id) {
        return Err(into_command_error(KernelError::Module(format!(
            "模块 `{id}` 正在运行，请先停止后再卸载"
        ))));
    }

    // 先做纯校验（id 合法性 + 目录存在 + 落点未越界），**不产生任何副作用**。
    // 校验通过后才进入有副作用的三步，避免"校验失败但状态已改了一半"。
    let module_dir = resolve_uninstall_target(kernel.paths().modules_dir(), &id)
        .map_err(into_command_error)?;

    // 撤权：清掉附加模块的授权档位与越权留痕。
    kernel.sandbox().revoke(&id);

    // 删目录。
    std::fs::remove_dir_all(&module_dir).map_err(|e| {
        into_command_error(KernelError::Module(format!(
            "删除模块 `{id}` 目录失败: {e}"
        )))
    })?;

    // 最后清注册表状态（states / errors / origins）。
    let removed = kernel.modules().unregister(&id).map_err(into_command_error)?;

    log::info!("module `{id}` uninstalled (registry entry removed: {removed})");
    Ok(())
}

/// 卸载目标的**只读**校验：返回通过全部安全检查的规范化目录。
///
/// 单独抽出来是为了让"校验"与"破坏"在代码结构上分离——校验失败时调用方尚未
/// 产生任何副作用（未撤权、未删文件）。
fn resolve_uninstall_target(
    modules_dir: &std::path::Path,
    id: &str,
) -> Result<std::path::PathBuf, KernelError> {
    // 复用注册表侧的落点校验（id 形态 + 路径规范化 + 组件级前缀比较）。
    let dir = crate::registry::modules::resolve_addon_dir(modules_dir, id)?;

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
