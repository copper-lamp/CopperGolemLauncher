//! 真实游戏启动探针：在真实实例目录上做「注入 / 不注入」对照并采集退出码。
//!
//! # 为什么 `pe::tests` 不够
//!
//! [`super::tests`] 能证明「Windows 自己的 PE 解析器认得我们重排后的镜像」
//! （`LoadLibraryExW` + `LOAD_LIBRARY_AS_DATAFILE` 会通过）。但那是**只当数据
//! 文件打开**：不解析导入、不执行入口点、不跑游戏逻辑。
//!
//! 「游戏愿不愿意带着这个改动启动」是另一个问题，而且失败形态非常安静：
//! 进程直接结束，`Application` 事件日志里没有任何条目，`copper-hook.log`
//! 也可能根本没被创建。只有把真实 exe 跑起来并读退出码，才能把两类原因分开：
//!
//! 1. **PE 改写本身不被接受**（映像被判定篡改，或加载器不接受我们的导入布局）；
//! 2. **hook 逻辑把游戏弄死**（路径重定向破坏了 GDK 的包身份/目录解析）。
//!
//! `inert` 模式正是为此设计：注入照做，但**不下发任何 `COPPER_HOOK_*`**，
//! 于是 [`copper_core_hook`] 的 `DllMain` 走 `plan_from_env` 拿不到方案，
//! 既不装 hook 也不起 worker —— DLL 在游戏里是完全惰性的。
//! `inert` 挂而 `original` 活，说明问题出在第 1 类。
//!
//! # 用法
//!
//! ```text
//! COPPER_PE_LAUNCH_DIR=<实例目录> \
//!   cargo test --lib pe::launch_probe -- --ignored --nocapture
//! ```
//!
//! 环境变量：
//!
//! | 变量 | 含义 |
//! |------|------|
//! | `COPPER_PE_LAUNCH_DIR`  | 必填，实例目录（含 `Minecraft.Windows.exe`） |
//! | `COPPER_PE_LAUNCH_MODE` | `original` / `inert` / `plan`，默认 `inert` |
//! | `COPPER_PE_LAUNCH_SECS` | 观察时长（秒），默认 20 |
//! | `COPPER_PE_HOOK_DLL`    | hook DLL 来源，默认为 embed 产物路径 |
//!
//! 探针会在退出前把原 exe 还原（含 panic 路径），并打印 `copper-hook.log`。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::Image;

/// 注入的 DLL 名（须与 [`copper_core_hook`] 实际投放的文件名一致）。
const HOOK_DLL: &str = "CopperCoreHook.dll";
/// 导入项名：必须是 DLL 真实导出的符号，否则加载器找不到入口。
const ANCHOR: &str = "CopperCoreHookAnchor";
/// 宿主可执行文件名。
const HOST_EXE: &str = "Minecraft.Windows.exe";
/// hook 日志文件名，与 `contract::HOOK_LOG_FILE_NAME` 一致。
const HOOK_LOG: &str = "copper-hook.log";
/// 对照组 DLL：与本方案一起放在应用目录里的系统 COM 组件。
const NEUTRAL_DLL: &str = "marshal.dll";
/// 对照组导入项：COM 服务器必然导出该符号。
const NEUTRAL_ANCHOR: &str = "DllGetClassObject";
/// 对照组 DLL：KnownDLL，永远从 System32 解析。
const SYS_DLL: &str = "KERNEL32.dll";
/// 对照组导入项：`kernel32` 必定导出的公开 API。
const SYS_ANCHOR: &str = "Sleep";

