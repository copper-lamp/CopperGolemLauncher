//! 导入表改写：算出「追加一条导入」所需的全部布局。
//!
//! 这一层**只算不写**（[`ImportPlan::apply`] 才落盘）。这样节表满、文件头
//! 扩张、被第三方改写等边界用例都能在纯内存里断言，不必准备真实游戏包。
//!
//! 新节内布局（与 LeviLauncher `internal/peeditor/importpatch.go` 同构）：
//!
//! ```text
//! [已有描述符 × n][新描述符][空描述符][ILT 2 项][IAT 2 项][hint+name][dll 名]
//! ```
//!
//! 已有描述符的 `OriginalFirstThunk` / `Name` / `FirstThunk` **原地不动**：
//! 它们指向原节里的 ILT / 名称 / IAT，只有描述符表本身被复制到新节并重定向。
//! Windows 加载器只按数据目录里那份描述符表逐项加载，IAT 仍写回原处。

use super::parse::{Image, DIR_BOUND_IMPORT, DIR_IMPORT};
use super::PeError;

/// `IMAGE_SCN_CNT_INITIALIZED_DATA | MEM_READ | MEM_WRITE`。
/// 必须可写：新节里要放重建的描述符表与 thunk，加载器会写 IAT。
const SCN_INITIALIZED_DATA: u32 = 0x0000_0040;
const SCN_MEM_READ: u32 = 0x4000_0000;
const SCN_MEM_WRITE: u32 = 0x8000_0000;

/// 一次「追加导入」的完整计划。
#[derive(Debug, Clone)]
pub struct ImportPlan {
    /// 目标 DLL 名（写入新节的那份）。
    pub dll: String,
    /// 锚点导出名。
    pub entry: String,
    /// 新节的文件名（8 字节内）。
    pub section_name: String,
    /// 新节的 RVA。
    pub new_section_rva: u32,
    /// 新节的 `VirtualSize`（有效载荷长度）。
    pub new_section_vsize: u32,
    /// 新节的 `SizeOfRawData`（按文件对齐取整）。
    pub new_section_raw_size: u32,
    /// 新节载荷内容（未按对齐补齐）。
    pub blob: Vec<u8>,
    /// 导入表描述符总数（含新描述符与末尾空描述符）。
    pub descriptor_count: u32,
    /// 新的 `SizeOfHeaders`。
    pub new_headers: u32,
    /// 新的 `SizeOfImage`。
    pub new_image_size: u32,
}

impl ImportPlan {
    /// 新节有效载荷的校验和增量（并入 `SizeOfInitializedData`）。
    pub fn initialized_size_delta(&self) -> u32 {
        self.new_section_raw_size
    }
}

/// 校验导入项名：非空、无 NUL、长度不超过 255（远小于任何实际需求）。
fn check_name(kind: &str, value: &str) -> Result<(), PeError> {
    if value.is_empty() {
        return Err(PeError::BadImportName(if kind == "dll" {
            "DLL 名为空"
        } else {
            "导出名为空"
        }));
    }
    if value.len() > 255 || value.contains('\0') {
        return Err(PeError::BadImportName(if kind == "dll" {
            "DLL 名过长或含 NUL"
        } else {
            "导出名过长或含 NUL"
        }));
    }
    Ok(())
}

