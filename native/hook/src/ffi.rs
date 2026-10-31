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
use std::sync::{Mutex, OnceLock};

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

/// 本 DLL 的模块基址（`DllMain` 的 `hinst`）。
///
/// 必须记下来：**`GetModuleFileNameW(NULL)` 返回的是宿主 exe 的路径，不是本 DLL 的**。
/// 回退读 `version.json` 时要按「本 DLL 所在目录」找，那才是实例目录。
static SELF_MODULE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// 目录重定向 hook 的安装结果（供 worker 在日志就绪后补写）。
///
/// 为什么要有这批计数器：`install_hooks` 在 `DllMain` 里同步执行，那时日志文件
/// **还没打开**，那一段「替换了几个槽 / 一个都没命中」是整个隔离最关键的诊断信息，
/// 却只会进 `OutputDebugString` —— 也就是等于没有。计数记在这里，由 worker 在
/// `log::init` 之后原样补写一遍，保证用户能查。
static HOOK_MODULES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static HOOK_HITS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static HOOK_REPLACED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// 已写进日志的命中项条数（重扫时只打印增量）。
static HOOK_REPORTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// 已替换的导入项名（`dll!function`），日志补写时逐条打印。
static HOOKED_NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// 扫到的全部导入模块名；只在「一个槽都没命中」时打进日志，供下次改 `REQUESTS` 用。
static HOOK_SCAN_MODULES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// 每个重定向函数是否已记录过「首次被游戏调用」。
///
/// 有了它才能回答那个唯一有意义的问题：**游戏进程自己真的走了我们给的路径吗**。
/// 「导入槽替换成功」只证明指针换掉了，「游戏调用被改写」才证明隔离真的在生效 ——
/// 而这两件事在纯 IAT 视角下长得一模一样。
static FIRST_CALL_SEEN: [AtomicBool; 3] = [
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
];

/// `SHGetKnownFolderPath` 是否被游戏调用过（无论 FOLDERID）。
static SH_CALLS_SEEN: AtomicBool = AtomicBool::new(false);

/// 下标约定：与 [`REQUESTS`] 中按函数名归并后的顺序一致。
const IDX_SH_GET_KNOWN_FOLDER_PATH: usize = 0;
const IDX_GET_TEMP_PATH_A: usize = 1;
const IDX_GET_TEMP_PATH_W: usize = 2;

/// 是否把补丁扩散到**依赖模块**的导入表。
///
/// `false` 与 LeviLauncher 一致：只改主映像。实机 A/B 显示，把 `GetTempPath*`
/// 也改到实例目录后，游戏与 TokenBroker / Xbox 应用这类**跨进程**助手通过临时
/// 文件握手时对不上，Xbox 登录永远拿不到 token。依赖模块上的目录调用本来也
/// 会先经过主映像的同一个函数指针，多数情况不需要逐个模块改。
const PATCH_DEPENDENCY_MODULES: bool = false;


