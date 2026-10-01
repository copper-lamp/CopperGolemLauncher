//! 版本元数据：`version.json` 读写、版本目录扫描、图标处理。
//!
//! 文件格式与 LeviLauncher 兼容（camelCase 字段），保证游戏下载模块
//! 安装的版本可被开始页直接识别；图标沿用 `LargeLogo.png` 约定。

use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::KernelError;

/// 版本元数据文件名（与 LeviLauncher 一致）。
pub const META_FILE: &str = "version.json";
/// 版本图标文件名（与 LeviLauncher 一致，256×256 PNG）。
pub const LOGO_FILE: &str = "LargeLogo.png";

/// 实例名（= 版本目录名）允许出现的字符集。
///
/// 这套规则不是审美选择，而是**跨语言可移植性约束**：安卓宿主里存在
/// 同一条规则的 Kotlin 实现（`CopperGameLayout.instanceId`），用来按名字
/// 定位 `data/versions/<name>`。两侧只要不完全一致，带空格或非 ASCII 的
/// 实例名就会出现「列表里看得到、点启动却说实例不存在」。见
/// [安卓端能力差距与优先级](../../../../docs/安卓端能力差距与优先级.md) P0-2。
pub const INSTANCE_NAME_ALPHABET: &str = "A-Za-z0-9._-";

/// 实例名长度上限（按**字符**计）。
pub const INSTANCE_NAME_MAX_CHARS: usize = 96;

/// 把任意字符串规整为合法实例名（目录名）。
///
/// 与 Kotlin 侧 `CopperGameLayout.sanitizeInstance` 逐条对应，任一侧改动
/// 都必须同时改另一侧：
/// 1. 去掉首尾空白；
/// 2. 字符集外的字符替换为 `_`；
/// 3. 连续 `_` 折叠为一个；
/// 4. 去掉首尾的 `.`、`_`、`-`（Windows 不允许目录名以点结尾，`.` / `..` 更是路径逃逸）；
/// 5. 按字符截断到 [`INSTANCE_NAME_MAX_CHARS`]；
/// 6. 结果为空则回落 `instance`。
pub fn sanitize_instance_name(name: &str) -> String {
    let mut cleaned = String::with_capacity(name.len());
    let mut last_underscore = false;
    for ch in name.trim().chars() {
        let mapped = if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            ch
        } else {
            '_'
        };
        if mapped == '_' {
            if last_underscore {
                continue;
            }
            last_underscore = true;
        } else {
            last_underscore = false;
        }
        cleaned.push(mapped);
    }
    let trimmed = cleaned.trim_matches(|c| c == '.' || c == '_' || c == '-');
    if trimmed.is_empty() {
        return "instance".to_string();
    }
    let take = trimmed.chars().count().min(INSTANCE_NAME_MAX_CHARS);
    trimmed.chars().take(take).collect()
}

/// 判断名字是否已是规整形态（规整后与自身一致）。
///
/// 导入与重命名用它做**自检**：只要提交的名字通过了，安卓宿主就一定
/// 能按同一个名字定位到目录，不需要宿主再猜一次。
pub fn is_instance_name_canonical(name: &str) -> bool {
    !name.trim().is_empty() && sanitize_instance_name(name) == name
}

/// Android APK 运行时元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidVersionMeta {
    pub package_name: String,
    /// 来自二进制 AndroidManifest 的 `versionCode`。
    pub version_code: u64,
    /// 来自二进制 AndroidManifest 的 `versionName`（例如 `1.21.130.20`）。
    ///
    /// 该值决定安卓运行时加载哪一套原生库，必须持久化：加载顺序
    /// 在 `CopperGameRuntimePreparer` 中按版本号分支。
    pub version_name: String,
    pub abi: String,
    pub package_dir: String,
    #[serde(default)]
    pub lib_cache_dir: String,
}

