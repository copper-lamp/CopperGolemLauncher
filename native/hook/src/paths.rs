//! 重定向目标计算与路径安全校验（平台中立）。
//!
//! 两类职责，都必须能在 Linux CI 上测：
//!
//! 1. **重定向目标**：由交接变量（或回退的 `version.json`）算出
//!    `%APPDATA%` / `%TEMP%` / `%LOCALAPPDATA%` 各自的替代值。
//! 2. **路径逃逸校验**：清单里的每一条 `path` 都必须落在实例目录内。DLL 以
//!    启动器给的清单去 `LoadLibrary`，一旦清单被写坏（用户手改、别的工具写坏、
//!    路径拼接漏了边界检查），越界就等于让游戏进程加载任意 dll。
//!
//! # 为什么 `SHGetKnownFolderPath` 是唯一可行手段
//!
//! 它不读环境变量，只读用户令牌与注册表（docs §1.3 F1）。因此「启动时设
//! `APPDATA`」「改 cwd」对游戏无效，必须在进程内拦截本模块算出的目标。

use std::path::{Component, Path, PathBuf};

use crate::contract::{
    ENV_DATA_DIR, ENV_PRELOAD, ENV_ROOT, FLAT_LAYOUT_MIN, PRELOADER_MARKER, PREVIEW_DATA_DIR,
    RELEASE_DATA_DIR,
};

/// 目录重定向方案。
///
/// # 三个目标各指向哪里
///
/// | API | 重定向到 | 为什么 |
/// |---|---|---|
/// | `SHGetKnownFolderPath(FOLDERID_RoamingAppData)` | **版本目录** | 游戏自己会在 `%APPDATA%` 后拼渠道目录。指向版本目录后，1.26 之前得到 `<版本目录>/Minecraft Bedrock`，1.26+（少拼一层）自然得到 `<版本目录>`。**布局由游戏自己决定，注入侧不必再实现一遍规则** —— 两侧各算一遍正是 F7 分叉的根因 |
/// | `GetTempPathA/W` | `<数据目录>/temp` | 临时文件属于实例数据，必须跟着实例走 |
/// | `SHGetKnownFolderPath(FOLDERID_LocalAppData)` | 空字符串 | 见 [`local_empty`](Self::local_empty) |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectPlan {
    /// 版本目录（实例根），同时是 `%APPDATA%` 的重定向目标。
    pub root: PathBuf,
    /// 实例数据目录（`<版本目录>` 或 `<版本目录>/<渠道目录>`），由启动器下发。
    pub data: PathBuf,
    /// `GetTempPathA/W` 的返回值（`<数据目录>/temp`）。
    pub temp: PathBuf,
    /// `FOLDERID_LocalAppData` 是否重定向为空字符串。
    ///
    /// 置空而非指向某个目录：游戏的 LocalAppData 用途是崩溃 dump、遥测缓存
    /// 一类「往用户目录里扔东西」的地方，给一个真实可写路径只会让实例目录被
    /// 垃圾文件填满；空路径让游戏无处可写，退化行为与官方一致。做法取自
    /// LeviLauncher `native/levilauncher/src/hook/folder_redirect.cpp:145`。
    pub local_empty: bool,
}

impl RedirectPlan {
    /// 需要在重定向前确保存在的目录（数据目录 + temp）。
    ///
    /// `%TEMP%` 指向一个不存在的目录时，游戏会退化到系统 temp —— 表现为
    /// 「隔离看起来生效，实际临时文件还在系统目录」。
    pub fn directories(&self) -> Vec<&Path> {
        vec![self.data.as_path(), self.temp.as_path()]
    }
}

/// 渠道取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Release,
    Preview,
}

impl Channel {
    /// 由字符串解析；非 `preview` 一律按正式渠道处理（与内核 `isolate::is_preview` 一致）。
    pub fn parse(raw: &str) -> Channel {
        if raw.eq_ignore_ascii_case("preview") {
            Channel::Preview
        } else {
            Channel::Release
        }
    }

    /// 渠道对应的数据目录名。
    pub fn data_dir_name(self) -> &'static str {
        match self {
            Channel::Release => RELEASE_DATA_DIR,
            Channel::Preview => PREVIEW_DATA_DIR,
        }
    }
}

/// 解析四段版本号；段数不足补零，含非数字段返回 `None`（不猜版本）。
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

/// 该游戏版本是否使用扁平数据布局（`>= 1.26.0.0` 少一层渠道目录）。
pub fn uses_flat_layout(game_version: &str) -> bool {
    match parse_version(game_version) {
        Some(version) => version >= FLAT_LAYOUT_MIN,
        // 版本号读不出来时按**旧的**带层级布局：它覆盖了 1.26 之前所有版本，
        // 猜「新布局」只会在老版本上把数据写到启动器读不到的地方。
        None => false,
    }
}