/// 记录「游戏首次调用经由本 DLL 重定向的函数」，只记一次。
fn note_first_call(index: usize, api: &str, handed_out: &str) {
    if FIRST_CALL_SEEN[index].swap(true, Ordering::AcqRel) {
        return;
    }
    log::line(
        "INFO",
        &format!("游戏首次调用 {api}，已重定向为：{handed_out}"),
    );
}

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
            SELF_MODULE.store(instance as *mut c_void, Ordering::Release);
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
    //
    //    必须带 `GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS`：只传 PIN 而 lpModuleName
    //    为 NULL 时，`GetModuleHandleExW` 解析的是**宿主 exe**（主映像），
    //    被 pin 的是游戏本体而不是我们 —— hook 依然是「没 pin」。
    let mut self_module = ptr::null_mut();
    let flags = sys::GET_MODULE_HANDLE_EX_FLAG_PIN | sys::GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS;
    let anchor = worker_main as *const () as *const u16;
    if unsafe { sys::GetModuleHandleExW(flags, anchor, &mut self_module) } != 0
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

    // 4) hook：主映像已在 `DllMain` 装好；此刻把进程内**其余已加载模块**也扫一遍。
    //    实机验证表明游戏的真实调用点在依赖模块里，只改主映像会静默失效。
    install_hooks();
    let modules = if PATCH_DEPENDENCY_MODULES {
        install_hooks_all_modules()
    } else {
        0
    };
    HOOK_MODULES.fetch_add(modules, Ordering::AcqRel);
    log::line("INFO", &format!("首轮模块扫描：{modules} 个模块（含主映像）"));
    // 补写安装结论：`install_hooks` 多半是在 `DllMain` 里跑的，那时日志文件
    // 还不存在，那几行只会进 OutputDebugString。
    flush_hook_report();


    // 5) 懒加载的模块也要覆盖：XAML / PlayFab 之类依赖是后面才起来的，
    //    目录 API 的真实调用点很可能住在里面。线程本来就常驻，多跑一个节拍循环
    //    不额外占资源。
    let _ = std::thread::Builder::new()
        .name("copper-hook-rescan".into())
        .spawn(rescan_loop);

    // 5) 目录骨架：temp 必须真实存在，否则游戏会退到系统 temp
    for dir in plan.directories() {
        if let Err(error) = std::fs::create_dir_all(dir) {
            log::line("WARN", &format!("创建 {} 失败: {error}", dir.display()));
        }
    }

    // 6) 可选：加载 LeviLamina 客户端链的 PreLoader。
    //
    // PeEditor 原本把 PreLoader.dll 写进 exe 导入表来强制加载；这里改由
    // hook 在远程注入后、游戏主线程恢复前手动 LoadLibrary，exe 保持原版。
    load_preloader(&plan);

    // 7) 预加载
    let manifest_path = paths::manifest_path(&plan, &env_reader());
    run_preload(&plan, &manifest_path);
    0
}

