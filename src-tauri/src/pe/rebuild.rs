//! 落盘：把 [`ImportPlan`] 应用到镜像字节上。
//!
//! # 为什么是「单次 uniform 位移」
//!
//! 追加节要求文件变长。若文件头需要扩张（节表项放不下），参考实现的做法是
//! 逐节搬移并逐字段修补 `PointerToRawData` —— 容易漏掉**非 RVA 的文件偏移
//! 字段**：COFF 符号表指针、调试目录条目的 `PointerToRawData`、证书表
//! （Authenticode 用文件偏移而非 RVA）。
//!
//! 这里改成：只在旧文件头末尾插入 `delta` 字节，其后所有内容整体右移
//! `delta`，然后**所有文件偏移字段统一加上同一个 `delta`**。既有 RVA 全部不变，
//! 于是只存在一个需要修正的量，漏项在原理上就不可能发生。
//!
//! 前提条件（`plan_add_import` 已校验）：扩张后的文件头不压到任何节的数据。

use super::checksum::pe_checksum;
use super::import::{BOUND_IMPORT_DIR, IMPORT_DIR};
use super::parse::{Image, Section, MACHINE_AMD64};
use super::PeError;

/// `IMAGE_SCN_CNT_INITIALIZED_DATA | MEM_READ | MEM_WRITE`。
const SCN_CHARACTERISTICS: u32 = 0x0000_0040 | 0x4000_0000 | 0x8000_0000;
/// 调试目录条目长度（`IMAGE_DEBUG_DIRECTORY`）。
const DEBUG_ENTRY_SIZE: usize = 28;

