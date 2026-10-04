//! PE 模块测试：用**合成镜像**覆盖布局边界，不依赖真实游戏包。
//!
//! 合成的意义：真实 `Minecraft.Windows.exe` 动辄上百 MB、每次升级都变，
//! 不能当夹具；而这里要验的恰恰是「文件头只剩 N 字节」「节表塞满」这类
//! 极端布局 —— 自己造镜像才能精确构造出来。

use super::*;

/// 合成镜像的构造参数。
struct Fixture {
    data: Vec<u8>,
    section_count: u16,
    /// 故意把 `SizeOfHeaders` 压到最小，逼出「扩张文件头」分支。
    tight_headers: bool,
}

const FILE_ALIGNMENT: u32 = 0x200;
const SECTION_ALIGNMENT: u32 = 0x1000;

impl Fixture {
    /// 造一个带导入表（kernel32 + 一个导出）的 PE32+ 镜像。
    fn new(section_count: u16, tight_headers: bool) -> Self {
        let opt_size: u16 = 240;
        let pe_offset: usize = 0x80;
        let coff = pe_offset + 4;
        let opt = coff + 20;
        let section_table = opt + opt_size as usize;
        let headers_end = section_table + section_count as usize * 40;
        let mut headers_size = headers_end.div_ceil(FILE_ALIGNMENT as usize) as u32
            * FILE_ALIGNMENT;
        if tight_headers {
            // 只保证「装得下已有节表项」，再加一项就必须扩张。
            headers_size =
                (headers_end as u32).next_multiple_of(FILE_ALIGNMENT);
        }

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
            // tight_headers 时让第 0 节紧贴文件头，扩张就没有空间了 —— 这是
            // 「正常」夹具；HeaderOverlap 用例单独构造。
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

        Self {
            data,
            section_count,
            tight_headers,
        }
    }

    fn image(&self) -> Image {
        Image::parse(self.data.clone()).expect("夹具应可解析")
    }
}

#[test]
fn parses_headers_and_sections() {
    let fixture = Fixture::new(3, false);
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
    let mut fixture = Fixture::new(1, false);
    // 改成 PE32（0x10b）→ 必须报「只支持 x64」
    let opt = fixture.data[0x3c] as usize + 4 + 20;
    fixture.data[opt..opt + 2].copy_from_slice(&0x10bu16.to_le_bytes());
    assert_eq!(parse(fixture.data.clone()).unwrap_err(), PeError::NotPe32Plus);
    // Machine 改成 ARM64
    let mut fixture = Fixture::new(1, false);
    let coff = fixture.data[0x3c] as usize + 4;
    fixture.data[coff..coff + 2].copy_from_slice(&0xaa64u16.to_le_bytes());
    let image = parse(fixture.data).unwrap();
    assert!(!image.is_x64());
}

#[test]
fn add_import_preserves_existing_imports() {
    let fixture = Fixture::new(3, false);
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
    let fixture = Fixture::new(2, false);
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
    let fixture = Fixture::new(96, false);
    let image = fixture.image();
    assert_eq!(image.number_of_sections, 96);
    assert_eq!(
        image.plan_add_import("CopperCoreHook.dll", "E").unwrap_err(),
        PeError::NoSectionRoom
    );

    // 95 节：还差一格，文件头必须扩张
    let fixture = Fixture::new(95, false);
    let image = fixture.image();
    let plan = image
        .plan_add_import("CopperCoreHook.dll", "E")
        .expect("第 96 格可用");
    assert!(
        plan.new_headers > image.size_of_headers,
        "节表项放不下时 SizeOfHeaders 必须扩张（LeviLauncher 在 MC 1.21.124.02 上踩过这个坑）"
    );
    let patched = plan.apply(&image).expect("扩张后仍可应用");
    let after = Image::parse(patched).expect("扩张后仍可解析");
    assert_eq!(after.number_of_sections, 96);
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
fn refuses_when_grown_headers_would_overlap_section_data() {
    // tight_headers：文件头刚好贴住第 0 节数据，扩张 1px 就会重叠
    let mut fixture = Fixture::new(95, true);
    // 人为把 SizeOfHeaders 抬到紧贴第 0 节数据起点
    let image = fixture.image();
    let first_raw = image
        .sections
        .iter()
        .filter(|s| s.size_of_raw_data != 0)
        .map(|s| s.pointer_to_raw_data)
        .min()
        .unwrap();
    let opt = fixture.data[0x3c] as usize + 4 + 20;
    fixture.data[opt + 60..opt + 64].copy_from_slice(&first_raw.to_le_bytes());
    // 第 0 节起点不动，于是「需要的头部」> 可用的头部
    let image = fixture.image();
    if image.plan_add_import("CopperCoreHook.dll", "E").is_ok() {
        // 夹具没构造出重叠场景时跳过断言意义不大，直接标红
        panic!("夹具未能构造出 HeaderOverlap 场景");
    } else {
        assert_eq!(
            image.plan_add_import("CopperCoreHook.dll", "E").unwrap_err(),
            PeError::HeaderOverlap
        );
    }
}

#[test]
fn shifts_every_file_offset_field_uniformly() {
    let fixture = Fixture::new(95, false);
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
    // COFF 符号表指针 0x30
    patched_source[coff + 12..coff + 16].copy_from_slice(&0x30u32.to_le_bytes());

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
    // COFF 符号表指针
    let sym = u32::from_le_bytes(patched[coff + 12..coff + 16].try_into().unwrap());
    assert_eq!(sym, 0x30 + delta);
    assert_eq!(size, 56);
}

#[test]
fn recomputes_checksum_after_patch() {
    let fixture = Fixture::new(3, false);
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
    let fixture = Fixture::new(2, false);
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
    let image = Fixture::new(2, false).image();
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
    let mut fixture = Fixture::new(3, false);
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
