//! 内容下载模块 · 内置 Rust LIP 安装编排。
//!
//! 将界面包标识规范化为 LIP package ref，解析具体游戏目录，并调用内置引擎完成依赖求解、
//! 资产获取与事务安装。游戏下载复用同一安装核心与进度事件接口。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crate::state::KernelContext;

use crate::modules::lip::{self, Cancel, Environment, LipError, Sink};

/// LeviLamina 客户端包引用（安装前置依赖）。
const LEVI_LAMINA_CLIENT_PACKAGE_REF_BASE: &str = "github.com/LiteLDev/LeviLamina#client";

/// 未检测到 lipd 运行环境。
pub const ERR_LIP_NOT_INSTALLED: &str = "ERR_LIP_NOT_INSTALLED";
/// 包标识无法规范化为合法包引用。
pub const ERR_LIP_PACKAGE_INVALID_IDENTIFIER: &str = "ERR_LIP_PACKAGE_INVALID_IDENTIFIER";
/// 未提供版本号。
pub const ERR_LIP_PACKAGE_VERSION_REQUIRED: &str = "ERR_LIP_PACKAGE_VERSION_REQUIRED";
/// 目标版本目录不存在。
pub const ERR_TARGET_NOT_FOUND: &str = "ERR_TARGET_NOT_FOUND";
/// 安装被用户取消。
pub const ERR_LIP_CANCELLED: &str = "ERR_LIP_CANCELLED";
/// 安装 / 更新失败。
pub const ERR_LIP_PACKAGE_INSTALL_FAILED: &str = "ERR_LIP_PACKAGE_INSTALL_FAILED";

// ---------------------------------------------------------------- 取消登记

/// 正在执行的安装任务 → 取消标记。
///
/// 登记表放在模块级而不是 `KernelContext` 上：上下文按设计只聚合服务、不持有业务状态，
/// 而「哪个作业正在跑」正是短生命周期的业务状态。键是作业 id（内容安装的
/// `lip-job:<uuid>`、游戏下载的 `game:<实例名>`），值是该作业的取消标记。
static ACTIVE_JOBS: OnceLock<Mutex<HashMap<String, Arc<Cancel>>>> = OnceLock::new();

fn active_jobs() -> &'static Mutex<HashMap<String, Arc<Cancel>>> {
    ACTIVE_JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 登记一次安装作业，返回它的取消标记（同一作业重复登记会替换旧标记）。
pub fn begin_job(job: &str) -> Arc<Cancel> {
    let cancel = Arc::new(Cancel::new());
    if let Ok(mut jobs) = active_jobs().lock() {
        jobs.insert(job.to_string(), Arc::clone(&cancel));
    }
    cancel
}

/// 作业结束后撤销登记。只移除仍指向自己那个标记的条目，避免误删同名后续作业。
pub fn finish_job(job: &str, cancel: &Arc<Cancel>) {
    if let Ok(mut jobs) = active_jobs().lock() {
        if jobs.get(job).is_some_and(|current| Arc::ptr_eq(current, cancel)) {
            jobs.remove(job);
        }
    }
}

/// 请求取消作业；找不到（已完成 / 未开始）返回 `false`。
pub fn cancel_job(job: &str) -> bool {
    let cancel = active_jobs().lock().ok().and_then(|jobs| jobs.get(job).cloned());
    match cancel {
        Some(cancel) => {
            cancel.cancel();
            true
        }
        None => false,
    }
}

/// lip 安装结果（域内失败以 `success=false` + `error_code` 表达，不抛内核错误）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LipInstallOutcome {
    pub success: bool,
    /// 实际下发的包引用（`github.com/owner/repo#variant@version`）。
    pub package: String,
    /// daemon 日志回调汇总。
    pub stdout: String,
    /// 失败原因（成功时为空）。
    pub stderr: String,
    /// 机器可读错误码（成功时为 `None`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

impl LipInstallOutcome {
    /// 构造域内失败结果（`package` 未知时留空）。
    fn failure(code: &str, message: impl Into<String>) -> Self {
        Self {
            success: false,
            package: String::new(),
            stdout: String::new(),
            stderr: message.into(),
            error_code: Some(code.to_string()),
        }
    }

    /// 构造 daemon 调用失败结果（保留已收集的日志）。
    pub(crate) fn lip_failure(package: &str, code: &str, error: LipError) -> Self {
        Self {
            success: false,
            package: package.to_string(),
            stdout: String::new(),
            stderr: error.to_string(),
            error_code: Some(if matches!(error, LipError::Cancelled) { "ERR_LIP_CANCELLED" } else { code }.to_string()),
        }
    }

