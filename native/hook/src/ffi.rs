//! Windows 注入侧入口：`DllMain`、目录重定向 hook、工作线程（仅 Windows）。
//!
//! # 生命周期
//!
//! ```text
//! 进程加载本 DLL（游戏按导入表拉起）
//!  └─ DllMain(DLL_PROCESS_ATTACH)
//!      ├─ DisableThreadLibraryCalls            （不再收到线程通知）
//!      ├─ 宿主 exe 名白名单检查                 （非游戏进程立即返回）
//!      ├─ 读交接环境变量 → 算出重定向目标        （无文件 I/O）
//!      ├─ 立刻装 IAT hook                        （只在内存里换指针）
//!      └─ CreateThread → worker                 （loader lock 内不做重活）
//!
//! worker
//!  ├─ 模块 pin                               （防止游戏运行期卸载 → 悬空指针）
//!  ├─ 无交接变量时回退读 version.json，再补装 hook
//!  ├─ 初始化日志 <版本目录>/copper-hook.log
//!  ├─ 创建数据目录 + temp
//!  └─ 读 copper-preload.json → 逐条 LoadLibraryExW
//! ```
//!
//! # 为什么 hook 在 `DllMain` 里同步装、预加载在工作线程里
//!
//! docs §2.9 要求「hook 必须在游戏首次查询目录**之前**装好」，否则前面的调用
//! 已经把数据写进真实目录，隔离出现「半截」数据。工作线程的启动延迟是毫秒级，
//! 而游戏初始化恰好就在那之后几毫秒 —— 赌它赢不了。
//!
//! IAT hook 本身**不碰 loader lock**：只做 `VirtualProtect` + 一个原子写，
//! 不加载任何 DLL、不等待任何对象，因此可以在 `DllMain` 内安全执行。
//! 反过来 `LoadLibrary` 绝对不行（docs §2.9 第一条约束），所以预加载一律挪到
//! 工作线程。
//!
//! # 跨 FFI 边界的纪律
//!
//! - 本文件**不出现 `unwrap` / `expect` / 显式 panic**：宿主是游戏，一次 panic
//!   就是整个游戏崩掉，而这份崩溃对用户毫无信息量。
//! - 所有 Win32 调用失败都只记日志并保持原行为（`installed = false` 时透传
//!   原函数），失败不改变「游戏能启动」这件事。
//! - 递归防护：hook 内部调用原函数用的是**装载前保存的槽原值**，绕开已打补丁的
//!   IAT，因此不会无限递归爆栈。

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::OnceLock;

use crate::contract::{
    HOST_EXE_NAMES, HOOK_ABI_VERSION, HOOK_LOG_FILE_NAME, HOOK_DLL_FILE_NAME,
};
use crate::iat::{self, ImportRequest};
use crate::log;
use crate::meta;
use crate::paths::{self, Channel, RedirectPlan};
use crate::preload;
use crate::sys;

/// 生效中的重定向方案。`None` = 不隔离（本进程按官方路径跑）。
static PLAN: OnceLock<Option<RedirectPlan>> = OnceLock::new();

/// 已安装的 hook 标记：保证「只装一次」，也防止二次进入 `DllMain` 时重复换指针。
static HOOKS_INSTALLED: AtomicBool = AtomicBool::new(false);

/// 工作线程已启动标记。
static WORKER_STARTED: AtomicBool = AtomicBool::new(false);

// 原函数地址（装载前从 IAT 槽里取得）
static REAL_SH_GET_KNOWN_FOLDER_PATH: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static REAL_GET_TEMP_PATH_A: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static REAL_GET_TEMP_PATH_W: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// 导入表锚点：PE 导入表需要一个真实导出符号作为 thunk 指向。
///
/// 函数体故意什么都不做 —— 游戏不会调用它，它存在的唯一意义是让
/// `add_import` 有个可写的 thunk 目标（见 docs §2.5）。
#[no_mangle]
pub extern "C" fn CopperCoreHookAnchor() -> u32 {
    HOOK_ABI_VERSION
}