/// 加载实例目录下的 `PreLoader.dll`（仅 `COPPER_HOOK_LOAD_PRELOADER` 设置时）。
fn load_preloader(plan: &RedirectPlan) {
    if std::env::var_os("COPPER_HOOK_LOAD_PRELOADER").is_none() {
        return;
    }
    use std::os::windows::ffi::OsStrExt;
    let dll = plan.root.join("PreLoader.dll");
    if !dll.is_file() {
        log::line("WARN", &format!("PreLoader 不存在: {}", dll.display()));
        return;
    }
    let wide: Vec<u16> = dll
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let module = unsafe {
        sys::LoadLibraryExW(wide.as_ptr(), ptr::null_mut(), 0)
    };
    if module.is_null() {
        log::line(
            "ERROR",
            &format!(
                "PreLoader 加载失败: {}（Win32 {}）",
                dll.display(),
                std::io::Error::last_os_error()
            ),
        );
    } else {
        log::line("INFO", &format!("PreLoader 已加载: {}", dll.display()));
    }

    // PreLoader 只提供符号解析，不主动拉 LeviLamina；客户端链的入口是
    // `mods\LeviLamina\LeviLamina.dll`。这里在 PreLoader 之后手动加载它。
    let levilamina = plan
        .root
        .join("mods")
        .join("LeviLamina")
        .join("LeviLamina.dll");
    if levilamina.is_file() {
        let wide: Vec<u16> = levilamina
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let module = unsafe { sys::LoadLibraryExW(wide.as_ptr(), ptr::null_mut(), 0) };
        if module.is_null() {
            log::line(
                "ERROR",
                &format!(
                    "LeviLamina 加载失败: {}（Win32 {}）",
                    levilamina.display(),
                    std::io::Error::last_os_error()
                ),
            );
        } else {
            log::line("INFO", &format!("LeviLamina 已加载: {}", levilamina.display()));
        }
    }
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

/// 本 DLL 所在目录（按本 DLL 自己的模块句柄取路径）。
///
/// 传 `NULL` 给 `GetModuleFileNameW` 拿到的是**宿主 exe** 的路径 —— 两者碰巧同目录
/// 时看不出差别，但回退逻辑必须锚在「DLL 实际被加载的位置」上：一旦启动器把
/// hook DLL 放到别处（例如放进 `mods/` 由加载器再拉起），读 `version.json` 就会
/// 去错目录找，静默退化成「不隔离」。
fn self_dir() -> PathBuf {
    let mut buffer = [0u16; 260];
    let module = SELF_MODULE.load(Ordering::Acquire);
    // SAFETY: `module` 是 `DllMain` 记下的本 DLL 句柄（进程未卸载前一直有效）；
    // `buffer` 长度与 `n_size` 一致。
    let length = unsafe { sys::GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) };
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
/// `GetTempPath*` 必须同时列出 **kernel32 / kernelbase 与游戏自己写的 api-set 名**：
/// 真实 GDK 游戏（1.26.52.03）的导入表里这两个函数是
/// `api-ms-win-core-file-l1-2-2.dll!GetTempPathA` 与
/// `api-ms-win-core-file-l1-2-0.dll!GetTempPathW`，**并没有**从 kernel32 导入。
/// 只按 `kernel32.dll` / `kernelbase.dll` 匹配的话，三项里只会命中一项，
/// 表现为「`%APPDATA%` 隔离生效、`%TEMP%` 漏到系统目录」——
/// 后者恰恰是让用户觉得「隔离没完全生效」的那一半。
///
/// api-set 名由**游戏的导入表**决定，不随 Windows 版本变化，因此可以写死；
/// 万一将来换了名字，`[flush_hook_report]` 在一个槽都没命中时会把扫到的全部
/// 模块名打进日志，那一行就是下一次的答案。
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
    ImportRequest {
        module: "api-ms-win-core-file-l1-2-2.dll",
        function: "GetTempPathA",
    },
    ImportRequest {
        module: "api-ms-win-core-file-l1-2-0.dll",
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
///
/// `DllMain` 里只装**主映像**：那是最早的时机，且此时只有一次「游戏可能已经查过
/// 目录」的窗口。依赖模块的扫描放在工作线程（[`install_hooks_all_modules`]），
/// 因为 `CreateToolhelp32Snapshot` 要枚举模块列表，不该在加载器锁内做。
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
    apply_scan(&scan);
    HOOK_MODULES.store(scan.modules.len(), Ordering::Release);
    HOOK_HITS.store(scan.hits.len(), Ordering::Release);
}


/// 把进程内**所有已加载模块**的导入表都扫一遍并替换。
///
/// 主映像之外必须一起覆盖：实机验证显示游戏真正调用目录 API 的位置在依赖模块里，
/// 只改主映像会「替换 3 个槽但一次也不生效」。
///
/// # 为什么放在工作线程
///
/// `DllMain` 里持有加载器锁，而模块枚举要读 PEB 的加载器链表 —— 那种「锁内回头看
/// 自己的链表」的用法没有必要冒险。工作线程里做，慢的那几毫秒只影响依赖模块上的
/// 重定向，主映像上的重定向早已在 `DllMain` 装好。
fn install_hooks_all_modules() -> usize {
    let self_module = SELF_MODULE.load(Ordering::Acquire) as usize;
    let snapshot = unsafe { sys::CreateToolhelp32Snapshot(sys::TH32CS_SNAPMODULE, 0) };
    if snapshot == sys::INVALID_HANDLE_VALUE || snapshot.is_null() {
        log::line("WARN", "枚举已加载模块失败：依赖模块上的重定向未生效");
        return 0;
    }
    let mut entry = sys::MODULEENTRY32W {
        dw_size: std::mem::size_of::<sys::MODULEENTRY32W>() as u32,
        th32_module_id: 0,
        th32_process_id: 0,
        glblcnt_usage: 0,
        proccnt_usage: 0,
        mod_base_addr: ptr::null_mut(),
        mod_base_size: 0,
        h_module: ptr::null_mut(),
        sz_module: [0; 256],
        sz_exe_path: [0; 260],
    };
    let mut modules = 0usize;
    let mut ok = unsafe { sys::Module32FirstW(snapshot, &mut entry) } != 0;
    while ok {
        let base = entry.mod_base_addr;
        modules += 1;
        // 跳过自己：本 DLL 若也改自己的 IAT，`log` / `std::fs` 内部的
        // `GetTempPath*` 会被绕进重定向函数，属于自己打自己。
        if !base.is_null() && base as usize != self_module {
            // SAFETY: `base` 来自 ToolHelp 快照，是本进程已加载映像的基址；
            // `mod_base_size` 是同一快照给出的映像字节数，作为读穿护栏。
            let scan = unsafe {
                iat::scan_module_imports(base, entry.mod_base_size as usize, REQUESTS)
            };
            apply_scan(&scan);
        }
        ok = unsafe { sys::Module32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { sys::CloseHandle(snapshot) };
    modules
}

/// 周期性重扫：把**之后**才加载的模块也覆盖上。
///
/// 一次全量扫描只解决「此刻已加载」的模块，而游戏的 XAML / PlayFab 之类依赖
/// 是懒加载的 —— 目录 API 的真实调用点很可能就住在后面才起来的模块里。
/// 重扫靠 [`mark_slot_once`] 去重，已经换过的槽不会再写一次，所以可以放心地
/// 按固定节拍跑；代价只是每 10 秒一次 `CreateToolhelp32Snapshot`。
fn rescan_loop() {
    const RESCAN_INTERVAL_MS: u32 = 10_000;
    loop {
        // `Sleep` 在工作线程里等，不占 CPU；本 DLL 已 pin，线程活到进程结束。
        unsafe { sys::Sleep(RESCAN_INTERVAL_MS) };
        let modules = if PATCH_DEPENDENCY_MODULES {
            install_hooks_all_modules()
        } else {
            0
        };
        if modules == 0 {
            // 枚举失败通常意味着快照被抢占了；重试即可，不必退出循环。
            continue;
        }
        HOOK_MODULES.fetch_add(modules, Ordering::AcqRel);
        flush_hook_report();
    }
}

/// 替换一批导入槽。
///
/// # 为什么按「槽当前值」判断，而不是一次性去重
///
/// 早先按槽地址做一次性去重，埋了一个静默失效的真故障：`DllMain` 阶段装好的
/// **主映像**槽，会被**后加载的 DLL** 覆写回去 —— exe 静态导入的别家注入 DLL
/// 就是在他自己的 `DllMain` 里改同一批槽，而那发生在我们之后。去重之后重扫再也
/// 不会夺回，日志里「替换 N 个槽」一切正常，游戏却一次都没走进我们的函数。
///
/// 现在只看槽里是不是我们的函数：不是就写回。代价是每轮重扫多几次
/// `VirtualProtect`，收益是「被覆写后自愈」。
fn apply_scan(scan: &iat::ScanResult) {
    let mut replaced = 0usize;
    for hit in &scan.hits {
        let hook = hook_for(hit.request);
        if hook.is_null() {
            continue;
        }
        // SAFETY: 槽来自本次扫描，指向本进程某个模块的 IAT 项，可读。
        let current = unsafe { *hit.slot.slot };
        if current == hook {
            continue;
        }
        // 保存槽原值：hook 内部靠它透传未重定向的调用（绕开已打补丁的 IAT，
        // 避免无限递归）。同一函数可能被多个槽命中，先写者为准。
        remember_original(hit.request, hit.slot.original);
        // SAFETY: 槽来自本次扫描，进程有写权限。
        if unsafe { iat::install(hit.slot, hook) } {
            replaced += 1;
            // 日志只报一次：重扫每 10 秒跑一轮，每次都报会把日志刷成噪声。
            if announce_slot(hit.slot.slot as usize) {
                if let Ok(mut names) = HOOKED_NAMES.lock() {
                    names.push(format!(
                        "{}!{}",
                        REQUESTS[hit.request].module, REQUESTS[hit.request].function
                    ));
                }
            }
        } else {
            log::line("WARN", "替换导入槽失败（VirtualProtect 被拒）");
        }
    }
    if replaced > 0 {
        HOOK_REPLACED.fetch_add(replaced, Ordering::AcqRel);
        HOOK_HITS.fetch_add(scan.hits.len(), Ordering::AcqRel);
        if let Ok(mut slot) = HOOK_SCAN_MODULES.lock() {
            for module in &scan.modules {
                slot.push(module.clone());
            }
        }
    }
}

/// 该槽是否首次成功替换；只用于**日志**去重，不参与替换决策。
fn announce_slot(address: usize) -> bool {
    static ANNOUNCED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    match ANNOUNCED.lock() {
        Ok(mut seen) if !seen.contains(&address) => {
            seen.push(address);
            true
        }
        _ => false,
    }
}

/// 把 hook 安装结果补写进日志文件（在 `log::init` 之后调用）。
///
/// 幂等且**只追加增量**：重扫循环每 10 秒调一次，若每次都把全部命中项重打一遍，
/// 一次游戏会话就能写掉几 MB 日志，把排障材料本身变成噪声。所以已打印过的条目
/// 记在 `REPORTED` 里，后续只报新增。
fn flush_hook_report() {
    let modules = HOOK_MODULES.load(Ordering::Acquire);
    let hits = HOOK_HITS.load(Ordering::Acquire);
    let replaced = HOOK_REPLACED.load(Ordering::Acquire);
    let hooked: Vec<String> = HOOKED_NAMES
        .lock()
        .map(|names| names.clone())
        .unwrap_or_default();
    let printed = HOOK_REPORTED.load(Ordering::Acquire);
    if hooked.len() > printed {
        for name in &hooked[printed..] {
            log::line("INFO", &format!("已重定向 {name}"));
        }
        HOOK_REPORTED.store(hooked.len(), Ordering::Release);
        log::line(
            if replaced == 0 { "ERROR" } else { "INFO" },
            &format!(
                "目录重定向 hook：替换 {replaced} 个导入槽（命中 {hits}）；累计扫描模块数 {modules}"
            ),
        );
    }
    if replaced == 0 {
        // 一个槽都没命中时，把扫到的模块名全打出来：下一次改 `REQUESTS`
        // 需要的答案就在这一行里，不必再手工 dump 一次导入表。
        let scanned: Vec<String> = HOOK_SCAN_MODULES
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default();
        log::line(
            "WARN",
            &format!(
                "未命中任何目标导入槽：本次运行不隔离。主映像导入模块：{}",
                scanned.join(", ")
            ),
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
        if !redirect_disabled() && guid_eq(rfid, &sys::FOLDERID_ROAMING_APP_DATA) {
            let roaming = plan.root.to_string_lossy().into_owned();
            note_first_call(
                IDX_SH_GET_KNOWN_FOLDER_PATH,
                "SHGetKnownFolderPath(RoamingAppData)",
                &roaming,
            );
            return allocate_path(&roaming, output);
        }
        // 只查我们不关心的 FOLDERID 也记一笔：用来区分「游戏根本没调这个 API」
        // 与「调了、但用的不是 Roaming/Local」——这两者在纯 IAT 视角下不可分辨。
        if !SH_CALLS_SEEN.swap(true, Ordering::AcqRel) {
            let id = unsafe { rfid.as_ref() }
                .map(|guid| format!("{:08X}-{:04X}-{:04X}", guid.data1, guid.data2, guid.data3))
                .unwrap_or_else(|| "<null>".to_string());
            log::line(
                "INFO",
                &format!("游戏调用了 SHGetKnownFolderPath(FOLDERID {{{id}}})，未重定向"),
            );
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
    if !redirect_disabled() {
    if let Some(plan) = PLAN.get().cloned().flatten() {
        let value = temp_with_separator(&plan);
        note_first_call(IDX_GET_TEMP_PATH_W, "GetTempPathW", &value);
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
    if !redirect_disabled() {
    if let Some(plan) = PLAN.get().cloned().flatten() {
        let value = temp_with_separator(&plan);
        note_first_call(IDX_GET_TEMP_PATH_A, "GetTempPathA", &value);
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
fn redirect_disabled() -> bool {
    // 仅用于二分定位：设置后 hook 仍然加载，但所有目录 API 都返回真实路径。
    std::env::var_os("COPPER_HOOK_NO_REDIRECT").is_some()
}

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
        // 与 LL 一致：temp 指向版本目录本身（< 1.26 才是数据目录），不再有 "temp" 子目录
        assert_eq!(value, format!("{}\\", plan.temp.display()));
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