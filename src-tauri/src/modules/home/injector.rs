//! 进程创建与注入：普通启动、挂起启动 + 远程线程注入。
//!
//! # 启动上下文与降权
//!
//! 游戏必须在与 Explorer 相同的完整性级别下运行。启动器经常以管理员身份
//! 运行（开发入口、某些安装方式），而 Xbox / WAM 的登录链对跨完整性级别的
//! COM 调用非常敏感：游戏以 High 运行时，Xbox 身份代理会静默失败，界面停在
//! 「正在用 Microsoft 帐户登录」且不产生任何网络请求。LeviLauncher v1.0.3
//! 也专门修过同类问题（fix(auth): select sign-in flow based on process elevation）。
//!
//! 因此当启动器自己处于提升状态时，优先取当前会话 Explorer 的主令牌，
//! 用 CreateProcessWithTokenW 拉起游戏；拿不到令牌或调用失败才退回当前令牌。
//!
//! # 环境块
//!
//! 开发入口 dev.cmd 会把 TEMP/TMP 指向仓库内的 target\tmp（规避沙箱写
//! 限制）。这个值不能原样传给游戏：Xbox 的跨进程临时文件握手要求双方看到
//! 同一个用户 Temp。这里只把指向 target\tmp 的 TEMP/TMP 还原为用户真实
//! Temp，用户自己设置的其他值保持不动。
//!
//! # 注入
//!
//! 导入表注入是正常路径（见 home::inject）；本模块的远程线程注入只在导入表
//! 改写不可用时兜底，时序上会输给加载器，但至少保证游戏能带 hook 跑起来。

use std::ffi::c_void;
use std::path::Path;

use windows::core::{PCSTR, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HLOCAL, LocalFree, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows::Win32::Security::{
    DuplicateTokenEx, GetLengthSid, GetSidSubAuthority, GetSidSubAuthorityCount,
    GetTokenInformation, SecurityImpersonation, SetTokenInformation, TokenElevation,
    TokenIntegrityLevel, TokenLinkedToken, TokenPrimary, PSID, SID_AND_ATTRIBUTES,
    TOKEN_ALL_ACCESS, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_ELEVATION,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{
    VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
};
use windows::Win32::System::Threading::{
    CreateProcessW, CreateProcessWithTokenW, CreateRemoteThread, GetCurrentProcess,
    GetExitCodeThread, OpenProcess, OpenProcessToken, ResumeThread, TerminateProcess,
    WaitForSingleObject, CREATE_NEW_CONSOLE, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
    LOGON_WITH_PROFILE,
    PROCESS_CREATE_THREAD, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
    STARTUPINFOW,
};

use crate::error::KernelError;

/// 远程线程最长等待（毫秒）。
const INJECT_TIMEOUT_MS: u32 = 60_000;

/// STILL_ACTIVE（winbase.h 259）。
const STILL_ACTIVE: u32 = 259;

/// 开发入口注入的临时目录特征。只还原命中这些前缀的 TEMP/TMP。
const DEV_TEMP_MARKERS: &[&str] = &["\\coppercore\\target\\tmp"];

/// 还没恢复主线程的挂起进程。
struct Suspended {
    process: HANDLE,
    thread: HANDLE,
    resumed: bool,
}

impl Suspended {
    fn resume(&mut self) {
        if self.resumed {
            return;
        }
        self.resumed = true;
        let previous = unsafe { ResumeThread(self.thread) };
        if previous == u32::MAX {
            log::error!("[home/injector] 恢复主线程失败");
        }
    }
}

impl Drop for Suspended {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.thread);
            if !self.resumed {
                let _ = TerminateProcess(self.process, 0xC0000135);
            }
            let _ = CloseHandle(self.process);
        }
    }
}

