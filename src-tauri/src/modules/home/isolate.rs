//! 实例隔离：数据目录规则与版本相关硬规则的**唯一事实源**。
//!
//! # 为什么要单独成文件
//!
//! 隔离路径此前由 `content.rs` 与 `content_download/install_target.rs` 各自拼装，
//! 而**游戏真正写到哪里由注入 DLL 决定**（`docs/启动链路与实例隔离.md` §2.3）。
//! 三方各算一遍必然漂移 —— LeviLauncher 已经在 1.26 系列上分叉过一次：
//! 它的 Go 侧内容根不带版本分支，C++ 侧对 `>= 1.26.0.0` 少算一层
//! （`libs/LeviLauncher/internal/mcservice/versions.go:220` vs
//! `native/levilauncher/src/config/version_config.cpp:97`）。
//!
//! 所以规则只在这里实现一次，其余模块一律引用本文件。
//!
//! # 隔离是强制的
//!
//! 隔离没有关闭入口（产品决策）。读到的 `enableIsolation: false` 一律按
//! `true` 处理并由 [`enforce_isolation`] 自愈写盘。

use std::path::{Path, PathBuf};

use crate::error::KernelError;

use super::meta::VersionMeta;

/// 正式渠道的数据目录名（与游戏内部 sAppName 一致）。
const RELEASE_DATA_DIR: &str = "Minecraft Bedrock";
/// 预览渠道的数据目录名。
const PREVIEW_DATA_DIR: &str = "Minecraft Bedrock Preview";

/// 共享内容根相对数据根的路径（资源包 / 行为包 / 世界都在这下面）。
pub const SHARED_GAME_DIR: &str = "games/com.mojang";
/// 世界目录名。
pub const WORLDS_DIR: &str = "minecraftWorlds";

/// 模板目录名（MCBE 只在开发者模式下装载）。
pub const TEMPLATES_DIR: &str = "development_behavior_packs";
/// 资源包目录名。
pub const RESOURCE_PACKS_DIR: &str = "resource_packs";
/// 行为包目录名。
pub const BEHAVIOR_PACKS_DIR: &str = "behavior_packs";

/// 安卓实例内的游戏数据目录名（与 Java 宿主 `CopperGameLayout.GAME_DATA_DIR` 一致）。
const ANDROID_GAME_DIR: &str = "game";
/// 安卓实例内 Minecraft 的 files 目录名（与 `CopperGameLayout.gameFilesDir` 一致）。
const ANDROID_FILES_DIR: &str = "files";

/// 编辑器模式最低版本：正式渠道。
const EDITOR_RELEASE_MIN: [u64; 4] = [1, 21, 50, 0];
/// 编辑器模式最低版本：预览渠道（预览版比正式版早很多）。
const EDITOR_PREVIEW_MIN: [u64; 4] = [1, 19, 80, 0];

/// 内容根目录视图。
#[derive(Debug, Clone)]
pub struct ContentRoots {
    /// 共享内容根：`.../com.mojang`。
    pub com_mojang: PathBuf,
    /// 用户目录根：`.../Users`。
    pub users_root: PathBuf,
}

/// 是否预览渠道。
pub fn is_preview(meta: &VersionMeta) -> bool {
    meta.version_type.eq_ignore_ascii_case("preview")
}

/// 渠道对应的数据目录名。
fn data_dir_name(meta: &VersionMeta) -> &'static str {
    if is_preview(meta) {
        PREVIEW_DATA_DIR
    } else {
        RELEASE_DATA_DIR
    }
}

/// 解析版本号为四段数字；段数不足补零，无法解析返回 `None`。
///
/// 只接受纯数字段：`1.21.130.20` → `[1,21,130,20]`。`1.21.x` 这类含非数字
/// 段的写法（如目录名残留）一律视为未知，交由调用方按「不支持门槛」处理，
/// 不猜版本 —— 猜错会让编辑器模式或布局规则落到错误的分支上。
pub fn parse_version(raw: &str) -> Option<[u64; 4]> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = [0u64; 4];
    for (index, part) in trimmed.split('.').take(4).enumerate() {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        out[index] = part.parse().ok()?;
    }
    Some(out)
}

/// 判断 `version` 是否不低于 `min`；无法解析时返回 `false`（按不支持处理）。
fn version_at_least(version: [u64; 4], min: [u64; 4]) -> bool {
    version >= min
}

/// 该版本是否支持游戏内编辑器模式。
///
/// 门槛来自 LeviLauncher `internal/versions/editor.go:5`：编辑器在预览版
/// 自 1.19.80.20、正式版自 1.21.50 起可用。`beta` 渠道走正式版门槛
/// （Legacy Beta 用的是正式包）。版本号解析不出来时按**不支持**处理：
/// 宁可不给这个开关，也不能给一个必然启动失败的 `-Editor true`。
pub fn supports_editor_mode(game_version: &str, version_type: &str) -> bool {
    let Some(version) = parse_version(game_version) else {
        return false;
    };
    let min = if version_type.eq_ignore_ascii_case("preview") {
        EDITOR_PREVIEW_MIN
    } else {
        EDITOR_RELEASE_MIN
    };
    version_at_least(version, min)
}

