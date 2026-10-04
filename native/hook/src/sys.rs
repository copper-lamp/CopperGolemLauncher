//! Win32 原始 FFI 声明 —— 本 crate **唯一**的外部符号依赖入口。
//!
//! # 为什么不用 `windows` crate
//!
//! 符号面只有十余个（见下）。手写声明换来三件事：
//!
//! 1. 注入进游戏进程的产物不随 `windows` crate 的 feature 变动而漂移 —— 这个
//!    DLL 出问题时无法在宿主里断点排查，体积与符号面必须**冻结**；
//! 2. 不引入 `Win32_*` 大模块的链接开销；
//! 3. 与 docs §2.9 的工程边界一致：FFI 面越小，越不容易出现跨语言事故。
//!
//! 代价是签名必须手写正确 —— 因此每条声明旁都标了官方原型；
//! `src/iat.rs` 里的 PE 结构体同理，字段偏移已在注释中给出。
//!
//! 全部标 `unsafe extern` 语义由调用方承担：调用点一律 `unsafe`，且都要能
//! 承受「宿主不是游戏」的情形（白名单之外的进程会走不到这些调用）。

#![allow(non_snake_case)]

use std::ffi::c_void;

/// `HRESULT`（`winerror.h`）。`S_OK == 0`。
pub type HRESULT = i32;

/// `FOLDERID`（`knownfolders.h`），实际是 `GUID`。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GUID {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

/// `FOLDERID_RoamingAppData` = `{F1B32785-6FBA-4FCF-9D55-7B8E7F157091}`。
/// `FOLDERID_RoamingAppData`（重定向到实例版本目录；游戏自己会再拼渠道目录）。
pub const FOLDERID_ROAMING_APP_DATA: GUID = GUID {
    data1: 0x3EB685DB,
    data2: 0x65F9,
    data3: 0x4CF6,
    data4: [0xA0, 0x3A, 0xE3, 0xEF, 0x65, 0x72, 0x9F, 0x3D],
};

/// `FOLDERID_LocalAppData`（重定向为空字符串）。
pub const FOLDERID_LOCAL_APP_DATA: GUID = GUID {
    data1: 0xF1B32785,
    data2: 0x6FBA,
    data3: 0x4FCF,
    data4: [0x9D, 0x55, 0x7B, 0x8E, 0x7F, 0x15, 0x70, 0x91],
};

// GUID 取值直接抄自 Windows SDK `um\KnownFolders.h`（本机核对过），
// 不用记忆里的值 —— 这两个常量一旦写错，隔离会「看起来生效但写错地方」：
// Roaming 认成 Local 的话游戏数据会落到实例外的用户目录，且没有任何报错。

