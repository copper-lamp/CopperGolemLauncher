//! Android APK/APKS import.
//!
//! Layout produced inside an instance directory (matched by
//! `CopperGameLayout` on the Android side):
//!
//! ```text
//! <versions>/<instance>/
//!   base.apk.bundle          single base APK
//!   splits/<name>.apk.bundle split APKs, flat
//!   package.json             importer record (package / version / digest)
//! ```
//!
//! The `.apk.bundle` suffix keeps APK payloads distinguishable from packages
//! intended for Android installation while preserving their ZIP contents.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::error::KernelError;

use super::axml::{self, AndroidManifestIdentity};

/// Minecraft Bedrock's official package name.
pub const MCBE_PACKAGE: &str = "com.mojang.minecraftpe";

/// Base APK file name inside the instance directory.
pub const BASE_APK: &str = "base.apk.bundle";
/// Split APK directory inside the instance directory.
pub const SPLITS_DIR: &str = "splits";
/// Split APK file suffix.
pub const SPLIT_SUFFIX: &str = ".apk.bundle";
/// Importer record written next to the packages.
pub const PACKAGE_JSON: &str = "package.json";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApkPackageInfo {
    pub package_name: String,
    pub version_code: u64,
    pub version_name: String,
    pub abi: String,
    pub sha256: String,
    pub has_splits: bool,
}

/// 导入结果：**规整后的实例名** + 从包内解出的身份信息。
///
/// 实例名必须由内核回传，而不是让调用方沿用自己提交的名字：这个名字同时是
/// 版本目录名与安卓宿主定位实例的键，两侧各推导一次就会分叉
/// （见 `docs/安卓端能力差距与优先级.md` P0-2）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApkImportResult {
    /// 权威实例名（= `<versions>/<instance_name>` 目录名）。
    pub instance_name: String,
    pub package_info: ApkPackageInfo,
}

fn sha256_file(path: &Path) -> Result<String, KernelError> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// Read `AndroidManifest.xml` out of an APK and decode it.
fn read_manifest(zip: &mut ZipArchive<File>) -> Result<AndroidManifestIdentity, KernelError> {
    let mut entry = zip
        .by_name("AndroidManifest.xml")
        .map_err(|_| KernelError::InvalidArgument("APK 缺少 AndroidManifest.xml".into()))?;
    let mut xml = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut xml)?;
    axml::read_identity(&xml)
}

/// Read the first ABI represented by an APK without rejecting other layouts.
fn discover_abi(zip: &mut ZipArchive<File>) -> String {
    for index in 0..zip.len() {
        let name = match zip.by_index(index) {
            Ok(entry) => entry.name().to_string(),
            Err(_) => continue,
        };
        let Some(rest) = name.strip_prefix("lib/") else {
            continue;
        };
        let Some(abi) = rest.split('/').next() else {
            continue;
        };
        if !abi.is_empty() && name.ends_with(".so") {
            return abi.to_string();
        }
    }
    String::new()
}

/// Validate the base APK package and build its import record.
///
/// Package and version fields come from the binary manifest, not from the caller.
pub fn inspect_base(path: &Path) -> Result<ApkPackageInfo, KernelError> {
    if !path.is_file() {
        return Err(KernelError::InvalidArgument("APK 文件不存在".into()));
    }
    let f = File::open(path)?;
    let mut zip = ZipArchive::new(f)
        .map_err(|e| KernelError::InvalidArgument(format!("APK ZIP 无效: {e}")))?;
    let identity = read_manifest(&mut zip)?;
    if identity.package_name != MCBE_PACKAGE {
        return Err(KernelError::InvalidArgument(format!(
            "不是 Minecraft Bedrock 包: {}",
            identity.package_name
        )));
    }
    let abi = discover_abi(&mut zip);
    drop(zip);

    Ok(ApkPackageInfo {
        package_name: identity.package_name,
        version_code: identity.version_code,
        version_name: identity.version_name,
        abi,
        sha256: sha256_file(path)?,
        has_splits: false,
    })
}

