//! XAL（Xbox Authentication Library）本机登录。
//!
//! 阿里 levilauncher 的 GDK 登录方案：直接加载并调用微软 GDK 的 XAL
//! 封装 DLL（`launcher_api.dll`，导出符号被混淆为 `nh_xxxxxxxx`），读取
//! **本机已登录的 Xbox 用户**（XUID / gamertag）。全程不经过微软浏览器授权页，
//! 因此不受 MSN 网络/策略限制；前提是系统里先存在一个已登录的 Xbox 账户。
//!
//! 部署：GDK 运行库（`launcher_core.dll`、`libHttpClient.dll`、
//! `vcruntime140_1.dll`）随本 crate 交付（`include_bytes!` 嵌入），运行时释放到
//! 本地缓存目录 `%APPDATA%/copper-golem/xal/` 后按依赖顺序加载。仅支持 Windows。
//!
//! 调用自 levilauncher `internal/launchercore/xbox.go` 一比一还原：
//! `nh_04d8c8d4`=GetLocalUserId、`nh_f53b7b5e`=GetLocalUserGamertag、
//! `nh_3e61c2af`=XUserGetState、`nh_5c8e2c0b`=ResetSession（登出用）。

use std::path::PathBuf;
use std::sync::OnceLock;

/// 函数指针类型（无需持有 `Library`，普通 fn 指针天然 Send + Sync，
/// 可安全放入 `OnceLock` 供线程/异步复用）。
#[derive(Clone, Copy)]
struct XalSymbols {
    get_local_user_id: unsafe extern "system" fn(*mut u64) -> i32,
    get_local_user_gamertag: unsafe extern "system" fn(*mut u8, i32, *mut i32) -> i32,
    user_get_state: unsafe extern "system" fn(*mut u32) -> i32,
    reset_session: unsafe extern "system" fn() -> i32,
}

/// XAL 开放句柄（惰性初始化，进程内只加载一次 DLL）。
static XAL: OnceLock<Result<XalSymbols, String>> = OnceLock::new();

/// GDK 运行库字节（随工程交付，见 `res/xal/`）。
const API_DLL: &[u8] = include_bytes!("../../res/xal/launcher_api.dll");
const CORE_DLL: &[u8] = include_bytes!("../../res/xal/launcher_core.dll");
const HTTP_DLL: &[u8] = include_bytes!("../../res/xal/libHttpClient.dll");
const CRT_DLL: &[u8] = include_bytes!("../../res/xal/vcruntime140_1.dll");

/// 本机 Xbox 登录结果。
#[derive(Debug, Clone)]
pub struct XalProfile {
    pub xuid: u64,
    pub gamertag: String,
}

/// 取本机已登录的 Xbox 账户。失败返回可展示给用户的中文原因。
pub fn local_profile() -> Result<XalProfile, String> {
    let sym = match XAL.get_or_init(load_once) {
        Ok(s) => *s,
        Err(e) => return Err(e.clone()),
    };
    profile_from(&sym)
}

/// 登出后调用，重置 XAL 会话会话（尽力而为）。
pub fn reset_session() {
    if let Ok(sym) = XAL.get_or_init(load_once) {
        unsafe { (sym.reset_session)() };
    }
}

// ---- 内部实现 ----

fn cache_dir() -> Result<PathBuf, String> {
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("LOCALAPPDATA"))
        .map_err(|_| "未找到 APPDATA，无法定位 XAL 运行库".to_string())?;
    Ok(PathBuf::from(base).join("copper-golem").join("xal"))
}

fn write_once(dir: &PathBuf, name: &str, data: &[u8]) -> Result<(), String> {
    if let Ok(existing) = std::fs::read(dir.join(name)) {
        if existing.len() == data.len() && existing == data {
            return Ok(());
        }
    }
    std::fs::write(dir.join(name), data).map_err(|e| format!("写入 {name} 失败: {e}"))
}

