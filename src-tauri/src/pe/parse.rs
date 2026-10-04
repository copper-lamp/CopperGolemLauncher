//! PE32+ 头部解析与 RVA 映射。
//!
//! 只做「读」和「校验」，不做任何写入；写由 [`super::rebuild`] 负责。

use super::PeError;

/// PE32+ Optional Header 的 Magic。
const MAGIC_PE32_PLUS: u16 = 0x20b;
/// COFF `Machine`：x64。
pub const MACHINE_AMD64: u16 = 0x8664;
/// PE 规范给出的节数量上限。
const MAX_SECTIONS: u16 = 96;
/// 导入表在数据目录中的索引。
pub(crate) const DIR_IMPORT: usize = 1;
/// Bound Import 在数据目录中的索引（追加节后必须清零）。
pub(crate) const DIR_BOUND_IMPORT: usize = 11;

/// 一个节表项。
#[derive(Debug, Clone)]
pub struct Section {
    /// 节名（最多 8 字节，NUL 结尾）。
    pub name: String,
    /// 节表项在文件中的偏移。
    pub header_offset: usize,
    /// 内存地址（RVA）。
    pub virtual_address: u32,
    /// 内存大小。
    pub virtual_size: u32,
    /// 文件偏移。
    pub pointer_to_raw_data: u32,
    /// 文件大小（0 表示 .bss 这类无文件数据的节）。
    pub size_of_raw_data: u32,
    pub characteristics: u32,
}

impl Section {
    /// 该节覆盖的 RVA 结束位置（按虚拟大小与文件大小取大者）。
    pub fn rva_end(&self) -> u32 {
        self.virtual_address
            .saturating_add(self.virtual_size.max(self.size_of_raw_data))
    }

    /// 该节在内存中实际占用的最大偏移（文件对齐后的节间距）。
    pub fn aligned_extent(&self) -> u32 {
        align_up(self.rva_end(), 1).max(align_up(self.rva_end(), 0x1000))
    }
}

/// 已解析的镜像（持有字节与解析结果）。
#[derive(Debug, Clone)]
pub struct Image {
    data: Vec<u8>,
    /// `PE\0\0` 的文件偏移。
    pub(crate) pe_offset: usize,
    /// COFF 头偏移。
    pub(crate) coff: usize,
    /// Optional Header 偏移（PE32+）。
    pub(crate) opt: usize,
    pub machine: u16,
    pub number_of_sections: u16,
    pub size_of_optional_header: u16,
    pub section_alignment: u32,
    pub file_alignment: u32,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub num_rva_and_sizes: u32,
    pub sections: Vec<Section>,
    /// 节表首项偏移。
    pub(crate) section_table: usize,
    /// 数据目录数组偏移（Optional Header + 112）。
    pub(crate) data_dirs: usize,
    /// Optional Header 内 `CheckSum` 字段偏移。
    pub(crate) checksum_offset: usize,
    /// Optional Header 内 `Subsystem` 字段偏移。
    pub subsystem_offset: usize,
}

