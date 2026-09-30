//! Android APK/APKS import.
//!
//! Layout produced inside an instance directory (matched by
//! `CopperGameLayout` on the Android side):
//!
//! ```text
//! <versions>/<instance>/
//!   base.apk.levi          single base APK
//!   splits/<name>.apk.levi split APKs, flat
//!   package.json           importer record (package / version / digest)
//! ```
//!
//! The `.apk.levi` suffix is deliberate. A file named `*.apk` inside app
//! private storage is a candidate for package installers and scanners; keeping
//! the ZIP intact but renaming the file means the Android runtime can still
//! mount it (`AssetManager#addAssetPath` and `ZipFile` inspect content, not the
//! name) while nothing on the device treats it as installable.

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
pub const BASE_APK: &str = "base.apk.levi";
/// Split APK directory inside the instance directory.
pub const SPLITS_DIR: &str = "splits";
/// Split APK file suffix.
pub const SPLIT_SUFFIX: &str = ".apk.levi";
/// Importer record written next to the packages.
pub const PACKAGE_JSON: &str = "package.json";

/// Native ABI the launcher targets. Anything else cannot run on an arm64 device.
pub const TARGET_ABI: &str = "arm64-v8a";

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

/// Confirm the APK actually ships native code for the target ABI.
fn require_target_abi(zip: &mut ZipArchive<File>) -> Result<(), KernelError> {
    let prefix = format!("lib/{TARGET_ABI}/");
    let found = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok())
        .any(|entry| {
            let name = entry.name().to_string();
            name.starts_with(&prefix) && name.ends_with(".so")
        });
    if found {
        Ok(())
    } else {
        Err(KernelError::InvalidArgument(format!(
            "APK 不包含 {TARGET_ABI} 原生库"
        )))
    }
}

/// Validate a staged base APK and build its import record.
///
/// `package` / `versionName` / `versionCode` come from the binary manifest, not
/// from the caller: these values decide which native library set the Android
/// runtime loads, so they must be read from the artifact itself.
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
    require_target_abi(&mut zip)?;
    drop(zip);

    Ok(ApkPackageInfo {
        package_name: identity.package_name,
        version_code: identity.version_code,
        version_name: identity.version_name,
        abi: TARGET_ABI.into(),
        sha256: sha256_file(path)?,
        has_splits: false,
    })
}

fn copy_file(src: &Path, dst: &Path) -> Result<(), KernelError> {
    let mut input = File::open(src)?;
    let mut output = File::create(dst)?;
    io::copy(&mut input, &mut output)?;
    output.flush()?;
    Ok(())
}

/// Import one APK or an `.apks` bundle into `staging`.
///
/// The caller must atomically rename `staging` into the final instance
/// directory; on any error `staging` is left for the caller to discard.
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
    } else {
        copy_file(source, &base)?;
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
    fn rejects_unsupported_extension() {
        let source = std::env::temp_dir().join("copper-apk-import.txt");
        fs::write(&source, b"not an apk").unwrap();
        let staging = std::env::temp_dir().join("copper-apk-import-ext");
        let _ = fs::remove_dir_all(&staging);
        assert!(import_file(&source, &staging).is_err());
        let _ = fs::remove_file(&source);
    }

    #[test]
    fn base_apk_path_uses_levi_suffix() {
        let dir = Path::new("/data/versions/demo");
        assert_eq!(
            base_apk_path(dir),
            Path::new("/data/versions/demo").join("base.apk.levi")
        );
    }
}