/// 探针模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// 基线：不注入、不下发交接变量。
    Original,
    /// 注入但不下发交接变量 —— DLL 应当完全惰性。
    Inert,
    /// 注入并下发完整交接变量，等价于启动器的真实启动路径。
    Plan,
    /// 对照组：注入一个**应用目录里本来就存在**的 COM DLL
    /// （`marshal.dll!DllGetClassObject`，COM 约定必然导出）。
    ///
    /// 它与本方案经历**完全相同**的 PE 改写流程，却与铜内核毫无关系：
    /// - `neutral` 活 → 改写后的镜像可被加载器正常解析，问题出在
    ///   `CopperCoreHook.dll` 本身（名字解析 / 依赖 / 时序）。
    /// - `neutral` 死 → 改写本身即致命，与注入的 DLL 是谁无关。
    Neutral,
    /// 对照组：注入 `KERNEL32.dll!Sleep` —— KnownDLL，必在 System32，
    /// 绝不依赖应用目录搜索。
    ///
    /// 与 `neutral` 合起来能把「加载器找不到应用目录里的 DLL」和
    /// 「我们写入的描述符整体不可读」分开：
    /// - `sysdll` 活、`neutral` 死 → 加载器压根不搜应用目录（新节里的名字它
    ///   读到了，但按规则找不到同名文件）。
    /// - `sysdll` 也死 → 问题在描述符/节布局本身。
    SysDll,
    /// 二分用：走完整改写流程，但把导入数据目录**指回原处**，让加载器根本不看
    /// 新追加的那一项。
    ///
    /// 它和 `neutral` 的差别只有「导入目录指向谁」，因此：
    /// - `sectiononly` 活 → 追加节 / 整体重排无害，问题出在导入目录这一项。
    /// - `sectiononly` 死 → 问题在重排本身（节表、SizeOfImage、文件偏移改写）。
    SectionOnly,
    /// 二分用：导入目录**指向新数组**，但把新增的那一项清零 —— 数组内容回到
    /// 与原始完全一致，只剩「位置换了」这一个变量。
    ///
    /// - `newdir_noextra` 活 → 新增描述符的内容有问题。
    /// - `newdir_noextra` 死 → 新数组所在的位置（RVA / 所在节）有问题。
    NewDirNoExtra,
    /// 只改写、不启动、**不还原**：把改写结果留在磁盘上，交给外部工具
    /// （dumpbin / 第三方解析器）独立评判。用完自行删除。
    PatchOnly,
}

impl Mode {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "original" => Some(Self::Original),
            "inert" => Some(Self::Inert),
            "plan" => Some(Self::Plan),
            "neutral" => Some(Self::Neutral),
            "sysdll" => Some(Self::SysDll),
            "sectiononly" => Some(Self::SectionOnly),
            "newdir_noextra" => Some(Self::NewDirNoExtra),
            "patchonly" => Some(Self::PatchOnly),
            _ => None,
        }
    }

    fn injected(self) -> bool {
        !matches!(self, Self::Original)
    }

    /// 是否要投放本方 hook DLL（对照组投放的是系统自带 DLL）。
    fn stages_hook_dll(self) -> bool {
        matches!(self, Self::Inert | Self::Plan)
    }

    fn handoff(self) -> bool {
        matches!(self, Self::Plan)
    }
}

/// 退出时把原 exe 还原回去。
///
/// 探针会就地改写真实游戏文件，任何提前返回或 panic 都必须留下完好的
/// 实例，所以还原放在 `Drop` 里而不是函数末尾。
struct Restore {
    exe: PathBuf,
    backup: PathBuf,
    armed: bool,
}

impl Drop for Restore {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Err(error) = std::fs::rename(&self.backup, &self.exe) {
            eprintln!("!! 还原原 exe 失败：{}（{error}）", self.exe.display());
            eprintln!("!! 原文件仍在：{}", self.backup.display());
        }
    }
}