impl Image {
    /// 计划「给 exe 追加一条 `dll!entry` 导入」。
    ///
    /// 已导入时返回 [`PeError::AlreadyImported`]（幂等命中，由调用方跳过写入）。
    pub fn plan_add_import(&self, dll: &str, entry: &str) -> Result<ImportPlan, PeError> {
        check_name("dll", dll)?;
        check_name("entry", entry)?;

        let descriptors = self.import_descriptors()?;
        if descriptors
            .iter()
            .any(|descriptor| descriptor.dll.eq_ignore_ascii_case(dll))
        {
            return Err(PeError::AlreadyImported(dll.to_string()));
        }
        // 体检：本方追加节不存在，却又存在其它「非标准」节时，说明镜像已被第三方
        // 改写（如 lipd / LeviLamina 自己 patch 过导入表）。此时追加第二个
        // 改写节极可能互相破坏布局 —— 如实拒绝，并提示人工确认。
        if !self.has_hook_section() && self.looks_patched_by_other() {
            return Err(PeError::ForeignPatch);
        }
        if self.number_of_sections >= 96 {
            return Err(PeError::NoSectionRoom);
        }

        // ---- 前置校验（与 LeviLauncher `internal/peeditor/rebuild.go` 同源）
        //
        // 这些检查的价值不在「本 exe 过不过得去」，而在于：追加节要同时动节表、
        // 文件头、多个文件偏移字段，任何一处布局不自洽都会产出**能解析但加载器
        // 拒绝**的镜像。宁可如实拒绝，也不要写出半成品。
        let align = self.file_alignment;
        if align == 0
            || !align.is_power_of_two()
            || !self.section_alignment.is_power_of_two()
            || self.section_alignment < align
        {
            return Err(PeError::Malformed("节/文件对齐参数非法"));
        }

        // ---- 新节 RVA：接在所有节的内存末尾**与 SizeOfImage 两者之后**
        //
        // `SizeOfImage` 必须参与取大：它是加载器分配镜像的依据，有些镜像的节
        // 虚拟末尾到不了那里（节之间留了空洞）。只看节的 `rva_end()` 会把新节
        // 放进「节没占满、`SizeOfImage` 留了余量」的那段里。
        let rva_end = self
            .sections
            .iter()
            .map(|section| section.rva_end())
            .chain(std::iter::once(self.size_of_image))
            .max()
            .unwrap_or(self.size_of_headers);
        let new_section_rva = super::parse::align_up(rva_end, self.section_alignment);

        // ---- 文件头要长出一个节表项，可能需要扩张
        let section_table_end = self.section_table + self.number_of_sections as usize * 40;
        // 注意取整顺序：**先与现有 SizeOfHeaders 取大，再按文件对齐**。
        // 反过来（先对齐再取大）会在头部本已对齐的镜像上多扩张一档。
        let new_headers = super::parse::align_up(
            (section_table_end + 40).max(self.size_of_headers as usize) as u32,
            self.file_alignment,
        );

        // 新节表项的落点必须在现有文件头之内，且那段必须是空的 —— 否则会盖掉
        // 真实的头部数据。
        let slot_end = section_table_end + 40;
        let slot_check_end = slot_end.min(self.size_of_headers as usize);
        if self.data()[section_table_end..slot_check_end]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(PeError::Malformed("新节表项落点不是空区"));
        }
        // 新节表项不得与任何数据目录重叠（证书表用文件偏移，不参与）。
        for index in 0..self.num_rva_and_sizes as usize {
            if index == SECURITY_DIR {
                continue;
            }
            if let Some((rva, size)) = self.data_dir(index) {
                if size != 0
                    && rva < slot_end as u32
                    && rva as u64 + size as u64 > section_table_end as u64
                {
                    return Err(PeError::Malformed("新节表项与数据目录重叠"));
                }
            }
        }
        // 扩张后的文件头不得压到任何节的 RVA。
        if let Some(first) = self
            .sections
            .iter()
            .map(|section| section.virtual_address)
            .min()
        {
            if new_headers > first {
                return Err(PeError::Malformed("扩张后的文件头压到节 RVA"));
            }
        }
        // 节的原始数据必须完整落在文件体内，且彼此不重叠：重排要按「旧偏移 +
        // delta」整体搬动，源区间一旦越界或互相压盖，搬出来就是乱的。
        let mut occupied: Vec<(u32, u32)> = Vec::new();
        for section in &self.sections {
            if section.size_of_raw_data == 0 {
                continue;
            }
            let start = section.pointer_to_raw_data;
            let end = start.saturating_add(section.size_of_raw_data);
            if start < self.size_of_headers || end as usize > self.len() {
                return Err(PeError::Malformed("节的原始数据越出文件体"));
            }
            if occupied
                .iter()
                .any(|(other_start, other_end)| start < *other_end && end > *other_start)
            {
                return Err(PeError::Malformed("多个节的原始数据互相重叠"));
            }
            occupied.push((start, end));
        }

