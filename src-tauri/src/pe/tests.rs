//! PE 模块测试：用**合成镜像**覆盖布局边界，不依赖真实游戏包。
//!
//! 合成的意义：真实 `Minecraft.Windows.exe` 动辄上百 MB、每次升级都变，
//! 不能当夹具；而这里要验的恰恰是「文件头只剩 N 字节」「节表塞满」这类
//! 极端布局 —— 自己造镜像才能精确构造出来。

use super::*;
use super::parse::MACHINE_AMD64;

/// 合成镜像的构造参数。
struct Fixture {
    data: Vec<u8>,
    #[allow(dead_code)]
    section_count: u16,
}

const FILE_ALIGNMENT: u32 = 0x200;
const SECTION_ALIGNMENT: u32 = 0x1000;

impl Fixture {
    /// 造一个带导入表（kernel32 + 一个导出）的 PE32+ 镜像。
    fn new(section_count: u16) -> Self {
        let opt_size: u16 = 240;
        let pe_offset: usize = 0x80;
        let coff = pe_offset + 4;
        let opt = coff + 20;
        let section_table = opt + opt_size as usize;
        let headers_end = section_table + section_count as usize * 40;
        // SizeOfHeaders 只保证装得下已有节表项：节表再加一项时**必须**扩张。
        let headers_size = (headers_end as u32).next_multiple_of(FILE_ALIGNMENT);

        // 导入表放在第 0 节里；第 0 节的 RVA 从 0x1000 起。
        let text_rva = SECTION_ALIGNMENT;
        // 布局：[描述符][空描述符][ILT][IAT][hint+name][dll 名][func 名]
        let mut import_blob: Vec<u8> = Vec::new();
        let desc_count_off = 0usize;
        let null_off = 20usize;
        let ilt_off = 40usize;
        let iat_off = 56usize;
        let hint_off = 72usize;
        let dll_name_off = 74usize;
        let func_name_off = 74 + ("kernel32.dll".encode_utf16().count() as usize + 1) * 2;
        let blob_len = func_name_off + ("CreateFileW".encode_utf16().count() as usize + 1) * 2;
        import_blob.resize(blob_len, 0);
        let put_u32 = |buf: &mut Vec<u8>, at: usize, v: u32| {
            buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };
        let put_wide = |buf: &mut Vec<u8>, at: usize, s: &str| {
            for (i, unit) in s.encode_utf16().enumerate() {
                buf[at + i * 2..at + i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
            }
        };
        put_u32(&mut import_blob, desc_count_off + 0, text_rva + ilt_off as u32);
        put_u32(&mut import_blob, desc_count_off + 12, text_rva + dll_name_off as u32);
        put_u32(&mut import_blob, desc_count_off + 16, text_rva + iat_off as u32);
        put_u32(&mut import_blob, ilt_off, text_rva + hint_off as u32);
        put_u32(&mut import_blob, iat_off, text_rva + hint_off as u32);
        put_wide(&mut import_blob, dll_name_off, "kernel32.dll");
        put_wide(&mut import_blob, func_name_off, "CreateFileW");
        let import_blob = import_blob;

        let mut data = vec![0u8; headers_size as usize];
        data[0..2].copy_from_slice(b"MZ");
        data[0x3c..0x40].copy_from_slice(&(pe_offset as u32).to_le_bytes());
        data[pe_offset..pe_offset + 4].copy_from_slice(b"PE\0\0");
        data[coff..coff + 2].copy_from_slice(&MACHINE_AMD64.to_le_bytes());
        data[coff + 2..coff + 4].copy_from_slice(&section_count.to_le_bytes());
        data[coff + 16..coff + 18].copy_from_slice(&opt_size.to_le_bytes());
        data[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        // SizeOfInitializedData(+8) / SizeOfImage(+56) / SizeOfHeaders(+60) /
        // CheckSum(+64) / Subsystem(+68) / SectionAlignment(+32) / FileAlignment(+36)
        data[opt + 8..opt + 12].copy_from_slice(&0x1000u32.to_le_bytes());
        data[opt + 32..opt + 36].copy_from_slice(&SECTION_ALIGNMENT.to_le_bytes());
        data[opt + 36..opt + 40].copy_from_slice(&FILE_ALIGNMENT.to_le_bytes());
        let mut image_size = SECTION_ALIGNMENT;
        // 调试目录 / 证书表 / Bound Import 全部留空
        data[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes());

        // 节表
        let mut cursor = headers_size;
        for index in 0..section_count as usize {
            let at = section_table + index * 40;
            let name = if index == 0 { ".text" } else { ".pad" };
            data[at..at + name.len()].copy_from_slice(name.as_bytes());
            let (vsize, blob) = if index == 0 {
                (blob_len as u32, Some(import_blob.as_slice()))
            } else {
                (0x10, None)
            };
            data[at + 8..at + 12].copy_from_slice(&vsize.to_le_bytes());
            data[at + 12..at + 16].copy_from_slice(&(text_rva + index as u32 * SECTION_ALIGNMENT).to_le_bytes());
            let raw_size = blob
                .map(|b| b.len().div_ceil(FILE_ALIGNMENT as usize) as u32 * FILE_ALIGNMENT)
                .unwrap_or(FILE_ALIGNMENT);
            data[at + 16..at + 20].copy_from_slice(&raw_size.to_le_bytes());
            // 第 0 节紧贴文件头：头部扩张时它会整体右移，因此夹具能覆盖扩张场景。
            let raw_offset = cursor;
            data[at + 20..at + 24].copy_from_slice(&raw_offset.to_le_bytes());
            data[at + 36..at + 40]
                .copy_from_slice(&0x4000_0040u32.to_le_bytes()); // CODE|EXECUTE|READ|INITIALIZED
            cursor = raw_offset + raw_size;
            if index == 0 {
                data.resize(raw_offset as usize + blob_len, 0);
                data[raw_offset as usize..raw_offset as usize + blob_len]
                    .copy_from_slice(&import_blob);
                // 补齐到 raw_size
                data.resize((raw_offset + raw_size) as usize, 0);
                cursor = raw_offset + raw_size;
                image_size = SECTION_ALIGNMENT + SECTION_ALIGNMENT;
            } else {
                data.resize((raw_offset + raw_size) as usize, 0);
                cursor = raw_offset + raw_size;
            }
        }
        data[opt + 56..opt + 60].copy_from_slice(&image_size.to_le_bytes());
        data[opt + 60..opt + 64].copy_from_slice(&headers_size.to_le_bytes());
        // 导入表目录（索引 1）
        let data_dirs = opt + 112;
        data[data_dirs + 8..data_dirs + 12].copy_from_slice(&text_rva.to_le_bytes());
        data[data_dirs + 12..data_dirs + 16].copy_from_slice(&40u32.to_le_bytes());

        Self { data, section_count }
    }

    fn image(&self) -> Image {
        Image::parse(self.data.clone()).expect("夹具应可解析")
    }
}

#[test]
fn parses_headers_and_sections() {
    let fixture = Fixture::new(3);
    let image = fixture.image();
    assert!(image.is_x64());
    assert_eq!(image.number_of_sections, 3);
    assert_eq!(image.sections.len(), 3);
    assert_eq!(image.file_alignment, FILE_ALIGNMENT);
    assert_eq!(image.section_alignment, SECTION_ALIGNMENT);
    assert!(image.dll_is_imported("KERNEL32.DLL").unwrap());
    assert!(!image.dll_is_imported("CopperCoreHook.dll").unwrap());
}

#[test]
fn rejects_non_pe_and_non_x64() {
    assert!(matches!(parse(vec![0u8; 16]), Err(PeError::NotPe(_))));
    let mut fixture = Fixture::new(1);
    // 改成 PE32（0x10b）→ 必须报「只支持 x64」
    let opt = fixture.data[0x3c] as usize + 4 + 20;
    fixture.data[opt..opt + 2].copy_from_slice(&0x10bu16.to_le_bytes());
    assert_eq!(parse(fixture.data.clone()).unwrap_err(), PeError::NotPe32Plus);
    // Machine 改成 ARM64
    let mut fixture = Fixture::new(1);
    let coff = fixture.data[0x3c] as usize + 4;
    fixture.data[coff..coff + 2].copy_from_slice(&0xaa64u16.to_le_bytes());
    let image = parse(fixture.data).unwrap();
    assert!(!image.is_x64());
}

#[test]
fn add_import_preserves_existing_imports() {
    let fixture = Fixture::new(3);
    let image = fixture.image();
    let plan = image
        .plan_add_import("CopperCoreHook.dll", "CopperCoreHookEntry")
        .expect("应能计划注入");
    let patched = plan.apply(&image).expect("应能应用");

    let after = Image::parse(patched).expect("注入后仍可解析");
    assert_eq!(after.number_of_sections, 4);
    assert!(after.dll_is_imported("kernel32.dll").unwrap());
    assert!(after.dll_is_imported("CopperCoreHook.dll").unwrap());
    assert!(after.has_hook_section());
    // 原有导入的 thunk / 名称 RVA 必须原样保留在原节
    let before_desc = image.import_descriptors().unwrap();
    let after_desc = after.import_descriptors().unwrap();
    assert_eq!(before_desc.len(), 1);
    assert_eq!(after_desc.len(), 2);
    assert_eq!(before_desc[0].original_first_thunk, after_desc[0].original_first_thunk);
    assert_eq!(before_desc[0].first_thunk, after_desc[0].first_thunk);
    // 节表项与数据目录都已更新
    assert_eq!(after.size_of_headers, plan.new_headers);
    let (rva, size) = after.data_dir(1).unwrap();
    assert_eq!(rva, plan.new_section_rva);
    assert_eq!(size, plan.descriptor_count * 20);
    assert!(after.size_of_image > image.size_of_image);
}

#[test]
fn add_import_is_idempotent() {
    let fixture = Fixture::new(2);
    let image = fixture.image();
    let plan = image.plan_add_import("CopperCoreHook.dll", "E").unwrap();
    let patched = plan.apply(&image).unwrap();
    let after = Image::parse(patched).unwrap();
    // 第二次计划 → AlreadyImported，且不产生新的字节
    assert_eq!(
        after.plan_add_import("CopperCoreHook.dll", "E").unwrap_err(),
        PeError::AlreadyImported("CopperCoreHook.dll".into())
    );
    assert_eq!(
        plan.apply(&after).unwrap_err(),
        PeError::AlreadyImported("CopperCoreHook.dll".into())
    );
}

#[test]
fn grows_headers_when_section_table_is_full() {
    // 96 节 = 节表塞满，追加必然需要扩张文件头
    let fixture = Fixture::new(96);
    let image = fixture.image();
    assert_eq!(image.number_of_sections, 96);
    assert_eq!(
        image.plan_add_import("CopperCoreHook.dll", "E").unwrap_err(),
        PeError::NoSectionRoom
    );

    // 节表末项正好贴着文件对齐边界时，再加一项就必须扩张文件头。
    // 3 节时 `section_table_end == SizeOfHeaders`，是最小可复现的扩张场景。
    let fixture = Fixture::new(3);
    let image = fixture.image();
    let plan = image
        .plan_add_import("CopperCoreHook.dll", "E")
        .expect("还有空余节表项");
    assert!(
        plan.new_headers > image.size_of_headers,
        "节表项放不下时 SizeOfHeaders 必须扩张（LeviLauncher 在 MC 1.21.124.02 上踩过这个坑）"
    );
    let patched = plan.apply(&image).expect("扩张后仍可应用");
    let after = Image::parse(patched).expect("扩张后仍可解析");
    assert_eq!(after.number_of_sections, 4);
    assert!(after.has_hook_section());
    // 扩张后文件头不得压到任何节的数据
    let first_raw = after
        .sections
        .iter()
        .filter(|s| s.size_of_raw_data != 0)
        .map(|s| s.pointer_to_raw_data)
        .min()
        .unwrap();
    assert!(after.size_of_headers <= first_raw, "文件头压到了节数据");
}


#[test]
fn shifts_every_file_offset_field_uniformly() {
    let fixture = Fixture::new(3);
    let image = fixture.image();
    let opt = 0x3c_usize;
    let pe_offset = u32::from_le_bytes([
        image.data()[opt],
        image.data()[opt + 1],
        image.data()[opt + 2],
        image.data()[opt + 3],
    ]) as usize;
    let coff = pe_offset + 4;
    let data_dirs = coff + 20 + 112;
    // 人为放一个「调试目录」（索引 6）与「证书表」（索引 4），内容放在节数据里
    let mut patched_source = fixture.data.clone();
    let text = &image.sections[0];
    let debug_rva = text.virtual_address; // 复用第 0 节 RVA
    patched_source[data_dirs + 6 * 8..data_dirs + 6 * 8 + 4]
        .copy_from_slice(&debug_rva.to_le_bytes());
    patched_source[data_dirs + 6 * 8 + 4..data_dirs + 6 * 8 + 8]
        .copy_from_slice(&(28u32 * 2).to_le_bytes());
    // 调试目录两个条目各 28 字节，条目 +24 处放一个文件偏移
    let dir_off = text.pointer_to_raw_data as usize;
    patched_source[dir_off + 24..dir_off + 28].copy_from_slice(&0x40u32.to_le_bytes());
    patched_source[dir_off + 28 + 24..dir_off + 28 + 28]
        .copy_from_slice(&0x80u32.to_le_bytes());
    // 证书表：文件偏移 0x100，size 8
    patched_source[data_dirs + 4 * 8..data_dirs + 4 * 8 + 4]
        .copy_from_slice(&0x100u32.to_le_bytes());
    patched_source[data_dirs + 4 * 8 + 4..data_dirs + 4 * 8 + 8]
        .copy_from_slice(&8u32.to_le_bytes());
    // COFF 符号表指针。字段是 `coff+8` 的 `PointerToSymbolTable`；
    // `coff+12` 是 `NumberOfSymbols`——改错字段不报错，只会让符号表指错地方。
    patched_source[coff + 8..coff + 12].copy_from_slice(&0x30u32.to_le_bytes());
    patched_source[coff + 12..coff + 16].copy_from_slice(&0x99u32.to_le_bytes());

    let image = Image::parse(patched_source.clone()).unwrap();
    let plan = image.plan_add_import("CopperCoreHook.dll", "E").unwrap();
    let delta = plan.new_headers - image.size_of_headers;
    assert!(delta > 0, "该用例必须真的扩张文件头，否则测不到位移");
    let patched = plan.apply(&image).unwrap();

    // 调试目录条目里的文件偏移
    let after = Image::parse(patched.clone()).unwrap();
    let (rva, size) = after.data_dir(6).unwrap();
    let dir_off = after.rva_to_offset(rva).unwrap();
    let e0 = u32::from_le_bytes(patched[dir_off + 24..dir_off + 28].try_into().unwrap());
    let e1 = u32::from_le_bytes(patched[dir_off + 28 + 24..dir_off + 28 + 28].try_into().unwrap());
    assert_eq!(e0, 0x40 + delta);
    assert_eq!(e1, 0x80 + delta);
    // 证书表文件偏移
    let cert = u32::from_le_bytes(
        patched[data_dirs + 4 * 8..data_dirs + 4 * 8 + 4].try_into().unwrap(),
    );
    assert_eq!(cert, 0x100 + delta);
    // COFF 符号表指针随头部扩张右移；符号个数不���位移影响。
    let sym = u32::from_le_bytes(patched[coff + 8..coff + 12].try_into().unwrap());
    assert_eq!(sym, 0x30 + delta);
    let count = u32::from_le_bytes(patched[coff + 12..coff + 16].try_into().unwrap());
    assert_eq!(count, 0x99, "NumberOfSymbols 不是文件偏移，不能被改写");
    assert_eq!(size, 56);
}

#[test]
fn recomputes_checksum_after_patch() {
    let fixture = Fixture::new(3);
    let image = fixture.image();
    let plan = image.plan_add_import("CopperCoreHook.dll", "E").unwrap();
    let patched = plan.apply(&image).unwrap();
    let after = Image::parse(patched).unwrap();
    let stored = u32::from_le_bytes(
        after.data()[after.checksum_offset..after.checksum_offset + 4]
            .try_into()
            .unwrap(),
    );
    // 重新算一次应当得到同一个值（字段自身被跳过）
    assert_eq!(stored, checksum::pe_checksum_with(after.data(), after.checksum_offset));
    assert_ne!(stored, 0);
}

#[test]
fn set_subsystem_flips_console_flag() {
    let fixture = Fixture::new(2);
    let image = fixture.image();
    assert_eq!(image.subsystem(), 0);
    let mut image = image;
    image.set_subsystem_in_place(3);
    assert_eq!(image.subsystem(), 3);
    // 改完仍可解析
    let reparsed = Image::parse(image.into_data()).unwrap();
    assert_eq!(reparsed.subsystem(), 3);
}

#[test]
fn refuses_bad_import_names() {
    let image = Fixture::new(2).image();
    assert!(matches!(
        image.plan_add_import("", "E").unwrap_err(),
        PeError::BadImportName(_)
    ));
    assert!(matches!(
        image.plan_add_import("a.dll", "").unwrap_err(),
        PeError::BadImportName(_)
    ));
}

#[test]
fn rejects_foreign_injected_section() {
    // 末节可写且位于文件末尾 → 判定为第三方注入痕迹
    let mut fixture = Fixture::new(3);
    let image = fixture.image();
    let last = image.sections.last().unwrap().clone();
    fixture.data[last.header_offset + 36..last.header_offset + 40]
        .copy_from_slice(&0xE000_0040u32.to_le_bytes());
    let image = fixture.image();
    assert_eq!(
        image.plan_add_import("CopperCoreHook.dll", "E").unwrap_err(),
        PeError::ForeignPatch
    );
}

// ── 真实 PE 验证 ────────────────────────────────────────────────────────
//
// 合成夹具能覆盖我**设计出来**的边界，但覆盖不了真实 exe 的那些特征：
// 十几节、`.bss` 节、调试目录（CodeView/PDB）、Authenticode 证书表、
// 低 slack 的文件头。用真实镜像跑一遍才有说服力。
//
// 夹具路径来自环境变量 `COPPER_PE_FIXTURE`，不设置就跳过（CI 上没有游戏 exe）。

/// 跳过时的统一提示，避免「没跑」被误读成「通过了」。
fn real_fixture() -> Option<Vec<u8>> {
    match std::env::var("COPPER_PE_FIXTURE") {
        Ok(path) => match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) => panic!("COPPER_PE_FIXTURE 指向的文件读不到：{path}（{error}）"),
        },
        Err(_) => {
            eprintln!("跳过真实 PE 用例：未设置 COPPER_PE_FIXTURE");
            None
        }
    }
}

