//! `.cglm` 模块包（zip 容器）的解析与安全解包。
//!
//! 包布局（冻结契约，见 `CopperModles/scripts/package-module.mjs` 头注释）：
//!
//! ```text
//! module.json          清单（容器根，唯一数据源）
//! manifest.sha256      包内逐文件 sha256（`<hex>  <相对路径>`，不含自身）
//! <icon>               图标（从清单 icon 复制到包根）
//! backend/<产物>       后端动态库
//! frontend/**          前端产物
//! LICENSE              许可
//! ```
//!
//! 安全边界（与 `modules::normalize_path` / `home::mods` 的解包同类）：
//! - 拒绝符号链接；
//! - 拒绝绝对路径、盘符、`..` 穿越、Windows 保留名；
//! - 解包到**临时目录**后由安装链路原子 `rename` 到目标，避免半成品目录被注册表当成可用模块。
//!
//! 本文件只负责「解包 + 清单解析 + 包内校验」，不负责下载与落位（见 [`crate::registry::install`]）。

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::KernelError;
use crate::registry::manifest::ModuleManifest;

/// 包内清单文件名。
pub const MANIFEST_FILE: &str = "module.json";
/// 包内逐文件校验清单文件名。
pub const CHECKSUM_FILE: &str = "manifest.sha256";

/// 解包结果：目标目录内的清单。
pub struct ExtractedPackage {
    /// 容器根的清单（已解析，未做语义校验）。
    pub manifest: ModuleManifest,
    /// 解包落点目录。
    pub dir: PathBuf,
}

/// 把 `.cglm` 解包到 `dest`（调用方应传一个**空临时目录**）。
///
/// 成功返回容器根清单。任一条目非法即整包拒绝，不留半成品。
pub fn extract_package(archive_path: &Path, dest: &Path) -> Result<ExtractedPackage, KernelError> {
    let file = std::fs::File::open(archive_path).map_err(|e| {
        KernelError::Module(format!("打开模块包失败 {}: {e}", archive_path.display()))
    })?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| KernelError::Module(format!("模块包不是合法的 zip 容器: {e}")))?;

    std::fs::create_dir_all(dest)
        .map_err(|e| KernelError::Module(format!("创建解包临时目录失败: {e}")))?;

    // 逐条目解包并做路径安全校验。
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| KernelError::Module(format!("读取模块包条目失败: {e}")))?;

        if entry.is_symlink() {
            return Err(KernelError::Module("模块包内含符号链接，拒绝解包".into()));
        }
        let raw_name = entry.name().to_string();
        if entry.is_dir() {
            continue;
        }
        let relative = safe_relative_path(&raw_name).ok_or_else(|| {
            KernelError::Module(format!("模块包内含非法路径: {raw_name}"))
        })?;

        let out_path = dest.join(&relative);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| KernelError::Module(format!("创建解包子目录失败: {e}")))?;
        }
        let mut out = std::fs::File::create(&out_path)
            .map_err(|e| KernelError::Module(format!("写出解包文件失败: {e}")))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| KernelError::Module(format!("解包写入失败: {e}")))?;
    }

    // 包内逐文件校验（存在则校验，缺失只告警——外层整包 sha256 已由下载引擎保证）。
    verify_inner_checksums(dest);

    // 容器根清单：必须存在且可解析。
    let manifest_path = dest.join(MANIFEST_FILE);
    let raw = std::fs::read(&manifest_path).map_err(|e| {
        KernelError::Module(format!(
            "模块包缺少清单 {}: {e}",
            manifest_path.display()
        ))
    })?;
    let manifest = ModuleManifest::parse(&raw)?;

    Ok(ExtractedPackage {
        manifest,
        dir: dest.to_path_buf(),
    })
}