/// 隔离实例的数据根目录。
///
/// 规则：`<版本目录>/<渠道目录>`（`Minecraft Bedrock` / `Minecraft Bedrock
/// Preview`）。这与游戏自身的行为一致 —— 游戏把 sAppName 拼在 `%APPDATA%`
/// 之后，而隔离把 `%APPDATA%` 重定向到版本目录（注入 DLL 负责，规则见
/// `docs/启动链路与实例隔离.md`）。
pub fn data_dir(version_dir: &Path, meta: &VersionMeta) -> PathBuf {
    version_dir.join(data_dir_name(meta))
}

/// 「扁平布局」候选根：数据直接落在版本目录（不带渠道目录那层）。
///
/// **尚未在实机确认为 1.26+ 的真实布局**，见 [`content_roots_existing`] 的说明。
/// 保留它是因为一旦游戏换了布局，我们至少能自动认出来而不是读到空目录。
fn data_dir_flat(version_dir: &Path) -> PathBuf {
    version_dir.to_path_buf()
}

/// 解析内容根（不做存在性判断，写入路径用这个）。
pub fn content_roots(version_dir: &Path, meta: &VersionMeta) -> ContentRoots {
    // 安卓实例：游戏按官方布局在 files 根下找 `games/com.mojang`。该 files 根
    // 由 Java 宿主的 `CopperGameLayout.gameFilesDir()` 决定，并作为
    // `EXTRA_FILES_DIR` 传入游戏 Activity，故不能套用桌面的隔离布局。
    if meta.android.is_some() {
        let files_root = version_dir.join(ANDROID_GAME_DIR).join(ANDROID_FILES_DIR);
        return ContentRoots {
            com_mojang: files_root.join(SHARED_GAME_DIR),
            users_root: files_root,
        };
    }
    roots_from(data_dir(version_dir, meta))
}

/// 由数据根拼出内容根。
fn roots_from(data: PathBuf) -> ContentRoots {
    ContentRoots {
        com_mojang: data.join("Users").join("Shared").join(SHARED_GAME_DIR),
        users_root: data.join("Users"),
    }
}

/// 解析内容根（读取路径用这个）：标准布局的 `com.mojang` 不存在、而扁平布局
/// 存在时，改用扁平布局。
///
/// # 为什么需要探测
///
/// 有未解开的疑问：游戏在 1.26 系列起是否还往 `%APPDATA%` 后面拼 sAppName。
/// LeviLauncher 的两侧在这点上是分叉的 —— Go 侧内容根始终带渠道目录
/// （`mcservice/versions.go:220`），C++ 侧却对 `>= 1.26.0.0` 少算一层，而且
/// 只对 `1.26.0.24` 这一个版本实测过（`version_config.cpp:97`）。本仓无法从
/// 代码确定答案，因此**不猜**：按标准布局建目录（写入侧唯一口径），读取侧在
/// 标准布局缺失而扁平布局存在时自动认后者。
///
/// 这样两种布局都不会让用户「读到空目录」，而真实布局会在
/// `copper-hook.log` 与诊断信息里暴露出来，等实机确认后再把规则固化。
pub fn content_roots_existing(version_dir: &Path, meta: &VersionMeta) -> ContentRoots {
    let canonical = content_roots(version_dir, meta);
    if meta.android.is_some() || canonical.com_mojang.is_dir() {
        return canonical;
    }
    let flat = roots_from(data_dir_flat(version_dir));
    if flat.com_mojang.is_dir() {
        log::warn!(
            "[home/isolate] 实例 `{}` 的内容根不在标准布局（`{}` 不存在，`{}` 存在），按扁平布局读取；数据目录规则需按实机确认后更新",
            meta.name,
            canonical.com_mojang.display(),
            flat.com_mojang.display()
        );
        return flat;
    }
    canonical
}

/// 幂等创建实例的数据骨架。
///
/// 装完游戏、用户还没启动过时，隔离目录是不存在的：此时内容列表为空、
/// 内容下载的落点目录也不存在。骨架先行创建让这两条链路从一开始就有确定落点。
pub fn ensure_skeleton(version_dir: &Path, meta: &VersionMeta) -> Result<(), KernelError> {
    let roots = content_roots(version_dir, meta);
    for dir in [
        roots.users_root.clone(),
        roots.com_mojang.join(RESOURCE_PACKS_DIR),
        roots.com_mojang.join(BEHAVIOR_PACKS_DIR),
        roots.com_mojang.join(TEMPLATES_DIR),
    ] {
        std::fs::create_dir_all(&dir)?;
    }
    Ok(())
}