/// 版本元数据。
///
/// 字段与 LeviLauncher `versions.VersionMeta` 对应，`created_at` 以
/// RFC3339 字符串存储（兼容 Go `time.Time` 序列化）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionMeta {
    pub name: String,
    pub game_version: String,
    /// Android runtime metadata. Omitted on desktop versions for compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android: Option<AndroidVersionMeta>,
    #[serde(rename = "type")]
    pub version_type: String,
    #[serde(default)]
    pub enable_isolation: bool,
    #[serde(default)]
    pub enable_console: bool,
    #[serde(default)]
    pub enable_editor_mode: bool,
    #[serde(default)]
    pub enable_render_dragon: bool,
    #[serde(default)]
    pub enable_ctrl_r_reload_resources: bool,
    #[serde(default)]
    pub launch_args: String,
    #[serde(default)]
    pub env_vars: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub registered: bool,
}

impl VersionMeta {
    /// 读取版本目录下的元数据。目录不存在或解析失败返回 `None`（非致命）。
    pub fn read(dir: &Path) -> Option<Self> {
        let path = dir.join(META_FILE);
        let raw = std::fs::read(path).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    /// 写入版本元数据（必要时创建目录）。
    pub fn write(dir: &Path, meta: &Self) -> Result<(), KernelError> {
        std::fs::create_dir_all(dir)?;
        let raw = serde_json::to_vec_pretty(meta)?;
        std::fs::write(dir.join(META_FILE), raw)?;
        Ok(())
    }

    /// 目录是否存在（作为已安装版本判定）。
    pub fn dir_exists(dir: &Path) -> bool {
        dir.is_dir()
    }
}

/// 扫描版本根目录，返回全部带元数据的版本（无 `version.json` 的目录跳过）。
pub fn scan_versions(versions_root: &Path) -> Vec<VersionMeta> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(versions_root) else {
        return out;
    };
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        if let Some(meta) = VersionMeta::read(&entry.path()) {
            out.push(meta);
        }
    }
    out
}

/// 解析版本目录：防路径逃逸。`name` 必须是版本根下的直接子目录名。
pub fn resolve_version_dir(versions_root: &Path, name: &str) -> Result<PathBuf, KernelError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(KernelError::InvalidArgument("版本名不能为空".into()));
    }
    if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err(KernelError::InvalidArgument("非法的版本名".into()));
    }
    let dir = versions_root.join(name);
    // 双重保险：确保解析结果仍在版本根内。
    let root_canon = versions_root.canonicalize().unwrap_or_else(|_| versions_root.to_path_buf());
    let dir_canon = dir.canonicalize().unwrap_or(dir.clone());
    if !dir_canon.starts_with(&root_canon) {
        return Err(KernelError::InvalidArgument("非法的版本路径".into()));
    }
    Ok(dir)
}

/// 校验版本名合法性（Windows 文件命名规则 + 保留名 + 冲突检查）。
pub fn validate_version_name(versions_root: &Path, name: &str) -> Result<(), KernelError> {
    let n = name.trim();
    if n.is_empty() {
        return Err(KernelError::InvalidArgument("版本名不能为空".into()));
    }
    if n.len() > 64 {
        return Err(KernelError::InvalidArgument("版本名过长（最多 64 字符）".into()));
    }
    if n.ends_with('.') || n.ends_with(' ') {
        return Err(KernelError::InvalidArgument("版本名不能以点或空格结尾".into()));
    }
    if n.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')) {
        return Err(KernelError::InvalidArgument("版本名含非法字符".into()));
    }
    if n.chars().any(|c| c.is_control()) {
        return Err(KernelError::InvalidArgument("版本名含控制字符".into()));
    }
    let lower = n.to_lowercase();
    const RESERVED: &[&str] = &[
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
        "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    if RESERVED.contains(&lower.as_str()) {
        return Err(KernelError::InvalidArgument("版本名与系统保留名冲突".into()));
    }
    if versions_root.join(n).exists() {
        return Err(KernelError::InvalidArgument("同名版本已存在".into()));
    }
    Ok(())
}

