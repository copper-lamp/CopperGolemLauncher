//! 内容下载模块 · LIP 安装编排。
//!
//! 移植 LeviLauncher `internal/mcservice/lip_package.go` 的安装链路，把它对 lipd
//! 的调用落到 [`super::lipd`]：
//!
//! 1. 标识规范化 —— `owner/repo[#variant]` 或 `https://host/owner/repo` 统一成
//!    `github.com/owner/repo#<variant>`（缺省 variant 为 `client`）；
//! 2. 目标目录解析 —— 必须是**具体已安装版本目录**（LeviLauncher `resolveLIPTargetDir`
//!    用「版本名 → 版本目录」并要求目录存在）；lipd 在版本根目录执行会找不到 BDS 环境；
//! 3. 查询安装状态 —— 一次 `List` 得到全部包的安装状态（查询失败仅记日志，不阻断）；
//! 4. 锁定已装 LeviLamina 版本 —— 避免依赖求解器在宽范围（如 `1.9.*`）下拉取大量候选
//!    或替换 LL 版本；
//! 5. 安装 / 更新 —— 已显式安装则 `Update`，否则 `Install`；命中「已显式安装」冲突时
//!    先仅用目标包重试 `Install`，再回退为仅 `Update` 目标包。

use std::collections::HashMap;
use std::path::PathBuf;

use crate::state::KernelContext;

use super::lipd;

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
/// 安装 / 更新失败。
pub const ERR_LIP_PACKAGE_INSTALL_FAILED: &str = "ERR_LIP_PACKAGE_INSTALL_FAILED";

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
    fn daemon_failure(package: &str, code: &str, failure: lipd::DaemonFailure) -> Self {
        Self {
            success: false,
            package: package.to_string(),
            stdout: failure.logs.join("\n"),
            stderr: failure.message,
            error_code: Some(code.to_string()),
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
fn resolve_target_dir(kernel: &KernelContext, dir: Option<&str>) -> Result<PathBuf, &'static str> {
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

/// 经 lipd 安装 / 更新 LL 模组到目标版本目录。
///
/// `variant` 缺省时按 lip 约定回退到 `client`；`id` 需带 `lip:` 来源前缀或为裸标识。
/// 永不返回内核错误：域内失败以 `success=false` + `errorCode` 表达，便于前端按码提示。
pub async fn install(
    kernel: &KernelContext,
    id: &str,
    version: &str,
    variant: Option<&str>,
    dir: Option<&str>,
) -> LipInstallOutcome {
    let identifier = id.strip_prefix("lip:").unwrap_or(id).trim().to_string();
    // 前端把 variant 独立传参；若标识内已含 `#variant` 则不重复拼接。
    let identifier = match variant.map(str::trim).filter(|v| !v.is_empty()) {
        Some(variant) if !identifier.contains('#') => format!("{identifier}#{variant}"),
        _ => identifier,
    };

    let Some(exe) = lipd::find_lip_executable() else {
        return LipInstallOutcome::failure(
            ERR_LIP_NOT_INSTALLED,
            "未检测到 lipd，请先安装 lip（需要 .NET 10 运行时）",
        );
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

    let package = format!("{package_ref_base}@{version}");
    let target_only = vec![package.clone()];
    let mut install_packages = vec![package.clone()];
    let mut pinned_levilamina = false;

    // 查询安装状态：失败仅记日志，不阻断安装（与 LeviLauncher 一致）。
    let states = match lipd::list_package_states(&exe, &target_dir).await {
        Ok(states) => states,
        Err(e) => {
            log::warn!("[content-download] 查询 lip 安装状态失败（忽略）：{e}");
            HashMap::new()
        }
    };

    // 锁定已安装的 LeviLamina 版本，避免依赖求解器替换 LL 或拉取大量候选。
    if !package_ref_base.eq_ignore_ascii_case(LEVI_LAMINA_CLIENT_PACKAGE_REF_BASE) {
        if let Some(state) = states.get(&LEVI_LAMINA_CLIENT_PACKAGE_REF_BASE.to_lowercase()) {
            let pinned = state.installed_version.trim();
            if state.installed && !state.explicit_installed && !pinned.is_empty() {
                install_packages.push(format!("{LEVI_LAMINA_CLIENT_PACKAGE_REF_BASE}@{pinned}"));
                pinned_levilamina = true;
            }
        }
    }

    let target_explicit = states
        .get(&package_ref_base.to_lowercase())
        .is_some_and(|state| state.explicit_installed);

    if target_explicit {
        log::info!("[content-download] lip 更新 {package} → {}", target_dir.display());
        return match lipd::update_packages(&exe, &target_dir, &install_packages).await {
            Ok(logs) => LipInstallOutcome::done(&package, logs),
            Err(failure) => {
                LipInstallOutcome::daemon_failure(&package, ERR_LIP_PACKAGE_INSTALL_FAILED, failure)
            }
        };
    }

    log::info!("[content-download] lip 安装 {package} → {}", target_dir.display());
    match lipd::install_packages(&exe, &target_dir, &install_packages).await {
        Ok(logs) => LipInstallOutcome::done(&package, logs),
        Err(mut failure) => {
            // 锁定的 LeviLamina 已装导致冲突 → 仅用目标包重试一次 Install。
            if pinned_levilamina
                && is_already_installed_error_for_package(
                    &failure.message,
                    LEVI_LAMINA_CLIENT_PACKAGE_REF_BASE,
                )
            {
                match lipd::install_packages(&exe, &target_dir, &target_only).await {
                    Ok(logs) => return LipInstallOutcome::done(&package, logs),
                    Err(retry) => failure = retry,
                }
            }
            // 任意「已显式安装」冲突 → 回退为仅更新目标包。
            if is_already_installed_error(&failure.message) {
                log::warn!("[content-download] lip 安装冲突，回退更新：{}", failure.message);
                return match lipd::update_packages(&exe, &target_dir, &target_only).await {
                    Ok(logs) => LipInstallOutcome::done(&package, logs),
                    Err(update) => LipInstallOutcome::daemon_failure(
                        &package,
                        ERR_LIP_PACKAGE_INSTALL_FAILED,
                        update,
                    ),
                };
            }
            log::error!("[content-download] lip 安装失败：{}", failure.message);
            LipInstallOutcome::daemon_failure(&package, ERR_LIP_PACKAGE_INSTALL_FAILED, failure)
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