/// 隔离实例的数据根目录（temp 与诊断的落点基准）。
pub fn data_dir(root: &Path, channel: Channel, game_version: &str) -> PathBuf {
    if uses_flat_layout(game_version) {
        root.to_path_buf()
    } else {
        root.join(channel.data_dir_name())
    }
}

/// 由重定向目标构造完整方案（回退路径与单测共用）。
pub fn plan_for(root: &Path, channel: Channel, game_version: &str) -> RedirectPlan {
    let data = data_dir(root, channel, game_version);
    RedirectPlan {
        temp: data.join("temp"),
        root: root.to_path_buf(),
        data,
        local_empty: true,
    }
}

/// 环境变量读取器（便于测试注入）。
pub type EnvReader<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// 由环境变量构造重定向方案；缺 [`ENV_ROOT`] 或 [`ENV_DATA_DIR`] 返回 `None`。
///
/// 两个变量都是硬要求：
///
/// - [`ENV_ROOT`] 是 `%APPDATA%` 的重定向目标，缺了等于没有隔离；
/// - [`ENV_DATA_DIR`] 是 temp 的落点。启动器按版本规则算好下发，注入侧**不再
///   按版本重算**：两侧各跑一遍规则正是 F7 分叉的成因。
///
/// `COPPER_HOOK_CHANNEL` 缺失只影响日志，不阻断重定向。
pub fn plan_from_env(root_hint: Option<&Path>, env: &EnvReader<'_>) -> Option<RedirectPlan> {
    let root = match env(ENV_ROOT) {
        Some(value) => PathBuf::from(value),
        None => root_hint.map(Path::to_path_buf)?,
    };
    let data = PathBuf::from(env(ENV_DATA_DIR)?);
    Some(RedirectPlan {
        temp: data.join("temp"),
        root,
        data,
        local_empty: true,
    })
}

/// 预加载清单路径：优先 [`ENV_PRELOAD`]，否则 `<版本目录>/copper-preload.json`。
pub fn manifest_path(plan: &RedirectPlan, env: &EnvReader<'_>) -> PathBuf {
    match env(ENV_PRELOAD) {
        Some(value) if !value.trim().is_empty() => PathBuf::from(value),
        _ => plan.root.join(crate::contract::MANIFEST_FILE_NAME),
    }
}

/// 清单里是否出现外部预加载器标记文件（存在则本 DLL 让位）。
pub fn has_external_preloader(root: &Path) -> bool {
    root.join(PRELOADER_MARKER).is_file()
}

