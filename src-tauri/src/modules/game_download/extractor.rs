//! 游戏包解包器：GDK `.msixvc`（加密 XVC 容器）走内嵌原生 DLL，历史 `.appx`（真 ZIP）走 zip crate 回退。
//!
//! 技术背景：新版 GDK 游戏包是加密容器，无纯 Rust 开源解包方案。LeviLauncher 靠闭源
//! `launcher_core.dll`（导出 `GetW`(宽字符) / `Get`(ANSI) `(in,out)->i32`）解包成功；
//! 该方案依赖用户曾装过 Store 版并保留授权（返回码 3/4 即缺失）。本模块将该 DLL 内置，
//! 首次运行把二进制落地到 `cache_dir()/gdkshared/`（比对 SHA256 + `.tmp` 原子落盘），
//! 进程内 `OnceLock` 单例装载并保活模块句柄。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::error::KernelError;

// ---------------------------------------------------------------- 内嵌二进制

/// 需拷落盘的 DLL 清单（顺序影响依赖装载）。
const DLL_FILES: &[&str] = &[
    "vcruntime140_1.dll",
    "launcher_api.dll",
    "libHttpClient.dll",
    "launcher_core.dll",
];

const CORE_DLL: &str = "launcher_core.dll";

macro_rules! embed {
    ($name:literal) => {
        include_bytes!(concat!("../../../resources/gdkshared/", $name))
    };
}

/// 内嵌 DLL 字节（编译期注入，避免运行期依赖打包目录）。
struct Embedded;
impl Embedded {
    fn bytes(name: &str) -> &'static [u8] {
        match name {
            "vcruntime140_1.dll" => embed!("vcruntime140_1.dll"),
            "launcher_api.dll" => embed!("launcher_api.dll"),
            "libHttpClient.dll" => embed!("libHttpClient.dll"),
            "launcher_core.dll" => embed!("launcher_core.dll"),
            _ => &[],
        }
    }
}

/// 确保 `dir` 下落地全部内嵌 DLL（内容或 SHA256 变化才重写，`.tmp`+rename 原子落盘）。
pub fn ensure_dll_dir(dir: &Path) -> Result<(), ExtractError> {
    std::fs::create_dir_all(dir)?;
    for name in DLL_FILES {
        let data = Embedded::bytes(name);
        if data.is_empty() {
            return Err(ExtractError::DllMissing(format!("缺少内嵌资源 {name}")));
        }
        write_if_changed(dir, name, data)?;
    }
    Ok(())
}

fn file_sha256(path: &Path) -> Option<[u8; 32]> {
    let raw = std::fs::read(path).ok()?;
    Some(Sha256::digest(&raw).into())
}

fn write_if_changed(dir: &Path, name: &str, data: &[u8]) -> Result<(), ExtractError> {
    let target = dir.join(name);
    let needs_write = match std::fs::metadata(&target) {
        Ok(m) if m.len() == data.len() as u64 => file_sha256(&target) != Some(Sha256::digest(data).into()),
        _ => true,
    };
    if !needs_write {
        return Ok(());
    }
    let tmp = dir.join(format!("{name}.tmp"));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(data)?;
    drop(f);
    std::fs::rename(&tmp, &target)?;
    Ok(())
}

// ---------------------------------------------------------------- 错误与返回码

/// 解包错误。
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("压缩包解析错误: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("原生解包库缺失: {0}")]
    DllMissing(String),
    #[error("原生解包库装载失败: {0}")]
    DllLoad(String),
    #[error("解包结果码: {0:?}")]
    Code(ExtractReturnCode),
    /// ZIP 解包后未发现游戏主程序（`Minecraft.Windows.exe`）。
    #[error("解包产物缺少游戏主程序")]
    MissingExecutable,
    /// ZIP 条目路径非法（防路径穿越）。
    #[error("解包条目路径非法: {0}")]
    UnsafeEntry(String),
    #[error("不支持的平台（解包需 Windows）")]
    UnsupportedPlatform,
}