#[test]
#[ignore = "会真的启动游戏；需 COPPER_PE_LAUNCH_DIR 指向实例目录"]
fn probe_game_startup() {
    let Ok(raw_dir) = std::env::var("COPPER_PE_LAUNCH_DIR") else {
        eprintln!("跳过真实启动探针：未设置 COPPER_PE_LAUNCH_DIR");
        return;
    };
    let mode = match std::env::var("COPPER_PE_LAUNCH_MODE")
        .unwrap_or_else(|_| "inert".to_string())
        .as_str()
    {
        raw => match Mode::parse(raw) {
            Some(mode) => mode,
            None => panic!(
                "COPPER_PE_LAUNCH_MODE 只接受 original / inert / plan / neutral / sysdll，收到 {raw}"
            ),
        },
    };
    let observe = Duration::from_secs(
        std::env::var("COPPER_PE_LAUNCH_SECS")
            .ok()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(20u64),
    );

    let dir = PathBuf::from(raw_dir);
    let exe = dir.join(HOST_EXE);
    assert!(exe.is_file(), "实例目录里没有 {HOST_EXE}：{}", exe.display());
    warn_if_instance_running();

    // 先备份，再做任何破坏性操作。
    let backup = dir.join(format!("{HOST_EXE}.probeorig"));
    let _ = std::fs::remove_file(&backup);
    std::fs::copy(&exe, &backup).expect("备份原 exe");
    let mut restore = Restore {
        exe: exe.clone(),
        backup: backup.clone(),
        armed: true,
    };
    if mode == Mode::PatchOnly {
        // 留下改写结果供外部工具检查，备份一并留下以便随时还原。
        restore.armed = false;
    }

    let log_path = dir.join(HOOK_LOG);
    let _ = std::fs::remove_file(&log_path);

    // 投放 hook DLL（仅需要本方 DLL 的模式）。
    let dll = dir.join(HOOK_DLL);
    let placed_dll = mode.stages_hook_dll() && stage_hook_dll(&dll);

    if mode == Mode::PatchOnly {
        let target = dir.join("patched.exe");
        let original = std::fs::read(&exe).expect("读取 exe");
        let image = Image::parse(original).expect("解析 exe");
        let patched = image
            .plan_add_import(HOOK_DLL, ANCHOR)
            .expect("计划注入")
            .apply(&image)
            .expect("应用注入");
        std::fs::write(&target, &patched).expect("写出 patched.exe");
        eprintln!("已写出 {}", target.display());
        return;
    }
    if mode.injected() {
        let (target_dll, anchor) = match mode {
            Mode::Neutral => (NEUTRAL_DLL, NEUTRAL_ANCHOR),
            Mode::SysDll => (SYS_DLL, SYS_ANCHOR),
            _ => (HOOK_DLL, ANCHOR),
        };
        match mode {
            Mode::SectionOnly => inject_section_only(&exe, target_dll, anchor),
            Mode::NewDirNoExtra => inject_new_dir_without_extra(&exe, target_dll, anchor),
            _ => inject_into(&exe, target_dll, anchor),
        }
    }

    let data_dir = dir.join("probe-data");
    let _ = std::fs::create_dir_all(&data_dir);

    let mut cmd = Command::new(&exe);
    cmd.current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if mode.handoff() {
        cmd.env("COPPER_HOOK_DATA_DIR", &data_dir)
            .env("COPPER_HOOK_ROOT", &dir)
            .env("COPPER_HOOK_CHANNEL", "release")
            .env("COPPER_HOOK_PRELOAD", dir.join("copper-preload.json"));
    } else {
        // 不能让宿主进程残留的交接变量混进来，否则 `inert` 名不副实。
        for key in [
            "COPPER_HOOK_DATA_DIR",
            "COPPER_HOOK_ROOT",
            "COPPER_HOOK_CHANNEL",
            "COPPER_HOOK_PRELOAD",
        ] {
            cmd.env_remove(key);
        }
    }

    eprintln!("== 启动探针 ==");
    eprintln!("模式      : {mode:?}（注入={} 交接={}）", mode.injected(), mode.handoff());
    eprintln!("实例目录  : {}", dir.display());
    eprintln!("观察时长  : {}s", observe.as_secs());

    let started = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => panic!("启动 {HOST_EXE} 失败：{error}"),
    };
    let pid = child.id();

    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => break format!("exited: {status}"),
            Ok(None) => {}
            Err(error) => break format!("try_wait 失败: {error}"),
        }
        if started.elapsed() >= observe {
            let _ = child.kill();
            let _ = child.wait();
            break "survived".to_string();
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let elapsed = started.elapsed();

    eprintln!("pid        : {pid}");
    eprintln!("结果       : {outcome}");
    eprintln!("耗时       : {:.2}s", elapsed.as_secs_f64());
    report_hook_log(&log_path);
    report_data_dir(&data_dir);

    if placed_dll {
        let _ = std::fs::remove_file(&dll);
    }
}

/// 投放 hook DLL；返回是否由本次探针投放（用于收尾时清理）。
fn stage_hook_dll(target: &Path) -> bool {
    if target.exists() {
        return false;
    }
    let source = match std::env::var("COPPER_PE_HOOK_DLL") {
        Ok(raw) => PathBuf::from(raw),
        Err(_) => Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../native/hook/target-embed/x86_64-pc-windows-msvc/debug/copper_core_hook.dll",
        ),
    };
    assert!(source.is_file(), "找不到 hook DLL：{}", source.display());
    std::fs::copy(&source, target).expect("投放 hook DLL");
    true
}

/// 就地注入：追加导入项 + 追加节。
fn inject_into(exe: &Path, dll: &str, anchor: &str) {
    let bytes = std::fs::read(exe).expect("读取 exe");
    let image = Image::parse(bytes).expect("解析 exe");
    match image.plan_add_import(dll, anchor) {
        Ok(plan) => {
            let patched = plan.apply(&image).expect("应用注入");
            std::fs::write(exe, &patched).expect("写回注入后的 exe");
        }
        // 已经被注入过也算成功，探针继续跑。
        Err(super::PeError::AlreadyImported(_)) => {}
        Err(error) => panic!("计划注入失败：{error}"),
    }
}