// ── 入口 ────────────────────────────────────────────────────────────────

/// DLL 入口。
///
/// # SAFETY
/// 只能由 Windows 加载器在 `DllMain` 语义下调用；参数由加载器提供。
#[no_mangle]
pub unsafe extern "system" fn DllMain(
    instance: *mut c_void,
    reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    match reason {
        // DLL_PROCESS_ATTACH
        1 => {
            // 不需要线程 attach/detach 通知：省掉每个游戏线程的两次回调。
            sys::DisableThreadLibraryCalls(instance);
            if !host_is_game() {
                // 非白名单宿主：连日志都不初始化（可能连文件都不该碰）。
                return 1;
            }
            // 交接变量能给出方案时就同步装 hook（无文件 I/O，loader lock 安全）。
            if let Some(plan) = paths::plan_from_env(None, &env_reader()) {
                let _ = PLAN.set(Some(plan));
                install_hooks();
            }
            start_worker();
            1
        }
        _ => 1,
    }
}

/// 当前进程的可执行文件名（小写）。
fn host_image_name() -> String {
    // 260 = MAX_PATH。游戏路径不会超过它；超长的实例目录属于异常，交由白名单
    // 判定失败处理（拿不到名字 = 不注入），不会误伤。
    let mut buffer = [0u16; 260];
    // SAFETY: 主映像句柄传 NULL 表示「当前进程」；`buffer` 长度与 `nSize` 一致。
    let length = unsafe { sys::GetModuleFileNameW(ptr::null_mut(), buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        return String::new();
    }
    // SAFETY: `GetModuleFileNameW` 以 NUL 结尾写入，最多写 `length` 个字符。
    let full = unsafe { std::slice::from_raw_parts(buffer.as_ptr(), length as usize) };
    let full = String::from_utf16_lossy(full);
    Path::new(&full)
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// 宿主是否在白名单内。
fn host_is_game() -> bool {
    let name = host_image_name();
    if HOST_EXE_NAMES.contains(&name.as_str()) {
        return true;
    }
    log::line("INFO", &format!("宿主不是受支持的 Minecraft 宿主，不注入: {name}"));
    false
}

/// 环境变量读取器（`std::env::var`）。
fn env_reader() -> impl Fn(&str) -> Option<String> + 'static {
    |key: &str| std::env::var(key).ok()
}

/// 工作线程入口（`CreateThread` 的线程函数）。
extern "system" fn worker_main() -> u32 {
    // 1) 模块 pin：不 pin 的话游戏运行期任何一次 FreeLibrary 都会让重定向
    //    指针悬空。加载器锁此刻已释放，pin 是安全的。
    let mut self_module = ptr::null_mut();
    if unsafe { sys::GetModuleHandleExW(
        sys::GET_MODULE_HANDLE_EX_FLAG_PIN,
        ptr::null(),
        &mut self_module,
    ) } != 0
    {
        log::line("INFO", "模块已 pin");
    } else {
        log::line("WARN", "模块 pin 失败：游戏运行期卸载本 DLL 会导致悬空指针");
    }

    // 2) 交接优先；缺失则回退读同目录 version.json（非本启动器启动的实例）
    let plan = match paths::plan_from_env(None, &env_reader()) {
        Some(plan) => Some(plan),
        None => match meta::plan_from_file(&self_dir()) {
            Ok(plan) => plan,
            Err(error) => {
                log::line("WARN", &format!("回退读取 version.json 失败: {error}"));
                None
            }
        },
    };
    // 若 DllMain 已同步装过 hook，这里补装的是「回退路径」的情形。
    let plan = match (PLAN.get().and_then(|slot| slot.clone()), plan) {
        (_, Some(plan)) => {
            let _ = PLAN.set(Some(plan.clone()));
            plan
        }
        (Some(existing), None) => existing,
        (None, None) => {
            log::line("WARN", "无交接变量且无可用 version.json：本次运行不隔离");
            return 0;
        }
    };

    // 3) 日志必须在「读/写实例目录」之前就绪，否则第一次报错无处可查。
    log::init(&plan.root.join(HOOK_LOG_FILE_NAME));
    log::line("INFO", &format!("注入侧就绪，abi={HOOK_ABI_VERSION}"));
    log::line(
        "INFO",
        &format!(
            "重定向目标：%APPDATA% -> {}，%TEMP% -> {}，数据目录 -> {}",
            plan.root.display(),
            plan.temp.display(),
            plan.data.display()
        ),
    );

    // 4) hook 若还没装（回退路径），此刻补装。晚于目录查询的风险由日志暴露。
    install_hooks();

    // 5) 目录骨架：temp 必须真实存在，否则游戏会退到系统 temp
    for dir in plan.directories() {
        if let Err(error) = std::fs::create_dir_all(dir) {
            log::line("WARN", &format!("创建 {} 失败: {error}", dir.display()));
        }
    }

    // 6) 预加载
    let manifest_path = paths::manifest_path(&plan, &env_reader());
    run_preload(&plan, &manifest_path);
    0
}

/// 读清单并执行预加载；任何一步失败都只记日志。
fn run_preload(plan: &RedirectPlan, manifest_path: &Path) {
    let bytes = match std::fs::read(manifest_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            log::line("INFO", "无预加载清单，本实例不加载任何原生模组");
            return;
        }
        Err(error) => {
            log::line("WARN", &format!("读取 {} 失败: {error}", manifest_path.display()));
            return;
        }
    };
    match crate::manifest::PreloadManifest::parse(&bytes) {
        Ok(manifest) => {
            preload::run(plan, &manifest);
        }
        Err(error) => log::line("WARN", &format!("预加载清单不可用: {error}")),
    }
}

