//! 主映像导入表（IAT）遍历与槽位替换（仅 Windows）。
//!
//! # 为什么只改 IAT
//!
//! 目标 API（`SHGetKnownFolderPath` / `GetTempPathA` / `GetTempPathW`）都由主映像
//! **静态导入**，因此它们的地址在 IAT 里有槽位。把槽位换成我们的函数，游戏
//! 每次调用就走重定向。delay-load 与 `GetProcAddress` 动态解析抓不到 ——
//! 这三个 API 目前靠 IAT 够用（LeviLauncher 的隔离是出货功能即证明），
//! 但它被列为实机验证项（docs §2.9）。
//!
//! # 为什么保存 IAT 槽原值而不是靠 `GetProcAddress`
//!
//! 槽里存的就是**加载器解析出来的真实地址**：一定有效，且就是游戏原本会调到的
//! 那一个（对 forwarder 链也是最终地址）。`GetModuleHandleW` + `GetProcAddress`
//! 是次优方案（可能拿到链上另一个模块的转发目标），这里只在槽原值为空时用它兜底。
//!
//! # 只改内存，不写文件
//!
//! 写盘是启动器侧 PE 注入的职责（`home/inject.rs`）。本模块只在**当前进程**的
//! 地址空间里换指针，游戏退出即消失。
//!
//! # 为什么按字节偏移读头，而不是声明整个结构体
//!
//! `IMAGE_OPTIONAL_HEADER64` 有 240 字节，本模块只用 5 个字段。声明整个结构体
//! 的话，字段顺序写错一位就是「静默读到别的字段」，而且没有任何编译期检查。
//! 偏移集中成常量并逐条注明来源（`winnt.h`），配合下方单测钉死。

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use crate::sys;

// ── PE 头字段偏移（`winnt.h`，相对各自基址）───────────────────────────────
/// `e_lfanew` 在 `IMAGE_DOS_HEADER` 中的偏移。
const DOS_E_LFANEW: usize = 0x3C;
/// `IMAGE_FILE_HEADER.Machine` 相对 NT 头的偏移。
const NT_MACHINE: usize = 4;
/// `IMAGE_FILE_HEADER.SizeOfOptionalHeader` 相对 NT 头的偏移。
const NT_SIZE_OF_OPTIONAL: usize = 4 + 16;
/// `IMAGE_OPTIONAL_HEADER64` 相对 NT 头的偏移（Signature 4 + FileHeader 20 中未被
/// `SizeOfOptionalHeader` 覆盖部分已计入）。
const NT_OPTIONAL64: usize = 4 + 20;
/// `OptionalHeader.Magic`。
const OPT_MAGIC: usize = 0x00;
/// `OptionalHeader.Subsystem`。
const OPT_SUBSYSTEM: usize = 0x44;
/// `OptionalHeader.NumberOfRvaAndSizes`。
const OPT_NUMBER_OF_RVA: usize = 0x6C;
/// `OptionalHeader.DataDirectory[0]`。
const OPT_DATA_DIRECTORY: usize = 0x70;
/// `OptionalHeader.SizeOfImage`。
const OPT_SIZE_OF_IMAGE: usize = 0x38;
/// `DataDirectory` 每项字节数。
const DATA_DIRECTORY_SIZE: usize = 8;
/// `IMAGE_DIRECTORY_ENTRY_IMPORT` 的下标。
const DIRECTORY_ENTRY_IMPORT: usize = 1;
/// `IMAGE_IMPORT_DESCRIPTOR` 字节数。
const IMPORT_DESCRIPTOR_SIZE: usize = 20;
/// `IMAGE_IMPORT_DATA_DESCRIPTOR` 中 `OriginalFirstThunk` 的偏移。
const IMPORT_OFT: usize = 0;
/// `IMAGE_IMPORT_DATA_DESCRIPTOR` 中 `Name` 的偏移。
const IMPORT_NAME: usize = 12;
/// `IMAGE_IMPORT_DATA_DESCRIPTOR` 中 `FirstThunk` 的偏移。
const IMPORT_FIRST_THUNK: usize = 16;
/// x64 thunk 大小。
#[allow(dead_code)] // 仅单测钉死；遍历用的是 `*const u64` 的步长
const THUNK64_SIZE: usize = 8;
/// 序号导入标记（`IMAGE_SNAP_BY_ORDINAL64`）。
const ORDINAL64_FLAG: u64 = 1 << 63;
/// `IMAGE_ORDINAL64(u32)`：低 16 位是序号。
const ORDINAL64_MASK: u64 = 0xFFFF;