/// 归一化隔离标记：强制为开启。
///
/// 返回是否发生了改写 —— 调用方据此决定要不要落盘（写盘失败只记日志，
/// 隔离本身不依赖这个字段）。
pub fn enforce_isolation(meta: &mut VersionMeta) -> bool {
    if meta.enable_isolation {
        return false;
    }
    meta.enable_isolation = true;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_with(version_type: &str, game_version: &str) -> VersionMeta {
        VersionMeta {
            name: "demo".into(),
            version_type: version_type.into(),
            game_version: game_version.into(),
            enable_isolation: true,
            ..Default::default()
        }
    }

    #[test]
    fn parses_four_part_versions() {
        assert_eq!(parse_version("1.21.130.20"), Some([1, 21, 130, 20]));
        // 段数不足补零
        assert_eq!(parse_version("1.21.50"), Some([1, 21, 50, 0]));
        assert_eq!(parse_version(" 1.26 "), Some([1, 26, 0, 0]));
        // 含非数字段 / 空串一律视为未知，不猜
        assert_eq!(parse_version("1.21.x"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("latest"), None);
    }

    #[test]
    fn editor_mode_respects_channel_thresholds() {
        // 正式 1.21.50 起
        assert!(supports_editor_mode("1.21.50.0", "release"));
        assert!(supports_editor_mode("1.26.45.1", "release"));
        assert!(!supports_editor_mode("1.21.49.9", "release"));
        // 预览 1.19.80.20 起
        assert!(supports_editor_mode("1.19.80.20", "preview"));
        assert!(!supports_editor_mode("1.19.80.19", "preview"));
        // beta 走正式门槛
        assert!(supports_editor_mode("1.21.50.0", "beta"));
        assert!(!supports_editor_mode("1.19.80.20", "beta"));
        // 版本号不可解析时按不支持处理（不给必然失败的开关）
        assert!(!supports_editor_mode("", "release"));
        assert!(!supports_editor_mode("1.21.x", "preview"));
    }

    #[test]
    fn content_roots_follow_channel_directory() {
        let root = Path::new("D:/v");
        let release = content_roots(root, &meta_with("release", "1.21.130.20"));
        assert!(release
            .com_mojang
            .ends_with("Minecraft Bedrock/Users/Shared/games/com.mojang"));
        let preview = content_roots(root, &meta_with("preview", "1.21.130.20"));
        assert!(preview
            .com_mojang
            .ends_with("Minecraft Bedrock Preview/Users/Shared/games/com.mojang"));
        // 两个实例互不影响：数据根挂在各自的版本目录下
        let a = data_dir(Path::new("D:/v/A"), &meta_with("release", "1.21.130.20"));
        let b = data_dir(Path::new("D:/v/B"), &meta_with("release", "1.21.130.20"));
        assert_ne!(a, b);
        assert!(a.starts_with("D:/v/A"));
    }

    #[test]
    fn android_instance_uses_official_layout() {
        let mut meta = meta_with("release", "1.21.130.20");
        meta.android = Some(super::super::meta::AndroidVersionMeta {
            package_name: "com.mojang.minecraftpe".into(),
            version_code: 1,
            version_name: "1.21.130.20".into(),
            abi: "arm64-v8a".into(),
            package_dir: String::new(),
            lib_cache_dir: String::new(),
        });
        let roots = content_roots(Path::new("/data/versions/demo"), &meta);
        assert!(roots
            .com_mojang
            .ends_with("game/files/games/com.mojang"));
    }

    #[test]
    fn ensure_skeleton_is_idempotent_and_creates_load_dirs() {
        let dir = std::env::temp_dir().join(format!("copper_iso_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let meta = meta_with("release", "1.21.130.20");
        ensure_skeleton(&dir, &meta).unwrap();
        let roots = content_roots(&dir, &meta);
        for sub in [RESOURCE_PACKS_DIR, BEHAVIOR_PACKS_DIR, TEMPLATES_DIR] {
            assert!(roots.com_mojang.join(sub).is_dir(), "{sub} 未创建");
        }
        // 二次调用不报错、不改动已有内容
        std::fs::write(roots.com_mojang.join(RESOURCE_PACKS_DIR).join("probe.txt"), b"x").unwrap();
        ensure_skeleton(&dir, &meta).unwrap();
        assert!(roots
            .com_mojang
            .join(RESOURCE_PACKS_DIR)
            .join("probe.txt")
            .is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_roots_fall_back_to_flat_layout() {
        let dir = std::env::temp_dir().join(format!("copper_iso_flat_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let meta = meta_with("release", "1.26.45.1");
        // 只有扁平布局存在时，读取侧改用它
        let flat = roots_from(dir.clone());
        std::fs::create_dir_all(&flat.com_mojang).unwrap();
        let picked = content_roots_existing(&dir, &meta);
        assert_eq!(picked.com_mojang, flat.com_mojang);

        // 两边都在时以标准布局为准
        ensure_skeleton(&dir, &meta).unwrap();
        let canonical = content_roots(&dir, &meta);
        assert_eq!(content_roots_existing(&dir, &meta).com_mojang, canonical.com_mojang);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn isolation_is_forced_on() {
        let mut meta = meta_with("release", "1.21.130.20");
        meta.enable_isolation = false;
        assert!(enforce_isolation(&mut meta));
        assert!(meta.enable_isolation);
        // 已开启时不报告改写
        assert!(!enforce_isolation(&mut meta));
    }
}
