//! 模组管理（文件层）：`<版本目录>/mods/<模组文件夹>/manifest.json`。
//!
//! 协议对齐 LeviLauncher / LiteLoader（`libs/LeviLauncher/internal/mods/mods.go`）：
//! - 清单文件名 `manifest.json`，字段 `name / entry / version / type / author`；
//! - 启用 / 停用 = `manifest.json` ⇄ `manifest.json.close` 改名；
//! - 删除 = 移除整个模组文件夹。
//!
//! **范围**：本模块只做文件层管理，不涉及模组加载器本体（注入 / 预加载）。
//! 模组能否真正生效取决于版本内是否已安装加载器，本页不做保证。
//!
//! **导入安全**：压缩包内路径经规范化（拒绝 `..` / 绝对路径 / 盘符），拒绝符号链接，
//! 包内 `manifest.json` 必须唯一、`entry` 必须命中；解包先落 staging 临时目录，
//! 完成后原子替换目标目录；重名默认拒绝（`KernelError::Conflict`），`overwrite` 显式覆盖。

use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::KernelError;
use crate::state::KernelContext;

use super::meta::resolve_version_dir;

/// 模组目录名。
pub const MODS_DIR: &str = "mods";
/// 清单文件名。
pub const MANIFEST_FILE: &str = "manifest.json";
/// 停用态清单后缀（改名即停用）。
pub const MANIFEST_DISABLED_SUFFIX: &str = ".close";
/// 单 DLL 导入的默认模组类型。
pub const DEFAULT_DLL_TYPE: &str = "preload-native";
/// 单 DLL 导入的默认版本号。
pub const DEFAULT_DLL_VERSION: &str = "0.0.0";
/// 单 DLL 导入的大小上限（防误选超大文件）。
const MAX_DLL_BYTES: u64 = 64 * 1024 * 1024;

/// 模组清单（与 LeviLauncher `types.ModManifestJson` 一致）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModManifest {
    pub name: String,
    #[serde(default)]
    pub entry: String,
    #[serde(default)]
    pub version: String,
    #[serde(default, rename = "type")]
    pub mod_type: String,
    /// 空作者不落盘（与 LeviLauncher 一致）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
}

/// 模组视图（前端列表用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ModView {
    /// 模组文件夹名（后续操作的句柄）。
    pub folder: String,
    pub name: String,
    pub version: String,
    pub mod_type: String,
    pub author: String,
    pub entry: String,
    pub enabled: bool,
    /// 模组文件夹绝对路径。
    pub path: String,
}

/// 模组清单结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ModListResult {
    pub mods: Vec<ModView>,
    /// 缺少清单被跳过的目录数（前端据此提示）。
    pub skipped: usize,
}

// ---------------------------------------------------------------- 上下文绑定层

/// 解析版本的模组目录；`create` 为真时不存在则创建。
///
/// 目录落在 `<versions_root>/<版本名>/mods`，与版本隔离无关，也不依赖 AppX 包。
pub fn mods_dir(kernel: &KernelContext, name: &str, create: bool) -> Result<PathBuf, KernelError> {
    let version_dir = resolve_version_dir(&kernel.versions_root(), name)?;
    if !version_dir.is_dir() {
        return Err(KernelError::InvalidArgument(format!("版本 `{name}` 不存在")));
    }
    let dir = version_dir.join(MODS_DIR);
    if create {
        std::fs::create_dir_all(&dir)?;
    }
    Ok(dir)
}

/// 列出模组。
pub fn list_mods(kernel: &KernelContext, name: &str) -> Result<ModListResult, KernelError> {
    let dir = mods_dir(kernel, name, true)?;
    list_mods_at(&dir)
}

/// 从 ZIP 导入模组（`source` 为本地压缩包路径）。
pub fn import_zip(
    kernel: &KernelContext,
    name: &str,
    source: &str,
    overwrite: bool,
) -> Result<ModView, KernelError> {
    let root = mods_dir(kernel, name, true)?;
    let view = import_zip_at(&root, Path::new(source), overwrite)?;
    publish_changed(kernel, name);
    Ok(view)
}

/// 从单个 DLL 导入模组（自动生成清单）。
pub fn import_dll(
    kernel: &KernelContext,
    name: &str,
    source: &str,
    mod_name: &str,
    mod_type: &str,
    version: &str,
    overwrite: bool,
) -> Result<ModView, KernelError> {
    let root = mods_dir(kernel, name, true)?;
    let view = import_dll_at(
        &root,
        Path::new(source),
        mod_name,
        mod_type,
        version,
        overwrite,
    )?;
    publish_changed(kernel, name);
    Ok(view)
}