/// 启动工作线程（只启动一次）。
fn start_worker() {
    if WORKER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: `worker_main` 是 `extern "system" fn() -> u32`，符合线程函数签名；
    // 它不依赖 `DllMain` 的栈。
    let handle = unsafe {
        sys::CreateThread(
            ptr::null_mut(),
            0,
            worker_main,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
        )
    };
    if handle.is_null() {
        // 线程没起来：hook 仍已在 DllMain 装好，隔离基本能力保留，只是没有
        // 预加载与日志。必须让用户看得见这件事。
        log::line("ERROR", "工作线程创建失败：注入侧只剩目录重定向，无预加载与日志");
        // SAFETY: `handle` 为空时不该调用 CloseHandle；此处已判空。
        return;
    }
    // 立即关闭句柄：线程不因句柄关闭而终止。
    // SAFETY: `handle` 是刚创建的有效句柄，且无其他使用者。
    unsafe { sys::CloseHandle(handle) };
}

/// 本 DLL 所在目录（`GetModuleHandleExW(NULL)` 得到自身模块，再取路径）。
fn self_dir() -> PathBuf {
    let mut buffer = [0u16; 260];
    // SAFETY: 传 NULL 取当前 exe 路径（游戏 exe 目录 = 实例目录，与注入位置一致）。
    let length = unsafe { sys::GetModuleFileNameW(ptr::null_mut(), buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        return PathBuf::from(".");
    }
    // SAFETY: `GetModuleFileNameW` 以 NUL 结尾写入。
    let full = unsafe { std::slice::from_raw_parts(buffer.as_ptr(), length as usize) };
    Path::new(&String::from_utf16_lossy(full))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 需要 hook 的导入项。顺序即 [`dispatch_slots`] 的下标约定。
///
/// `GetTempPath*` 同时列出 kernel32 与 kernelbase：同一导出可能被任一模块
/// 转发，主映像导入哪个就 patch 哪个，两个都列上才不会漏。
const REQUESTS: &[ImportRequest<'static>] = &[
    ImportRequest {
        module: "shell32.dll",
        function: "SHGetKnownFolderPath",
    },
    ImportRequest {
        module: "kernel32.dll",
        function: "GetTempPathA",
    },
    ImportRequest {
        module: "kernel32.dll",
        function: "GetTempPathW",
    },
    ImportRequest {
        module: "kernelbase.dll",
        function: "GetTempPathA",
    },
    ImportRequest {
        module: "kernelbase.dll",
        function: "GetTempPathW",
    },
];

/// [`REQUESTS`] 下标对应的 hook 实现。
fn hook_for(request: usize) -> *mut c_void {
    match REQUESTS[request].function {
        "SHGetKnownFolderPath" => hook_sh_get_known_folder_path as *mut c_void,
        "GetTempPathA" => hook_get_temp_path_a as *mut c_void,
        "GetTempPathW" => hook_get_temp_path_w as *mut c_void,
        _ => ptr::null_mut(),
    }
}

/// 安装 IAT hook（幂等）。
fn install_hooks() {
    if HOOKS_INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    let base = unsafe { sys::GetModuleHandleW(ptr::null()) };
    if base.is_null() {
        log::line("ERROR", "取主映像基址失败");
        return;
    }
    // SAFETY: `base` 是当前进程主映像基址，导入表在进程内可读。
    let scan = unsafe { iat::scan_main_imports(base as *mut u8, REQUESTS) };

    let mut replaced = 0usize;
    for hit in &scan.hits {
        let hook = hook_for(hit.request);
        if hook.is_null() {
            continue;
        }
        // 保存槽原值：hook 内部靠它透传未重定向的调用（绕开已打补丁的 IAT，
        // 避免无限递归）。同一函数可能被多个槽命中，先写者为准。
        remember_original(hit.request, hit.slot.original);
        // SAFETY: 槽来自本次扫描，进程有写权限。
        if unsafe { iat::install(hit.slot, hook) } {
            replaced += 1;
            log::line(
                "INFO",
                &format!(
                    "已重定向 {}!{}",
                    REQUESTS[hit.request].module,
                    REQUESTS[hit.request].function
                ),
            );
        } else {
            log::line("WARN", "替换导入槽失败（VirtualProtect 被拒）");
        }
    }

    log::line(
        if replaced == 0 { "ERROR" } else { "INFO" },
        &format!(
            "目录重定向 hook：替换 {replaced} 个导入槽；主映像导入模块数 {}",
            scan.modules.len()
        ),
    );
    if replaced == 0 {
        log::line(
            "WARN",
            "未命中任何目标导入槽：本次运行不隔离（游戏若经 GetProcAddress 动态解析则抓不到）",
        );
    }
}

/// 记录某个 hook 的原函数地址（首个命中的槽为准）。
fn remember_original(request: usize, original: *mut c_void) {
    let slot = match REQUESTS[request].function {
        "SHGetKnownFolderPath" => &REAL_SH_GET_KNOWN_FOLDER_PATH,
        "GetTempPathA" => &REAL_GET_TEMP_PATH_A,
        "GetTempPathW" => &REAL_GET_TEMP_PATH_W,
        _ => return,
    };
    let _ = slot.compare_exchange(ptr::null_mut(), original, Ordering::AcqRel, Ordering::Acquire);
}

// ── 重定向函数 ──────────────────────────────────────────────────────────

/// `CoTaskMemAlloc` + 拷贝（按调用方约定由 `CoTaskMemFree` 释放）。
///
/// # SAFETY
/// `output` 必须是有效的 `PWSTR*`。
unsafe fn allocate_path(value: &str, output: *mut *mut u16) -> sys::HRESULT {
    if output.is_null() {
        return -2147024809; // E_INVALIDARG
    }
    *output = ptr::null_mut();
    let units: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = units.len() * std::mem::size_of::<u16>();
    // SAFETY: `bytes` 为非零长度；失败时返回 NULL，标准做法。
    let buffer = sys::CoTaskMemAlloc(bytes) as *mut u16;
    if buffer.is_null() {
        return -2147024882; // E_OUTOFMEMORY
    }
    ptr::copy_nonoverlapping(units.as_ptr(), buffer, units.len());
    *output = buffer;
    sys::S_OK
}

/// `SHGetKnownFolderPath` 的重定向。
///
/// # SAFETY
/// 签名与 `SHGetKnownFolderPath` 一致；`rfid` 与 `output` 由调用方（游戏）提供。
unsafe extern "system" fn hook_sh_get_known_folder_path(
    rfid: *const sys::GUID,
    flags: u32,
    token: *mut c_void,
    output: *mut *mut u16,
) -> sys::HRESULT {
    if let Some(plan) = PLAN.get().and_then(|slot| slot.clone()) {
        if plan.local_empty && guid_eq(rfid, &sys::FOLDERID_LOCAL_APP_DATA) {
            return allocate_path("", output);
        }
        if guid_eq(rfid, &sys::FOLDERID_ROAMING_APP_DATA) {
            let roaming = plan.root.to_string_lossy().into_owned();
            return allocate_path(&roaming, output);
        }
    }
    // 透传原函数：用装载前保存的槽原值，绕开已打补丁的 IAT。
    let real = REAL_SH_GET_KNOWN_FOLDER_PATH.load(Ordering::Acquire);
    if real.is_null() {
        return -2147467262; // E_FAIL
    }
    let function: unsafe extern "system" fn(*const sys::GUID, u32, *mut c_void, *mut *mut u16) -> sys::HRESULT =
        std::mem::transmute(real);
    function(rfid, flags, token, output)
}

/// `GetTempPathW` 的重定向。
///
/// # SAFETY
/// 签名与 `GetTempPathW` 一致。
unsafe extern "system" fn hook_get_temp_path_w(capacity: u32, buffer: *mut u16) -> u32 {
    if let Some(plan) = PLAN.get().cloned().flatten() {
        let value = temp_with_separator(&plan);
        // Win32 语义：容量不足（含 0）时返回所需长度（含结尾 NUL）。
        let needed = value.len() as u32;
        if capacity == 0 || buffer.is_null() {
            if capacity != 0 {
                sys::SetLastError(sys::ERROR_INVALID_PARAMETER);
                return 0;
            }
            return needed;
        }
        if capacity < needed {
            sys::SetLastError(sys::ERROR_INSUFFICIENT_BUFFER);
            return needed;
        }
        ptr::copy_nonoverlapping(
            value.as_ptr() as *const u8,
            buffer as *mut u8,
            value.len() * std::mem::size_of::<u16>(),
        );
        return needed;
    }
    let real = REAL_GET_TEMP_PATH_W.load(Ordering::Acquire);
    if real.is_null() {
        sys::SetLastError(sys::ERROR_INVALID_PARAMETER);
        return 0;
    }
    let function: unsafe extern "system" fn(u32, *mut u16) -> u32 = std::mem::transmute(real);
    function(capacity, buffer)
}

/// `GetTempPathA` 的重定向。
///
/// # SAFETY
/// 签名与 `GetTempPathA` 一致。
unsafe extern "system" fn hook_get_temp_path_a(capacity: u32, buffer: *mut u8) -> u32 {
    if let Some(plan) = PLAN.get().cloned().flatten() {
        let value = temp_with_separator(&plan);
        // 绝不退回系统 temp 或做 `'?'` 替换：宁可失败，让调用方走它自己的
        // 回退分支（LeviLauncher 同款取舍，`folder_redirect.cpp:159`）。
        let Some(ansi) = sys::to_ansi(&value) else {
            sys::SetLastError(sys::ERROR_NO_UNICODE_TRANSLATION);
            return 0;
        };
        let needed = ansi.len() as u32 + 1;
        if capacity == 0 {
            return needed;
        }
        if buffer.is_null() {
            sys::SetLastError(sys::ERROR_INVALID_PARAMETER);
            return 0;
        }
        if capacity < needed {
            sys::SetLastError(sys::ERROR_INSUFFICIENT_BUFFER);
            return needed;
        }
        ptr::copy_nonoverlapping(ansi.as_ptr(), buffer, ansi.len());
        *buffer.add(ansi.len()) = 0;
        return needed;
    }
    let real = REAL_GET_TEMP_PATH_A.load(Ordering::Acquire);
    if real.is_null() {
        sys::SetLastError(sys::ERROR_INVALID_PARAMETER);
        return 0;
    }
    let function: unsafe extern "system" fn(u32, *mut u8) -> u32 = std::mem::transmute(real);
    function(capacity, buffer)
}

/// `%TEMP%` 的值：目录 + 结尾反斜杠。
///
/// Win32 契约要求 `GetTempPath*` 返回**带结尾分隔符**的路径，否则调用方
/// 直接拼文件名会得到 `C:\foo.tempbar`。
fn temp_with_separator(plan: &RedirectPlan) -> String {
    let mut value = plan.temp.to_string_lossy().into_owned();
    if !value.ends_with('\\') && !value.ends_with('/') {
        value.push('\\');
    }
    value
}

/// GUID 等值比较（`IsEqualGUID` 的语义）。
fn guid_eq(left: *const sys::GUID, right: &sys::GUID) -> bool {
    // SAFETY: 调用方保证 `left` 非空（Win32 契约要求 REFKNOWNFOLDERID 有效）。
    match unsafe { left.as_ref() } {
        Some(value) => value == right,
        None => false,
    }
}

/// 供诊断/测试读取「当前是否已隔离」。
pub fn isolation_active() -> bool {
    matches!(PLAN.get().and_then(|slot| slot.clone()), Some(plan) if !plan.root.as_os_str().is_empty())
}

/// 供诊断读取生效的方案（未初始化时 `None`）。
pub fn active_plan() -> Option<RedirectPlan> {
    PLAN.get().and_then(|slot| slot.clone())
}

/// 供诊断读取注入侧日志路径（未初始化时 `None`）。
pub fn log_path() -> Option<PathBuf> {
    active_plan().map(|plan| plan.root.join(HOOK_LOG_FILE_NAME))
}

/// 本 DLL 的文件名（供日志与内核核对落位）。
pub fn dll_file_name() -> &'static str {
    HOOK_DLL_FILE_NAME
}

/// 渠道名（日志用）。
pub fn channel_label(channel: Channel) -> &'static str {
    preload::channel_label(channel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_value_always_ends_with_separator() {
        let plan = crate::paths::plan_for(
            Path::new("/v/demo"),
            Channel::Release,
            "1.21.130.20",
        );
        let value = temp_with_separator(&plan);
        assert!(value.ends_with('\\'), "{value} 缺少结尾分隔符");
        assert!(value.contains("temp"));
    }

    #[test]
    fn guid_comparison_matches_sdk_values() {
        // 两个常量一旦写错，隔离会「生效但写错地方」且无任何报错
        assert_ne!(sys::FOLDERID_ROAMING_APP_DATA, sys::FOLDERID_LOCAL_APP_DATA);
        assert!(guid_eq(
            &sys::FOLDERID_ROAMING_APP_DATA as *const sys::GUID,
            &sys::FOLDERID_ROAMING_APP_DATA
        ));
        assert!(!guid_eq(
            &sys::FOLDERID_ROAMING_APP_DATA as *const sys::GUID,
            &sys::FOLDERID_LOCAL_APP_DATA
        ));
        assert!(!guid_eq(std::ptr::null(), &sys::FOLDERID_ROAMING_APP_DATA));
    }

    #[test]
    fn anchor_export_is_callable_and_stable() {
        assert_eq!(copper_core_hook_anchor_value(), HOOK_ABI_VERSION);
    }

    fn copper_core_hook_anchor_value() -> u32 {
        // 直接调用导出的锚点：它在游戏里不会被调用，但内核实机验证会经导入表
        // 调到它，签名必须稳定。
        CopperCoreHookAnchor()
    }

    #[test]
    fn not_isolated_in_test_process() {
        // 测试进程不是游戏宿主，绝不能处于隔离态（否则测试会把自己的目录
        // 写进临时目录之外的路径）。
        assert!(!isolation_active());
        assert!(active_plan().is_none());
    }

    #[test]
    fn host_whitelist_rejects_test_host() {
        assert!(!host_is_game(), "测试宿主不是 Minecraft，必须被白名单拒绝");
    }
}