/// 二分用：完整走一遍改写，再把导入数据目录指回原处。
fn inject_section_only(exe: &Path, dll: &str, anchor: &str) {
    let bytes = std::fs::read(exe).expect("读取 exe");
    let image = Image::parse(bytes).expect("解析 exe");
    let original_dir = image.data_dir(DIR_IMPORT).expect("原始镜像应有导入目录");
    let mut patched = image
        .plan_add_import(dll, anchor)
        .expect("计划注入")
        .apply(&image)
        .expect("应用注入");
    // PE32+ 的数据目录从可选头 +112 开始，每项 8 字节。
    let pe_off = u32::from_le_bytes([patched[0x3C], patched[0x3D], patched[0x3E], patched[0x3F]]) as usize;
    let entry = pe_off + 24 + 112 + DIR_IMPORT * 8;
    patched[entry..entry + 4].copy_from_slice(&original_dir.0.to_le_bytes());
    patched[entry + 4..entry + 8].copy_from_slice(&original_dir.1.to_le_bytes());
    std::fs::write(exe, &patched).expect("写回注入后的 exe");
}

/// 二分用：导入目录指向新数组，但把新增描述符清零。
///
/// 清零后数组内容与原始逐字节相同，Size 也还原成原始值，于是与原始镜像的
/// 唯一差别就是「这串描述符放在哪个 RVA / 哪个节里」。
fn inject_new_dir_without_extra(exe: &Path, dll: &str, anchor: &str) {
    let bytes = std::fs::read(exe).expect("读取 exe");
    let image = Image::parse(bytes).expect("解析 exe");
    let mut patched = image
        .plan_add_import(dll, anchor)
        .expect("计划注入")
        .apply(&image)
        .expect("应用注入");
    // 新数组在 `.copperh` 节里，新增描述符是「原有数量」之后的第一项。
    let parsed = Image::parse(patched.clone()).expect("解析改写结果");
    let hook = parsed.hook_section().expect("应有追加节");
    let existing = parsed.imported_dlls().expect("读导入").len();
    let at = hook.pointer_to_raw_data as usize + existing * DESCRIPTOR_LEN;
    patched[at..at + DESCRIPTOR_LEN].fill(0);
    // Size 还原成原始值：此时新数组与原始数组内容一致。
    let pe_off = u32::from_le_bytes([patched[0x3C], patched[0x3D], patched[0x3E], patched[0x3F]]) as usize;
    let entry = pe_off + 24 + 112 + DIR_IMPORT * 8;
    let size = ((existing + 1) * DESCRIPTOR_LEN) as u32;
    patched[entry + 4..entry + 8].copy_from_slice(&size.to_le_bytes());
    std::fs::write(exe, &patched).expect("写回注入后的 exe");
}

/// GDK 游戏是单实例的：已有一个实例在跑的话，本次结果不作数。
fn warn_if_instance_running() {
    let running = Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {HOST_EXE}"), "/NH"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains(HOST_EXE))
        .unwrap_or(false);
    if running {
        eprintln!("!! 警告：检测到已在运行的 {HOST_EXE}，单实例互斥会让本次结果失真");
    }
}

/// 打印 hook 日志。这是判断「DLL 到底有没有跑起来」的第一手证据。
fn report_hook_log(path: &Path) {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            eprintln!("hook 日志  : 有（{} 字节）", text.len());
            for line in text.lines().take(60) {
                eprintln!("  | {line}");
            }
        }
        Err(_) => eprintln!("hook 日志  : 无 —— DLL 很可能没执行到写日志那一步"),
    }
}

/// 打印重定向数据目录的实际落点。
fn report_data_dir(path: &Path) {
    let mut count = 0usize;
    let mut deepest = PathBuf::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            count += 1;
            if entry.path().components().count() > deepest.components().count() {
                deepest = entry.path();
            }
            if entry.path().is_dir() {
                stack.push(entry.path());
            }
        }
    }
    eprintln!(
        "数据目录  : {}（{} 项，最深 {}）",
        path.display(),
        count,
        deepest.display()
    );
}

// ── 导入表原样转储 ──────────────────────────────────────────────────────
//
// 上面那组对照已经证明：换成系统自带的 `marshal.dll` 一样死，所以问题不在
// 「注入的是哪个 DLL」，而在**改写后的导入表加载器读不懂**。而我们自己的解析器
// 读得懂（`dll_is_imported` 返回 true），说明两边的理解不一致，只能把字节
// 摊开逐字段比对。

/// 导入表数据目录索引。
const DIR_IMPORT: usize = 1;

/// 导入描述符长度（5 × u32）。
const DESCRIPTOR_LEN: usize = 20;