/// 启用 / 停用模组。
pub fn set_mod_enabled(
    kernel: &KernelContext,
    name: &str,
    folder: &str,
    enabled: bool,
) -> Result<(), KernelError> {
    let root = mods_dir(kernel, name, false)?;
    set_enabled_at(&root, folder, enabled)?;
    publish_changed(kernel, name);
    Ok(())
}

/// 删除模组（移除整个模组文件夹）。
pub fn remove_mod(kernel: &KernelContext, name: &str, folder: &str) -> Result<(), KernelError> {
    let root = mods_dir(kernel, name, false)?;
    remove_mod_at(&root, folder)?;
    publish_changed(kernel, name);
    Ok(())
}

/// 编辑模组清单（`name / entry / version / type / author`；`type` 为自由文本）。
#[allow(clippy::too_many_arguments)]
pub fn save_mod_manifest(
    kernel: &KernelContext,
    name: &str,
    folder: &str,
    mod_name: &str,
    entry: &str,
    version: &str,
    mod_type: &str,
    author: &str,
) -> Result<ModView, KernelError> {
    let root = mods_dir(kernel, name, false)?;
    let view = save_manifest_at(&root, folder, mod_name, entry, version, mod_type, author)?;
    publish_changed(kernel, name);
    Ok(view)
}

/// 广播模组变更（事件名点号转连字符后前端订阅 `mods-changed`）。
fn publish_changed(kernel: &KernelContext, name: &str) {
    kernel
        .events()
        .publish("mods.changed", serde_json::json!({ "name": name }));
}

// ---------------------------------------------------------------- 纯文件层

/// 列出模组目录下的模组（无清单目录跳过并计数）。
fn list_mods_at(root: &Path) -> Result<ModListResult, KernelError> {
    let mut mods = Vec::new();
    let mut skipped = 0usize;
    if root.is_dir() {
        for entry in std::fs::read_dir(root)?.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let folder = entry.file_name().to_string_lossy().into_owned();
            // 跳过临时目录（导入 staging / 覆盖备份）。
            if folder.starts_with('.') {
                continue;
            }
            match load_manifest(&path) {
                Some((manifest, enabled)) => {
                    mods.push(to_view(&path, folder, manifest, enabled));
                }
                None => skipped += 1,
            }
        }
    }
    mods.sort_by_key(|m| m.folder.to_lowercase());
    Ok(ModListResult { mods, skipped })
}

/// 启用 / 停用：清单文件改名（幂等，状态已一致时直接返回）。
fn set_enabled_at(root: &Path, folder: &str, enabled: bool) -> Result<(), KernelError> {
    let dir = resolve_mod_dir(root, folder)?;
    let enabled_path = dir.join(MANIFEST_FILE);
    let disabled_path = dir.join(disabled_manifest_name());
    if enabled {
        if enabled_path.is_file() {
            return Ok(());
        }
        if !disabled_path.is_file() {
            return Err(KernelError::InvalidArgument("模组清单缺失".into()));
        }
        std::fs::rename(&disabled_path, &enabled_path)?;
    } else {
        if disabled_path.is_file() {
            return Ok(());
        }
        if !enabled_path.is_file() {
            return Err(KernelError::InvalidArgument("模组清单缺失".into()));
        }
        std::fs::rename(&enabled_path, &disabled_path)?;
    }
    Ok(())
}