    /// 构造成功结果。
    fn done(package: &str, logs: Vec<String>) -> Self {
        Self {
            success: true,
            package: package.to_string(),
            stdout: logs.join("\n"),
            stderr: String::new(),
            error_code: None,
        }
    }
}

// ---------------------------------------------------------------- 标识规范化

/// 标识解析中间结果（与 LeviLauncher `lipPackageIdentifierParts` 对应）。
#[derive(Debug, Default)]
struct IdentifierParts {
    /// `owner/repo`（无 host、无 variant）。
    base: String,
    /// variant（`#` 之后部分）。
    variant: String,
    /// 是否出现过 `#` 标记（区分「无 variant」与「variant 为空」）。
    has_variant_marker: bool,
}

/// 规范化标识：去空白、剥离 `#variant`、剥离 `http(s)://`、去首尾 `/`，
/// 并抽取 `owner/repo`（3 段且首段含 `.` 时视为 host）。
fn canonicalize_identifier(identifier: &str) -> IdentifierParts {
    let mut normalized = identifier.trim().to_string();
    if normalized.is_empty() {
        return IdentifierParts::default();
    }

    let mut variant = String::new();
    let mut has_variant_marker = false;
    if let Some(index) = normalized.find('#') {
        has_variant_marker = true;
        variant = normalized[index + 1..].trim().to_string();
        normalized = normalized[..index].trim().to_string();
    }

    let lower = normalized.to_lowercase();
    if lower.starts_with("https://") {
        normalized = normalized["https://".len()..].to_string();
    } else if lower.starts_with("http://") {
        normalized = normalized["http://".len()..].to_string();
    }
    let normalized = normalized.trim_matches('/').to_string();
    if normalized.is_empty() {
        return IdentifierParts::default();
    }

    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() == 2 {
        return IdentifierParts {
            base: parts.join("/"),
            variant,
            has_variant_marker,
        };
    }
    if parts.len() == 3 && parts[0].contains('.') {
        return IdentifierParts {
            base: format!("{}/{}", parts[1], parts[2]),
            variant,
            has_variant_marker,
        };
    }
    IdentifierParts::default()
}

/// 标识 → `(规范化标识, 包引用基址)`；非法标识返回 `None`。
///
/// - 无 variant：规范化标识 = `owner/repo`，包引用 = `github.com/owner/repo#client`
///   （lip 客户端包默认 variant 为 `client`）；
/// - 有 variant：规范化标识 = `owner/repo#variant`，包引用 = `github.com/owner/repo#variant`；
/// - 有 `#` 但 variant 为空：规范化标识 = `owner/repo#`，包引用 = `github.com/owner/repo`。
fn build_package_ref_base(identifier: &str) -> Option<(String, String)> {
    let parts = canonicalize_identifier(identifier);
    if !is_valid_package_base(&parts.base) {
        return None;
    }
    if !parts.variant.is_empty() {
        return Some((
            format!("{}#{}", parts.base, parts.variant),
            format!("github.com/{}#{}", parts.base, parts.variant),
        ));
    }
    if parts.has_variant_marker {
        return Some((format!("{}#", parts.base), format!("github.com/{}", parts.base)));
    }
    Some((
        parts.base.clone(),
        format!("github.com/{}#client", parts.base),
    ))
}

/// `owner/repo` 合法性（等价 LeviLauncher `lipIdentifierPattern`：
/// `^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$`）。
fn is_valid_package_base(base: &str) -> bool {
    let mut segments = base.split('/');
    let (Some(owner), Some(repo), None) = (segments.next(), segments.next(), segments.next()) else {
        return false;
    };
    !owner.is_empty()
        && !repo.is_empty()
        && owner.chars().all(is_identifier_char)
        && repo.chars().all(is_identifier_char)
}

fn is_identifier_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

// ---------------------------------------------------------------- 目标目录

/// 解析安装目标目录。
///
/// - 显式 `dir` 非空：直接采用（要求已是目录）；
/// - 否则取设置 `launch.default_version`，解析为版本根下的具体版本目录并要求存在。
pub(super) fn resolve_target_dir(kernel: &KernelContext, dir: Option<&str>) -> Result<PathBuf, &'static str> {
    if let Some(dir) = dir.map(str::trim).filter(|d| !d.is_empty()) {
        let path = PathBuf::from(dir);
        return if path.is_dir() {
            Ok(path)
        } else {
            Err(ERR_TARGET_NOT_FOUND)
        };
    }

    let version = kernel
        .settings()
        .get::<String>("launch.default_version")
        .unwrap_or_default();
    let version = version.trim().to_string();
    if version.is_empty() {
        return Err(ERR_TARGET_NOT_FOUND);
    }
    let path = crate::modules::home::meta::resolve_version_dir(&kernel.versions_root(), &version)
        .map_err(|_| ERR_TARGET_NOT_FOUND)?;
    if path.is_dir() {
        Ok(path)
    } else {
        Err(ERR_TARGET_NOT_FOUND)
    }
}