/// Import one APK or an `.apks` bundle into `staging`.
///
/// The source must already be inside `staging`. A single APK is renamed into
/// its final package path; an APKS bundle is extracted and removed on success.
/// The caller atomically renames `staging` into the instance directory and
/// discards it on any error.
pub fn import_file(source: &Path, staging: &Path) -> Result<ApkPackageInfo, KernelError> {
    if !source.is_file() {
        return Err(KernelError::InvalidArgument("APK 文件不存在".into()));
    }
    let is_apks = source
        .extension()
        .and_then(|v| v.to_str())
        .map(|v| v.eq_ignore_ascii_case("apks"))
        .unwrap_or(false);
    if !is_apks && !is_apk_extension(source) {
        return Err(KernelError::InvalidArgument("只支持 .apk 或 .apks 文件".into()));
    }

    fs::create_dir_all(staging)?;
    if !source.starts_with(staging) || source == staging {
        return Err(KernelError::InvalidArgument(
            "APK 导入源必须位于事务目录内".into(),
        ));
    }
    let base = staging.join(BASE_APK);
    let mut split_count = 0usize;

    if is_apks {
        let f = File::open(source)?;
        let mut archive = ZipArchive::new(f)
            .map_err(|e| KernelError::InvalidArgument(format!("APKS ZIP 无效: {e}")))?;
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| KernelError::InvalidArgument(format!("读取 APKS 失败: {e}")))?;
            let name = entry.name().to_string();
            if !name.ends_with(".apk") {
                continue;
            }
            if name.contains('/') || name.contains('\\') || name.contains("..") {
                return Err(KernelError::InvalidArgument(format!(
                    "APKS 条目名非法: {name}"
                )));
            }
            let is_base = name.eq_ignore_ascii_case("base.apk");
            let dst = if is_base {
                base.clone()
            } else {
                split_count += 1;
                staging
                    .join(SPLITS_DIR)
                    .join(format!("{name}{SPLIT_SUFFIX}"))
            };
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out = File::create(&dst)?;
            io::copy(&mut entry, &mut out)?;
            out.flush()?;
        }
        if !base.is_file() {
            return Err(KernelError::InvalidArgument("APKS 缺少 base.apk".into()));
        }
        fs::remove_file(source)?;
    } else {
        fs::rename(source, &base)?;
    }

    let mut info = inspect_base(&base)?;
    info.has_splits = split_count > 0;
    let record = serde_json::to_vec_pretty(&info)?;
    fs::write(staging.join(PACKAGE_JSON), record)?;
    Ok(info)
}

fn is_apk_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|v| v.to_str())
        .map(|v| v.eq_ignore_ascii_case("apk"))
        .unwrap_or(false)
}

/// Path of the base APK for an instance directory.
pub fn base_apk_path(instance_dir: &Path) -> std::path::PathBuf {
    instance_dir.join(BASE_APK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_source() {
        let missing = Path::new("does-not-exist.apk");
        let staging = std::env::temp_dir().join("copper-apk-import-missing");
        let _ = fs::remove_dir_all(&staging);
        assert!(import_file(missing, &staging).is_err());
    }

    #[test]
    fn rejects_source_outside_transaction_directory() {
        let root = std::env::temp_dir().join("copper-apk-import-boundary");
        let staging = root.join(".import").join("request");
        let source = root.join("outside.apk");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&staging).unwrap();
        fs::write(&source, b"not an apk").unwrap();
        assert!(import_file(&source, &staging).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_unsupported_extension() {
        let source = std::env::temp_dir().join("copper-apk-import.txt");
        fs::write(&source, b"not an apk").unwrap();
        let staging = std::env::temp_dir().join("copper-apk-import-ext");
        let _ = fs::remove_dir_all(&staging);
        assert!(import_file(&source, &staging).is_err());
        let _ = fs::remove_file(&source);
    }

    #[test]
    fn base_apk_path_uses_package_suffix() {
        let dir = Path::new("/data/versions/demo");
        assert_eq!(
            base_apk_path(dir),
            Path::new("/data/versions/demo").join("base.apk.bundle")
        );
    }
}