/// 删除模组文件夹。
fn remove_mod_at(root: &Path, folder: &str) -> Result<(), KernelError> {
    let dir = resolve_mod_dir(root, folder)?;
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

/// 编辑清单：保留清单中的其它字段，仅覆盖这 5 项。
#[allow(clippy::too_many_arguments)]
fn save_manifest_at(
    root: &Path,
    folder: &str,
    mod_name: &str,
    entry: &str,
    version: &str,
    mod_type: &str,
    author: &str,
) -> Result<ModView, KernelError> {
    let dir = resolve_mod_dir(root, folder)?;
    let mod_name = mod_name.trim();
    let entry = entry.trim();
    let version = version.trim();
    let mod_type = mod_type.trim();
    let author = author.trim();
    if mod_name.is_empty() || entry.is_empty() || version.is_empty() || mod_type.is_empty() {
        return Err(KernelError::InvalidArgument(
            "名称 / 入口 / 版本 / 类型均不能为空".into(),
        ));
    }

    // 启用态与停用态的清单文件都要同步（存在哪个写哪个）。
    let mut targets: Vec<(PathBuf, bool)> = Vec::new();
    let enabled_path = dir.join(MANIFEST_FILE);
    let disabled_path = dir.join(disabled_manifest_name());
    if enabled_path.is_file() {
        targets.push((enabled_path, true));
    }
    if disabled_path.is_file() {
        targets.push((disabled_path, false));
    }
    if targets.is_empty() {
        return Err(KernelError::InvalidArgument("模组清单缺失".into()));
    }

    for (path, _) in &targets {
        let mut map = read_manifest_map(path)?;
        map.insert("name".into(), Value::String(mod_name.into()));
        map.insert("entry".into(), Value::String(entry.into()));
        map.insert("version".into(), Value::String(version.into()));
        map.insert("type".into(), Value::String(mod_type.into()));
        if author.is_empty() {
            map.remove("author");
        } else {
            map.insert("author".into(), Value::String(author.into()));
        }
        write_manifest_map(path, &map)?;
    }

    let manifest = ModManifest {
        name: mod_name.into(),
        entry: entry.into(),
        version: version.into(),
        mod_type: mod_type.into(),
        author: author.into(),
    };
    Ok(to_view(
        &dir,
        folder.trim().to_string(),
        manifest,
        targets.iter().any(|(_, enabled)| *enabled),
    ))
}

/// ZIP 导入核心：解包 → 校验 → staging → 原子替换。
fn import_zip_at(root: &Path, source: &Path, overwrite: bool) -> Result<ModView, KernelError> {
    std::fs::create_dir_all(root)?;
    let file = File::open(source)
        .map_err(|e| KernelError::InvalidArgument(format!("无法读取压缩包: {e}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| KernelError::InvalidArgument(format!("压缩包解析失败: {e}")))?;

    let layout = inspect_archive(&mut archive)?;
    let target = root.join(&layout.folder);
    if target.exists() && !overwrite {
        return Err(KernelError::Conflict(format!(
            "模组文件夹 `{}` 已存在",
            layout.folder
        )));
    }

    let staging = staging_dir(root);
    std::fs::create_dir_all(&staging)?;
    if let Err(e) = extract_archive(&mut archive, &layout.manifest_dir, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    if let Err(e) = replace_dir(&staging, &target, overwrite) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    let (manifest, enabled) = load_manifest(&target)
        .ok_or_else(|| KernelError::InvalidArgument("导入后清单缺失".into()))?;
    Ok(to_view(&target, layout.folder, manifest, enabled))
}

/// 单 DLL 导入核心：写入 DLL + 生成清单。
fn import_dll_at(
    root: &Path,
    source: &Path,
    mod_name: &str,
    mod_type: &str,
    version: &str,
    overwrite: bool,
) -> Result<ModView, KernelError> {
    std::fs::create_dir_all(root)?;
    let file_name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| KernelError::InvalidArgument("非法的 DLL 路径".into()))?;
    if !file_name.to_ascii_lowercase().ends_with(".dll") {
        return Err(KernelError::InvalidArgument("请选择 DLL 文件".into()));
    }
    let stem = &file_name[..file_name.len() - ".dll".len()];
    let folder = {
        let given = mod_name.trim();
        if given.is_empty() { stem } else { given }
    }
    .to_string();
    validate_folder_name(&folder)?;
    validate_folder_name(file_name.as_str())?;

    if std::fs::metadata(source)?.len() > MAX_DLL_BYTES {
        return Err(KernelError::InvalidArgument("DLL 文件过大".into()));
    }

    let target = root.join(&folder);
    if target.exists() && !overwrite {
        return Err(KernelError::Conflict(format!("模组文件夹 `{folder}` 已存在")));
    }

    let data = std::fs::read(source)?;
    let manifest = ModManifest {
        name: folder.clone(),
        entry: file_name.clone(),
        version: if version.trim().is_empty() {
            DEFAULT_DLL_VERSION.into()
        } else {
            version.trim().into()
        },
        mod_type: if mod_type.trim().is_empty() {
            DEFAULT_DLL_TYPE.into()
        } else {
            mod_type.trim().into()
        },
        author: String::new(),
    };

    let staging = staging_dir(root);
    std::fs::create_dir_all(&staging)?;
    let extracted = (|| -> Result<(), KernelError> {
        std::fs::write(staging.join(&file_name), &data)?;
        write_manifest(&staging.join(MANIFEST_FILE), &manifest)
    })();
    if let Err(e) = extracted {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    if let Err(e) = replace_dir(&staging, &target, overwrite) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    let (saved, enabled) = load_manifest(&target)
        .ok_or_else(|| KernelError::InvalidArgument("导入后清单缺失".into()))?;
    Ok(to_view(&target, folder, saved, enabled))
}

/// 压缩包布局（目标模组文件夹名 + 清单所在层级）。
struct ArchiveLayout {
    /// 目标模组文件夹名（清单所在目录名，或根布局下的模组名）。
    folder: String,
    /// 清单所在目录（相对包根；根布局为空串）。
    manifest_dir: String,
}

/// 扫描压缩包：拒绝非法路径与符号链接，定位唯一清单并校验 `entry` 命中。
fn inspect_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<ArchiveLayout, KernelError> {
    let mut files: Vec<String> = Vec::new();
    let mut manifest_dir: Option<String> = None;
    let mut manifest_raw: Option<Vec<u8>> = None;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| KernelError::InvalidArgument(format!("压缩包解析失败: {e}")))?;
        if entry.is_symlink() {
            return Err(KernelError::InvalidArgument("压缩包内含符号链接".into()));
        }
        let raw_name = entry.name().to_string();
        let normalized = normalize_archive_path(&raw_name).ok_or_else(|| {
            KernelError::InvalidArgument(format!("压缩包内含非法路径: {raw_name}"))
        })?;
        if normalized.is_empty() || entry.is_dir() {
            continue;
        }
        let base = normalized.rsplit('/').next().unwrap_or(&normalized);
        if base.eq_ignore_ascii_case(MANIFEST_FILE) {
            if manifest_dir.is_some() {
                return Err(KernelError::InvalidArgument(
                    "压缩包内含多个 manifest.json".into(),
                ));
            }
            let mut raw = Vec::new();
            entry
                .read_to_end(&mut raw)
                .map_err(|e| KernelError::InvalidArgument(format!("压缩包读取失败: {e}")))?;
            manifest_dir = Some(
                normalized
                    .rsplit_once('/')
                    .map(|(dir, _)| dir.to_string())
                    .unwrap_or_default(),
            );
            manifest_raw = Some(raw);
        }
        files.push(normalized);
    }

    let manifest_raw = manifest_raw
        .ok_or_else(|| KernelError::InvalidArgument("压缩包内缺少 manifest.json".into()))?;
    let manifest_dir = manifest_dir.unwrap_or_default();
    let manifest = parse_manifest(&manifest_raw)
        .map_err(|_| KernelError::InvalidArgument("manifest.json 格式错误".into()))?;
    for (label, value) in [
        ("name", &manifest.name),
        ("entry", &manifest.entry),
        ("version", &manifest.version),
        ("type", &manifest.mod_type),
    ] {
        if value.trim().is_empty() {
            return Err(KernelError::InvalidArgument(format!(
                "manifest.json 缺少字段 {label}"
            )));
        }
    }

    let folder = if manifest_dir.is_empty() {
        manifest.name.trim().to_string()
    } else {
        manifest_dir
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string()
    };
    validate_folder_name(&folder)?;

    // entry 相对清单所在目录，必须命中包内文件。
    let entry_path = normalize_archive_path(manifest.entry.trim())
        .ok_or_else(|| KernelError::InvalidArgument("manifest.json 的 entry 非法".into()))?;
    if entry_path.is_empty() {
        return Err(KernelError::InvalidArgument("manifest.json 的 entry 非法".into()));
    }
    let prefix = if manifest_dir.is_empty() {
        String::new()
    } else {
        format!("{manifest_dir}/")
    };
    let hit = files.iter().any(|file| {
        let relative = if prefix.is_empty() {
            Some(file.as_str())
        } else {
            file.strip_prefix(&prefix)
        };
        matches!(relative, Some(rel) if rel.eq_ignore_ascii_case(&entry_path))
    });
    if !hit {
        return Err(KernelError::InvalidArgument(format!(
            "压缩包内未找到入口文件 {entry_path}"
        )));
    }

    Ok(ArchiveLayout {
        folder,
        manifest_dir,
    })
}

/// 把清单所在目录下的条目平铺解到 `staging`（清单目录之外的条目忽略）。
fn extract_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    manifest_dir: &str,
    staging: &Path,
) -> Result<(), KernelError> {
    let prefix = if manifest_dir.is_empty() {
        String::new()
    } else {
        format!("{manifest_dir}/")
    };
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| KernelError::InvalidArgument(format!("压缩包解析失败: {e}")))?;
        let raw_name = entry.name().to_string();
        let normalized = normalize_archive_path(&raw_name).ok_or_else(|| {
            KernelError::InvalidArgument(format!("压缩包内含非法路径: {raw_name}"))
        })?;
        let relative = if prefix.is_empty() {
            normalized
        } else {
            match normalized.strip_prefix(&prefix) {
                Some(rest) => rest.to_string(),
                None => continue,
            }
        };
        if relative.is_empty() {
            continue;
        }
        let target = staging.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if entry.is_symlink() {
            return Err(KernelError::InvalidArgument("压缩包内含符号链接".into()));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = File::create(&target)?;
        std::io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}

/// 原子替换目标目录：`staging` → `target`；覆盖时先把旧目录挪到备份，失败则回滚。
fn replace_dir(staging: &Path, target: &Path, overwrite: bool) -> Result<(), KernelError> {
    if !target.exists() {
        std::fs::rename(staging, target)?;
        return Ok(());
    }
    if !overwrite {
        return Err(KernelError::Conflict(format!(
            "模组文件夹 `{}` 已存在",
            target.file_name().unwrap_or_default().to_string_lossy()
        )));
    }
    let backup = staging_dir(target);
    std::fs::rename(target, &backup)?;
    if let Err(e) = std::fs::rename(staging, target) {
        if let Err(rollback) = std::fs::rename(&backup, target) {
            log::error!("[home] 模组导入回滚失败: {rollback}");
        }
        return Err(KernelError::Io(e));
    }
    if let Err(e) = std::fs::remove_dir_all(&backup) {
        log::warn!("[home] 模组导入后清理备份失败: {e}");
    }
    Ok(())
}

/// 模组目录内的临时目录（同盘，保证 rename 原子）。
fn staging_dir(parent: &Path) -> PathBuf {
    parent.join(format!(".mod-tmp-{}", uuid::Uuid::new_v4()))
}

/// 停用态清单文件名。
fn disabled_manifest_name() -> String {
    format!("{MANIFEST_FILE}{MANIFEST_DISABLED_SUFFIX}")
}

/// 读取模组清单：优先启用态，其次停用态；两者都不可读返回 `None`。
fn load_manifest(mod_dir: &Path) -> Option<(ModManifest, bool)> {
    if let Some(manifest) = read_manifest(&mod_dir.join(MANIFEST_FILE)) {
        return Some((manifest, true));
    }
    read_manifest(&mod_dir.join(disabled_manifest_name())).map(|manifest| (manifest, false))
}

/// 解析模组目录：必须是模组目录下的直接子目录（防路径逃逸）。
fn resolve_mod_dir(root: &Path, folder: &str) -> Result<PathBuf, KernelError> {
    let folder = folder.trim();
    if folder.is_empty() || folder == "." || folder == ".." {
        return Err(KernelError::InvalidArgument("非法的模组文件夹名".into()));
    }
    validate_folder_name(folder)?;
    let dir = root.join(folder);
    if !dir.is_dir() {
        return Err(KernelError::InvalidArgument(format!(
            "模组 `{folder}` 不存在"
        )));
    }
    Ok(dir)
}

/// 校验模组文件夹名 / 文件名：单段、无分隔符与盘符、符合 Windows 命名规则。
fn validate_folder_name(name: &str) -> Result<(), KernelError> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return Err(KernelError::InvalidArgument("非法的模组文件夹名".into()));
    }
    if name.len() > 96 {
        return Err(KernelError::InvalidArgument("模组文件夹名过长".into()));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(KernelError::InvalidArgument("模组文件夹名不能以点或空格结尾".into()));
    }
    if name.chars().any(|c| {
        matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*') || c.is_control()
    }) {
        return Err(KernelError::InvalidArgument("模组文件夹名含非法字符".into()));
    }
    Ok(())
}