extern "system" {
    /// `HRESULT SHGetKnownFolderPath(REFKNOWNFOLDERID, DWORD, HANDLE, PWSTR*)`
    pub fn SHGetKnownFolderPath(
        rfid: *const GUID,
        dw_flags: u32,
        h_token: *mut c_void,
        ppsz_path: *mut *mut u16,
    ) -> HRESULT;

    /// `DWORD GetTempPathW(DWORD nBufferLength, LPWSTR lpBuffer)`
    pub fn GetTempPathW(n_buffer_length: u32, lp_buffer: *mut u16) -> u32;

    /// `DWORD GetTempPathA(DWORD nBufferLength, LPARSTR lpBuffer)`
    pub fn GetTempPathA(n_buffer_length: u32, lp_buffer: *mut u8) -> u32;

    /// `HMODULE WINAPI GetModuleHandleW(LPCWSTR)`
    pub fn GetModuleHandleW(lp_module_name: *const u16) -> *mut c_void;

    /// `HMODULE WINAPI GetModuleHandleExW(DWORD, LPCWSTR, HMODULE*)`
    pub fn GetModuleHandleExW(dw_flags: u32, lp_module_name: *const u16, ph_module: *mut *mut c_void) -> i32;

    /// `DWORD WINAPI GetModuleFileNameW(HMODULE, LPWSTR, DWORD)`
    pub fn GetModuleFileNameW(h_module: *mut c_void, lp_filename: *mut u16, n_size: u32) -> u32;

    /// `HMODULE WINAPI LoadLibraryExW(LPCWSTR, HANDLE, DWORD)`
    pub fn LoadLibraryExW(lp_lib_file_name: *const u16, h_file: *mut c_void, dw_flags: u32) -> *mut c_void;

    /// `FARPROC WINAPI GetProcAddress(HMODULE, LPCSTR)`
    pub fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const u8) -> *mut c_void;

    /// `BOOL WINAPI DisableThreadLibraryCalls(HMODULE)`
    pub fn DisableThreadLibraryCalls(h_module: *mut c_void) -> i32;

    /// `HANDLE WINAPI CreateThread(..., LPTHREAD_START_ROUTINE, LPVOID, ...)`
    pub fn CreateThread(
        lp_thread_attributes: *mut c_void,
        dw_stack_size: usize,
        lp_start_address: unsafe extern "system" fn() -> u32,
        lp_parameter: *mut c_void,
        dw_creation_flags: u32,
        lp_thread_id: *mut u32,
    ) -> *mut c_void;

    /// `BOOL WINAPI CloseHandle(HANDLE)`
    pub fn CloseHandle(h_object: *mut c_void) -> i32;

    /// `BOOL WINAPI VirtualProtect(LPVOID, SIZE_T, DWORD, PDWORD)`
    pub fn VirtualProtect(
        lp_address: *mut c_void,
        dw_size: usize,
        fl_new_protect: u32,
        lp_old_protect: *mut u32,
    ) -> i32;

    /// `HRESULT CoTaskMemAlloc(ULONG cb)`
    #[link_name = "CoTaskMemAlloc"]
    pub fn CoTaskMemAlloc(cb: usize) -> *mut c_void;

    /// `void CoTaskMemFree(LPVOID)`
    #[link_name = "CoTaskMemFree"]
    pub fn CoTaskMemFree(pv: *mut c_void);

    /// `void OutputDebugStringA(LPCSTR)`
    #[link_name = "OutputDebugStringA"]
    pub fn OutputDebugStringA(lp_output: *const u8);

    /// `void SetLastError(DWORD)`
    pub fn SetLastError(dw_error: u32);

    /// `int WideCharToMultiByte(UINT, DWORD, LPCWSTR, int, LPSTR, int, LPCCH, LPBOOL)`
    pub fn WideCharToMultiByte(
        code_page: u32,
        dw_flags: u32,
        lp_wide_char_str: *const u16,
        cch_wide_char: i32,
        lp_multi_byte_str: *mut u8,
        cb_multi_byte: i32,
        lp_default_char: *mut u8,
        lp_used_default_char: *mut i32,
    ) -> i32;
}

/// `CP_ACP`（`GetTempPathA` 的语义：ANSI 版按系统代码页转换）。
pub const CP_ACP: u32 = 0;

/// `ERROR_NO_UNICODE_TRANSLATION`（`winerror.h` 1113）。
///
/// `GetTempPathA` 无法把实例路径表示成 ANSI 时用它失败 —— 绝不退回 `'?'`
/// 替换（那会把游戏重定向到一个**不同的**目录，比失败更糟）。
pub const ERROR_NO_UNICODE_TRANSLATION: u32 = 1113;

/// `ERROR_INSUFFICIENT_BUFFER`（122）。
pub const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

/// `ERROR_INVALID_PARAMETER`（87）。
pub const ERROR_INVALID_PARAMETER: u32 = 87;