const DOS_SIGNATURE: u16 = 0x5A4D; // 'MZ'
const PE_SIGNATURE: u32 = 0x0000_4550; // 'PE\0\0'
const PE32_PLUS_MAGIC: u16 = 0x20B;
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const IMAGE_SUBSYSTEM_WINDOWS_GUI: u16 = 2;
const IMAGE_SUBSYSTEM_WINDOWS_CUI: u16 = 3;
/// 字符串读取上限：dll 名超过它只可能是内存被破坏。
const MAX_NAME: usize = 512;

/// 一个待替换的导入槽。
#[derive(Debug, Clone, Copy)]
pub struct IatSlot {
    /// 槽地址（进程内绝对地址）。
    pub slot: *mut *mut c_void,
    /// 槽里当前的值 —— 也就是原函数地址。
    pub original: *mut c_void,
}

/// 一个待替换的导入项。
#[derive(Debug, Clone, Copy)]
pub struct ImportRequest<'a> {
    /// 导入的 dll 名（小写；扫描结果统一小写化后等值比较）。
    pub module: &'a str,
    /// 导入的函数名（大小写敏感，与 PE 里的原样一致）。
    pub function: &'a str,
}

/// 一个命中的导入槽（带它在请求表里的下标，便于调用方分派对应实现）。
#[derive(Debug, Clone, Copy)]
pub struct IatHit {
    /// 命中的是 `ImportRequest` 里的第几项。
    pub request: usize,
    /// 槽地址与槽原值。
    pub slot: IatSlot,
}

/// 扫描结果。
#[derive(Debug, Default)]
pub struct ScanResult {
    /// 命中的槽。
    pub hits: Vec<IatHit>,
    /// 出现过的导入模块名（小写），用于诊断「目标 dll 根本没被导入」。
    pub modules: Vec<String>,
}

/// 从镜像基址读 NT 头偏移；头不合法返回 `None`。
///
/// # SAFETY
/// `base` 必须是已加载映像的基址，且 `[base, base + 偏移]` 可读。
unsafe fn nt_offset(base: *mut u8) -> Option<usize> {
    if base.is_null() {
        return None;
    }
    if ptr::read_unaligned(base as *const u16) != DOS_SIGNATURE {
        return None;
    }
    let lfanew = ptr::read_unaligned((base as *const u8).add(DOS_E_LFANEW) as *const i32);
    // 合法 PE 的 `e_lfanew` 不会超过 4KB；越界说明这不是 PE 或已损坏，
    // 继续算地址只会读到不可映射的页。
    // 这里取更紧的 0xE00：NT 头 + OptionalHeader 共约 0x1D8 字节，必须整块
    // 落在第一页内，后面对这些字段的无保护读取才有依据。
    if !(0..=0xE00).contains(&lfanew) {
        return None;
    }
    let nt = (base as usize).wrapping_add(lfanew as usize);
    if ptr::read_unaligned(nt as *const u32) != PE_SIGNATURE {
        return None;
    }
    Some(nt - base as usize)
}

/// 判断镜像是否为 x64 PE32+。
///
/// # SAFETY
/// `base` 必须是已加载映像的基址。
pub unsafe fn is_x64_pe(base: *mut u8) -> bool {
    let Some(nt) = nt_offset(base) else {
        return false;
    };
    if ptr::read_unaligned((base as *const u8).add(nt + NT_MACHINE) as *const u16)
        != IMAGE_FILE_MACHINE_AMD64
    {
        return false;
    }
    ptr::read_unaligned((base as *const u8).add(nt + NT_OPTIONAL64 + OPT_MAGIC) as *const u16)
        == PE32_PLUS_MAGIC
}

