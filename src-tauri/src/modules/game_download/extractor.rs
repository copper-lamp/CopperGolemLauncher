//! 游戏包解包器：历史 `.appx`（真 ZIP）走 zip crate；新版 MSIXVC 走 `msixvc` 的
//! 纯 Rust XVC 校验/解密/提取，**不再依赖任何外部 DLL**。
//!
//! 技术背景：新版 GDK 游戏包是加密 XVC 容器，早期方案依赖闭源 `launcher_core.dll`，
//! 该方案已被上游 LeviLauncher 弃用（其当前版本同样为纯 Go 解包），本模块也已整体
//! 移除 DLL，改为 `msixvc::extract_xvc`：读 XVD 头 → 验证 SHA256 哈希树 → 取商店授权
//! 得到的 32 字节 content key → AES-XTS 逐页解密 → staging 目录原子 rename 发布。
//! 密钥缺失时明确报 `MissingContentKey`，由上层把授权失败原因透传给前端，不回退。

use std::io::Read;
use std::path::Path;

use crate::error::KernelError;

use super::msixvc;

// ---------------------------------------------------------------- 错误

/// 解包错误。
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("压缩包解析错误: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// ZIP 解包后未发现游戏主程序（`Minecraft.Windows.exe`）。
    #[error("解包产物缺少游戏主程序")]
    MissingExecutable,
    /// ZIP 条目路径非法（防路径穿越）。
    #[error("解包条目路径非法: {0}")]
    UnsafeEntry(String),
    /// MSIXVC 原生解析/提取失败。
    #[error("MSIXVC 解包失败: {0}")]
    Msixvc(String),
    /// 包含加密区域但没有商店 content key。
    #[error("该包需要商店授权（缺少 content key）")]
    MissingStoreKey,
}

impl From<ExtractError> for KernelError {
    fn from(e: ExtractError) -> Self {
        KernelError::Config(format!("解包失败: {e}"))
    }
}

// ---------------------------------------------------------------- 解包入口

/// 解包 `src` 到 `out_dir`（不带密钥，供无加密区域的历史包 / 未加密包使用）。
pub fn extract_package(src: &Path, out_dir: &Path) -> Result<(), ExtractError> {
    extract_package_with_key(src, out_dir, None)
}

/// 解包入口：真 ZIP（历史 `.appx`）走 zip；MSIXVC 走纯 Rust 提取。
///
/// `content_key` 为商店授权链取得的 32 字节 AES-XTS key；为 `None` 时若包含
/// 加密区域则返回 [`ExtractError::MissingStoreKey`]，由调用方把授权失败原因
/// 透传给前端（不回退任何 DLL）。
pub fn extract_package_with_key(
    src: &Path,
    out_dir: &Path,
    content_key: Option<&[u8]>,
) -> Result<(), ExtractError> {
    if let Some(key) = content_key {
        if key.len() != 32 {
            return Err(ExtractError::Msixvc("content key 长度必须为 32 字节".into()));
        }
    }
    if is_appx_zip(src) {
        return extract_appx_zip(src, out_dir);
    }
    match msixvc::extract_xvc(src, out_dir, content_key) {
        Ok(()) => verify_package(out_dir),
        Err(msixvc::ParseError::MissingContentKey) => Err(ExtractError::MissingStoreKey),
        Err(error) => Err(ExtractError::Msixvc(error.to_string())),
    }
}

/// 是否为真 ZIP 容器（PK 魔数）。历史 `.appx` 与绝大多数压缩文件都是 ZIP。
fn is_appx_zip(path: &Path) -> bool {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut magic = [0u8; 4];
    if f.read_exact(&mut magic).is_err() {
        return false;
    }
    magic == [0x50, 0x4B, 0x03, 0x04]
}

/// 历史 `.appx`（真 ZIP）解包：遍历条目，跳过 `AppxMetadata/`，防路径穿越，成功需含主程序。
fn extract_appx_zip(src: &Path, out_dir: &Path) -> Result<(), ExtractError> {
    std::fs::create_dir_all(out_dir)?;
    let file = std::fs::File::open(src)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut found_exe = false;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let raw_name = entry.name().to_string().replace('\\', "/");
        // 目录项 / 元数据目录跳过。
        if entry.is_dir() || raw_name.ends_with('/') || raw_name.starts_with("AppxMetadata/") {
            continue;
        }
        // 防路径穿越：拒绝绝对路径与 `..`。
        let rel = sanitize_rel(&raw_name)?;
        let target = out_dir.join(&rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut out)?;
        if looks_like_executable(&rel) {
            found_exe = true;
        }
    }
    if !found_exe {
        return Err(ExtractError::MissingExecutable);
    }
    verify_package(out_dir)
}