// ---------------------------------------------------------------- 安装编排

/// 经内置引擎安装 / 更新 LL 模组到目标版本目录。
///
/// `variant` 缺省时按 lip 约定回退到 `client`；`id` 需带 `lip:` 来源前缀或为裸标识。
/// `sink` 用于把引擎的步骤与进度回传（下载中心据此画进度条）；`cancel` 由调用方登记，
/// 用户取消后引擎在每个可中断点退出并按 journal 回滚。
/// 永不返回内核错误：域内失败以 `success=false` + `errorCode` 表达，便于前端按码提示。
pub async fn install(
    kernel: &KernelContext,
    id: &str,
    version: &str,
    variant: Option<&str>,
    dir: Option<&str>,
    sink: Option<&Sink>,
    cancel: &Cancel,
) -> LipInstallOutcome {
    let identifier = id.strip_prefix("lip:").unwrap_or(id).trim().to_string();
    // 前端把 variant 独立传参；若标识内已含 `#variant` 则不重复拼接。
    let identifier = match variant.map(str::trim).filter(|v| !v.is_empty()) {
        Some(variant) if !identifier.contains('#') => format!("{identifier}#{variant}"),
        _ => identifier,
    };

    let cache = kernel.paths().cache_dir().join("lip");
    let engine = match lip::Engine::new(Environment::current(), cache) {
        Ok(engine) => engine,
        Err(error) => return LipInstallOutcome::lip_failure("", ERR_LIP_PACKAGE_INSTALL_FAILED, error),
    };

    let Some((_normalized, package_ref_base)) = build_package_ref_base(&identifier) else {
        return LipInstallOutcome::failure(
            ERR_LIP_PACKAGE_INVALID_IDENTIFIER,
            format!("无效的 lip 包标识：{identifier}"),
        );
    };

    let version = version.trim();
    if version.is_empty() {
        return LipInstallOutcome::failure(ERR_LIP_PACKAGE_VERSION_REQUIRED, "lip 安装需要指定版本号");
    }

    let target_dir = match resolve_target_dir(kernel, dir) {
        Ok(dir) => dir,
        Err(code) => {
            return LipInstallOutcome::failure(
                code,
                "未找到目标版本目录，请先在设置中指定「默认版本」",
            )
        }
    };

    install_resolved(&engine, &package_ref_base, version, &target_dir, false, sink, cancel).await
}

/// 内核无关的安装核心：把已解析的包引用装进一个**已存在**的目标目录。
///
/// 从 [`install`] 拆出来是为了让「游戏下载」模块能在装完游戏实例后自动补装
/// LeviLamina：那条链路跑在内核事件里、手里只有一组 Arc 服务（`game_download::Ctx`），
/// 既拿不到也用不上整个 `KernelContext`。拆出的边界正好落在「一切环境探测与路径解析
/// 都已完成」之后——本函数只依赖内置引擎、包引用、版本号、目标目录与取消标记。
pub async fn install_resolved(
    engine: &lip::Engine,
    package_ref_base: &str,
    version: &str,
    target_dir: &std::path::Path,
    with_dependencies: bool,
    sink: Option<&Sink>,
    cancel: &Cancel,
) -> LipInstallOutcome {
    if crate::modules::home::compatibility::is_bound(target_dir) { return LipInstallOutcome::failure("compat_mode_unsupported", "兼容绑定不支持加载器或模组安装"); }
    let version = version.trim();
    if version.is_empty() {
        return LipInstallOutcome::failure(ERR_LIP_PACKAGE_VERSION_REQUIRED, "lip 安装需要指定版本号");
    }
    if !target_dir.is_dir() {
        return LipInstallOutcome::failure(ERR_TARGET_NOT_FOUND, "目标版本目录不存在");
    }

    let package = format!("{package_ref_base}@{version}");
    log::info!("[content-download] 内置 LIP 安装 {package} → {}", target_dir.display());
    if let Some(sink) = sink { sink.stage("download.stage.lip_querying"); }
    let state = match engine.state(target_dir) {
        Ok(state) => state,
        Err(error) => return LipInstallOutcome::lip_failure(&package, ERR_LIP_PACKAGE_INSTALL_FAILED, error),
    };
    let states = state.package_states().unwrap_or_default();
    let target_explicit = states.get(&package_ref_base.to_ascii_lowercase()).is_some_and(|s| s.explicit_installed);
    if let Some(sink) = sink {
        sink.stage("download.stage.lip_installing");
        sink.progress(&format!("准备安装 {package}"), None);
    }
    let event_sink = sink.cloned().unwrap_or_default();
    let result = if target_explicit {
        engine.update(target_dir, &[package.clone()], with_dependencies, &event_sink, &cancel).await
    } else {
        engine.install(target_dir, &[package.clone()], false, with_dependencies, &event_sink, &cancel).await
    };
    match result {
        Ok(_) => LipInstallOutcome::done(&package, Vec::new()),
        Err(error) => {
            // 失败文本此前只进 outcome，日志里没有任何细节，实机上只能看到一行错误码。
            log::warn!("[content-download] 内置 LIP 安装失败 {package}: {error}");
            LipInstallOutcome::lip_failure(&package, ERR_LIP_PACKAGE_INSTALL_FAILED, error)
        }
    }
}