impl From<ExtractError> for KernelError {
    fn from(e: ExtractError) -> Self {
        KernelError::Config(format!("解包失败: {e}"))
    }
}

/// launcher_core.dll 的返回码 → 可读描述（对齐 Leviauncher `core.go::Extract`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractReturnCode {
    Success,
    Exception,
    InvalidParams,
    KeyNotFound,
    UnauthorizedCaller,
    PipeOpenFailed,
    InputNotFound,
    OutputDirInvalid,
    ParseFailed,
    ExtractFailed,
    Unknown(i32),
}

impl ExtractReturnCode {
    pub fn from_code(code: i32) -> Self {
        match code {
            0 => ExtractReturnCode::Success,
            1 => ExtractReturnCode::Exception,
            2 => ExtractReturnCode::InvalidParams,
            3 => ExtractReturnCode::KeyNotFound,
            4 => ExtractReturnCode::UnauthorizedCaller,
            5 => ExtractReturnCode::PipeOpenFailed,
            6 => ExtractReturnCode::InputNotFound,
            7 => ExtractReturnCode::OutputDirInvalid,
            8 => ExtractReturnCode::ParseFailed,
            9 => ExtractReturnCode::ExtractFailed,
            c => ExtractReturnCode::Unknown(c),
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, ExtractReturnCode::Success)
    }

    /// 供前端做 i18n 文案映射的稳定键（缺失授权 / 密钥等透传）。
    pub fn friendly_key(&self) -> &'static str {
        match self {
            ExtractReturnCode::Success => "game-download.error.success",
            ExtractReturnCode::Exception => "game-download.error.exception",
            ExtractReturnCode::InvalidParams => "game-download.error.invalid_params",
            ExtractReturnCode::KeyNotFound => "game-download.error.key_not_found",
            ExtractReturnCode::UnauthorizedCaller => "game-download.error.unauthorized",
            ExtractReturnCode::PipeOpenFailed => "game-download.error.pipe_open_failed",
            ExtractReturnCode::InputNotFound => "game-download.error.input_not_found",
            ExtractReturnCode::OutputDirInvalid => "game-download.error.output_dir_invalid",
            ExtractReturnCode::ParseFailed => "game-download.error.parse_failed",
            ExtractReturnCode::ExtractFailed => "game-download.error.extract_failed",
            ExtractReturnCode::Unknown(_) => "game-download.error.unknown",
        }
    }
}

// ---------------------------------------------------------------- 解包入口

