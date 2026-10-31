//! 版本元数据桥：复用开始页 `home::meta` 约定落 `version.json`，使安装版本可被开始页识别。

use std::path::{Path, PathBuf};

use crate::error::KernelError;
use crate::modules::home::meta;

/// 判定实例目录已装好游戏（存在可解析的 `version.json`；与开始页扫描口径一致）。
pub fn is_installed(dir: &Path) -> bool {
    meta::VersionMeta::read(dir).is_some()
}

/// 由 slug 还原数值版本号（快照版剥离 `_preview` 后缀）。
pub fn game_version_of(slug: &str) -> String {
    slug
        .strip_suffix("_preview")
        .unwrap_or(slug)
        .to_string()
}

/// 写版本元数据：`version.json`（兼容 LeviLauncher / 开始页 `VersionMeta`）。
///
/// `dir` 为实例目录，`instance` 为实例名（= 目录名），`version_id` 为版本 slug，
/// `kind` 取 `release` / `preview`，`loader` 为该实例已装好的加载器版本。
///
/// **读改写而不是整体覆盖**：实例装好后用户会在开始页改渲染龙、启动参数、环境变量，
/// 重装加载器时若整份重写，这些设置会被悄悄抹掉。这里只写本模块负责的字段
/// （`name` / `gameVersion` / `type` / `enableIsolation` / `loader`），其余原样保留。
///
/// **不写 `registered`**：本模块的安装动作只是把 MSIXVC / APPX **解包**到版本目录，
/// 全程不调用 `Add-AppxPackage`，包没有被注册进系统。而开始页把 `registered = true`
/// 理解为「该版本由系统 `minecraft://` 协议接管启动」，一旦误标为 true，开始页就会去
/// 唤起协议 —— 系统的协议处理程序指向的是商店安装的那个包（若装了）或根本不存在
/// （本项目场景），用户侧表现为唤起失败而不是启动游戏。真实可启动的路径始终是直接
/// 运行版本目录下的 `Minecraft.Windows.exe`。新建实例时该字段保持默认 `false`，
/// 已有实例则保留其现值，由开始页在启动时按系统事实纠正（见 `home::launch`）。
pub fn write_meta(
    dir: &Path,
    instance: &str,
    version_id: &str,
    kind: &str,
    loader: Option<&str>,
) -> Result<(), KernelError> {
    let mut meta = meta::VersionMeta::read(dir).unwrap_or_default();
    meta.name = instance.to_string();
    meta.game_version = game_version_of(version_id);
    meta.version_type = kind.to_string();
    // 实例隔离是本模块装出来的版本必备能力（共享 Store 授权、独立存档目录）。
    meta.enable_isolation = true;
    if meta.created_at.is_empty() {
        meta.created_at = meta::now_rfc3339();
    }
    if let Some(loader) = loader.map(str::trim).filter(|v| !v.is_empty()) {
        meta.loader = Some(loader.to_string());
    }
    // GDK 包没有系统包身份；显式清掉，防止「同一个实例目录先装 UWP 后又装 GDK」时
    // 旧身份残留（启动链会据 packageType 决定走松散注册还是直接 spawn）。
    meta.package_type = Some("gdk".to_string());
    meta.uwp = None;
    meta::VersionMeta::write(dir, &meta)
}

/// 写 UWP 实例的版本元数据。
///
/// 与 [`write_meta`] 同一套「读改写、只覆盖本模块字段」的规则，额外写两份东西：
/// - `packageType = "uwp"`：启动链据此走「系统松散注册 + 激活」；
/// - `uwp`：包身份三件套（身份名 / 包族名 / Application Id），缺失就无法激活。
///
/// `game_version` 用包清单换算出的版本号（`1.21.11401.0` → `1.21.114.1`），而不是
/// slug 字符串：前者是权威口径（开始页、编辑器门槛、加载器匹配都用它），后者只是
/// 目录名。
pub fn write_uwp_meta(
    dir: &Path,
    instance: &str,
    version_id: &str,
    kind: &str,
    manifest: &super::uwp::PackageManifest,
) -> Result<(), KernelError> {
    let mut meta = meta::VersionMeta::read(dir).unwrap_or_default();
    meta.name = instance.to_string();
    meta.game_version = manifest
        .game_version()
        .unwrap_or_else(|| game_version_of(version_id));
    meta.version_type = kind.to_string();
    meta.enable_isolation = true;
    if meta.created_at.is_empty() {
        meta.created_at = meta::now_rfc3339();
    }
    // UWP 实例不支持加载器（见 installer::enqueue_uwp）：把可能残留的旧值清掉。
    meta.loader = None;
    meta.package_type = Some("uwp".to_string());
    meta.uwp = Some(meta::UwpVersionMeta {
        identity_name: manifest.identity_name.clone(),
        family_name: manifest.family_name().to_string(),
        application_id: manifest.application_id.clone(),
        package_version: manifest.package_version.clone(),
    });
    // UWP 包不走「注册后由系统协议唤起」这条路：注册动作在每次启动时由启动链执行，
    // 这个字段对 UWP 没有意义，保持 false 以免开始页据此改变按钮语义。
    meta.registered = false;
    meta::VersionMeta::write(dir, &meta)
}

/// 读某实例目录里已装好的加载器版本（无元数据 / 未装加载器为 `None`）。
pub fn installed_loader(dir: &Path) -> Option<String> {
    meta::VersionMeta::read(dir).and_then(|m| m.loader)
}

/// 安装目录（版本根下）。
pub fn install_dir(versions_root: &Path, folder: &str) -> PathBuf {
    versions_root.join(folder)
}