/// 转储一份镜像的导入目录，逐字段打印每个描述符。
fn dump_import(label: &str, image: &Image) {
    eprintln!("== {label} 导入目录 ==");
    eprintln!(
        "  SizeOfHeaders=0x{:X} SizeOfImage=0x{:X} 节数={}",
        image.size_of_headers, image.size_of_image, image.number_of_sections
    );
    let Some((rva, size)) = image.data_dir(DIR_IMPORT) else {
        eprintln!("  <无导入目录>");
        return;
    };
    eprintln!("  RVA=0x{rva:08X} Size=0x{size:X}");
    let Some(mut offset) = image.rva_to_offset(rva) else {
        eprintln!("  <RVA 无法映射到文件偏移>");
        return;
    };
    let data = image.data();
    let mut index = 0usize;
    // 记下最后读到的描述符原始字节：终止符到底写没写、字段是什么，看字节最直接。
    let mut last_raw: Vec<u8> = Vec::new();
    loop {
        if offset + DESCRIPTOR_LEN > data.len() {
            eprintln!("  [{index}] <越界：描述符越过文件末尾>");
            return;
        }
        let raw = &data[offset..offset + DESCRIPTOR_LEN];
        last_raw = raw.to_vec();
        let fields: Vec<u32> = raw
            .chunks(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        if fields.iter().all(|value| *value == 0) {
            eprintln!("  [{index}] <全零描述符：数组到此终止>");
            break;
        }
        let name = match image.read_wide_string(fields[3]) {
            Some(name) => name,
            None => format!("<无法从 RVA 0x{:08X} 读出名称>", fields[3]),
        };
        eprintln!(
            "  [{index}] OFT=0x{:08X} TS=0x{:08X} FWD=0x{:08X} Name=0x{:08X}({name}) FT=0x{:08X}",
            fields[0], fields[1], fields[2], fields[3], fields[4]
        );
        index += 1;
        offset += DESCRIPTOR_LEN;
        if index > 4096 {
            eprintln!("  <超过 4096 个描述符仍未遇到终止符>");
            return;
        }
    }
    let declared = size as usize / DESCRIPTOR_LEN;
    eprintln!("  实际描述符数={index}，数据目录声明={declared}");
    if index + 1 != declared {
        eprintln!("  !! 终止符位置与数据目录 Size 不一致");
    }
    eprintln!("  末描述符原始字节：{}", hex(&last_raw));
}

/// 十六进制转储。
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

#[test]
#[ignore = "诊断用：需 COPPER_PE_FIXTURE 或 COPPER_PE_LAUNCH_DIR 指向真实 exe"]
fn dump_import_directory() {
    let path = match std::env::var("COPPER_PE_FIXTURE") {
        Ok(raw) => PathBuf::from(raw),
        Err(_) => PathBuf::from(std::env::var("COPPER_PE_LAUNCH_DIR").expect("设置 COPPER_PE_LAUNCH_DIR"))
            .join(HOST_EXE),
    };
    let original = std::fs::read(&path).expect("读取 exe");
    let image = Image::parse(original).expect("解析 exe");
    let patched = {
        let plan = image
            .plan_add_import(HOOK_DLL, ANCHOR)
            .expect("计划注入");
        plan.apply(&image).expect("应用注入")
    };
    let after = Image::parse(patched).expect("解析改写结果");
    dump_import("原始", &image);
    dump_import("改写后", &after);
    dump_sections("原始", &image);
    dump_sections("改写后", &after);
}

/// 转储节表，并核对每节在**文件里**的实际落点。
///
/// `import::tests` 与 `pe::tests` 都是从文件字节推断的，加载器却是从
/// `PointerToRawData` 映射出来的。两者只有在「指针真的指在那段字节上」时才
/// 等价，所以这里把每个节的原始字节直接打出来对账。
fn dump_sections(label: &str, image: &Image) {
    eprintln!("== {label} 节表（文件长度 {} 字节）==", image.len());
    let data = image.data();
    for section in &image.sections {
        eprintln!(
            "  {:9} RVA=0x{:08X} VSize=0x{:X} RawOff=0x{:X} RawSize=0x{:X} Chars=0x{:08X}",
            section.name,
            section.virtual_address,
            section.virtual_size,
            section.pointer_to_raw_data,
            section.size_of_raw_data,
            section.characteristics
        );
        let start = section.pointer_to_raw_data as usize;
        let end = start + section.size_of_raw_data as usize;
        if section.size_of_raw_data != 0 && end > data.len() {
            eprintln!("    !! 原始数据越过文件末尾");
            continue;
        }
        if section.size_of_raw_data != 0 {
            let head = &data[start..(start + 16).min(end)];
            eprintln!("    首 16 字节：{}", hex(head));
        }
    }
}