/// 构造以 NUL 结尾的环境块。
fn build_env_block(extra: &[(String, String)]) -> Vec<u16> {
    let mut entries: Vec<(String, String)> = std::env::vars().collect();
    sanitize_dev_temp(&mut entries);
    for (key, value) in extra {
        entries.retain(|(existing, _)| !existing.eq_ignore_ascii_case(key));
        entries.push((key.clone(), value.clone()));
    }
    let mut block: Vec<u16> = Vec::new();
    for (key, value) in entries {
        block.extend(key.encode_utf16());
        block.push(u16::from(b'='));
        block.extend(value.encode_utf16());
        block.push(0);
    }
    block.push(0);
    block
}

/// 把开发入口覆盖的 TEMP/TMP 还原为用户真实 Temp。
fn sanitize_dev_temp(entries: &mut [(String, String)]) {
    let Some(profile) = std::env::var_os("USERPROFILE") else {
        return;
    };
    let real = Path::new(&profile)
        .join("AppData")
        .join("Local")
        .join("Temp")
        .to_string_lossy()
        .into_owned();
    for (key, value) in entries.iter_mut() {
        if !key.eq_ignore_ascii_case("TEMP") && !key.eq_ignore_ascii_case("TMP") {
            continue;
        }
        let lower = value.to_ascii_lowercase();
        if DEV_TEMP_MARKERS.iter().any(|marker| lower.contains(marker)) {
            *value = real.clone();
        }
    }
}

/// UTF-16 编码并补 NUL 终止符。
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 拼命令行：参数不能带 NUL（否则命令行在第一个参数处被截断）。
fn build_command_line(exe: &[u16], args: &[String]) -> Vec<u16> {
    let mut line = exe.to_vec();
    while line.last() == Some(&0) {
        line.pop();
    }
    for arg in args {
        line.push(u16::from(b' '));
        line.extend(arg.encode_utf16());
    }
    line
}

/// 当前进程是否处于提升状态。
fn current_token_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// Medium 完整性的 RID（SECURITY_MANDATORY_MEDIUM_RID）。
///
/// 注意：Low 是 0x1000，Medium 是 0x2000，High 是 0x3000。把 Low 当 Medium
/// 会让游戏以 Low 运行，连用户 Temp 都写不了，Xbox 登录必然挂起。
const MEDIUM_INTEGRITY_RID: u32 = 0x2000;

/// 令牌的完整性级别 RID。
fn token_integrity_rid(token: HANDLE) -> Option<u32> {
    unsafe {
        // TOKEN_MANDATORY_LABEL 要求 8 字节对齐；用 [u8; N] 做缓冲时，栈地址
        // 只保证 1 字节对齐，Release/High 上下文里会出现“misaligned pointer
        // dereference”，读到垃圾 SID 后直接 panic。用 u64 数组强制对齐。
        let mut buffer = [0u64; 16];
        let mut returned = 0u32;
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buffer.as_mut_ptr() as *mut c_void),
            (buffer.len() * std::mem::size_of::<u64>()) as u32,
            &mut returned,
        )
        .ok()?;
        let label = &*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let count = GetSidSubAuthorityCount(label.Label.Sid);
        if count.is_null() || *count == 0 {
            return None;
        }
        let sub = GetSidSubAuthority(label.Label.Sid, u32::from(*count) - 1);
        if sub.is_null() {
            return None;
        }
        Some(*sub)
    }
}

/// 选一个 Medium 完整性的令牌；拿不到就返回 None（宁可保持当前级别，
/// 也不能把游戏降到 Low —— Low 连用户 Temp 都写不了）。
fn medium_token() -> Option<HANDLE> {
    for token in [linked_limited_token(), explorer_primary_token()]
        .into_iter()
        .flatten()
    {
        if token_integrity_rid(token) == Some(MEDIUM_INTEGRITY_RID) {
            return Some(token);
        }
        unsafe { let _ = CloseHandle(token); }
    }
    // UAC 关闭时既没有 linked token，Explorer 也可能是 Low；此时复制当前
    // 令牌并显式把完整性降到 Medium，才能让游戏和普通桌面进程一样去调
    // Xbox 的 Medium 级 COM/RPC。
    create_medium_token()
}