/// 归一化 ZIP 相对路径并校验防穿越；返回 out_dir 内的安全相对路径。
fn sanitize_rel(name: &str) -> Result<String, ExtractError> {
    let trimmed = name.trim_matches('/');
    if trimmed.is_empty() {
        return Err(ExtractError::UnsafeEntry(name.to_string()));
    }
    let mut parts = Vec::new();
    for part in trimmed.split('/') {
        match part {
            "" | "." => continue,
            ".." => return Err(ExtractError::UnsafeEntry(name.to_string())),
            p => parts.push(p),
        }
    }
    Ok(parts.join("/"))
}

/// 是否为游戏主程序（`Minecraft.Windows.exe`）。
fn looks_like_executable(rel: &str) -> bool {
    Path::new(rel)
        .file_name()
        .map(|n| n.eq_ignore_ascii_case("Minecraft.Windows.exe"))
        .unwrap_or(false)
}

// ---------------------------------------------------------------- 解包产物校验

/// 校验解包产物完整可启动：主程序存在、`MicrosoftGame.config` 存在、
/// PE 头为 x64（对照 LeviLauncher `extract_windows.go` 的 PE 架构校验）。
pub fn verify_package(out_dir: &Path) -> Result<(), ExtractError> {
    let exe = out_dir.join("Minecraft.Windows.exe");
    if !exe.is_file() {
        return Err(ExtractError::MissingExecutable);
    }
    if !out_dir.join("MicrosoftGame.config").is_file() {
        return Err(ExtractError::MissingExecutable);
    }
    verify_pe_x64(&exe)?;
    Ok(())
}