/// 解包 `src` 到 `out_dir`：`src` 是 ZIP（历史 `.appx`）走 zip 回退；否则视为 XVC 走原生 DLL。
///
/// `dll_dir` 为原生 DLL 落盘目录（通常 `cache_dir()/gdkshared`），仅在走 DLL 分支时使用。
pub fn extract_package(src: &Path, out_dir: &Path, dll_dir: &Path) -> Result<(), ExtractError> {
    if is_appx_zip(src) {
        return extract_appx_zip(src, out_dir);
    }
    extract_xvc(src, out_dir, dll_dir)
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
    Ok(())
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

// ---------------------------------------------------------------- 原生 DLL 解 XVC

/// XVC 校验产物：目录须含游戏主程序。
pub fn verify_executable(out_dir: &Path) -> Result<(), ExtractError> {
    if out_dir.join("Minecraft.Windows.exe").is_file() {
        Ok(())
    } else {
        Err(ExtractError::MissingExecutable)
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use libloading::Library;
    use std::sync::OnceLock;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{LoadLibraryW, SetDllDirectoryW};

    type FnGetW = unsafe extern "system" fn(*const u16, *const u16) -> i32;

    /// 已装载的 launcher_core.dll 单例（进程生命周期，绝不卸载）。
    ///
    /// 只保存扁平 fn 指针（Send+Sync，供静态存放）；库本体经 `&'static Library` 泄漏保活。
    struct LoadedCore {
        /// 依赖库句柄保活（vcruntime / libHttpClient / launcher_core）。
        #[allow(dead_code)]
        handles: Vec<HMODULE>,
        get_w: FnGetW,
    }

    // `HMODULE` 是 `*mut c_void` 原生包装，非 `Send+Sync`。本类型仅作为进程级静态单例
    // （`OnceLock<Arc<LoadedCore>>`）存放、绝不跨线程实例转移，故安全。
    unsafe impl Send for LoadedCore {}
    unsafe impl Sync for LoadedCore {}

    /// 首次调用固定的 DLL 落盘目录（先到者胜，进程内固定）。
    static CORE_DIR: OnceLock<PathBuf> = OnceLock::new();
    static CORE: OnceLock<Arc<LoadedCore>> = OnceLock::new();

    /// UTF-8 字符串 → NUL 结尾的 UTF-16 缓冲区。
    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// 调用 GetW 解包。`dll_dir` 为首次装载时固定的落盘目录。
    pub fn extract_xvc(src: &Path, out_dir: &Path, dll_dir: &Path) -> Result<(), ExtractError> {
        std::fs::create_dir_all(out_dir)?;
        let _ = CORE_DIR.set(dll_dir.to_path_buf());
        let dir = CORE_DIR
            .get()
            .ok_or_else(|| ExtractError::DllMissing("DLL 目录未设置".into()))?;
        ensure_dll_dir(dir)?;
        let core = Arc::new(load(dir)?);
        // 并发时以先装载者为准。
        if CORE.get().is_some() {
            // 已有装载：用现成实例。
        } else {
            let _ = CORE.set(core.clone());
        }
        let core = CORE.get().cloned().unwrap_or(core);

        let ws = utf16(&src.to_string_lossy());
        let wo = utf16(&out_dir.to_string_lossy());
        let code = unsafe { (core.get_w)(ws.as_ptr(), wo.as_ptr()) };
        let rc = ExtractReturnCode::from_code(code);
        if !rc.is_success() {
            return Err(ExtractError::Code(rc));
        }
        verify_executable(out_dir)
    }

    fn load(dir: &Path) -> Result<LoadedCore, ExtractError> {
        // —— 依赖装载：先按全路径预载 vcruntime / libHttpClient，再设置 DLL 搜索目录。
        let mut handles = Vec::new();
        unsafe {
            for dep in ["vcruntime140_1.dll", "libHttpClient.dll"] {
                let path = utf16(&dir.join(dep).to_string_lossy());
                if let Ok(h) = LoadLibraryW(PCWSTR(path.as_ptr())) {
                    handles.push(h);
                }
            }
            let dir_w = utf16(&dir.to_string_lossy());
            let _ = SetDllDirectoryW(PCWSTR(dir_w.as_ptr()));
        }

        // —— 装载主库并解析 GetW（本 DLL 的主导出；缺则报清晰错误）。
        // 库一旦装载即永久泄漏保活，故 fn 指针在进程内始终有效。
        // `Library::new` 在 libloading 0.8 起为 unsafe（装载外部代码）。
        let lib: &'static Library = Box::leak(Box::new(
            unsafe { Library::new(dir.join(CORE_DLL)) }
                .map_err(|e| ExtractError::DllLoad(e.to_string()))?,
        ));
        let sym = unsafe { lib.get::<FnGetW>(b"GetW\0") }
            .map_err(|e| ExtractError::DllLoad(format!("缺少导出 GetW: {e}")))?;
        let get_w: FnGetW = *sym;

        Ok(LoadedCore { handles, get_w })
    }
}

#[cfg(not(windows))]
mod native {
    use super::*;
    /// 非 Windows 平台提供存根（模块仍可编译，解包运行时返回不支持）。
    pub fn extract_xvc(_src: &Path, _out_dir: &Path, _dll_dir: &Path) -> Result<(), ExtractError> {
        Err(ExtractError::UnsupportedPlatform)
    }
}