        // ---- 节内布局
        let existing_bytes = (descriptors.len() * 20) as u32;
        let new_descriptor_offset = existing_bytes;
        let null_descriptor_offset = new_descriptor_offset + 20;
        let ilt_offset = null_descriptor_offset + 20;
        let iat_offset = ilt_offset + 16;
        let hint_offset = iat_offset + 16;
        let dll_name_offset = hint_offset + 2 + (entry.encode_utf16().count() as u32 + 1) * 2;
        let blob_len = dll_name_offset + (dll.encode_utf16().count() as u32 + 1) * 2;

        let mut blob = vec![0u8; blob_len as usize];
        // 已有描述符原样复制（它们的 thunk / 名称 RVA 不变）。
        for (index, descriptor) in descriptors.iter().enumerate() {
            let source = descriptor.descriptor_offset;
            let target = (index * 20) as usize;
            blob[target..target + 20].copy_from_slice(&self.data()[source..source + 20]);
        }
        let base = new_section_rva;
        // 新描述符：ILT / 名称 / IAT 都指向新节内的偏移。
        write_u32_at(&mut blob, new_descriptor_offset as usize, base + ilt_offset);
        write_u32_at(&mut blob, (new_descriptor_offset + 4) as usize, 0);
        write_u32_at(&mut blob, (new_descriptor_offset + 8) as usize, 0);
        write_u32_at(
            &mut blob,
            (new_descriptor_offset + 12) as usize,
            base + dll_name_offset,
        );
        write_u32_at(&mut blob, (new_descriptor_offset + 16) as usize, base + iat_offset);
        // 空描述符已由零填充天然满足。
        // ILT / IAT：一条 hint+name RVA + 一个 0 结束项。
        write_u32_at(&mut blob, ilt_offset as usize, base + hint_offset);
        write_u32_at(&mut blob, iat_offset as usize, base + hint_offset);
        // hint(2 字节，0) + 导出名 + NUL
        write_wide_at(&mut blob, (hint_offset + 2) as usize, entry);
        write_wide_at(&mut blob, dll_name_offset as usize, dll);

        let new_section_raw_size =
            super::parse::align_up(blob_len, self.file_alignment);
        let new_image_size = super::parse::align_up(
            base.saturating_add(blob_len),
            self.section_alignment,
        );

        Ok(ImportPlan {
            dll: dll.to_string(),
            entry: entry.to_string(),
            section_name: super::HOOK_SECTION_NAME.to_string(),
            new_section_rva: base,
            new_section_vsize: blob_len,
            new_section_raw_size,
            blob,
            descriptor_count: (descriptors.len() + 2) as u32,
            new_headers,
            new_image_size,
        })
    }

    /// 镜像是否看起来已被第三方追加过节。
    ///
    /// 判据：存在位于文件**末尾**、且声明可写的节（PE 加载器按 `Characteristics`
    /// 加载，但注入器追加的节通常都可写）。标准发行版游戏 exe 不含这种节；
    /// 若有，说明有第三方注入痕迹。
    fn looks_patched_by_other(&self) -> bool {
        let raw_end = self.raw_end();
        self.sections.iter().any(|section| {
            section.size_of_raw_data != 0
                && section.pointer_to_raw_data + section.size_of_raw_data >= raw_end
                && section.characteristics & SCN_MEM_WRITE != 0
        })
    }
}

fn write_u32_at(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// 写 UTF-16 字符串 + NUL 终止符。
fn write_wide_at(buf: &mut [u8], offset: usize, value: &str) {
    for (index, unit) in value.encode_utf16().enumerate() {
        let at = offset + index * 2;
        buf[at..at + 2].copy_from_slice(&unit.to_le_bytes());
    }
}

/// 供 `rebuild` 使用：Bound Import 目录必须清零，否则加载器会用陈旧绑定地址。
pub(crate) const BOUND_IMPORT_DIR: usize = DIR_BOUND_IMPORT;
/// 供 `rebuild` 使用：导入表目录索引。
pub(crate) const IMPORT_DIR: usize = DIR_IMPORT;
/// 供 `rebuild` 使用：证书表目录索引。这一项的 `VirtualAddress` 存的是
/// **文件偏移**而不是 RVA，是唯一会指向「最后一个节之后」的目录项。
pub(crate) const SECURITY_DIR: usize = 4;