/// 校验 PE 文件头 Machine == 0x8664（x64）。读前 4 KiB 即可。
fn verify_pe_x64(path: &Path) -> Result<(), ExtractError> {
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 4096];
    let n = file.read(&mut buf)?;
    if n < 64 {
        return Err(ExtractError::MissingExecutable);
    }
    if &buf[0..2] != b"MZ" {
        return Err(ExtractError::MissingExecutable);
    }
    let pe_off = u32::from_le_bytes([buf[60], buf[61], buf[62], buf[63]]) as usize;
    if pe_off + 6 > n {
        // PE 头在 4 KiB 之外（极罕见），再读一次覆盖。
        let mut file2 = std::fs::File::open(path)?;
        let mut buf2 = vec![0u8; pe_off + 64];
        let n2 = file2.read(&mut buf2)?;
        if n2 < pe_off + 6 || &buf2[pe_off..pe_off + 4] != b"PE\0\0" {
            return Err(ExtractError::MissingExecutable);
        }
        let machine = u16::from_le_bytes([buf2[pe_off + 4], buf2[pe_off + 5]]);
        if machine != 0x8664 {
            return Err(ExtractError::MissingExecutable);
        }
        return Ok(());
    }
    if &buf[pe_off..pe_off + 4] != b"PE\0\0" {
        return Err(ExtractError::MissingExecutable);
    }
    let machine = u16::from_le_bytes([buf[pe_off + 4], buf[pe_off + 5]]);
    if machine != 0x8664 {
        return Err(ExtractError::MissingExecutable);
    }
    Ok(())
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use zip::write::SimpleFileOptions;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("copper_gd_ext_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 构造一个最小可校验的 x64 PE（DOS 头 + PE 签名 + COFF 头 Machine=0x8664）。
    fn write_min_pe(path: &Path) {
        let mut pe = vec![0u8; 256];
        pe[0] = b'M';
        pe[1] = b'Z';
        pe[60] = 0x80; // e_lfanew = 0x80
        pe[0x80] = b'P';
        pe[0x81] = b'E';
        pe[0x84] = 0x64;
        pe[0x85] = 0x86; // Machine = 0x8664
        std::fs::write(path, &pe).unwrap();
    }

    fn make_appx_zip(path: &Path, with_exe: bool, with_config: bool, traversal: bool) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default();
        zip.start_file("AppxMetadata/AppxManifest.xml", opts).unwrap();
        zip.write_all(b"<manifest metadata/>").unwrap();
        if with_exe {
            // ZIP 里塞假 exe 不满足 PE 校验 → 只用于验证 MissingExecutable 分支。
            zip.start_file("Minecraft.Windows.exe", opts).unwrap();
            zip.write_all(b"MZ").unwrap();
        }
        if with_config {
            zip.start_file("MicrosoftGame.config", opts).unwrap();
            zip.write_all(b"<config/>").unwrap();
        }
        if traversal {
            zip.start_file("../../evil.txt", opts).unwrap();
            zip.write_all(b"evil").unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn detects_zip_magic() {
        let dir = temp_dir("magic");
        let p = dir.join("pkg.msixvc");
        make_appx_zip(&p, true, true, false);
        assert!(is_appx_zip(&p));
        let not_zip = dir.join("plain.bin");
        std::fs::write(&not_zip, b"\x00\x01\x02\x03blah").unwrap();
        assert!(!is_appx_zip(&not_zip));
    }

    #[test]
    fn extract_appx_drops_metadata_and_finds_exe() {
        let dir = temp_dir("appx");
        let pkg = dir.join("pkg.appx");
        // ZIP 内假 exe 不是合法 PE → 期望 MissingExecutable（PE 校验生效）。
        make_appx_zip(&pkg, true, true, false);
        let out = dir.join("out");
        let err = extract_appx_zip(&pkg, &out).unwrap_err();
        assert!(matches!(err, ExtractError::MissingExecutable), "{err:?}");
        // 但元数据目录确实被跳过、config 被解出。
        assert!(!out.join("AppxMetadata").exists());
        assert!(out.join("MicrosoftGame.config").is_file());
    }

    #[test]
    fn verify_package_requires_config_and_pe() {
        let dir = temp_dir("verify");
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        // 缺主程序。
        assert!(matches!(
            verify_package(&out),
            Err(ExtractError::MissingExecutable)
        ));
        // 有主程序但缺 config。
        write_min_pe(&out.join("Minecraft.Windows.exe"));
        assert!(matches!(
            verify_package(&out),
            Err(ExtractError::MissingExecutable)
        ));
        // 补 config → 通过。
        std::fs::write(out.join("MicrosoftGame.config"), b"<config/>").unwrap();
        assert!(verify_package(&out).is_ok());
        // 非 PE 的主程序被拒。
        std::fs::write(out.join("Minecraft.Windows.exe"), b"not a pe").unwrap();
        assert!(matches!(
            verify_package(&out),
            Err(ExtractError::MissingExecutable)
        ));
    }

    #[test]
    fn sanitize_blocks_traversal() {
        assert!(sanitize_rel("../x").is_err());
        assert!(sanitize_rel("a/../../b").is_err());
        assert_eq!(sanitize_rel("a/./b/c").unwrap(), "a/b/c");
        assert_eq!(sanitize_rel("Windows/x.pdb").unwrap(), "Windows/x.pdb");
    }

    #[test]
    fn missing_key_is_explicit_not_a_fallback() {
        // 构造非 ZIP 的假 MSIXVC：纯 Rust 路径解析失败 → Msixvc 错误（不回退）。
        let dir = temp_dir("nokey");
        let pkg = dir.join("pkg.msixvc");
        std::fs::write(&pkg, b"\x00\x01\x02\x03not-a-zip-or-xvd").unwrap();
        let err = extract_package_with_key(&pkg, &dir.join("out"), None).unwrap_err();
        match err {
            ExtractError::MissingStoreKey | ExtractError::Msixvc(_) => {}
            other => panic!("预期 MissingStoreKey 或 Msixvc，实际 {other:?}"),
        }
    }

    #[test]
    fn bad_key_length_rejected_upfront() {
        let dir = temp_dir("badkey");
        let pkg = dir.join("pkg.msixvc");
        std::fs::write(&pkg, b"not-a-zip").unwrap();
        let err = extract_package_with_key(&pkg, &dir.join("out"), Some(&[0u8; 16])).unwrap_err();
        assert!(matches!(err, ExtractError::Msixvc(_)));
    }
}