#[test]
fn real_executable_survives_injection_and_still_loads() {
    let Some(original) = real_fixture() else {
        return;
    };
    let image = Image::parse(original.clone()).expect("真实 exe 应可解析");
    assert!(image.is_x64(), "真实 exe 应为 x64");
    assert!(!image.has_hook_section(), "夹具不应已被注入过");

    let plan = image
        .plan_add_import("CopperCoreHook.dll", "CopperCoreHookEntry")
        .expect("真实 exe 应能计划注入");
    let patched = plan.apply(&image).expect("真实 exe 应能应用");

    // 1) 结构仍然合法
    let after = Image::parse(patched.clone()).expect("注入后仍可解析");
    assert_eq!(after.number_of_sections, image.number_of_sections + 1);
    assert!(after.has_hook_section());
    assert!(
        after.dll_is_imported("CopperCoreHook.dll").unwrap(),
        "注入的导入必须能被读回"
    );
    // 原有导入一个不少
    let before_dlls = image.imported_dlls().unwrap();
    let after_dlls = after.imported_dlls().unwrap();
    for dll in &before_dlls {
        assert!(
            after_dlls.iter().any(|name| name.eq_ignore_ascii_case(dll)),
            "原有导入 {dll} 丢失"
        );
    }
    // 头部与 SizeOfImage 自洽
    assert!(after.size_of_headers as usize <= patched.len());
    assert!(after.size_of_image >= after.sections.last().unwrap().rva_end());

    // 2) **Windows 自己认不认**：以「只当数据文件打开」的方式加载。
    //    头文件、节表、导入目录任一处算错，LoadLibraryEx 都会失败 ——
    //    这是对重排结果最强的验证，比任何字段断言都硬。
    let temp = std::env::temp_dir().join(format!("copper_pe_real_{}.exe", std::process::id()));
    std::fs::write(&temp, &patched).expect("写入临时 exe");
    let loaded = unsafe {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::HINSTANCE;
        use windows::Win32::System::LibraryLoader::{
            LoadLibraryExW, LOAD_LIBRARY_AS_DATAFILE,
        };
        // `LOAD_LIBRARY_AS_DATAFILE`：只把镜像当数据映射，**不解析导入、
        // 不执行入口点** —— 正是我们要的：验证 Windows 自己的 PE 解析器
        // 认不认我们重排的结果，而不是让 notepad.exe 真的跑起来。
        let wide: Vec<u16> = temp
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let handle = LoadLibraryExW(
            PCWSTR(wide.as_ptr()),
            None,
            LOAD_LIBRARY_AS_DATAFILE,
        );
        let ok = handle.is_ok();
        if let Ok(module) = handle {
            let _ = windows::Win32::Foundation::FreeLibrary(HINSTANCE(module.0));
        }
        ok
    };
    let _ = std::fs::remove_file(&temp);
    assert!(loaded, "Windows 加载器拒绝了我们改写后的镜像");

    // 3) 幂等：第二次注入不产生任何字节变化
    assert_eq!(
        after.plan_add_import("CopperCoreHook.dll", "CopperCoreHookEntry").unwrap_err(),
        PeError::AlreadyImported("CopperCoreHook.dll".into())
    );
}