/// 复制当前进程令牌并设置完整性级别为 Medium（S-1-16-8192）。
fn create_medium_token() -> Option<HANDLE> {
    unsafe {
        let mut current = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY,
            &mut current,
        )
        .map_err(|error| {
            eprintln!("[create_medium_token] OpenProcessToken: {error}");
        })
        .ok()?;
        let mut duplicated = HANDLE::default();
        let dup_ok = DuplicateTokenEx(
            current,
            TOKEN_ALL_ACCESS,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut duplicated,
        )
        .is_ok();
        let _ = CloseHandle(current);
        if !dup_ok {
            eprintln!(
                "[create_medium_token] DuplicateTokenEx: {}",
                std::io::Error::last_os_error()
            );
            return None;
        }

        let mut sid = PSID(std::ptr::null_mut());
        if let Err(error) = ConvertStringSidToSidW(windows::core::w!("S-1-16-8192"), &mut sid) {
            eprintln!("[create_medium_token] ConvertStringSidToSidW: {error}");
            let _ = CloseHandle(duplicated);
            return None;
        }
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: sid,
                // SE_GROUP_INTEGRITY（winnt.h）= 0x20；该 feature 未启用，直接写字面量。
                Attributes: 0x20,
            },
        };
        let size = std::mem::size_of::<TOKEN_MANDATORY_LABEL>()
            + GetLengthSid(sid) as usize;
        let set_ok = SetTokenInformation(
            duplicated,
            TokenIntegrityLevel,
            &label as *const _ as *const c_void,
            size as u32,
        )
        .is_ok();
        let _ = LocalFree(HLOCAL(sid.0));
        if set_ok {
            Some(duplicated)
        } else {
            eprintln!(
                "[create_medium_token] SetTokenInformation: {}",
                std::io::Error::last_os_error()
            );
            let _ = CloseHandle(duplicated);
            None
        }
    }
}

/// 当前提升进程的 UAC 过滤令牌（Medium 完整性）。
///
/// 这是最可靠的降权来源：它和当前进程同用户、同会话、同环境，只是完整性级别
/// 被 UAC 过滤到 Medium。Explorer 的令牌在某些环境下本身是 Low（本机实测），
/// 只能作为次选。
fn linked_limited_token() -> Option<HANDLE> {
    unsafe {
        let mut current = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut current).is_err() {
            return None;
        }
        let mut linked = HANDLE::default();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            current,
            TokenLinkedToken,
            Some(&mut linked as *mut _ as *mut c_void),
            std::mem::size_of::<HANDLE>() as u32,
            &mut returned,
        )
        .is_ok();
        let _ = CloseHandle(current);
        ok.then_some(linked)
    }
}

/// 取当前会话 Explorer 的主令牌。
fn explorer_primary_token() -> Option<HANDLE> {
    let pid = explorer_pid()?;
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(
            process,
            TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_QUERY,
            &mut token,
        )
        .is_ok();
        let _ = CloseHandle(process);
        if !opened {
            return None;
        }
        let mut primary = HANDLE::default();
        let duplicated = DuplicateTokenEx(
            token,
            TOKEN_ALL_ACCESS,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        )
        .is_ok();
        let _ = CloseHandle(token);
        duplicated.then_some(primary)
    }
}