/// 校验一个清单 `path` 并拼成实例目录内的绝对路径。
///
/// # 拒绝清单
///
/// | 形态 | 例子 | 原因 |
/// |---|---|---|
/// | 空 / 全空白 | `"   "` | 无意义 |
/// | 绝对路径 | `"/c/evil.dll"`、`"C:\\evil.dll"` | 越界 |
/// | `..` 组件 | `"mods/../../evil.dll"` | 越界（按字符串 `starts_with` 判断会被 `a/../..` 绕过） |
/// | 驱动器 / 设备前缀 | `"\\\\?\\C:\\x"`、`"\\\\.\\pipe\\x"` | NT 路径前缀绕过普通前缀检查 |
/// | ADS | `"mods/a.dll:evil"` | NTFS 备用数据流可藏可执行内容 |
/// | 控制字符 / NUL | `"mods/a\u{0}.dll"` | 路径截断 |
/// | 冗余组件 | `"a//b"`、`"./a"`、`"a/./b"` | 归一化歧义，一律要求显式写法 |
/// | 反斜杠 | `"mods\\a.dll"` | 清单是启动器生成的文本产物，混用分隔符说明有人手改过 |
///
/// 通过的形态：`mods/LeviLamina/LeviLamina.dll`、`plugins/a.dll`。
pub fn resolve_entry_path(root: &Path, entry_path: &str) -> Option<PathBuf> {
    let raw = entry_path.trim();
    if raw.is_empty() || raw.contains('\0') || raw.contains('\\') {
        return None;
    }
    if raw.chars().any(|c| c.is_control()) {
        return None;
    }
    // `:` 同时覆盖绝对路径的驱动器前缀（`C:/x`）与 ADS（`a.dll:evil`），
    // 也顺带挡住 NT 前缀里的盘符。
    if raw.starts_with('/') || raw.contains(':') {
        return None;
    }

    let mut resolved = root.to_path_buf();
    for segment in raw.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return None;
        }
        resolved.push(segment);
    }
    // 兜底：以 `Path` 的组件枚举为最终事实源。上面的字符串判断只是快速拒绝，
    // 组件枚举才能保证结果里没有 ParentDir / RootDir / Prefix。
    let relative = resolved.strip_prefix(root).ok()?;
    if relative.components().all(|c| matches!(c, Component::Normal(_))) && !relative.as_os_str().is_empty() {
        Some(resolved)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn layout_switches_at_1_26() {
        assert!(uses_flat_layout("1.26.0.0"));
        assert!(uses_flat_layout("1.26.45.1"));
        assert!(!uses_flat_layout("1.25.99.9"));
        // 版本号不可解析时按旧布局（覆盖全部 1.26 之前版本）
        assert!(!uses_flat_layout(""));
        assert!(!uses_flat_layout("latest"));
    }

    #[test]
    fn data_dir_follows_layout_and_channel() {
        let root = Path::new("/v/demo");
        assert_eq!(
            data_dir(root, Channel::Release, "1.21.130.20"),
            Path::new("/v/demo/Minecraft Bedrock")
        );
        assert_eq!(
            data_dir(root, Channel::Preview, "1.21.130.20"),
            Path::new("/v/demo/Minecraft Bedrock Preview")
        );
        assert_eq!(data_dir(root, Channel::Release, "1.26.0.0"), root);
    }

    #[test]
    fn roaming_is_version_dir_and_temp_lives_in_data_dir() {
        let plan = plan_for(Path::new("/v/demo"), Channel::Release, "1.21.130.20");
        // %APPDATA% 指向版本目录，由游戏自己拼渠道目录
        assert_eq!(plan.root, Path::new("/v/demo"));
        assert_eq!(
            plan.data,
            Path::new("/v/demo/Minecraft Bedrock")
        );
        assert_eq!(plan.temp, Path::new("/v/demo/Minecraft Bedrock/temp"));
        assert_eq!(
            plan.directories(),
            vec![
                Path::new("/v/demo/Minecraft Bedrock"),
                Path::new("/v/demo/Minecraft Bedrock/temp")
            ]
        );
    }

    #[test]
    fn env_plan_requires_both_root_and_data_dir() {
        assert!(plan_from_env(Some(Path::new("/v/demo")), &no_env).is_none());
        let only_root = |key: &str| (key == ENV_ROOT).then(|| "/v/demo".to_string());
        assert!(plan_from_env(None, &only_root).is_none());
        let only_data = |key: &str| (key == ENV_DATA_DIR).then(|| "/v/demo/Minecraft Bedrock".to_string());
        assert!(plan_from_env(None, &only_data).is_none());
    }

    #[test]
    fn env_plan_trusts_handover_values() {
        let env = |key: &str| match key {
            ENV_ROOT => Some("/v/demo".to_string()),
            ENV_DATA_DIR => Some("/v/demo/flat".to_string()),
            _ => Some("preview".to_string()),
            _ => None,
        };
        let plan = plan_from_env(None, &env).unwrap();
        assert_eq!(plan.root, Path::new("/v/demo"));
        // 交接值是权威值：即便 channel=preview 也不重算目录名
        assert_eq!(plan.data, Path::new("/v/demo/flat"));
        assert_eq!(plan.temp, Path::new("/v/demo/flat/temp"));
    }

    #[test]
    fn env_plan_falls_back_to_root_hint() {
        let env = |key: &str| (key == ENV_DATA_DIR).then(|| "/hint/data".to_string());
        let plan = plan_from_env(Some(Path::new("/hint")), &env).unwrap();
        assert_eq!(plan.root, Path::new("/hint"));
    }

    #[test]
    fn manifest_path_prefers_env() {
        let plan = plan_for(Path::new("/v/demo"), Channel::Release, "1.21.130.20");
        assert_eq!(
            manifest_path(&plan, &no_env),
            Path::new("/v/demo/copper-preload.json")
        );
        let env = |key: &str| (key == ENV_PRELOAD).then(|| "/custom/p.json".to_string());
        assert_eq!(manifest_path(&plan, &env), Path::new("/custom/p.json"));
        let blank = |key: &str| (key == ENV_PRELOAD).then(|| "   ".to_string());
        assert_eq!(
            manifest_path(&plan, &blank),
            Path::new("/v/demo/copper-preload.json")
        );
    }

    #[test]
    fn accepts_paths_inside_instance() {
        let root = Path::new("/v/demo");
        assert_eq!(
            resolve_entry_path(root, "mods/LeviLamina/LeviLamina.dll"),
            Some(PathBuf::from("/v/demo/mods/LeviLamina/LeviLamina.dll"))
        );
        assert_eq!(
            resolve_entry_path(root, " LeviLamina.dll "),
            Some(PathBuf::from("/v/demo/LeviLamina.dll"))
        );
    }

    #[test]
    fn rejects_every_escape_shape() {
        let root = Path::new("/v/demo");
        for bad in [
            "",
            "   ",
            "/etc/passwd",
            "C:\\evil.dll",
            "c:/evil.dll",
            "\\\\?\\C:\\evil.dll",
            "\\\\.\\pipe\\evil",
            "mods/../../evil.dll",
            "mods/..",
            "..",
            ".",
            "mods//a.dll",
            "./mods/a.dll",
            "mods/./a.dll",
            "mods/a.dll:evil",
            "mods/a\\b.dll",
            "mods/a.dll\u{0}.x",
            "mods/\u{7}a.dll",
        ] {
            assert!(
                resolve_entry_path(root, bad).is_none(),
                "应拒绝越界路径: {bad:?}"
            );
        }
    }
}