fn write_u16(buf: &mut [u8], offset: usize, value: u16) {
    buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn read_u32(buf: &[u8], offset: usize) -> Option<u32> {
    buf.get(offset..offset + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

impl super::ImportPlan {
    /// 应用计划，返回新的镜像字节。
    ///
    /// 调用方负责「写临时文件 → `ReplaceFileW` → 回读校验」，本函数只做内存变换。
    pub fn apply(&self, image: &Image) -> Result<Vec<u8>, PeError> {
        if image.machine != MACHINE_AMD64 {
            return Err(PeError::NotPe32Plus);
        }
        if image.has_hook_section() {
            return Err(PeError::AlreadyImported(self.dll.clone()));
        }

        let delta = self.new_headers.saturating_sub(image.size_of_headers);
        let mut out = Vec::with_capacity(image.len() + delta as usize + self.blob.len() + 512);
        out.extend_from_slice(&image.data()[..image.size_of_headers as usize]);
        out.resize(out.len() + delta as usize, 0);
        out.extend_from_slice(&image.data()[image.size_of_headers as usize..]);

        // ---- 所有「文件偏移」统一右移 delta
        shift_sections(&mut out, image, delta);
        shift_coff_symbol_table(&mut out, image, delta);
        shift_debug_entries(&mut out, image, delta)?;
        shift_certificate_table(&mut out, image, delta);

        // ---- 新节追加在文件末尾
        let new_raw = super::parse::align_up(
            image.raw_end().saturating_add(delta),
            image.file_alignment,
        );
        out.resize(new_raw as usize, 0);
        out.extend_from_slice(&self.blob);
        out.resize((new_raw + self.new_section_raw_size) as usize, 0);

        // ---- 节表项
        let header_offset =
            image.section_table + image.number_of_sections as usize * 40;
        let mut name = [0u8; 8];
        let name_bytes = self.section_name.as_bytes();
        let take = name_bytes.len().min(8);
        name[..take].copy_from_slice(&name_bytes[..take]);
        out[header_offset..header_offset + 8].copy_from_slice(&name);
        write_u32(&mut out, header_offset + 8, self.new_section_vsize);
        write_u32(&mut out, header_offset + 12, self.new_section_rva);
        write_u32(&mut out, header_offset + 16, self.new_section_raw_size);
        write_u32(&mut out, header_offset + 20, new_raw);
        write_u32(&mut out, header_offset + 24, 0);
        write_u32(&mut out, header_offset + 28, 0);
        write_u16(&mut out, header_offset + 32, 0);
        write_u16(&mut out, header_offset + 34, 0);
        write_u32(&mut out, header_offset + 36, SCN_CHARACTERISTICS);

        // ---- 头部字段
        write_u16(&mut out, image.coff + 2, image.number_of_sections + 1);
        write_u32(&mut out, image.opt + 60, self.new_headers);
        write_u32(&mut out, image.opt + 56, self.new_image_size);
        let init_delta = self.initialized_size_delta();
        let init_size = read_u32(&out, image.opt + 8).unwrap_or(0);
        write_u32(&mut out, image.opt + 8, init_size.saturating_add(init_delta));
        // 导入表指向新节；Bound Import 必须清零（否则加载器使用陈旧绑定地址）。
        let data_dir_import = image.data_dirs + IMPORT_DIR * 8;
        write_u32(&mut out, data_dir_import, self.new_section_rva);
        write_u32(&mut out, data_dir_import + 4, self.descriptor_count * 20);
        if let Some((_, size)) = image.data_dir(BOUND_IMPORT_DIR) {
            if size != 0 {
                let offset = image.data_dirs + BOUND_IMPORT_DIR * 8;
                write_u32(&mut out, offset, 0);
                write_u32(&mut out, offset + 4, 0);
            }
        }

        // ---- 校验和：先清零字段再算，否则算出来的值含旧值
        write_u32(&mut out, image.checksum_offset, 0);
        let sum = pe_checksum(&out);
        write_u32(&mut out, image.checksum_offset, sum);
        Ok(out)
    }
}

/// 右移所有节的 `PointerToRawData`。
///
/// `SizeOfRawData == 0` 的节（.bss）在文件里没有数据，其指针必须**清零**：
/// 保留一个落在文件中间的野指针，加载器按它读会读到别的节的内容。
fn shift_sections(out: &mut [u8], image: &Image, delta: u32) {
    for section in &image.sections {
        let at = section.header_offset + 20;
        if section.size_of_raw_data == 0 {
            write_u32(out, at, 0);
        } else {
            let value = read_u32(out, at).unwrap_or(0);
            write_u32(out, at, value.saturating_add(delta));
        }
    }
}

/// COFF 符号表（若有）位于节数据之后，也是文件偏移。
fn shift_coff_symbol_table(out: &mut [u8], image: &Image, delta: u32) {
    let at = image.coff + 12;
    let Some(pointer) = read_u32(out, at) else { return };
    if pointer == 0 {
        return;
    }
    write_u32(out, at, pointer.saturating_add(delta));
}

/// 调试目录：条目本身在节内（RVA，不动），但它指向的调试数据用文件偏移。
fn shift_debug_entries(out: &mut [u8], image: &Image, delta: u32) -> Result<(), PeError> {
    let Some((rva, size)) = image.data_dir(6) else {
        return Ok(());
    };
    if rva == 0 || size == 0 {
        return Ok(());
    }
    let Some(base) = image.rva_to_offset(rva) else {
        // 调试目录映射不到：不影响加载，跳过（不改比改错好）。
        return Ok(());
    };
    let count = (size as usize) / DEBUG_ENTRY_SIZE;
    for index in 0..count {
        let entry = base + index * DEBUG_ENTRY_SIZE;
        let Some(pointer) = read_u32(out, entry + 24) else {
            continue;
        };
        if pointer == 0 {
            continue;
        }
        write_u32(out, entry + 24, pointer.saturating_add(delta));
    }
    Ok(())
}

/// Authenticode 证书表：数据目录项里存的是**文件偏移**，同样要右移。
///
/// 注意：改写镜像后签名必然失效（内容变了、证书还留着）。这里只保证加载器
/// 不会因为偏移错乱而读到垃圾数据。
fn shift_certificate_table(out: &mut [u8], image: &Image, delta: u32) {
    let at = image.data_dirs + 4 * 8;
    let (Some(offset), Some(size)) = (read_u32(out, at), read_u32(out, at + 4)) else {
        return;
    };
    if offset == 0 || size == 0 {
        return;
    }
    write_u32(out, at, offset.saturating_add(delta));
}

/// 供测试与诊断使用：新节信息。
#[derive(Debug, Clone)]
pub struct HookSectionInfo {
    pub name: String,
    pub virtual_address: u32,
    pub pointer_to_raw_data: u32,
    pub size_of_raw_data: u32,
}

impl Image {
    /// 读取本方追加节的信息（不存在则 `None`）。
    pub fn hook_section(&self) -> Option<HookSectionInfo> {
        self.sections
            .iter()
            .find(|section: &&Section| section.name == super::HOOK_SECTION_NAME)
            .map(|section| HookSectionInfo {
                name: section.name.clone(),
                virtual_address: section.virtual_address,
                pointer_to_raw_data: section.pointer_to_raw_data,
                size_of_raw_data: section.size_of_raw_data,
            })
    }
}
