//! Android APK/APKS import primitives.
//! The importer keeps packages opaque: APK resources and native libraries are not
//! unpacked into the instance package, so the Android runtime can mount them later.

use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
use std::path::Path;

use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::error::KernelError;

pub const MCBE_PACKAGE: &str = "com.mojang.minecraftpe";

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
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

fn inspect_zip(path: &Path) -> Result<(bool, bool), KernelError> {
    let f = File::open(path)?;
    let mut zip = ZipArchive::new(f).map_err(|e| KernelError::InvalidArgument(format!("APK ZIP 无效: {e}")))?;
    let mut manifest = false;
    let mut arm64 = false;
    for i in 0..zip.len() {
        let name = zip.by_index(i).map_err(|e| KernelError::InvalidArgument(format!("读取 APK 条目失败: {e}")))?.name().to_string();
        if name == "AndroidManifest.xml" { manifest = true; }
        if name.starts_with("lib/arm64-v8a/") && name.ends_with(".so") { arm64 = true; }
    }
    Ok((manifest, arm64))
}

/// Validate a copied base APK. Package identity is supplied by the Android
/// PackageManager bridge because AndroidManifest.xml is binary AXML.
pub fn inspect_base(path: &Path, package_name: &str, version_name: &str, version_code: u64) -> Result<ApkPackageInfo, KernelError> {
    if package_name != MCBE_PACKAGE { return Err(KernelError::InvalidArgument(format!("不是 Minecraft Bedrock 包: {package_name}"))); }
    let (manifest, arm64) = inspect_zip(path)?;
    if !manifest { return Err(KernelError::InvalidArgument("APK 缺少 AndroidManifest.xml".into())); }
    if !arm64 { return Err(KernelError::InvalidArgument("APK 不包含 arm64-v8a 原生库".into())); }
    Ok(ApkPackageInfo { package_name: package_name.into(), version_code, version_name: version_name.into(), abi: "arm64-v8a".into(), sha256: sha256_file(path)?, has_splits: false })
}

fn copy_file(src: &Path, dst: &Path) -> Result<(), KernelError> {
    let mut input = File::open(src)?;
    let mut output = File::create(dst)?;
    io::copy(&mut input, &mut output)?;
    output.flush()?;
    Ok(())
}

/// Import one APK or an .apks archive into a temporary package directory.
/// Caller must atomically rename the returned directory into its final location.
pub fn import_file(source: &Path, staging: &Path, package_name: &str, version_name: &str, version_code: u64) -> Result<ApkPackageInfo, KernelError> {
    if !source.is_file() { return Err(KernelError::InvalidArgument("APK 文件不存在".into())); }
    fs::create_dir_all(staging)?;
    let base = staging.join("base.apk");
    let is_apks = source.extension().and_then(|v| v.to_str()).map(|v| v.eq_ignore_ascii_case("apks")).unwrap_or(false);
    let mut split_names = Vec::new();
    if is_apks {
        let f = File::open(source)?;
        let mut archive = ZipArchive::new(f).map_err(|e| KernelError::InvalidArgument(format!("APKS ZIP 无效: {e}")))?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| KernelError::InvalidArgument(format!("读取 APKS 失败: {e}")))?;
            let name = entry.name().to_string();
            if !name.ends_with(".apk") || name.contains("..") || name.contains('/') { continue; }
            let dst = if name == "base.apk" { base.clone() } else {
                split_names.push(name.clone());
                staging.join("splits").join(&name)
            };
            if let Some(parent) = dst.parent() { fs::create_dir_all(parent)?; }
            let mut out = File::create(dst)?;
            io::copy(&mut entry, &mut out)?;
        }
        if !base.exists() { return Err(KernelError::InvalidArgument("APKS 缺少 base.apk".into())); }
    } else {
        copy_file(source, &base)?;
    }
    let mut info = inspect_base(&base, package_name, version_name, version_code)?;
    info.has_splits = !split_names.is_empty();
    let manifest = serde_json::to_vec_pretty(&info)?;
    fs::write(staging.join("package.json"), manifest)?;
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_wrong_package() {
        let p = std::path::PathBuf::from("does-not-exist.apk");
        let e = inspect_base(&p, "other.package", "1", 1).unwrap_err();
        assert!(e.to_string().contains("不是 Minecraft"));
    }
}