/// 把解包产出的目录**原子**提升为目标模块目录 `<modules_dir>/<id>`。
///
/// 同卷 `rename` 是原子的：一旦目标出现，它就是完整的。为避免「目标已存在」
/// （更新场景）导致 rename 失败，先把旧目录挪到旁路，失败可回滚。
pub fn promote_extracted(tmp_dir: &Path, modules_dir: &Path, id: &str) -> Result<PathBuf, KernelError> {
    let target = modules_dir.join(id);
    let backup = modules_dir.join(format!(".old-{id}"));

    let had_previous = target.exists();
    if had_previous {
        // 清掉可能残留的旧备份，再挪走当前版本。
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::rename(&target, &backup).map_err(|e| {
            KernelError::Module(format!("暂存旧版本目录失败 {}: {e}", target.display()))
        })?;
    }

    match std::fs::rename(tmp_dir, &target) {
        Ok(()) => {
            if had_previous {
                let _ = std::fs::remove_dir_all(&backup);
            }
            Ok(target)
        }
        Err(e) => {
            // 回滚：把旧版本挪回原位，尽量不留破损状态。
            if had_previous {
                let _ = std::fs::rename(&backup, &target);
            }
            Err(KernelError::Module(format!(
                "安装模块落位失败 {}: {e}",
                target.display()
            )))
        }
    }
}

/// 校验包内 `manifest.sha256` 列出的每个文件（存在则校验）。
fn verify_inner_checksums(dest: &Path) {
    let checksum_path = dest.join(CHECKSUM_FILE);
    let Ok(text) = std::fs::read_to_string(&checksum_path) else {
        log::warn!(
            "[package] 模块包缺少 {}，跳过包内逐文件校验（整包 sha256 已由下载引擎保证）",
            CHECKSUM_FILE
        );
        return;
    };

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let Some(expected) = parts.next() else { continue };
        let Some(rel) = parts.next().map(str::trim) else {
            continue;
        };
        if rel.is_empty() || rel == CHECKSUM_FILE {
            continue;
        }
        let Some(relative) = safe_relative_path(rel) else {
            log::warn!("[package] 校验清单含非法路径，跳过: {rel}");
            continue;
        };
        let file = dest.join(&relative);
        match std::fs::read(&file) {
            Ok(bytes) => {
                let got = hex_digest(&bytes);
                if !got.eq_ignore_ascii_case(expected) {
                    log::warn!(
                        "[package] 包内文件 {} sha256 与 manifest.sha256 不符（期望 {expected}，实际 {got}）",
                        relative.display()
                    );
                }
            }
            Err(e) => log::warn!(
                "[package] 校验清单列出的文件不存在 {}: {e}",
                file.display()
            ),
        }
    }
}

/// 计算字节的 sha256 十六进制串。
fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 把 zip 条目名规范化为安全的相对路径；非法即返回 `None`。
///
/// 拒绝：绝对路径、盘符前缀、`..` 穿越、空 / `.` 组件、Windows 保留名。
///
/// 除解包外，[`crate::registry::frontend`] 的自定义协议也复用它，保证「包内路径」
/// 与「运行时取文件」两处用同一套安全规则。
pub(crate) fn safe_relative_path(raw: &str) -> Option<PathBuf> {
    // zip 规范用 `/` 分隔；Windows 上也可能出现 `\`，统一处理。
    let normalized = raw.replace('\\', "/");
    let trimmed = normalized.trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }

    let mut out = PathBuf::new();
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return None;
        }
        // 盘符形态（`C:`）或含冒号的组件一律拒绝（Windows ADS / 盘符）。
        if part.contains(':') {
            return None;
        }
        if is_reserved_name(part) {
            return None;
        }
        out.push(part);
    }

    if out.as_os_str().is_empty() {
        return None;
    }
    // 兜底：规范化后仍须为纯相对路径。
    if out.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(out)
}