/// 读取 PE 子系统（`IMAGE_SUBSYSTEM_*`）；读不到返回 `None`。
///
/// 启动器侧 `pe::set_subsystem` 的只读校验位（Q6「控制台模式」需要在注入后
/// 确认改动真的生效，且改坏时能一眼看出）。
///
/// # SAFETY
/// `base` 必须是已加载映像的基址。
pub unsafe fn subsystem(base: *mut u8) -> Option<u16> {
    let nt = nt_offset(base)?;
    let header = (base as *const u8).add(nt + NT_OPTIONAL64);
    if ptr::read_unaligned(header as *const u16) != PE32_PLUS_MAGIC {
        return None;
    }
    let value = ptr::read_unaligned(header.add(OPT_SUBSYSTEM) as *const u16);
    if value == IMAGE_SUBSYSTEM_WINDOWS_GUI || value == IMAGE_SUBSYSTEM_WINDOWS_CUI {
        Some(value)
    } else {
        None
    }
}

/// 遍历主映像导入表，收集匹配的槽。
///
/// # SAFETY
/// `base` 必须是已加载主映像的基址。
pub unsafe fn scan_main_imports(base: *mut u8, requests: &[ImportRequest<'_>]) -> ScanResult {
    scan_module_imports(base, 0, requests)
}

/// 遍历**任意已加载模块**的导入表，收集匹配的槽。
///
/// # 为什么要参数化到「任意模块」
///
/// 真实 GDK 游戏（1.26.52.03）在主映像里确实静态导入了这三个 API
/// （`shell32!SHGetKnownFolderPath`、`api-ms-win-core-file-l1-2-2!GetTempPathA`、
/// `api-ms-win-core-file-l1-2-0!GetTempPathW`），但实机跑 5 分钟也**一次都没有**
/// 经主映像调用过它们 —— 真正的调用点在 `Microsoft.UI.Xaml.dll` 这类依赖模块里。
/// 只改主映像的 IAT 在这种情况下等于「替换成功但从不生效」，而这恰恰是静默失效
/// 最难被发现的一种形态。
///
/// # 为什么每个指针读都要过一遍 `in_image`
///
/// 依赖模块不像主映像那样「311MB、随便读」。某个 DLL 的 `hint+name` RVA 落���节尾时，
/// 直接按 512 字节扫下去就会跨进未映射页 —— 访问违例发生在**注入侧的工作线程**上，
/// 表现是整个游戏在启动瞬间崩掉，而日志里连一行都来不及写。
/// `module_limit`（`SizeOfImage` 与调用方给的 `size_hint` 取小）就是这一层的护栏：
/// 宁可漏掉一个模块，也不允许读穿。
///
/// # SAFETY
/// `base` 必须是某个已加载映像的基址；`size_hint` 是调用方知道的映像字节数
/// （0 表示完全信任映像自报的 `SizeOfImage`）。
pub unsafe fn scan_module_imports(
    base: *mut u8,
    size_hint: usize,
    requests: &[ImportRequest<'_>],
) -> ScanResult {
    let mut result = ScanResult::default();
    if base.is_null() {
        return result;
    }
    let Some(nt) = nt_offset(base) else {
        return result;
    };
    if ptr::read_unaligned((base as *const u8).add(nt + NT_MACHINE) as *const u16)
        != IMAGE_FILE_MACHINE_AMD64
    {
        return result;
    }
    let size_of_optional =
        ptr::read_unaligned((base as *const u8).add(nt + NT_SIZE_OF_OPTIONAL) as *const u16);
    if (size_of_optional as usize) < 0xF0 {
        // PE32+ 的 OptionalHeader 至少 240 字节（0xF0）；更短说明这是 PE32
        // 或头被破坏，字段偏移全部不可信。
        return result;
    }
    let header = (base as *const u8).add(nt + NT_OPTIONAL64);
    let size_of_image =
        ptr::read_unaligned(header.add(OPT_SIZE_OF_IMAGE) as *const u32) as usize;
    // 映像自报的 `SizeOfImage` 是主依据，调用方的快照值是交叉校验的上界。
    let limit_size = if size_hint == 0 {
        size_of_image
    } else {
        size_of_image.min(size_hint)
    };
    if limit_size == 0 || size_of_optional as usize + NT_OPTIONAL64 > limit_size {
        return result;
    }
    let number_of_rva =
        ptr::read_unaligned(header.add(OPT_NUMBER_OF_RVA) as *const u32) as usize;
    if number_of_rva <= DIRECTORY_ENTRY_IMPORT {
        return result;
    }
    let directory = header.add(OPT_DATA_DIRECTORY + DIRECTORY_ENTRY_IMPORT * DATA_DIRECTORY_SIZE);
    let virtual_address = ptr::read_unaligned(directory as *const u32);
    let size = ptr::read_unaligned(directory.add(4) as *const u32) as usize;
    if virtual_address == 0 || size < IMPORT_DESCRIPTOR_SIZE {
        return result;
    }
    // 描述符表整体必须在映像内：`size` 是加载器给的，可能大于实际映射范围。
    if !in_image(base, limit_size, virtual_address as usize, size.min(limit_size)) {
        return result;
    }

    let mut cursor = (base as *const u8).add(virtual_address as usize);
    let mut descriptor_rva = virtual_address as usize;
    // 描述符表以「全零项」结尾；`size` 只是上界。两层边界都要：损坏镜像里
    // `size` 可能谎报，只靠 size 会读穿。
    let limit = size / IMPORT_DESCRIPTOR_SIZE;
    for _ in 0..limit.max(1) {
        if !in_image(base, limit_size, descriptor_rva, IMPORT_DESCRIPTOR_SIZE) {
            break;
        }
        let descriptor = cursor;
        let oft = ptr::read_unaligned(descriptor.add(IMPORT_OFT) as *const u32);
        let name_rva = ptr::read_unaligned(descriptor.add(IMPORT_NAME) as *const u32);
        let first_thunk = ptr::read_unaligned(descriptor.add(IMPORT_FIRST_THUNK) as *const u32);
        if oft == 0 && name_rva == 0 && first_thunk == 0 {
            break;
        }
        if let Some(module) = read_ascii(base, limit_size, name_rva) {
            let lowered = module.to_ascii_lowercase();
            result.modules.push(lowered.clone());
            for (index, request) in requests.iter().enumerate() {
                if request.module != lowered {
                    continue;
                }
                if let Some(slot) =
                    find_thunk_slot(base, limit_size, oft, first_thunk, &lowered, request.function)
                {
                    result.hits.push(IatHit { request: index, slot });
                }
            }
        }
        cursor = cursor.add(IMPORT_DESCRIPTOR_SIZE);
        descriptor_rva = descriptor_rva.wrapping_add(IMPORT_DESCRIPTOR_SIZE);
    }
    result
}

/// `rva .. rva + need` 是否整段落在映像内（`rva` 是**相对基址**的偏移）。
///
/// # SAFETY
/// `base` 必须是已加载映像基址；`size` 是该映像的字节数。
unsafe fn in_image(_base: *mut u8, size: usize, rva: usize, need: usize) -> bool {
    if need == 0 || rva >= size {
        return false;
    }
    match rva.checked_add(need) {
        Some(end) => end <= size,
        None => false,
    }
}

/// 在一个导入描述符内定位函数名对应的 IAT 槽。
///
/// # SAFETY
/// `base` 必须是合法映像基址；`oft` / `first_thunk` 必须来自 [`scan_module_imports`]。
unsafe fn find_thunk_slot(
    base: *mut u8,
    size: usize,
    oft: u32,
    first_thunk: u32,
    module: &str,
    function: &str,
) -> Option<IatSlot> {
    let iat_rva = first_thunk as usize;
    if oft != 0 {
        // 首选导入名表：它保存 hint+name，与加载器解析无关，永远可读。
        let names_rva = oft as usize;
        let mut index = 0usize;
        while in_image(base, size, names_rva, (index + 1) * THUNK64_SIZE) {
            let thunk = ptr::read_unaligned((base as *const u8).add(names_rva + index * THUNK64_SIZE) as *const u64);
            if thunk == 0 {
                return None;
            }
            if thunk & ORDINAL64_FLAG == 0 {
                let hint_name_rva = (thunk as u32) as usize + 2; // 跳过 2 字节 hint
                if read_ascii(base, size, hint_name_rva as u32).as_deref() == Some(function) {
                    // 槽本身也必须在映像内才可读写。
                    if !in_image(base, size, iat_rva, (index + 1) * THUNK64_SIZE) {
                        return None;
                    }
                    let slot = (base as *const u8).add(iat_rva + index * THUNK64_SIZE) as *mut *mut c_void;
                    let original = ptr::read_unaligned(slot);
                    if original.is_null() {
                        return None;
                    }
                    return Some(IatSlot { slot, original });
                }
            }
            index += 1;
            if index > MAX_NAME {
                return None;
            }
        }
        return None;
    }

    // 没有导入名表（bound import 风格）：改为「槽原值是否等于运行时解析出的
    // 导出地址」。这是唯一可行的匹配方式。
    let wide = module_to_wide(module);
    let handle = sys::GetModuleHandleW(wide.as_ptr());
    if handle.is_null() {
        return None;
    }
    let target = resolve_export(handle, function);
    if target.is_null() {
        return None;
    }
    let mut index = 0usize;
    while in_image(base, size, iat_rva, (index + 1) * THUNK64_SIZE) {
        let value = ptr::read_unaligned(
            (base as *const u8).add(iat_rva + index * THUNK64_SIZE) as *const *mut c_void,
        );
        if value.is_null() {
            return None;
        }
        if value == target {
            return Some(IatSlot {
                slot: (base as *const u8).add(iat_rva + index * THUNK64_SIZE) as *mut *mut c_void,
                original: value,
            });
        }
        index += 1;
        if index > MAX_NAME {
            return None;
        }
    }
    None
}
/// 读一个以 NUL 结尾的 ASCII 字符串（RVA → 进程内地址）。
///
/// 扫描长度按「到映像末尾还剩多少」收敛：`MAX_NAME` 只是防畸形读穿的最后一道，
/// 真正防读穿的是 `size`。依赖模块的节尾往往紧贴映射边界，按固定长度读会跨页。
///
/// # SAFETY
/// `base` 必须是合法映像基址；`rva` 必须落在该映像内。
unsafe fn read_ascii(base: *mut u8, size: usize, rva: u32) -> Option<String> {
    if rva == 0 || (rva as usize) >= size {
        return None;
    }
    let remaining = size - rva as usize;
    let cap = remaining.min(MAX_NAME);
    let start = (base as *const u8).add(rva as usize);
    let mut length = 0usize;
    while length < cap {
        let byte = *start.add(length);
        if byte == 0 {
            break;
        }
        if !(0x20..0x7F).contains(&byte) {
            return None;
        }
        length += 1;
    }
    if length == 0 || length >= MAX_NAME {
        return None;
    }
    Some(String::from_utf8_lossy(std::slice::from_raw_parts(start, length)).into_owned())
}

fn module_to_wide(module: &str) -> Vec<u16> {
    module.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 动态解析一个导出（仅无 OFT 的兜底分支使用）。
///
/// # SAFETY
/// `module` 必须是有效模块句柄。
unsafe fn resolve_export(module: *mut c_void, function: &str) -> *mut c_void {
    let mut name: Vec<u8> = function.as_bytes().to_vec();
    name.push(0);
    sys::GetProcAddress(module, name.as_ptr())
}

/// 序号导入的序号（仅诊断用；本模块不按序号匹配）。
///
/// # SAFETY
/// 仅在调试断言里对已校验的 thunk 调用。
#[allow(dead_code)]
unsafe fn ordinal_of(thunk: u64) -> u16 {
    (thunk & ORDINAL64_MASK) as u16
}

/// 把一个槽换成新实现（`VirtualProtect` + 原子交换）。
///
/// # SAFETY
/// `slot` 必须来自 [`scan_main_imports`]，且进程仍有写权限。
pub unsafe fn install(slot: IatSlot, replacement: *mut c_void) -> bool {
    if slot.original == replacement || slot.slot.is_null() {
        return false;
    }
    let address = slot.slot as *mut c_void;
    let size = std::mem::size_of::<*mut c_void>();
    let mut previous = 0u32;
    if sys::VirtualProtect(address, size, sys::PAGE_READWRITE, &mut previous) == 0 {
        return false;
    }
    // 必须把槽**本身**当成 `AtomicPtr` 来交换。
    //
    // 曾经这里写的是 `AtomicPtr::new(slot.original).swap(replacement, ..)` ——
    // 那是把原值装进一个**栈上的临时变量**再交换，槽里的字节一个都没动，而函数
    // 依然返回 true。症状极其隐蔽：日志里「替换 N 个导入槽」全部正常，游戏却
    // 一次都没走进我们的函数。改成对槽地址重新借用之后才真正生效。
    //
    // SAFETY: 槽是一个指针宽度的进程内可写位置（上面已经把它改成 PAGE_READWRITE）；
    // `AtomicPtr` 只是借用它的对齐原子交换能力，不拥有那块内存。
    let target = &*(slot.slot as *const AtomicPtr<c_void>);
    target.swap(replacement, Ordering::AcqRel);
    let mut ignored = 0u32;
    sys::VirtualProtect(address, size, previous, &mut ignored);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `install` 必须真的改写槽内存 —— 这一条曾经是假绿灯的直接证据。
    ///
    /// 老实现 `AtomicPtr::new(slot.original).swap(...)` 只交换了一个栈上临时量，
    /// 槽一个字节都没变，函数却返回 true。没有这条测试，整条隔离链路就是
    /// 「日志全绿、功能全无」。
    #[test]
    fn install_really_writes_the_slot() {
        let mut slot_value: *mut c_void = 0x1000usize as *mut c_void;
        let slot = IatSlot {
            slot: &mut slot_value as *mut *mut c_void,
            original: slot_value,
        };
        let replacement = 0x2000usize as *mut c_void;
        // SAFETY: 槽指向本函数栈上的一个指针，`install` 只做对齐原子交换。
        let installed = unsafe { install(slot, replacement) };
        assert!(installed, "install 应报告成功");
        assert_eq!(
            slot_value, replacement,
            "install 必须把新函数地址真正写进槽里"
        );

        // 已经是新实现时不重复安装。
        let again = IatSlot {
            slot: &mut slot_value as *mut *mut c_void,
            original: slot_value,
        };
        // SAFETY: 同上。
        assert!(!unsafe { install(again, replacement) });
    }

    #[test]
    fn offsets_match_winnt() {
        // 这些数字错一位就是「静默读到别的字段」，因此逐条钉死。
        assert_eq!(DOS_E_LFANEW, 0x3C);
        assert_eq!(NT_MACHINE, 4);
        assert_eq!(NT_OPTIONAL64, 24);
        assert_eq!(OPT_MAGIC, 0x00);
        assert_eq!(OPT_SUBSYSTEM, 0x44);
        assert_eq!(OPT_NUMBER_OF_RVA, 0x6C);
        assert_eq!(OPT_DATA_DIRECTORY, 0x70);
        assert_eq!(IMPORT_OFT, 0);
        assert_eq!(IMPORT_NAME, 12);
        assert_eq!(IMPORT_FIRST_THUNK, 16);
        assert_eq!(THUNK64_SIZE, 8);
        assert_eq!(ORDINAL64_FLAG, 1 << 63);
        // PE32+ OptionalHeader 固定 240 字节
        assert_eq!(0xF0, OPT_DATA_DIRECTORY + 16 * DATA_DIRECTORY_SIZE);
    }

    #[test]
    fn subsystem_constants_are_the_documented_pair() {
        assert_eq!(IMAGE_SUBSYSTEM_WINDOWS_GUI, 2);
        assert_eq!(IMAGE_SUBSYSTEM_WINDOWS_CUI, 3);
    }

    #[test]
    fn null_and_garbage_bases_are_rejected_without_reading_far() {
        // SAFETY: 传入的不是映像基址；函数必须在触碰前就返回。
        let mut garbage = [0u8; 8];
        let base = garbage.as_mut_ptr();
        unsafe {
            assert!(!is_x64_pe(ptr::null_mut()));
            assert!(!is_x64_pe(base));
            assert!(nt_offset(ptr::null_mut()).is_none());
            assert!(subsystem(base).is_none());
            let requests = [ImportRequest {
                module: "shell32.dll",
                function: "SHGetKnownFolderPath",
            }];
            let scan = scan_main_imports(base, &requests);
            assert!(scan.hits.is_empty());
            assert!(scan.modules.is_empty());
        }
    }

    #[test]
    fn ascii_reader_rejects_non_ascii_and_overlong() {
        // 缓冲区必须真的够 `read_ascii` 扫完，否则测试本身就是越界读。
        let mut buffer = vec![0u8; MAX_NAME + 8];
        // 名称放在非零 rva 上：`rva == 0` 是 NULL 指针，`read_ascii` 按设计直接拒绝，
        // 用它当「基址」测会得到一个与本用例无关的 None。
        const NAME_RVA: u32 = 8;
        buffer[NAME_RVA as usize..NAME_RVA as usize + 3].copy_from_slice(b"abc");
        let size = buffer.len();
        let base = buffer.as_mut_ptr();
        unsafe {
            assert!(read_ascii(base, size, 0).is_none(), "NULL 指针必须直接拒绝");
            // rva 落在映像之外：直接拒绝，不去碰内存
            assert!(read_ascii(base, size, size as u32).is_none());
            assert_eq!(
                read_ascii(base, size, NAME_RVA).as_deref(),
                Some("abc"),
                "合法名称必须读出来"
            );
            assert!(read_ascii(base, size, NAME_RVA + 3).is_none(), "空串必须拒绝");
            // 0x80 不可打印 → 拒绝
            buffer[NAME_RVA as usize + 2] = 0x80;
            assert!(read_ascii(base, size, NAME_RVA).is_none());
            // 超长（无 NUL）必须拒绝，而不是读穿缓冲区
            buffer[NAME_RVA as usize + 2] = b'x';
            buffer[NAME_RVA as usize..NAME_RVA as usize + MAX_NAME].fill(b'x');
            assert!(read_ascii(base, size, NAME_RVA).is_none());
        }
    }

    /// 边界检查本身也必须有测试：扫描任意模块时一次读穿就是整个游戏崩掉。
    #[test]
    fn bounds_check_rejects_out_of_image_rvas() {
        let mut buffer = vec![0u8; 0x400];
        let base = buffer.as_mut_ptr();
        let size = buffer.len();
        unsafe {
            assert!(in_image(base, size, 0, 1));
            assert!(in_image(base, size, size - 1, 1));
            assert!(!in_image(base, size, size, 1));
            assert!(!in_image(base, size, size - 1, 2));
            assert!(!in_image(base, size, 0, 0), "空区间不算在内");
            assert!(!in_image(base, size, usize::MAX, 1), "不得溢出回绕");
            // 冒充映像的垃圾缓冲区不能被当成可扫描的 PE
            let scan = scan_module_imports(base, size, &[]);
            assert!(scan.hits.is_empty());
            assert!(scan.modules.is_empty());
        }
    }
}