fn load_once() -> Result<XalSymbols, String> {
    let dir = cache_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 XAL 目录失败: {e}"))?;
    for (name, data) in [
        ("vcruntime140_1.dll", CRT_DLL),
        ("libHttpClient.dll", HTTP_DLL),
        ("launcher_core.dll", CORE_DLL),
        ("launcher_api.dll", API_DLL),
    ] {
        write_once(&dir, name, data)?;
    }

    // 依赖按序加载到进程，且把目录加到 DLL 搜索路径，确保 launcher_api 能解析到 launcher_core。
    for name in ["vcruntime140_1.dll", "libHttpClient.dll", "launcher_core.dll"] {
        let path = dir.join(name);
        let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
        unsafe {
            windows::Win32::System::LibraryLoader::LoadLibraryW(
                windows::core::PCWSTR(wide.as_ptr()),
            )
        };
    }

    let api_path = dir.join("launcher_api.dll");
    let wide: Vec<u16> = api_path.to_string_lossy().encode_utf16().chain([0]).collect();
    let hmodule = unsafe {
        windows::Win32::System::LibraryLoader::LoadLibraryW(windows::core::PCWSTR(wide.as_ptr()))
            .map_err(|e| format!("加载 launcher_api.dll 失败: {e}"))?
    };

    // 取导出符号（ANSI 名，与 levilauncher 一致）。
    macro_rules! sym {
        ($name:literal, $ty:ty) => {{
            let ptr = unsafe {
                windows::Win32::System::LibraryLoader::GetProcAddress(
                    hmodule,
                    windows::core::PCSTR(concat!($name, "\0").as_ptr()),
                )
            };
            ptr.map(|f| unsafe { std::mem::transmute::<_, $ty>(f) })
                .ok_or_else(|| format!("launcher_api.dll 缺少导出符号 {}", $name))?
        }};
    }

    Ok(XalSymbols {
        get_local_user_id: sym!("nh_04d8c8d4", unsafe extern "system" fn(*mut u64) -> i32),
        get_local_user_gamertag: sym!(
            "nh_f53b7b5e",
            unsafe extern "system" fn(*mut u8, i32, *mut i32) -> i32
        ),
        user_get_state: sym!("nh_3e61c2af", unsafe extern "system" fn(*mut u32) -> i32),
        reset_session: sym!("nh_5c8e2c0b", unsafe extern "system" fn() -> i32),
    })
}

/// 读取本机当前用户（与 levilauncher `GetLocalUserId` / `GetLocalUserGamertag` 一致）。
fn profile_from(sym: &XalSymbols) -> Result<XalProfile, String> {
    let mut xuid: u64 = 0;
    if unsafe { (sym.get_local_user_id)(&mut xuid) } != 0 || xuid == 0 {
        return Err("未检测到本机 Xbox 登录。请在系统「设置 › 账户 › Xbox」先登录你的微软 Xbox 账号".to_string());
    }
    // gamertag 为 ACP 字节串；格式约定通常为 ASCII，按 UTF-8 lossy 解码。
    let mut buf = [0u8; 256];
    let mut used = 0i32;
    if unsafe { (sym.get_local_user_gamertag)(buf.as_mut_ptr(), 256, &mut used) } != 0 {
        return Err("读取 Xbox 玩家名失败".to_string());
    }
    let gamertag = String::from_utf8_lossy(&buf[..used.max(0) as usize]).to_string();
    Ok(XalProfile { xuid, gamertag })
}

/// 释放嵌入式运行库到指定目录（供打包/安装前置自检使用，正常调用指向缓存目录）。
pub fn prime(override_dir: Option<PathBuf>) -> Result<PathBuf, String> {
    let dir = override_dir.unwrap_or(cache_dir()?);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
    for (name, data) in [
        ("vcruntime140_1.dll", CRT_DLL),
        ("libHttpClient.dll", HTTP_DLL),
        ("launcher_core.dll", CORE_DLL),
        ("launcher_api.dll", API_DLL),
    ] {
        write_once(&dir, name, data)?;
    }
    std::fs::write(dir.join("VERSION"), b"1.0.0")
        .map_err(|e| format!("写入版本信息失败: {e}"))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_dlls_are_present() {
        assert!(!API_DLL.is_empty() && !CORE_DLL.is_empty());
        assert!(!HTTP_DLL.is_empty() && !CRT_DLL.is_empty());
        // DLL PE 头检查：MZ。
        assert_eq!(&API_DLL[0..2], b"MZ");
        assert_eq!(&CORE_DLL[0..2], b"MZ");
    }
}