/// 是否为「已显式安装」冲突（与 LeviLauncher `isLipInstallAlreadyInstalledError` 一致）。
fn is_already_installed_error(message: &str) -> bool {
    let message = message.trim().to_lowercase();
    if message.is_empty() {
        return false;
    }
    message.contains("already explicitly installed")
        || (message.contains("cannot install package") && message.contains("already installed"))
}

/// 冲突信息是否指向指定包（比对完整包引用，或 `#` 之前的路径）。
fn is_already_installed_error_for_package(message: &str, package_ref: &str) -> bool {
    if !is_already_installed_error(message) {
        return false;
    }
    let target = package_ref.trim().to_lowercase();
    if target.is_empty() {
        return true;
    }
    let message = message.trim().to_lowercase();
    if message.contains(&target) {
        return true;
    }
    match target.find('#') {
        Some(hash) => message.contains(&target[..hash]),
        None => false,
    }
}

// ---------------------------------------------------------------- 单测

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_ref_defaults_to_client_variant() {
        let (normalized, package_ref) = build_package_ref_base("liteldev/tstamp").unwrap();
        assert_eq!(normalized, "liteldev/tstamp");
        assert_eq!(package_ref, "github.com/liteldev/tstamp#client");
    }

    #[test]
    fn package_ref_keeps_explicit_variant() {
        let (normalized, package_ref) = build_package_ref_base("liteldev/tstamp#server").unwrap();
        assert_eq!(normalized, "liteldev/tstamp#server");
        assert_eq!(package_ref, "github.com/liteldev/tstamp#server");
    }

    #[test]
    fn package_ref_accepts_url_and_three_segments() {
        let (_, package_ref) = build_package_ref_base("https://github.com/liteldev/tstamp").unwrap();
        assert_eq!(package_ref, "github.com/liteldev/tstamp#client");
        let (_, package_ref) = build_package_ref_base("github.com/liteldev/tstamp").unwrap();
        assert_eq!(package_ref, "github.com/liteldev/tstamp#client");
    }

    #[test]
    fn package_ref_rejects_invalid_identifier() {
        assert!(build_package_ref_base("").is_none());
        assert!(build_package_ref_base("just-a-name").is_none());
        assert!(build_package_ref_base("a/b/c/d").is_none());
        assert!(build_package_ref_base("a b/c").is_none());
    }

    #[test]
    fn package_ref_handles_empty_variant_marker() {
        let (normalized, package_ref) = build_package_ref_base("liteldev/tstamp#").unwrap();
        assert_eq!(normalized, "liteldev/tstamp#");
        assert_eq!(package_ref, "github.com/liteldev/tstamp");
    }

    #[test]
    fn already_installed_error_detection() {
        assert!(is_already_installed_error(
            "cannot install package github.com/x/y#client@1.0.0: already explicitly installed"
        ));
        assert!(is_already_installed_error(
            "cannot install package: it is already installed as a dependency"
        ));
        assert!(!is_already_installed_error("network unreachable"));

        let message = "cannot install package github.com/LiteLDev/LeviLamina#client@1.0.0: already explicitly installed";
        assert!(is_already_installed_error_for_package(
            message,
            "github.com/LiteLDev/LeviLamina#client"
        ));
        assert!(!is_already_installed_error_for_package(
            message,
            "github.com/other/mod#client"
        ));
    }
}