fn align_up(value: u32, alignment: u32) -> u32 {
    if alignment == 0 {
        return value;
    }
    value.div_ceil(alignment) * alignment
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, PeError> {
    data.get(offset..offset + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or(PeError::NotPe("文件被截断"))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, PeError> {
    data.get(offset..offset + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(PeError::NotPe("文件被截断"))
}

fn write_u16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn align_up_pub(value: u32, alignment: u32) -> u32 {
    align_up(value, alignment)
}

impl Image {
    /// 解析镜像。只接受 PE32+（x64）。
    pub fn parse(data: Vec<u8>) -> Result<Self, PeError> {
        if data.len() < 0x40 || &data[0..2] != b"MZ" {
            return Err(PeError::NotPe("缺少 MZ 头"));
        }
        let pe_offset = read_u32(&data, 0x3c)? as usize;
        if data.get(pe_offset..pe_offset + 4) != Some(b"PE\0\0") {
            return Err(PeError::NotPe("缺少 PE 签名"));
        }
        let coff = pe_offset + 4;
        let machine = read_u16(&data, coff)?;
        let number_of_sections = read_u16(&data, coff + 2)?;
        let size_of_optional_header = read_u16(&data, coff + 16)?;
        let opt = coff + 20;
        if read_u16(&data, opt)? != MAGIC_PE32_PLUS {
            return Err(PeError::NotPe32Plus);
        }
        // PE32+ 的 Optional Header 至少 112 + 16*8 字节；这里只要求到数据目录。
        if size_of_optional_header < 112 {
            return Err(PeError::Malformed("Optional Header 过短"));
        }
        let section_alignment = read_u32(&data, opt + 32)?;
        let file_alignment = read_u32(&data, opt + 36)?;
        let size_of_image = read_u32(&data, opt + 56)?;
        let size_of_headers = read_u32(&data, opt + 60)?;
        let num_rva_and_sizes = read_u32(&data, opt + 108)?;
        if number_of_sections == 0 || number_of_sections > MAX_SECTIONS {
            return Err(PeError::Malformed("节数量非法"));
        }
        // 低对齐镜像（文件偏移必须等于 RVA）无法安全重排。
        if section_alignment < 0x1000 {
            return Err(PeError::Malformed("节对齐低于 0x1000，不支持重排"));
        }
        if file_alignment == 0 || !file_alignment.is_power_of_two() {
            return Err(PeError::Malformed("文件对齐非法"));
        }

        let section_table = opt + size_of_optional_header as usize;
        let mut sections = Vec::with_capacity(number_of_sections as usize);
        for index in 0..number_of_sections as usize {
            let header_offset = section_table + index * 40;
            let raw_name = data
                .get(header_offset..header_offset + 8)
                .ok_or(PeError::NotPe("节表被截断"))?;
            let end = raw_name.iter().position(|b| *b == 0).unwrap_or(8);
            let name = String::from_utf8_lossy(&raw_name[..end]).into_owned();
            sections.push(Section {
                name,
                header_offset,
                virtual_size: read_u32(&data, header_offset + 8)?,
                virtual_address: read_u32(&data, header_offset + 12)?,
                size_of_raw_data: read_u32(&data, header_offset + 16)?,
                pointer_to_raw_data: read_u32(&data, header_offset + 20)?,
                characteristics: read_u32(&data, header_offset + 36)?,
            });
        }

        let data_dirs = opt + 112;
        Ok(Self {
            data,
            pe_offset,
            coff,
            opt,
            machine,
            number_of_sections,
            size_of_optional_header,
            section_alignment,
            file_alignment,
            size_of_image,
            size_of_headers,
            num_rva_and_sizes,
            sections,
            section_table,
            data_dirs,
            checksum_offset: opt + 64,
            subsystem_offset: opt + 68,
        })
    }

    /// 是否 x64。
    pub fn is_x64(&self) -> bool {
        self.machine == MACHINE_AMD64
    }

    /// 镜像字节。
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// 消耗镜像，取出字节（写回路径用）。
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// 数据目录项（返回 `(rva, size)`；越界或未声明时 `None`）。
    pub fn data_dir(&self, index: usize) -> Option<(u32, u32)> {
        if index as u32 >= self.num_rva_and_sizes {
            return None;
        }
        let offset = self.data_dirs + index * 8;
        Some((read_u32(&self.data, offset).ok()?, read_u32(&self.data, offset + 4).ok()?))
    }

    pub(crate) fn set_data_dir(&mut self, index: usize, rva: u32, size: u32) {
        let offset = self.data_dirs + index * 8;
        write_u32(&mut self.data, offset, rva);
        write_u32(&mut self.data, offset + 4, size);
    }

    /// RVA → 文件偏移。
    ///
    /// 只对**有文件数据**的节做映射：`SizeOfRawData == 0` 的节（.bss）在文件里
    /// 不占空间，给它算偏移会把映射带进别的节的数据里。
    pub fn rva_to_offset(&self, rva: u32) -> Option<usize> {
        self.sections.iter().find_map(|section| {
            if section.size_of_raw_data == 0 {
                return None;
            }
            let start = section.virtual_address;
            let end = start.checked_add(section.virtual_size.max(section.size_of_raw_data))?;
            if rva < start || rva >= end {
                return None;
            }
            let offset = section
                .pointer_to_raw_data
                .checked_add(rva.checked_sub(start)?)?;
            // 落在节的文件数据范围之外（例如虚拟尾部）视为未映射。
            if offset >= section.pointer_to_raw_data + section.size_of_raw_data {
                return None;
            }
            usize::try_from(offset).ok()
        })
    }

    /// 读 C 宽字符串（UTF-16，NUL 结尾）。
    pub fn read_wide_string(&self, rva: u32) -> Option<String> {
        let start = self.rva_to_offset(rva)?;
        let mut units = Vec::new();
        let mut cursor = start;
        while cursor + 1 < self.data.len() {
            let unit = u16::from_le_bytes([self.data[cursor], self.data[cursor + 1]]);
            if unit == 0 {
                break;
            }
            units.push(unit);
            cursor += 2;
        }
        Some(String::from_utf16_lossy(&units))
    }

    /// 当前 Subsystem（2 = GUI，3 = 控制台）。
    pub fn subsystem(&self) -> u16 {
        read_u16(&self.data, self.subsystem_offset).unwrap_or(0)
    }

    /// 改 Subsystem（原地，不涉及重排；`2` = GUI，`3` = 控制台）。
    pub fn set_subsystem_in_place(&mut self, subsystem: u16) {
        write_u16(&mut self.data, self.subsystem_offset, subsystem);
    }

    /// 已导入的 DLL 列表。
    pub fn imported_dlls(&self) -> Result<Vec<String>, PeError> {
        Ok(self.import_descriptors()?.into_iter().map(|d| d.dll).collect())
    }

    /// 镜像是否已导入指定 DLL（大小写不敏感）。
    pub fn dll_is_imported(&self, dll: &str) -> Result<bool, PeError> {
        Ok(self
            .import_descriptors()?
            .iter()
            .any(|descriptor| descriptor.dll.eq_ignore_ascii_case(dll)))
    }

    /// 是否存在本方追加的节（`.copperh`）。
    pub fn has_hook_section(&self) -> bool {
        self.sections
            .iter()
            .any(|section| section.name == super::HOOK_SECTION_NAME)
    }

    /// 最后一个节在文件中的结束位置（不含对齐）。
    pub fn raw_end(&self) -> u32 {
        self.sections
            .iter()
            .filter(|section| section.size_of_raw_data != 0)
            .map(|section| section.pointer_to_raw_data + section.size_of_raw_data)
            .max()
            .unwrap_or(self.size_of_headers)
    }

    /// 镜像体积（用于「文件被截断」判断）。
    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// 导入表描述符。
    pub(crate) fn import_descriptors(&self) -> Result<Vec<ImportDescriptor>, PeError> {
        let Some((rva, _size)) = self.data_dir(DIR_IMPORT) else {
            return Ok(Vec::new());
        };
        if rva == 0 {
            return Ok(Vec::new());
        }
        let Some(mut offset) = self.rva_to_offset(rva) else {
            // 声明了导入目录却映射不到：该镜像已被破坏，如实报错而不是当作空表。
            return Err(PeError::Malformed("导入目录 RVA 无法映射到文件"));
        };
        let mut out = Vec::new();
        loop {
            let original_first_thunk = read_u32(&self.data, offset)?;
            let name_rva = read_u32(&self.data, offset + 12)?;
            let first_thunk = read_u32(&self.data, offset + 16)?;
            // 全零描述符 = 导入表结束
            if original_first_thunk == 0 && name_rva == 0 && first_thunk == 0 {
                break;
            }
            let dll = self.read_wide_string(name_rva).unwrap_or_default();
            out.push(ImportDescriptor {
                dll,
                original_first_thunk,
                first_thunk,
                descriptor_offset: offset,
            });
            offset += 20;
            if out.len() > 4096 {
                return Err(PeError::Malformed("导入描述符数量异常"));
            }
        }
        Ok(out)
    }
}

/// 一个导入表描述符。
#[derive(Debug, Clone)]
pub(crate) struct ImportDescriptor {
    /// DLL 名。
    pub dll: String,
    /// `OriginalFirstThunk`（ILT RVA）；为 0 表示镜像没有 ILT。
    pub original_first_thunk: u32,
    /// `FirstThunk`（IAT RVA）。
    pub first_thunk: u32,
    /// 描述符在文件中的偏移。
    pub descriptor_offset: usize,
}

pub(crate) use {align_up_pub as align_up, read_u16, read_u32, write_u16, write_u32};