/// 读取版本图标为 data URL（`data:image/png;base64,...`），无图标返回 `None`。
pub fn logo_data_url(dir: &Path) -> Option<String> {
    let path = dir.join(LOGO_FILE);
    let raw = std::fs::read(path).ok()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
    Some(format!("data:image/png;base64,{b64}"))
}

/// 解析并校验 PNG 尺寸（读 IHDR 头），避免引入完整图像解码依赖。
fn png_dimensions(raw: &[u8]) -> Option<(u32, u32)> {
    // PNG 签名 + IHDR：宽高位于偏移 16 / 20。
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if raw.len() < 24 || raw[..8] != SIGNATURE || &raw[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes([raw[16], raw[17], raw[18], raw[19]]);
    let height = u32::from_be_bytes([raw[20], raw[21], raw[22], raw[23]]);
    Some((width, height))
}

/// 保存版本图标：接受 `data:image/png;base64,...`，校验方形 PNG 后写入 `LargeLogo.png`。
///
/// 裁剪到 256×256 由前端 canvas 完成，后端只做形状与大小校验（≤4096，防超大文件）。
pub fn save_logo(dir: &Path, data_url: &str) -> Result<(), KernelError> {
    let lower = data_url.to_ascii_lowercase();
    let (prefix, b64) = data_url
        .split_once(',')
        .ok_or_else(|| KernelError::InvalidArgument("图标数据格式错误".into()))?;
    if !prefix.starts_with("data:image/") || !lower.contains(";base64") {
        return Err(KernelError::InvalidArgument("图标数据格式错误".into()));
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| KernelError::InvalidArgument("图标 base64 解码失败".into()))?;
    if raw.len() > 8 * 1024 * 1024 {
        return Err(KernelError::InvalidArgument("图标文件过大".into()));
    }
    let (w, h) = png_dimensions(&raw)
        .ok_or_else(|| KernelError::InvalidArgument("图标必须是 PNG 图片".into()))?;
    if w == 0 || h == 0 || w != h {
        return Err(KernelError::InvalidArgument("图标必须是正方形图片".into()));
    }
    if w > 4096 {
        return Err(KernelError::InvalidArgument("图标尺寸过大".into()));
    }
    std::fs::write(dir.join(LOGO_FILE), raw)?;
    Ok(())
}

/// 移除版本图标。
pub fn remove_logo(dir: &Path) -> Result<(), KernelError> {
    let path = dir.join(LOGO_FILE);
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// 生成 RFC3339 时间字符串（UTC，版本创建时间，与 LeviLauncher `time.Time` 兼容）。
///
/// 手写 UTC 日期换算（civil-from-days 算法）避免引入 chrono 依赖。
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs.div_euclid(86_400) as i64;
    let rem = secs.rem_euclid(86_400);
    let (hour, min, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil_from_days: 天数 → 公历日期。
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u64;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u64;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, min, sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("copper_home_meta_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn instance_name_sanitizer_is_canonical() {
        // 空格、中文、路径分隔符、通配符全部收敛为下划线。
        assert_eq!(sanitize_instance_name("Minecraft 1.21.130"), "Minecraft_1.21.130");
        assert_eq!(sanitize_instance_name("我的世界"), "instance");
        assert_eq!(sanitize_instance_name("a/b\\c"), "a_b_c");
        assert_eq!(sanitize_instance_name("a??b"), "a_b");
        // 连续下划线折叠，首尾不留下划线 / 点 / 横线。
        assert_eq!(sanitize_instance_name("a   b"), "a_b");
        assert_eq!(sanitize_instance_name("__a__"), "a");
        assert_eq!(sanitize_instance_name("..a.."), "a");
        assert_eq!(sanitize_instance_name("-1.21.0-"), "1.21.0");
        // 空 / 纯非法字符回落，绝不允许产出空目录名或 `.` / `..`。
        assert_eq!(sanitize_instance_name(""), "instance");
        assert_eq!(sanitize_instance_name("   "), "instance");
        assert_eq!(sanitize_instance_name("..."), "instance");
        assert_eq!(sanitize_instance_name("中"), "instance");
        // 长度按字符截断。
        let long = "a".repeat(200);
        assert_eq!(sanitize_instance_name(&long).chars().count(), INSTANCE_NAME_MAX_CHARS);
    }

    #[test]
    fn canonical_check_rejects_what_the_android_host_cannot_resolve() {
        assert!(is_instance_name_canonical("MC_1.21.130-preview"));
        assert!(is_instance_name_canonical("instance"));
        assert!(!is_instance_name_canonical("Minecraft 1.21"));
        assert!(!is_instance_name_canonical("我的世界"));
        assert!(!is_instance_name_canonical(""));
        assert!(!is_instance_name_canonical(".."));
        // 规整结果必须能通过自检：导入写目录名与宿主定位目录名因此必然一致。
        for raw in ["Minecraft 1.21", "我的世界", "a//b", "___", "x y z"] {
            assert!(is_instance_name_canonical(&sanitize_instance_name(raw)), "{raw}");
        }
    }

    #[test]
    fn meta_roundtrip() {
        let dir = temp_dir("roundtrip");
        let meta = VersionMeta {
            name: "1.21.0".into(),
            game_version: "1.21.0.28".into(),
            version_type: "release".into(),
            enable_render_dragon: true,
            ..Default::default()
        };
        VersionMeta::write(&dir, &meta).unwrap();
        let read = VersionMeta::read(&dir).expect("meta readable");
        assert_eq!(read.name, "1.21.0");
        assert_eq!(read.game_version, "1.21.0.28");
        assert!(read.enable_render_dragon);
        assert!(!read.enable_editor_mode);
    }

    #[test]
    fn scan_skips_dir_without_meta() {
        let root = temp_dir("scan");
        let a = root.join("with-meta");
        let b = root.join("no-meta");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        VersionMeta::write(&a, &VersionMeta { name: "A".into(), ..Default::default() }).unwrap();
        let metas = scan_versions(&root);
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].name, "A");
    }

    #[test]
    fn resolve_blocks_path_escape() {
        let root = temp_dir("escape");
        assert!(resolve_version_dir(&root, "..").is_err());
        assert!(resolve_version_dir(&root, "a/b").is_err());
        assert!(resolve_version_dir(&root, "").is_err());
        fs::create_dir_all(root.join("ok")).unwrap();
        assert!(resolve_version_dir(&root, "ok").is_ok());
    }

    #[test]
    fn validate_name_rules() {
        let root = temp_dir("name");
        assert!(validate_version_name(&root, "good-name").is_ok());
        assert!(validate_version_name(&root, "a<b").is_err());
        assert!(validate_version_name(&root, "con").is_err());
        assert!(validate_version_name(&root, "ends.").is_err());
    }

    #[test]
    fn logo_validation() {
        let dir = temp_dir("logo");
        // 非法：非 PNG
        assert!(save_logo(&dir, "data:image/png;base64,AAAA").is_err());
        // 非方形 PNG：1×1 像素 PNG 手工构造
        let png = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, // 签名
            0, 0, 0, 13, b'I', b'H', b'D', b'R', // IHDR 长度 + 类型
            0, 0, 0, 2, 0, 0, 0, 1, // 宽2 高1（非方形）
            8, 6, 0, 0, 0, 0, 0, 0, 0, // 位深/颜色等
        ];
        let b64 = base64::engine::general_purpose::STANDARD.encode(png);
        assert!(save_logo(&dir, &format!("data:image/png;base64,{b64}")).is_err());
        // 方形：1×1
        let square = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A,
            0, 0, 0, 13, b'I', b'H', b'D', b'R',
            0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 0, 0, 0, 0,
        ];
        let b64 = base64::engine::general_purpose::STANDARD.encode(square);
        assert!(save_logo(&dir, &format!("data:image/png;base64,{b64}")).is_ok());
        assert!(dir.join(LOGO_FILE).exists());
        assert!(logo_data_url(&dir).unwrap().starts_with("data:image/png;base64,"));
        remove_logo(&dir).unwrap();
        assert!(!dir.join(LOGO_FILE).exists());
    }
}