/// 找到当前会话里的 explorer.exe。
fn explorer_pid() -> Option<u32> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32 {
            dwSize: std::mem::size_of::<PROCESSENTRY32>() as u32,
            ..Default::default()
        };
        let mut found = None;
        if Process32First(snapshot, &mut entry).is_ok() {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|value| *value == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name: String = entry.szExeFile[..end]
                    .iter()
                    .map(|value| *value as u8 as char)
                    .collect();
                if name.eq_ignore_ascii_case("explorer.exe") {
                    found = Some(entry.th32ProcessID);
                    break;
                }
                if Process32Next(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

/// 创建进程：启动器提升时用 Explorer 令牌降权，否则用当前令牌。
fn create_process(
    exe_wide: &[u16],
    cwd_wide: &[u16],
    command_line: &mut [u16],
    env_block: &[u16],
    flags: PROCESS_CREATION_FLAGS,
) -> Result<PROCESS_INFORMATION, KernelError> {
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();

    // 默认不降权：CreateProcessWithTokenW 产出的子进程实测会被降成 Low，
    // Xbox 的本机 RPC 在 Low 下直接卡死。需要旧行为时显式设
    // COPPER_LAUNCH_DE_ELEVATE=1。
    if std::env::var_os("COPPER_LAUNCH_DE_ELEVATE").is_some() && current_token_elevated() {
        match medium_token() {
            Some(token) => {
                let result = unsafe {
                    CreateProcessWithTokenW(
                        token,
                        LOGON_WITH_PROFILE,
                        PCWSTR(exe_wide.as_ptr()),
                        PWSTR(command_line.as_mut_ptr()),
                        flags,
                        Some(env_block.as_ptr() as *const c_void),
                        PCWSTR(cwd_wide.as_ptr()),
                        &startup,
                        &mut info,
                    )
                };
                unsafe { let _ = CloseHandle(token); }
                match result {
                    Ok(()) => {
                        log::info!("[home/injector] 已用 Medium 令牌启动游戏");
                        return Ok(info);
                    }
                    Err(error) => {
                        log::warn!("[home/injector] 降权启动失败，退回当前令牌: {error}")
                    }
                }
            }
            None => log::warn!("[home/injector] 取不到 Explorer 令牌，退回当前令牌启动"),
        }
    }

    unsafe {
        CreateProcessW(
            PCWSTR(exe_wide.as_ptr()),
            PWSTR(command_line.as_mut_ptr()),
            None,
            None,
            false,
            flags,
            Some(env_block.as_ptr() as *const _),
            PCWSTR(cwd_wide.as_ptr()),
            &startup,
            &mut info,
        )
        .map_err(|error| {
            KernelError::Module(format!(
                "[home/injector] 启动失败（{}）: {error}",
                String::from_utf16_lossy(&exe_wide[..exe_wide.len().saturating_sub(1)])
            ))
        })?;
    }
    Ok(info)
}

/// 以挂起方式启动游戏、注入 DLL 后恢复主线程，返回进程 id。
pub fn launch_with_injection(
    exe: &Path,
    cwd: &Path,
    args: &[String],
    env: &[(String, String)],
    dll: &Path,
    console: bool,
) -> Result<u32, KernelError> {
    let exe_wide = wide(&exe.to_string_lossy());
    let cwd_wide = wide(&cwd.to_string_lossy());
    let env_block = build_env_block(env);

    let mut command_line = build_command_line(&exe_wide, args);

    let mut flags = CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT;
    if console {
        flags |= CREATE_NEW_CONSOLE;
    }
    let info = create_process(&exe_wide, &cwd_wide, &mut command_line, &env_block, flags)?;

    let mut suspended = Suspended {
        process: info.hProcess,
        thread: info.hThread,
        resumed: false,
    };
    let pid = info.dwProcessId;

    inject_into(&suspended, dll)?;
    suspended.resume();
    Ok(pid)
}

/// 普通启动：不挂起、不注入。
pub fn launch_plain(
    exe: &Path,
    cwd: &Path,
    args: &[String],
    env: &[(String, String)],
    console: bool,
) -> Result<u32, KernelError> {
    let exe_wide = wide(&exe.to_string_lossy());
    let cwd_wide = wide(&cwd.to_string_lossy());
    let env_block = build_env_block(env);

    let mut command_line = build_command_line(&exe_wide, args);

    let mut flags = CREATE_UNICODE_ENVIRONMENT;
    if console {
        flags |= CREATE_NEW_CONSOLE;
    }
    let info = create_process(&exe_wide, &cwd_wide, &mut command_line, &env_block, flags)?;
    unsafe {
        let _ = CloseHandle(info.hThread);
        let _ = CloseHandle(info.hProcess);
    }
    Ok(info.dwProcessId)
}

/// 在挂起的进程里起远程线程加载 dll（hook DLL 的主注入路径）。
fn inject_into(target: &Suspended, dll: &Path) -> Result<(), KernelError> {
    inject_library(target.process, dll, INJECT_TIMEOUT_MS)
}

/// 对已经运行的游戏进程注入一个 DLL（客户端模式用）。
///
/// 与启动注入共用同一套远程线程实现，但不改 exe、不参与启动时序：客户端
/// 必须在游戏起来 5 秒后加载，且用户随时可能点「重新注入」。
pub fn inject_dll_into_process(pid: u32, dll: &Path) -> Result<(), KernelError> {
    if !dll.is_file() {
        return Err(KernelError::Module(format!(
            "[home/injector] 客户端文件不存在：{}",
            dll.display()
        )));
    }
    let process = unsafe {
        OpenProcess(
            PROCESS_CREATE_THREAD
                | PROCESS_VM_OPERATION
                | PROCESS_VM_WRITE
                | PROCESS_VM_READ
                | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
    }
    .map_err(|error| {
        KernelError::Module(format!(
            "[home/injector] 打开游戏进程失败（pid={pid}）：{error}（游戏可能已退出）"
        ))
    })?;
    let result = inject_library(process, dll, INJECT_TIMEOUT_MS);
    unsafe { let _ = CloseHandle(process); }
    result
}

/// 目标进程的模块表里是否已有指定文件名的模块。
///
/// 远程线程的 LoadLibraryW 返回非零只代表调用返回了模块基址；客户端被安全
/// 软件「放行调用、拦截实际加载」时，模块表里不会有它。注入后核对一次模块表，
/// 才能把「调用成功」和「真的在进程里」分开。
pub fn is_module_loaded(pid: u32, module_file_name: &str) -> Option<bool> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        Module32FirstW, Module32NextW, MODULEENTRY32W, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32,
    };

    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid)
        else {
            // 快照都拿不到时不能断言「没加载」：LoadLibrary 已经返回成功，
            // 这里只是核对手段不可用。交给调用方按「无法核对」处理。
            return None;
        };
        let mut entry: MODULEENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
        let mut found = false;
        if Module32FirstW(snapshot, &mut entry).is_err() {
            let _ = CloseHandle(snapshot);
            return None;
        }
        loop {
            let end = entry
                .szModule
                .iter()
                .position(|value| *value == 0)
                .unwrap_or(entry.szModule.len());
            let name = String::from_utf16_lossy(&entry.szModule[..end]);
            if name.eq_ignore_ascii_case(module_file_name) {
                found = true;
                break;
            }
            if Module32NextW(snapshot, &mut entry).is_err() {
                break;
            }
        }
        let _ = CloseHandle(snapshot);
        Some(found)
    }
}