/// 把 UTF-16 转成系统 ANSI 代码页；不可转换返回 `None`。
///
/// 不使用 `'?'` 替换：`GetTempPathA` 的调用方（游戏）拿到一个被替换过的路径后
/// 会写到一个不存在的位置，症状是「实例目录里少了日志/临时文件」，极难归因。
/// 直接失败让调用方走它自己的回退分支。
pub fn to_ansi(text: &str) -> Option<Vec<u8>> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let needed = unsafe {
        WideCharToMultiByte(
            CP_ACP,
            0,
            wide.as_ptr(),
            wide.len() as i32,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if needed <= 0 {
        return None;
    }
    let mut buffer = vec![0u8; needed as usize];
    let written = unsafe {
        WideCharToMultiByte(
            CP_ACP,
            0,
            wide.as_ptr(),
            wide.len() as i32,
            buffer.as_mut_ptr(),
            needed,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if written <= 0 {
        None
    } else {
        buffer.truncate(written as usize);
        Some(buffer)
    }
}

/// `GET_MODULE_HANDLE_EX_FLAG_PIN` —— 让模块在进程运行期不被卸载。
///
/// 必须 pin：游戏运行期任何一次 `FreeLibrary`（例如加载失败的回滚路径）都会让
/// 我们替换进去的重定向函数指针变悬空，后续调用直接跳进野地址。
pub const GET_MODULE_HANDLE_EX_FLAG_PIN: u32 = 0x00000001;

/// `LOAD_WITH_ALTERED_SEARCH_PATH` —— 按传入的**绝对路径**解析依赖，
/// 不用 DLL 所在目录参与搜索。
///
/// 预加载的目标是第三方加载器（LeviLamina 等），它们依赖同目录的一批 dll；
/// 用默认搜索路径会优先命中系统目录里同名 dll，表现为「装了不生效」。
pub const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x00000008;

/// `PAGE_READWRITE`。
pub const PAGE_READWRITE: u32 = 0x04;

/// `S_OK`。
pub const S_OK: HRESULT = 0;

/// 把 Rust 字符串转成以 NUL 结尾的 UTF-16 缓冲（调用方保证生命周期）。
pub fn to_wide_nul(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 把 UTF-16 缓冲（到首个 NUL 为止）转回 `String`。
///
/// 用 `from_utf16_lossy` 而不是报错：路径里出现无法配对的代理项时，
/// 拿到一个近似路径远好过整个初始化流程失败。
pub fn from_wide_nul(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// 向 `OutputDebugStringA` 输出一行 ASCII 文本。
///
/// 只送 ASCII：这里是排障出口，不值得为它引入 `WideCharToMultiByte`。
/// 非 ASCII 内容退化成 `?`，不影响定位。
pub fn debug_output(text: &str) {
    let mut line: Vec<u8> = Vec::with_capacity(text.len() + 1);
    for byte in text.as_bytes() {
        if byte.is_ascii_graphic() || *byte == b' ' {
            line.push(*byte);
        } else {
            line.push(b'?');
        }
    }
    line.push(0);
    // SAFETY: `line` 以 NUL 结尾，长度与内容均由上面构造保证；OutputDebugStringA
    // 只读不写。
    unsafe { OutputDebugStringA(line.as_ptr()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_round_trips_with_nul() {
        let wide = to_wide_nul("D:/实例/mods/LeviLamina.dll");
        assert_eq!(*wide.last().unwrap(), 0);
        assert_eq!(from_wide_nul(&wide), "D:/实例/mods/LeviLamina.dll");
    }

    #[test]
    fn from_wide_stops_at_first_nul() {
        let wide = to_wide_nul("a");
        assert_eq!(from_wide_nul(&wide), "a");
        // 缓冲区被别的代码写脏时也不越界读
        let mut dirty = wide.clone();
        dirty.extend_from_slice(&[0x4E2D, 0x6587]);
        assert_eq!(from_wide_nul(&dirty), "a");
    }

    #[test]
    fn from_wide_handles_missing_nul() {
        assert_eq!(from_wide_nul(&[0x61, 0x62]), "ab");
    }

    #[test]
    fn ansi_conversion_keeps_ascii_and_refuses_unrepresentable() {
        assert_eq!(to_ansi("D:/mc/temp"), Some(b"D:/mc/temp".to_vec()));
        // 本机代码页是 GBK，中文路径可表示；用控制字符这类任何代码页都拒绝的
        // 输入验证「不替换成 ?」的行为。
        assert!(to_ansi("\u{1}").is_none());
    }
}