fn extract_xvc(src: &Path, out_dir: &Path, dll_dir: &Path) -> Result<(), ExtractError> {
    native::extract_xvc(src, out_dir, dll_dir)
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("copper_gd_ext_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_appx_zip(path: &Path, with_exe: bool, traversal: bool) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default();
        zip.start_file("AppxMetadata/AppxManifest.xml", opts).unwrap();
        zip.write_all(b"<manifest metadata/>").unwrap();
        if with_exe {
            zip.start_file("Minecraft.Windows.exe", opts).unwrap();
            zip.write_all(b"PE").unwrap();
            zip.start_file("Windows/Minecraft.Windows.pdb", opts).unwrap();
            zip.write_all(b"symbols").unwrap();
        }
        if traversal {
            zip.start_file("../../evil.txt", opts).unwrap();
            zip.write_all(b"evil").unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn return_code_mapping() {
        assert_eq!(ExtractReturnCode::from_code(0), ExtractReturnCode::Success);
        assert!(ExtractReturnCode::from_code(0).is_success());
        assert_eq!(ExtractReturnCode::from_code(3), ExtractReturnCode::KeyNotFound);
        assert_eq!(ExtractReturnCode::from_code(4), ExtractReturnCode::UnauthorizedCaller);
        assert_eq!(ExtractReturnCode::from_code(9), ExtractReturnCode::ExtractFailed);
        assert_eq!(ExtractReturnCode::from_code(99), ExtractReturnCode::Unknown(99));
        assert!(!ExtractReturnCode::from_code(4).is_success());
        // 每个返回码都有稳定文案键。
        for c in 0..10 {
            assert!(!ExtractReturnCode::from_code(c).friendly_key().is_empty());
        }
    }

    #[test]
    fn detects_zip_magic() {
        let dir = temp_dir("magic");
        let p = dir.join("pkg.msixvc");
        make_appx_zip(&p, true, false);
        assert!(is_appx_zip(&p));
        let not_zip = dir.join("plain.bin");
        std::fs::write(&not_zip, b"\x00\x01\x02\x03blah").unwrap();
        assert!(!is_appx_zip(&not_zip));
    }

    #[test]
    fn extract_appx_drops_metadata_and_finds_exe() {
        let dir = temp_dir("appx");
        let pkg = dir.join("pkg.appx");
        make_appx_zip(&pkg, true, false);
        let out = dir.join("out");
        extract_appx_zip(&pkg, &out).unwrap();
        assert!(out.join("Minecraft.Windows.exe").is_file());
        assert!(out.join("Windows/Minecraft.Windows.pdb").is_file());
        // AppxMetadata 被跳过。
        assert!(!out.join("AppxMetadata").exists());
    }

    #[test]
    fn extract_appx_rejects_missing_exe() {
        let dir = temp_dir("noexe");
        let pkg = dir.join("pkg.appx");
        make_appx_zip(&pkg, false, false);
        let err = extract_appx_zip(&pkg, &dir.join("out")).unwrap_err();
        assert!(matches!(err, ExtractError::MissingExecutable));
    }

    #[test]
    fn sanitize_blocks_traversal() {
        assert!(sanitize_rel("../x").is_err());
        assert!(sanitize_rel("a/../../b").is_err());
        assert_eq!(sanitize_rel("a/./b/c").unwrap(), "a/b/c");
        assert_eq!(sanitize_rel("Windows/x.pdb").unwrap(), "Windows/x.pdb");
    }

    #[test]
    fn write_if_changed_skips_identical() {
        let dir = temp_dir("dll");
        let data = b"hello-dll-bytes";
        write_if_changed(&dir, "dep.dll", data).unwrap();
        assert_eq!(std::fs::read(dir.join("dep.dll")).unwrap(), data);
        // 再次写入不落盘（内容一致）。
        std::fs::write(dir.join("dep.dll"), b"tampered").unwrap();
        write_if_changed(&dir, "dep.dll", data).unwrap();
        assert_eq!(std::fs::read(dir.join("dep.dll")).unwrap(), data);
        assert!(!dir.join("dep.dll.tmp").exists());
    }
}