/// 远程线程加载 DLL 的共用实现。
fn inject_library(process: HANDLE, dll: &Path, timeout_ms: u32) -> Result<(), KernelError> {
    let path = wide(&dll.to_string_lossy());
    let bytes = path.len() * std::mem::size_of::<u16>();

    unsafe {
        let remote = VirtualAllocEx(
            process,
            None,
            bytes,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        );
        if remote.is_null() {
            return Err(KernelError::Module(format!(
                "[home/injector] 目标进程分配内存失败（Win32 错误 {}）",
                std::io::Error::last_os_error()
            )));
        }

        let result = (|| -> Result<HANDLE, KernelError> {
            WriteProcessMemory(process, remote, path.as_ptr() as *const _, bytes, None)
                .map_err(|error| {
                    KernelError::Module(format!("[home/injector] 写入 DLL 路径失败: {error}"))
                })?;

            let kernel = GetModuleHandleW(PCWSTR(wide("kernel32.dll").as_ptr())).map_err(
                |error| {
                    KernelError::Module(format!("[home/injector] 取 kernel32 基址失败: {error}"))
                },
            )?;
            let symbol: Vec<u8> = b"LoadLibraryW\0".to_vec();
            let Some(entry) = GetProcAddress(kernel, PCSTR(symbol.as_ptr())).map(|raw| {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "system" fn(*mut c_void) -> u32,
                >(raw)
            }) else {
                return Err(KernelError::Module(
                    "[home/injector] kernel32 里找不到 LoadLibraryW".to_string(),
                ));
            };
            CreateRemoteThread(
                process,
                None,
                0,
                Some(entry),
                Some(remote as *const _),
                0,
                None,
            )
            .map_err(|error| {
                KernelError::Module(format!("[home/injector] 创建远程线程失败: {error}"))
            })
        })();

        let thread = match result {
            Ok(thread) => thread,
            Err(error) => {
                let _ = VirtualFreeEx(process, remote, 0, MEM_RELEASE);
                return Err(error);
            }
        };
        let waited = WaitForSingleObject(thread, timeout_ms);
        let mut code = 0u32;
        let read = GetExitCodeThread(thread, &mut code).is_ok();
        let _ = CloseHandle(thread);
        let _ = VirtualFreeEx(process, remote, 0, MEM_RELEASE);
        if waited != WAIT_OBJECT_0 {
            return Err(KernelError::Module(format!(
                "[home/injector] 远程线程在 {}ms 内未返回：{} 可能缺少依赖、被杀软拦截，或 DllMain 卡在加载器锁上",
                timeout_ms,
                dll.display()
            )));
        }
        if !read {
            return Err(KernelError::Module(
                "[home/injector] 读取远程线程退出码失败".to_string(),
            ));
        }
        if code == 0 || code == STILL_ACTIVE {
            return Err(KernelError::Module(format!(
                "[home/injector] 远程线程未能加载 {}：LoadLibraryW 返回 0（依赖缺失或文件被拦截）",
                dll.display()
            )));
        }
        log::info!("[home/injector] 注入成功，DLL 基址 0x{code:X}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

    fn symbol_of(name: &[u8]) -> Option<unsafe extern "system" fn() -> isize> {
        let kernel =
            unsafe { GetModuleHandleW(PCWSTR(super::wide("kernel32.dll").as_ptr())) }.ok()?;
        unsafe { GetProcAddress(kernel, PCSTR(name.as_ptr())) }
    }

    #[test]
    fn load_library_is_resolvable() {
        assert!(
            symbol_of(b"LoadLibraryW\0").is_some(),
            "LoadLibraryW 必须可解析"
        );
    }

    #[test]
    fn env_block_overrides_case_insensitively() {
        let block = super::build_env_block(&[
            ("COPPER_HOOK_ROOT".to_string(), "D:\\a".to_string()),
            ("copper_hook_channel".to_string(), "release".to_string()),
        ]);
        let text = String::from_utf16_lossy(&block);
        assert!(text.contains("COPPER_HOOK_ROOT=D:\\a\0"));
        assert!(text.contains("copper_hook_channel=release\0"));
        for key in ["COPPER_HOOK_ROOT", "copper_hook_channel"] {
            let count = text
                .split('\0')
                .filter(|entry| {
                    let (name, _) = entry.split_once('=').unwrap_or((entry, ""));
                    name.eq_ignore_ascii_case(key)
                })
                .count();
            assert_eq!(count, 1, "{key} 必须恰好出现一次");
        }
    }


    #[test]
    fn env_block_is_double_nul_terminated() {
        let block = super::build_env_block(&[]);
        assert!(block.len() >= 2);
        assert_eq!(*block.last().unwrap(), 0);
        assert_eq!(block[block.len() - 2], 0);
    }

    #[test]
    fn dev_temp_is_restored_to_user_temp() {
        let mut entries = vec![(
            "TEMP".to_string(),
            "D:\\CopperGolem\\CopperCore\\target\\tmp".to_string(),
        )];
        super::sanitize_dev_temp(&mut entries);
        let value = &entries[0].1;
        assert!(!value.to_ascii_lowercase().contains("target\\tmp"), "{value}");
        assert!(value.to_ascii_lowercase().ends_with("\\temp"), "{value}");
    }
}
