//! UWP 实例启动：松散注册 → 包数据目录重定向 → 系统激活。
//!
//! 与 GDK 实例的启动链路（挂起启动 + 远程线程注入 hook DLL）**完全分开**：
//!
//! | | GDK | UWP |
//! |---|---|---|
//! | 拉起方式 | `CreateProcessW(CREATE_SUSPENDED)` + 注入 | `IApplicationActivationManager::ActivateApplication` |
//! | 隔离实现 | hook DLL 重定向 `%APPDATA%` / `%TEMP%` | 包状态子目录做成指向实例目录的连接点 |
//! | 前提 | 实例目录里有可执行文件 | 系统里已松散注册该包（注册状态是全局的，同一时刻只指向一个实例） |
//! | 加载器 | LeviLamina 可注入 | 不支持（见 `uwp_deploy` 模块注释第 2 条） |
//!
//! # 为什么每次启动都要重新注册 / 重定向
//!
//! 包注册是**包族级**的全局状态：同一包族在系统里只能有一个注册，它指向哪个目录，
//! 激活就从哪个目录拉起。用户切换实例、或系统在别处（商店安装）动过这个包族之后，
//! 记录都会变。因此这两步放在**启动前每次执行**，而且都是幂等的：
//!
//! - 注册：已经指向本实例时 `RegisterPackageByUriAsync` 是空操作（`uwp_deploy::register`
//!   带 `ForceUpdateFromAnyVersion`，结束时回读校验指向）；
//! - 重定向：连接点已经指对时零写入（`uwp_deploy::redirect_data_dirs` 幂等）。
//!
//! # 前置条件与失败文案
//!
//! 松散注册要求系统开启**开发者模式**（`0x80073CFF`）；依赖框架缺失会报
//! `0x80073CF3`。这两类失败必须原样透传成可读原因——用户能据此去改系统设置，
//! 而一句「启动失败」永远引导不到那里。

use std::path::Path;

use crate::error::KernelError;
use crate::modules::game_download::uwp_deploy;

use super::isolate;
use super::meta::{UwpVersionMeta, VersionMeta};

/// 启动前的完整准备：注册包 → 重定向数据目录，返回游戏进程 pid。
///
/// 返回 pid 之后由调用方（`launch::launch_game`）交给监控任务；本函数不做任何
/// 「等窗口」的判定，那属于监控的职责。
pub fn start(name: &str, dir: &Path, meta: &VersionMeta) -> Result<u32, KernelError> {
    let identity = meta.uwp_identity().ok_or_else(|| {
        KernelError::InvalidArgument(format!(
            "实例 `{name}` 缺少 UWP 包身份信息（version.json 的 uwp 字段），请重新安装"
        ))
    })?;

    // 1) 松散注册：确保系统里的包指向**这个实例目录**。
    let registered = uwp_deploy::register(dir).map_err(|error| {
        KernelError::Config(format!(
            "实例 `{name}` 的 UWP 包注册失败（需要开启 Windows 开发者模式）: {error}"
        ))
    })?;
    log::info!(
        "[home/uwp] 实例 `{name}` 已注册：{}（开发模式={}，目录={}）",
        registered.full_name,
        registered.development_mode,
        registered.install_location.display()
    );
    // 注册信息与元数据必须一致：元数据里的包族用于隔离目录与 AUMID，错了就是
    // 「启动的是另一个包」或「数据写到别处」。
    if !registered
        .family_name
        .eq_ignore_ascii_case(&identity.family_name)
    {
        return Err(KernelError::Config(format!(
            "实例 `{name}` 的 UWP 包族不一致：系统记录 `{}`，元数据 `{}`",
            registered.family_name, identity.family_name
        )));
    }

    // 2) 数据隔离：把包状态下的游戏数据子目录指向实例目录（幂等）。
    let data_dir = isolate::data_dir(dir, meta);
    isolate::ensure_skeleton(dir, meta)?;
    let report = uwp_deploy::redirect_data_dirs(&identity.family_name, &data_dir).map_err(|error| {
        KernelError::Config(format!("实例 `{name}` 的 UWP 数据目录隔离失败: {error}"))
    })?;
    if !report.moved_aside.is_empty() {
        log::warn!(
            "[home/uwp] 实例 `{name}` 接管包数据时让位了 {} 个已有目录（用户数据未删除）: {:?}",
            report.moved_aside.len(),
            report.moved_aside
        );
    }
    log::info!(
        "[home/uwp] 实例 `{name}` 数据目录：{}（重定向 {} 项）",
        data_dir.display(),
        report.redirected.len()
    );

    // 3) 系统激活：走包身份，和商店安装的启动方式完全一致。
    let aumid = identity.app_user_model_id();
    let pid = uwp_deploy::activate(&aumid).map_err(|error| {
        KernelError::Config(format!("实例 `{name}` 激活失败（{aumid}）: {error}"))
    })?;
    log::info!("[home/uwp] 实例 `{name}` 已激活，pid={pid}");
    Ok(pid)
}

/// 删除实例前的清理：断开指向该实例数据目录的连接点，并注销指向该目录的包注册。
///
/// 顺序固定为「先断连接点，再注销」：连接点指向的是即将被删除的实例目录，
/// 先断开可以保证注销动作执行期间包状态目录里不存在悬空重解析点（注销本身不访问
/// 这些子目录，但留下悬空连接点会让下一次写入以 `0xC0000098` 一类错误失败）。
/// 两步都只依赖本机状态，不涉及网络。
///
/// 失败只记日志、不阻断删除：用户点的是「删除实例」，一条清理不掉的注册记录
/// 不应该让目录删不掉（下次启动时该包族的注册会被新的注册覆盖）。
pub fn cleanup(dir: &Path, meta: &VersionMeta) {
    let family = meta
        .uwp_identity()
        .map(|identity| identity.family_name.clone());
    if let Some(family) = family.as_deref() {
        if let Err(error) = uwp_deploy::clear_data_redirect(family, &isolate::data_dir(dir, meta)) {
            log::warn!("[home/uwp] 断开 UWP 数据目录连接点失败（不影响删除）: {error}");
        }
    }
    match uwp_deploy::unregister(dir) {
        Ok(true) => log::info!("[home/uwp] 已注销实例目录的 UWP 包注册: {}", dir.display()),
        Ok(false) => log::debug!("[home/uwp] 该目录没有对应的 UWP 包注册，无需注销"),
        Err(error) => log::warn!("[home/uwp] 注销 UWP 包失败（不影响删除）: {error}"),
    }
}

/// 该实例对应的 UWP 包当前是否注册在别处（供设置页/诊断用）。
pub fn registered_elsewhere(identity: &UwpVersionMeta, dir: &Path) -> Result<bool, KernelError> {
    let registered = uwp_deploy::query(&identity.identity_name)
        .map_err(|error| KernelError::Config(format!("查询 UWP 包注册状态失败: {error}")))?;
    Ok(match registered {
        Some(package) => !same_path(&package.install_location, dir),
        None => false,
    })
}

fn same_path(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        resolved
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    normalize(left) == normalize(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_identity_is_rejected_before_any_system_call() {
        // 元数据缺 UWP 身份时必须**在触碰系统之前**失败：没有身份就无从激活，
        // 先去注册只会把一个必然失败的包注册进系统。
        let dir = std::path::Path::new("D:/v/demo");
        let meta = VersionMeta {
            name: "demo".into(),
            version_type: "release".into(),
            package_type: Some("uwp".into()),
            ..Default::default()
        };
        let error = start("demo", dir, &meta).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("UWP 包身份"), "错误应点名缺失的身份信息: {text}");
    }
}