/// 规范化压缩包内路径。返回 `None` 表示非法（绝对路径 / 盘符 / `..` 逃逸），
/// 返回空串表示该条目规范化后无有效路径（如包根目录），调用方应跳过。
fn normalize_archive_path(raw: &str) -> Option<String> {
    let unified = raw.trim().replace('\\', "/");
    let stripped = unified.trim_start_matches("./");
    if stripped.starts_with('/') || stripped.contains(':') {
        return None;
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in stripped.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return None,
            other => segments.push(other),
        }
    }
    Some(segments.join("/"))
}

/// 读取清单为结构体（键名大小写不敏感，与 LeviLauncher `JsonCompatBytes` 同义）。
fn read_manifest(path: &Path) -> Option<ModManifest> {
    let raw = std::fs::read(path).ok()?;
    parse_manifest(&raw).ok()
}

/// 解析清单字节。
fn parse_manifest(raw: &[u8]) -> Result<ModManifest, KernelError> {
    let mut value: Value = serde_json::from_slice(raw)?;
    if let Value::Object(map) = &mut value {
        let lowered = map
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .collect();
        *map = lowered;
    }
    Ok(serde_json::from_value(value)?)
}

/// 读取清单原始键值（保留未知字段，键名统一小写）。
fn read_manifest_map(path: &Path) -> Result<serde_json::Map<String, Value>, KernelError> {
    let raw = std::fs::read(path)?;
    let mut value: Value = if raw.iter().all(u8::is_ascii_whitespace) {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_slice(&raw)?
    };
    let map = value
        .as_object_mut()
        .ok_or_else(|| KernelError::InvalidArgument("模组清单格式错误".into()))?;
    Ok(std::mem::take(map)
        .into_iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v))
        .collect())
}