/// Windows 保留设备名（大小写不敏感，含带扩展名形态如 `CON.txt`）。
fn is_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL"
            | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9"
            | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 用 zip 写入器造一个模块包到临时文件。
    fn build_package(tag: &str, files: &[(&str, &[u8])]) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cgl-pkg-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pkg.cglm");

        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        for (name, data) in files {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    const MANIFEST: &str = r#"{
      "schema_version": "1",
      "id": "copper-lamp.demo-tools",
      "i18n_namespace": "demo-tools",
      "display_name": "示例工具",
      "description": "示例模块",
      "author": { "name": "copper-lamp", "url": "https://github.com/copper-lamp" },
      "license": "MIT",
      "version": "0.1.0",
      "platforms": ["windows-x86_64", "android-arm64", "linux-x86_64", "windows-aarch64"],
      "launcher": { "min": "0.1.0", "max": null },
      "api_version": 1,
      "backend": { "crate": "copper-module-demo", "entry": "e", "artifact_glob": "target/release/copper_module_demo.dll" },
      "frontend": { "dist": "frontend/dist", "register": "register.js" },
      "permissions": [],
      "icon": "assets/icon.svg",
      "category": "utility"
    }"#;

    fn temp_dest(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cgl-pkgout-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn extracts_valid_package_and_reads_manifest() {
        let pkg = build_package(
            "ok",
            &[
                ("module.json", MANIFEST.as_bytes()),
                ("backend/copper_module_demo.so", b"dylib"),
                ("frontend/register.js", b"js"),
            ],
        );
        let dest = temp_dest("ok");
        let out = extract_package(&pkg, &dest).unwrap();
        assert_eq!(out.manifest.id, "copper-lamp.demo-tools");
        assert!(dest.join("backend/copper_module_demo.so").is_file());
        assert!(dest.join("frontend/register.js").is_file());

        let _ = std::fs::remove_dir_all(pkg.parent().unwrap());
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn rejects_traversal_and_absolute_entries() {
        for bad in ["../evil.txt", "a/../../evil.txt", "/abs.txt", "C:/win.txt", "CON"] {
            let pkg = build_package("bad", &[("module.json", MANIFEST.as_bytes()), (bad, b"x")]);
            let dest = temp_dest("bad");
            let err = extract_package(&pkg, &dest);
            assert!(err.is_err(), "条目 `{bad}` 必须被拒绝");
            assert!(!dest.join("evil.txt").exists());
            let _ = std::fs::remove_dir_all(pkg.parent().unwrap());
            let _ = std::fs::remove_dir_all(&dest);
        }
    }

    #[test]
    fn rejects_missing_manifest() {
        let pkg = build_package("nomani", &[("frontend/register.js", b"js")]);
        let dest = temp_dest("nomani");
        assert!(extract_package(&pkg, &dest).is_err());
        let _ = std::fs::remove_dir_all(pkg.parent().unwrap());
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn promote_swaps_and_backs_up_existing() {
        let modules = temp_dest("promote");
        std::fs::create_dir_all(&modules).unwrap();
        // 已存在旧版本
        std::fs::create_dir_all(modules.join("copper-lamp.demo-tools")).unwrap();
        std::fs::write(modules.join("copper-lamp.demo-tools/old.txt"), b"old").unwrap();
        // 新解包临时目录
        let tmp = modules.join(".tmp-new");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("new.txt"), b"new").unwrap();

        let target = promote_extracted(&tmp, &modules, "copper-lamp.demo-tools").unwrap();
        assert!(target.join("new.txt").is_file());
        assert!(!target.join("old.txt").exists(), "旧版本应被替换");
        assert!(!modules.join(".old-copper-lamp.demo-tools").exists(), "备份应被清理");

        let _ = std::fs::remove_dir_all(&modules);
    }

    #[test]
    fn safe_relative_path_rules() {
        assert!(safe_relative_path("frontend/register.js").is_some());
        assert!(safe_relative_path("a\\b.js").is_some());
        assert!(safe_relative_path("../x").is_none());
        assert!(safe_relative_path("/x").is_none());
        assert!(safe_relative_path("C:/x").is_none());
        assert!(safe_relative_path("COM1").is_none());
        assert!(safe_relative_path("con.txt").is_none());
    }
}