/// 写清单键值。
fn write_manifest_map(path: &Path, map: &serde_json::Map<String, Value>) -> Result<(), KernelError> {
    let raw = serde_json::to_vec_pretty(&Value::Object(map.clone()))?;
    write_bytes_atomic(path, &raw)
}

/// 写清单结构体。
fn write_manifest(path: &Path, manifest: &ModManifest) -> Result<(), KernelError> {
    write_bytes_atomic(path, &serde_json::to_vec_pretty(manifest)?)
}

/// 原子写文件（临时文件 + 改名），避免留下半截清单。
fn write_bytes_atomic(path: &Path, raw: &[u8]) -> Result<(), KernelError> {
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    {
        let mut file = File::create(&tmp)?;
        file.write_all(raw)?;
        file.sync_all()?;
    }
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(KernelError::Io(e));
    }
    Ok(())
}

/// 构建前端视图。
fn to_view(dir: &Path, folder: String, manifest: ModManifest, enabled: bool) -> ModView {
    ModView {
        folder,
        name: manifest.name,
        version: manifest.version,
        mod_type: manifest.mod_type,
        author: manifest.author,
        entry: manifest.entry,
        enabled,
        path: dir.to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("copper_home_mods_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 构造一个压缩包（返回内存字节），条目为 `(路径, 内容)`。
    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn write_zip(root: &Path, file_name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = root.join(file_name);
        std::fs::write(&path, build_zip(entries)).unwrap();
        path
    }

    fn manifest_json(name: &str, entry: &str) -> Vec<u8> {
        format!(
            r#"{{"name":"{name}","entry":"{entry}","version":"1.0.0","type":"preload-native","author":"tester"}}"#
        )
        .into_bytes()
    }

    #[test]
    fn manifest_roundtrip_and_toggle() {
        let root = temp_root("toggle");
        let mod_dir = root.join("mod-a");
        std::fs::create_dir_all(&mod_dir).unwrap();
        write_manifest(&mod_dir.join(MANIFEST_FILE), &ModManifest {
            name: "ModA".into(),
            entry: "mod-a.dll".into(),
            version: "1.2.3".into(),
            mod_type: "preload-native".into(),
            author: "tester".into(),
        })
        .unwrap();

        let listed = list_mods_at(&root).unwrap();
        assert_eq!(listed.mods.len(), 1);
        assert_eq!(listed.skipped, 0);
        assert!(listed.mods[0].enabled);
        assert_eq!(listed.mods[0].name, "ModA");
        assert_eq!(listed.mods[0].entry, "mod-a.dll");

        // 停用：清单改名，列表显示停用态。
        set_enabled_at(&root, "mod-a", false).unwrap();
        assert!(mod_dir.join("manifest.json.close").is_file());
        assert!(!mod_dir.join(MANIFEST_FILE).exists());
        let listed = list_mods_at(&root).unwrap();
        assert!(!listed.mods[0].enabled);
        // 幂等
        set_enabled_at(&root, "mod-a", false).unwrap();

        // 启用：改回。
        set_enabled_at(&root, "mod-a", true).unwrap();
        assert!(mod_dir.join(MANIFEST_FILE).is_file());
        assert!(list_mods_at(&root).unwrap().mods[0].enabled);

        // 删除。
        remove_mod_at(&root, "mod-a").unwrap();
        assert!(!mod_dir.exists());
    }

    #[test]
    fn list_skips_dir_without_manifest() {
        let root = temp_root("skip");
        std::fs::create_dir_all(root.join("no-manifest")).unwrap();
        std::fs::create_dir_all(root.join(".mod-tmp-x")).unwrap();
        let listed = list_mods_at(&root).unwrap();
        assert!(listed.mods.is_empty());
        assert_eq!(listed.skipped, 1);
    }

    #[test]
    fn archive_path_normalization() {
        assert_eq!(normalize_archive_path("a/b/c.dll").as_deref(), Some("a/b/c.dll"));
        assert_eq!(normalize_archive_path("./a\\b.dll").as_deref(), Some("a/b.dll"));
        assert_eq!(normalize_archive_path("a/./b").as_deref(), Some("a/b"));
        assert_eq!(normalize_archive_path("/etc/passwd"), None);
        assert_eq!(normalize_archive_path("C:\\evil.dll"), None);
        assert_eq!(normalize_archive_path("../evil.dll"), None);
        assert_eq!(normalize_archive_path("a/../../evil.dll"), None);
        assert_eq!(normalize_archive_path("").as_deref(), Some(""));
    }

    #[test]
    fn folder_name_validation() {
        assert!(validate_folder_name("mod-a").is_ok());
        assert!(validate_folder_name("").is_err());
        assert!(validate_folder_name("..").is_err());
        assert!(validate_folder_name("a/b").is_err());
        assert!(validate_folder_name("a:b").is_err());
        assert!(validate_folder_name("ends.").is_err());
    }

    #[test]
    fn zip_import_nested_layout() {
        let root = temp_root("zip_nested");
        let source = write_zip(
            &root,
            "mod.zip",
            &[
                ("MyMod/manifest.json", &manifest_json("MyMod", "MyMod.dll")),
                ("MyMod/MyMod.dll", b"binary"),
                ("MyMod/sub/data.txt", b"data"),
                ("MyMod/assets/readme.md", b"# readme"),
            ],
        );
        let view = import_zip_at(&root, &source, false).unwrap();
        assert_eq!(view.folder, "MyMod");
        assert_eq!(view.name, "MyMod");
        assert!(view.enabled);
        assert!(root.join("MyMod").join("MyMod.dll").is_file());
        assert!(root.join("MyMod").join("sub").join("data.txt").is_file());
        // staging 清理干净
        let leftovers = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".mod-tmp-"))
            .count();
        assert_eq!(leftovers, 0);

        // 重名：默认拒绝（Conflict），显式覆盖成功后内容被替换。
        let err = import_zip_at(&root, &source, false).unwrap_err();
        assert!(matches!(err, KernelError::Conflict(_)));
        std::fs::write(root.join("MyMod").join("stale.txt"), b"stale").unwrap();
        import_zip_at(&root, &source, true).unwrap();
        assert!(!root.join("MyMod").join("stale.txt").exists());
    }

    #[test]
    fn zip_import_root_layout_uses_mod_name() {
        let root = temp_root("zip_root");
        let source = write_zip(
            &root,
            "root.zip",
            &[
                ("manifest.json", &manifest_json("RootMod", "root.dll")),
                ("root.dll", b"binary"),
            ],
        );
        let view = import_zip_at(&root, &source, false).unwrap();
        assert_eq!(view.folder, "RootMod");
        assert!(root.join("RootMod").join("root.dll").is_file());
    }

    #[test]
    fn zip_import_rejects_unsafe_archives() {
        let root = temp_root("zip_bad");

        // entry 未命中
        let source = write_zip(
            &root,
            "missing-entry.zip",
            &[
                ("m/manifest.json", &manifest_json("m", "absent.dll")),
                ("m/present.dll", b"binary"),
            ],
        );
        assert!(import_zip_at(&root, &source, false).is_err());

        // 多个 manifest
        let source = write_zip(
            &root,
            "two-manifest.zip",
            &[
                ("m/manifest.json", &manifest_json("m", "a.dll")),
                ("m/a.dll", b"binary"),
                ("m/inner/manifest.json", &manifest_json("inner", "b.dll")),
                ("m/inner/b.dll", b"binary"),
            ],
        );
        assert!(import_zip_at(&root, &source, false).is_err());

        // 路径逃逸
        let source = write_zip(
            &root,
            "escape.zip",
            &[
                ("m/manifest.json", &manifest_json("m", "a.dll")),
                ("m/a.dll", b"binary"),
                ("../evil.dll", b"evil"),
            ],
        );
        let err = import_zip_at(&root, &source, false).unwrap_err();
        assert!(matches!(err, KernelError::InvalidArgument(_)));

        // 缺少清单
        let source = write_zip(&root, "no-manifest.zip", &[("m/a.dll", b"binary")]);
        assert!(import_zip_at(&root, &source, false).is_err());

        // 清单字段缺失
        let source = write_zip(
            &root,
            "empty-name.zip",
            &[
                ("m/manifest.json", br#"{"name":"","entry":"a.dll","version":"1","type":"t"}"#),
                ("m/a.dll", b"binary"),
            ],
        );
        assert!(import_zip_at(&root, &source, false).is_err());
    }

    #[test]
    fn dll_import_generates_manifest() {
        let root = temp_root("dll");
        let source = root.join("MyNative.dll");
        std::fs::write(&source, b"MZ-binary").unwrap();

        // 默认：文件夹名 = 文件名词干，类型 = preload-native，版本 0.0.0。
        let view = import_dll_at(&root, &source, "", "", "", false).unwrap();
        assert_eq!(view.folder, "MyNative");
        assert_eq!(view.mod_type, DEFAULT_DLL_TYPE);
        assert_eq!(view.version, DEFAULT_DLL_VERSION);
        assert_eq!(view.entry, "MyNative.dll");
        let manifest = read_manifest(&root.join("MyNative").join(MANIFEST_FILE)).unwrap();
        assert_eq!(manifest.name, "MyNative");
        assert!(manifest.author.is_empty());

        // 重名默认拒绝，显式覆盖后清单被重写。
        let err = import_dll_at(&root, &source, "", "", "", false).unwrap_err();
        assert!(matches!(err, KernelError::Conflict(_)));
        let view = import_dll_at(&root, &source, "MyNative", "custom-type", "2.0.0", true).unwrap();
        assert_eq!(view.folder, "MyNative");
        assert_eq!(view.mod_type, "custom-type");
        assert_eq!(view.version, "2.0.0");

        // 非 DLL 拒绝。
        let other = root.join("note.txt");
        std::fs::write(&other, b"text").unwrap();
        assert!(import_dll_at(&root, &other, "", "", "", false).is_err());
    }

    #[test]
    fn save_manifest_edits_fields_and_keeps_extras() {
        let root = temp_root("edit");
        let mod_dir = root.join("mod-a");
        std::fs::create_dir_all(&mod_dir).unwrap();
        std::fs::write(
            mod_dir.join(MANIFEST_FILE),
            br#"{"Name":"ModA","entry":"a.dll","version":"1.0.0","type":"preload-native","author":"old","custom":42}"#,
        )
        .unwrap();

        let view = save_manifest_at(&root, "mod-a", "Renamed", "b.dll", "2.0.0", "script", "new").unwrap();
        assert_eq!(view.name, "Renamed");
        assert_eq!(view.entry, "b.dll");
        assert_eq!(view.mod_type, "script");
        assert_eq!(view.author, "new");
        let raw = std::fs::read_to_string(mod_dir.join(MANIFEST_FILE)).unwrap();
        assert!(raw.contains("\"custom\": 42"), "未知字段应保留: {raw}");
        assert!(!raw.contains("\"Name\""), "键名应统一为小写: {raw}");

        // 作者留空 → 字段被移除。
        save_manifest_at(&root, "mod-a", "Renamed", "b.dll", "2.0.0", "script", "").unwrap();
        let raw = std::fs::read_to_string(mod_dir.join(MANIFEST_FILE)).unwrap();
        assert!(!raw.contains("author"), "空作者不应落盘: {raw}");

        // 停用态也要同步写入。
        set_enabled_at(&root, "mod-a", false).unwrap();
        save_manifest_at(&root, "mod-a", "Disabled", "b.dll", "2.0.0", "script", "x").unwrap();
        let raw =
            std::fs::read_to_string(mod_dir.join("manifest.json.close")).unwrap();
        assert!(raw.contains("\"name\": \"Disabled\""));

        // 必填校验。
        assert!(save_manifest_at(&root, "mod-a", "", "b.dll", "1", "t", "").is_err());
        // 非法目录名。
        assert!(remove_mod_at(&root, "../evil").is_err());
    }
}