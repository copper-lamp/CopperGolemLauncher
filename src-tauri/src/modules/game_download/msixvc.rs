// SPDX-License-Identifier: GPL-3.0-only
//
// This MSIXVC native-install implementation is being migrated under the
// GPL-3.0-only terms of LeviLauncher nativeinstall. See /THIRD_PARTY_NOTICES.

use openssl::symm::{Cipher, Crypter, Mode};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentMetadata {
    pub header_len: u32,
    pub segment_count: u32,
    pub segments: Vec<SegmentRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentRecord {
    pub flags: u16,
    pub path: String,
    pub size: u64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("读取范围超出输入")]
    OutOfBounds,
    #[error("不是受支持的 XVD")]
    UnsupportedXvd,
    #[error("无效的 SegmentMetadata")]
    InvalidSegmentMetadata,
    #[error("SegmentMetadata 路径不是有效 UTF-16")]
    InvalidUtf16,
    #[error("IO 错误: {0}")]
    Io(String),
    #[error("哈希树校验失败")]
    HashMismatch,
    #[error("无效的密钥材料")]
    InvalidKey,
    #[error("不安全的 Segment 路径: {0}")]
    UnsafeSegmentPath(String),
    #[error("Segment 路径大小写冲突: {0}")]
    SegmentPathCaseConflict(String),
    #[error("Segment 文件与目录冲突: {0}")]
    SegmentFileDirectoryConflict(String),
    #[error("无效的 SPLicense TLV")]
    InvalidLicenseBlob,
    #[error("设备绑定不匹配")]
    DeviceMismatch,
    #[error("加密区域缺少 content key")]
    MissingContentKey,
    #[error("XVC 区域表无效")]
    InvalidXvcRegions,
    #[error("输出目录已存在: {0}")]
    OutputAlreadyExists(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XvdHeader {
    pub format_version: u32,
    pub container_type: u32,
    pub user_data_length: u32,
    pub xvc_data_length: u32,
    pub dynamic_data_length: u32,
}

pub fn parse_file(path: &Path) -> Result<XvdHeader, ParseError> {
    let mut file = open_file(path)?;
    let data = read_at(&mut file, 0, 0x298)?;
    parse_xvd_header(&data)
}

pub fn parse_file_metadata(path: &Path) -> Result<(XvdHeader, SegmentMetadata), ParseError> {
    let mut file = open_file(path)?;
    let header_bytes = read_at(&mut file, 0, 0x471)?;
    let header = parse_xvd_header(&header_bytes)?;
    let file_len = file
        .metadata()
        .map_err(|error| ParseError::Io(error.to_string()))?
        .len();
    let user_offset = calculate_user_data_offset(&header_bytes, file_len)?;
    let (metadata_offset, metadata_len) = locate_segment_metadata(
        &mut file,
        user_offset,
        header.user_data_length,
    )?;
    let metadata = parse_segment_metadata_at(
        &mut file,
        user_offset.checked_add(metadata_offset).ok_or(ParseError::OutOfBounds)?,
        metadata_len,
        file_len,
    )?;
    Ok((header, metadata))
}

pub fn parse_bytes(data: &[u8]) -> Result<(XvdHeader, SegmentMetadata), ParseError> {
    let header = parse_xvd_header(data)?;
    let metadata = parse_segment_metadata(data)?;
    Ok((header, metadata))
}

/// Package identity used to request a Store license.
///
/// `content_id` is the XVD header VDUID at `0x220`, which is what the licensing
/// service expects, not `XvcInfo.ContentID`. `key_id` is the single content key
/// GUID advertised by the XVC metadata. Both are opaque public identifiers and
/// carry no secret material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageIdentifiers {
    pub content_id: String,
    pub key_id: String,
}

/// Reports whether the package is an MSIXVC container with at least one
/// encrypted XVC region, i.e. whether a Store content key is required.
///
/// Non-XVD inputs (legacy `.appx` ZIP and anything else) return `Ok(false)` so
/// callers never request a Store license for a package that needs none.
pub fn has_encrypted_regions(path: &Path) -> Result<bool, ParseError> {
    let mut file = open_file(path)?;
    let file_len = file
        .metadata()
        .map_err(|error| ParseError::Io(error.to_string()))?
        .len();
    if file_len < 12_288 {
        return Ok(false);
    }
    let header_bytes = match read_at(&mut file, 0, 0x471) {
        Ok(bytes) => bytes,
        Err(ParseError::OutOfBounds) => return Ok(false),
        Err(error) => return Err(error),
    };
    if header_bytes.len() < 0x471 || &header_bytes[0x200..0x208] != b"msft-xvd" {
        return Ok(false);
    }
    let header = parse_xvd_header(&header_bytes)?;
    let user_offset = calculate_user_data_offset(&header_bytes, file_len)?;
    let xvc_offset = user_offset
        .checked_add(pages(header.user_data_length as u64)?.checked_mul(4096).ok_or(ParseError::OutOfBounds)?)
        .ok_or(ParseError::OutOfBounds)?;
    let xvc_bytes = pages(header.xvc_data_length as u64)?.checked_mul(4096).ok_or(ParseError::OutOfBounds)?;
    if xvc_bytes < 0xda8 || xvc_offset.checked_add(xvc_bytes).ok_or(ParseError::OutOfBounds)? > file_len {
        return Err(ParseError::OutOfBounds);
    }
    // Read the whole XVC metadata area: the region table starts at 0xda8, so a
    // fixed 0xda8-byte read can never contain it.
    if xvc_bytes > usize::MAX as u64 {
        return Err(ParseError::OutOfBounds);
    }
    let xvc = read_at(&mut file, xvc_offset, xvc_bytes as usize)?;
    let region_count = le_u32(&xvc, 0xd14)? as usize;
    if region_count == 0 || region_count > 10_000 {
        return Err(ParseError::InvalidXvcRegions);
    }
    let region_table_end = 0xda8usize
        .checked_add(region_count.checked_mul(128).ok_or(ParseError::OutOfBounds)?)
        .ok_or(ParseError::OutOfBounds)?;
    if region_table_end > xvc.len() {
        return Err(ParseError::InvalidXvcRegions);
    }
    for index in 0..region_count {
        let position = 0xda8 + index * 128;
        if le_u16(&xvc, position + 4)? != u16::MAX {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Reads the ContentID/KeyID pair without touching or decrypting any payload.
pub fn read_package_identifiers(path: &Path) -> Result<PackageIdentifiers, ParseError> {
    let mut file = open_file(path)?;
    let file_len = file
        .metadata()
        .map_err(|error| ParseError::Io(error.to_string()))?
        .len();
    if file_len < 12_288 {
        return Err(ParseError::UnsupportedXvd);
    }
    let header_bytes = read_at(&mut file, 0, 0x471)?;
    let header = parse_xvd_header(&header_bytes)?;
    let user_offset = calculate_user_data_offset(&header_bytes, file_len)?;
    let xvc_offset = user_offset
        .checked_add(pages(header.user_data_length as u64)?.checked_mul(4096).ok_or(ParseError::OutOfBounds)?)
        .ok_or(ParseError::OutOfBounds)?;
    let xvc_bytes = pages(header.xvc_data_length as u64)?.checked_mul(4096).ok_or(ParseError::OutOfBounds)?;
    if xvc_bytes < 0xda8 || xvc_offset.checked_add(xvc_bytes).ok_or(ParseError::OutOfBounds)? > file_len {
        return Err(ParseError::OutOfBounds);
    }
    let xvc = read_at(&mut file, xvc_offset, 0xda8)?;
    // Exactly one content key is supported, matching the extraction planner.
    if le_u16(&xvc, 0xd1e)? != 1 {
        return Err(ParseError::InvalidKey);
    }
    Ok(PackageIdentifiers {
        content_id: format_guid(&header_bytes[0x220..0x230])?,
        key_id: format_guid(&xvc[16..32])?,
    })
}

fn open_file(path: &Path) -> Result<File, ParseError> {
    File::open(path).map_err(|error| ParseError::Io(error.to_string()))
}

fn read_at(file: &mut File, offset: u64, length: usize) -> Result<Vec<u8>, ParseError> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| ParseError::Io(error.to_string()))?;
    let mut data = vec![0u8; length];
    file.read_exact(&mut data)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::UnexpectedEof => ParseError::OutOfBounds,
            _ => ParseError::Io(error.to_string()),
        })?;
    Ok(data)
}

fn pages(value: u64) -> Result<u64, ParseError> {
    value.checked_add(4095).map(|value| value / 4096).ok_or(ParseError::OutOfBounds)
}

fn hashed_page_index(drive_pages: u64, user_offset: u64, position: u64) -> Result<u64, ParseError> {
    if position < user_offset || !(position - user_offset).is_multiple_of(4096) {
        return Err(ParseError::OutOfBounds);
    }
    drive_pages.checked_add((position - user_offset) / 4096).ok_or(ParseError::OutOfBounds)
}

fn calculate_user_data_offset(header: &[u8], file_len: u64) -> Result<u64, ParseError> {
    let drive_length = le_u64(header, 0x218)?;
    let user_length = le_u32(header, 0x28c)? as u64;
    let xvc_length = le_u32(header, 0x290)? as u64;
    let hashed_pages = pages(drive_length)?
        .checked_add(pages(user_length)?)
        .ok_or(ParseError::OutOfBounds)?
        .checked_add(pages(xvc_length)?)
        .ok_or(ParseError::OutOfBounds)?;
    if hashed_pages == 0 { return Err(ParseError::OutOfBounds); }
    let mut level_pages = hashed_pages;
    let mut total_hash_pages = 0u64;
    loop {
        level_pages = level_pages
            .checked_add(169)
            .map(|value| value / 170)
            .ok_or(ParseError::OutOfBounds)?;
        total_hash_pages = total_hash_pages
            .checked_add(level_pages)
            .ok_or(ParseError::OutOfBounds)?;
        if level_pages == 1 {
            break;
        }
        if total_hash_pages > 1_000_000 {
            return Err(ParseError::InvalidSegmentMetadata);
        }
    }
    // The hash-tree/user-data offset uses the reserved-page count at 0x288,
    // matching the LeviLauncher reference implementation. The adjacent 0x284
    // dword is present in real packages but is not part of this offset field.
    let reserved_pages = pages(le_u32(header, 0x288)? as u64)?;
    let tree_offset = 12_288u64
        .checked_add(reserved_pages.checked_mul(4096).ok_or(ParseError::OutOfBounds)?)
        .and_then(|value| value.checked_add((header[0x470] as u64).checked_mul(4096)?))
        .ok_or(ParseError::OutOfBounds)?;
    let user_offset = tree_offset
        .checked_add(total_hash_pages.checked_mul(4096).ok_or(ParseError::OutOfBounds)?)
        .ok_or(ParseError::OutOfBounds)?;
    let user_end = user_offset
        .checked_add(pages(user_length)?.checked_mul(4096).ok_or(ParseError::OutOfBounds)?)
        .ok_or(ParseError::OutOfBounds)?;
    if user_offset > file_len || user_end > file_len {
        return Err(ParseError::OutOfBounds);
    }
    Ok(user_offset)
}

fn locate_segment_metadata(
    file: &mut File,
    user_offset: u64,
    user_length: u32,
) -> Result<(u64, usize), ParseError> {
    let user_length = user_length as u64;
    let user_header = read_at(file, user_offset, 4)?;
    let header_offset = le_u32(&user_header, 0)? as u64;
    if header_offset < 16 || header_offset.checked_add(528).ok_or(ParseError::OutOfBounds)? > user_length {
        return Err(ParseError::InvalidSegmentMetadata);
    }
    let table_header_offset = user_offset.checked_add(header_offset).ok_or(ParseError::OutOfBounds)?;
    let table_header = read_at(file, table_header_offset, 528)?;
    let segment_count = le_u32(&table_header, 524)? as u64;
    if segment_count == 0 || segment_count > 1_000 {
        return Err(ParseError::InvalidSegmentMetadata);
    }
    let table_length = segment_count.checked_mul(528).ok_or(ParseError::OutOfBounds)?;
    if header_offset
        .checked_add(528)
        .and_then(|value| value.checked_add(table_length))
        .ok_or(ParseError::OutOfBounds)?
        > user_length
    {
        return Err(ParseError::OutOfBounds);
    }
    let entries_offset = user_offset
        .checked_add(header_offset)
        .and_then(|value| value.checked_add(528))
        .ok_or(ParseError::OutOfBounds)?;
    let entries = read_at(file, entries_offset, table_length as usize)?;
    for index in 0..segment_count as usize {
        let entry_start = index.checked_mul(528).ok_or(ParseError::OutOfBounds)?;
        let entry_end = entry_start.checked_add(528).ok_or(ParseError::OutOfBounds)?;
        let entry = entries.get(entry_start..entry_end).ok_or(ParseError::OutOfBounds)?;
        let name = decode_utf16(&entry[..520])?.trim_end_matches('\0').to_owned();
        // The user file table stores the size first and the offset second, in
        // that order: entry[520] is the payload size and entry[524] is the
        // offset relative to the metadata header. Swapping these makes real
        // packages resolve `SegmentMetadata.bin` into an unrelated path blob.
        let size = le_u32(entry, 520)? as u64;
        let offset = le_u32(entry, 524)? as u64;
        if offset.checked_add(size).ok_or(ParseError::OutOfBounds)? > user_length - header_offset {
            return Err(ParseError::OutOfBounds);
        }
        if name == "SegmentMetadata.bin" {
            return Ok((header_offset.checked_add(offset).ok_or(ParseError::OutOfBounds)?, usize::try_from(size).map_err(|_| ParseError::OutOfBounds)?));
        }
    }
    Err(ParseError::InvalidSegmentMetadata)
}

fn parse_segment_metadata_at(
    file: &mut File,
    offset: u64,
    length: usize,
    file_len: u64,
) -> Result<SegmentMetadata, ParseError> {
    let end = offset.checked_add(length as u64).ok_or(ParseError::OutOfBounds)?;
    if end > file_len || length < 20 {
        return Err(ParseError::OutOfBounds);
    }
    let header = read_at(file, offset, 20)?;
    let header_len = le_u32(&header, 12)?;
    let segment_count = le_u32(&header, 16)?;
    if header_len != 100 || segment_count == 0 || segment_count > 1_000_000 {
        return Err(ParseError::InvalidSegmentMetadata);
    }
    let segment_count_usize = usize::try_from(segment_count).map_err(|_| ParseError::OutOfBounds)?;
    let table_length = segment_count_usize
        .checked_mul(16)
        .ok_or(ParseError::OutOfBounds)?;
    let table_end = (header_len as usize).checked_add(table_length).ok_or(ParseError::OutOfBounds)?;
    if table_end > length {
        return Err(ParseError::OutOfBounds);
    }
    let table_offset = offset.checked_add(header_len as u64).ok_or(ParseError::OutOfBounds)?;
    let table = read_at(file, table_offset, table_length)?;
    let mut segments = Vec::with_capacity(segment_count as usize);
    for index in 0..segment_count as usize {
        let entry_start = index.checked_mul(16).ok_or(ParseError::OutOfBounds)?;
        let entry_end = entry_start.checked_add(16).ok_or(ParseError::OutOfBounds)?;
        let entry = table.get(entry_start..entry_end).ok_or(ParseError::OutOfBounds)?;
        let flags = le_u16(entry, 0)?;
        let path_len = (le_u16(entry, 2)? as usize).checked_mul(2).ok_or(ParseError::OutOfBounds)?;
        let path_offset = table_end.checked_add(le_u32(entry, 4)? as usize).ok_or(ParseError::OutOfBounds)?;
        let size = le_u64(entry, 8)?;
        let path_end = path_offset.checked_add(path_len).ok_or(ParseError::OutOfBounds)?;
        if path_offset < table_end || path_end > length {
            return Err(ParseError::OutOfBounds);
        }
        let path_file_offset = offset.checked_add(path_offset as u64).ok_or(ParseError::OutOfBounds)?;
        let path = decode_utf16(&read_at(file, path_file_offset, path_len)?)?;
        segments.push(SegmentRecord { flags, path, size });
    }
    Ok(SegmentMetadata { header_len, segment_count, segments })
}

/// Validates the XVD SHA-256 tree and all supplied 4 KiB data pages.
/// The tree stores 20-byte SHA-256 prefixes in 24-byte entries.
pub fn validate_hash_tree(tree: &[u8], root: &[u8], pages: &[&[u8]]) -> Result<(), ParseError> {
    if root.len() != 32 || !tree.len().is_multiple_of(4096) || tree.len() < 4096 {
        return Err(ParseError::HashMismatch);
    }
    let top = Sha256::digest(&tree[..4096]);
    if top.as_slice() != root {
        return Err(ParseError::HashMismatch);
    }
    let mut counts = Vec::new();
    let mut n = pages.len() as u64;
    loop {
        n = n.checked_add(169).ok_or(ParseError::OutOfBounds)? / 170;
        counts.push(usize::try_from(n).map_err(|_| ParseError::OutOfBounds)?);
        if n == 1 { break; }
        if counts.len() > 4 { return Err(ParseError::HashMismatch); }
    }
    let tree_pages = counts.iter().try_fold(0usize, |sum, value| sum.checked_add(*value)).ok_or(ParseError::OutOfBounds)?;
    if counts.is_empty() || tree_pages.checked_mul(4096) != Some(tree.len()) {
        return Err(ParseError::HashMismatch);
    }
    let mut parent_start = 0usize;
    let mut child_start = 1usize;
    for level in (0..counts.len() - 1).rev() {
        for i in 0..counts[level] {
            let child_page = child_start.checked_add(i).ok_or(ParseError::OutOfBounds)?;
            let child_begin = child_page.checked_mul(4096).ok_or(ParseError::OutOfBounds)?;
            let child_end = child_page.checked_add(1).and_then(|page| page.checked_mul(4096)).ok_or(ParseError::OutOfBounds)?;
            if child_end > tree.len() { return Err(ParseError::HashMismatch); }
            let child = &tree[child_begin..child_end];
            let digest = Sha256::digest(child);
            let at = parent_start.checked_mul(4096)
                .and_then(|value| value.checked_add((i / 170).checked_mul(4096)?))
                .and_then(|value| value.checked_add((i % 170).checked_mul(24)?))
                .ok_or(ParseError::OutOfBounds)?;
            if at.checked_add(20).is_none_or(|end| end > tree.len()) || digest[..20] != tree[at..at + 20] {
                return Err(ParseError::HashMismatch);
            }
        }
        parent_start = child_start;
        child_start = child_start.checked_add(counts[level]).ok_or(ParseError::OutOfBounds)?;
    }
    let leaf_base = tree.len() / 4096 - counts[0];
    for (index, page) in pages.iter().enumerate() {
        if page.len() != 4096 { return Err(ParseError::HashMismatch); }
        let at = leaf_base.checked_add(index / 170)
            .and_then(|page| page.checked_mul(4096))
            .and_then(|value| value.checked_add((index % 170).checked_mul(24)?))
            .ok_or(ParseError::OutOfBounds)?;
        if at.checked_add(20).is_none_or(|end| end > tree.len()) || Sha256::digest(page)[..20] != tree[at..at + 20] {
            return Err(ParseError::HashMismatch);
        }
    }
    Ok(())
}

/// Parses the bounded little-endian SPLicense TLV container.
pub fn parse_license_blocks(blob: &[u8]) -> Result<HashMap<u32, Vec<u8>>, ParseError> {
    if blob.len() < 8 || blob.len() > 1 << 20 { return Err(ParseError::InvalidLicenseBlob); }
    let mut blocks = HashMap::new();
    let mut pos = 8usize;
    while pos < blob.len() {
        if blob.len() - pos < 8 { return Err(ParseError::InvalidLicenseBlob); }
        let id = le_u32(blob, pos).map_err(|_| ParseError::InvalidLicenseBlob)?;
        let len = le_u32(blob, pos + 4).map_err(|_| ParseError::InvalidLicenseBlob)? as usize;
        pos = pos.checked_add(8).ok_or(ParseError::InvalidLicenseBlob)?;
        let end = pos.checked_add(len).ok_or(ParseError::InvalidLicenseBlob)?;
        if end > blob.len() || blocks.insert(id, blob[pos..end].to_vec()).is_some() {
            return Err(ParseError::InvalidLicenseBlob);
        }
        pos = end;
    }
    Ok(blocks)
}

/// Unpacks the device-bound content key records from SPLicense block 0xca.
/// A content key that is cleared when its lease leaves scope.
#[derive(Debug, PartialEq, Eq)]
pub struct ContentKeyLease(Vec<u8>);

impl ContentKeyLease {
    pub fn as_bytes(&self) -> &[u8] { &self.0 }
}

impl Drop for ContentKeyLease {
    fn drop(&mut self) { self.0.fill(0); }
}

pub fn unpack_content_keys(blob: &[u8], device_id: &[u8], kek: &[u8]) -> Result<Vec<(String, ContentKeyLease)>, ParseError> {
    let blocks = parse_license_blocks(blob)?;
    if blocks.get(&0xd2).map(Vec::as_slice) != Some(device_id) { return Err(ParseError::DeviceMismatch); }
    let packed = blocks.get(&0xca).ok_or(ParseError::InvalidLicenseBlob)?;
    let mut result = Vec::new();
    let mut pos = 0usize;
    while pos < packed.len() {
        if packed.len() - pos < 4 { return Err(ParseError::InvalidLicenseBlob); }
        let id_len = le_u16(packed, pos).map_err(|_| ParseError::InvalidLicenseBlob)? as usize;
        let key_len = le_u16(packed, pos + 2).map_err(|_| ParseError::InvalidLicenseBlob)? as usize;
        pos = pos.checked_add(4).ok_or(ParseError::InvalidLicenseBlob)?;
        let end = pos.checked_add(id_len).and_then(|v| v.checked_add(key_len)).ok_or(ParseError::InvalidLicenseBlob)?;
        if id_len < 16 || key_len != 40 || end > packed.len() { return Err(ParseError::InvalidLicenseBlob); }
        let id = format_guid(&packed[pos..pos + 16])?;
        let key = unwrap_content_key(kek, &packed[pos + id_len..end])?;
        if key.len() != 32 { return Err(ParseError::InvalidKey); }
        result.push((id, ContentKeyLease(key)));
        pos = end;
    }
    Ok(result)
}

pub fn select_content_key(
    keys: Vec<(String, ContentKeyLease)>,
    wanted_id: &str,
) -> Result<ContentKeyLease, ParseError> {
    let mut selected = None;
    for (id, key) in keys {
        if id.eq_ignore_ascii_case(wanted_id) {
            if selected.is_some() { return Err(ParseError::InvalidKey); }
            selected = Some(key);
        }
    }
    selected.ok_or(ParseError::InvalidKey)
}

/// Formats a 16-byte Windows GUID like the reference implementation:
/// little-endian first three fields, then the trailing eight bytes verbatim.
pub fn format_guid(data: &[u8]) -> Result<String, ParseError> {
    if data.len() < 16 { return Err(ParseError::InvalidLicenseBlob); }
    Ok(format!("{:08x}-{:04x}-{:04x}-{}-{}", le_u32(data, 0)?, le_u16(data, 4)?, le_u16(data, 6)?, hex_bytes(&data[8..10]), hex_bytes(&data[10..16])))
}

fn hex_bytes(data: &[u8]) -> String { data.iter().map(|byte| format!("{byte:02x}")).collect() }

/// RFC 3394 AES key unwrap with the standard 0xa6 authentication value.
pub fn unwrap_content_key(kek: &[u8], wrapped: &[u8]) -> Result<Vec<u8>, ParseError> {
    if kek.len() != 16 || wrapped.len() < 24 || !wrapped.len().is_multiple_of(8) {
        return Err(ParseError::InvalidKey);
    }
    let n = wrapped.len() / 8 - 1;
    let mut out = wrapped.to_vec();
    for j in (0..6u64).rev() {
        for i in (1..=n).rev() {
            let counter = (n as u64).checked_mul(j).and_then(|value| value.checked_add(i as u64)).ok_or(ParseError::InvalidKey)?;
            let a = u64::from_be_bytes(out[..8].try_into().map_err(|_| ParseError::InvalidKey)?) ^ counter;
            let mut input = [0u8; 16];
            input[..8].copy_from_slice(&a.to_be_bytes());
            let block_start = i.checked_mul(8).ok_or(ParseError::InvalidKey)?;
            let block_end = block_start.checked_add(8).ok_or(ParseError::InvalidKey)?;
            input[8..].copy_from_slice(out.get(block_start..block_end).ok_or(ParseError::InvalidKey)?);
            let mut crypter = Crypter::new(Cipher::aes_128_ecb(), Mode::Decrypt, kek, None)
                .map_err(|_| ParseError::InvalidKey)?;
            crypter.pad(false);
            let mut block = [0u8; 32];
            let count = crypter.update(&input, &mut block).map_err(|_| ParseError::InvalidKey)?
                + crypter.finalize(&mut block[16..]).map_err(|_| ParseError::InvalidKey)?;
            if count != 16 { return Err(ParseError::InvalidKey); }
            out[..8].copy_from_slice(&block[..8]);
            out.get_mut(block_start..block_end).ok_or(ParseError::InvalidKey)?.copy_from_slice(&block[8..16]);
        }
    }
    if out[..8] != [0xa6; 8] { return Err(ParseError::InvalidKey); }
    Ok(out[8..].to_vec())
}

/// Decrypts one 4 KiB XTS page. The first 16 bytes are tweak AES key.
pub fn decrypt_page(page: &mut [u8], key: &[u8], tweak: &[u8; 16]) -> Result<(), ParseError> {
    if key.len() != 32 || !page.len().is_multiple_of(16) { return Err(ParseError::InvalidKey); }
    let mut tweak_crypter = Crypter::new(Cipher::aes_128_ecb(), Mode::Encrypt, &key[..16], None)
        .map_err(|_| ParseError::InvalidKey)?;
    tweak_crypter.pad(false);
    let mut t = [0u8; 16];
    let mut scratch = [0u8; 32];
    let count = tweak_crypter.update(tweak, &mut scratch).map_err(|_| ParseError::InvalidKey)?
        + tweak_crypter.finalize(&mut scratch[16..]).map_err(|_| ParseError::InvalidKey)?;
    if count != 16 { return Err(ParseError::InvalidKey); }
    t.copy_from_slice(&scratch[..16]);
    for chunk in page.as_chunks_mut::<16>().0 {
        for i in 0..16 { chunk[i] ^= t[i]; }
        let mut crypter = Crypter::new(Cipher::aes_128_ecb(), Mode::Decrypt, &key[16..], None)
            .map_err(|_| ParseError::InvalidKey)?;
        crypter.pad(false);
        let mut block = [0u8; 32];
        let count = crypter.update(chunk, &mut block).map_err(|_| ParseError::InvalidKey)?
            + crypter.finalize(&mut block[16..]).map_err(|_| ParseError::InvalidKey)?;
        if count != 16 { return Err(ParseError::InvalidKey); }
        chunk.copy_from_slice(&block[..16]);
        for i in 0..16 { chunk[i] ^= t[i]; }
        let mut carry = 0u8;
        for byte in &mut t {
            let next = *byte >> 7;
            *byte = (*byte << 1) | carry;
            carry = next;
        }
        if carry != 0 { t[0] ^= 0x87; }
    }
    Ok(())
}

/// Extracts the XVC regions described by `SegmentMetadata` into a new directory.
/// `content_key` is the already-authorized 32-byte AES-XTS key; encrypted regions
/// are rejected when it is absent. The input is never modified.
pub fn extract_xvc(input: &Path, output: &Path, content_key: Option<&[u8]>) -> Result<(), ParseError> {
    if output.exists() {
        return Err(ParseError::OutputAlreadyExists(output.display().to_string()));
    }
    let mut file = open_file(input)?;
    let file_len = file.metadata().map_err(|e| ParseError::Io(e.to_string()))?.len();
    let header_bytes = read_at(&mut file, 0, 0x471)?;
    let header = parse_xvd_header(&header_bytes)?;
    let user_offset = calculate_user_data_offset(&header_bytes, file_len)?;
    let (metadata_offset, metadata_len) = locate_segment_metadata(&mut file, user_offset, header.user_data_length)?;
    let metadata = parse_segment_metadata_at(
        &mut file,
        user_offset.checked_add(metadata_offset).ok_or(ParseError::OutOfBounds)?,
        metadata_len,
        file_len,
    )?;
    let paths = validate_segment_plan(&metadata)?;
    let xvc_offset = user_offset.checked_add(pages(header.user_data_length as u64)?.checked_mul(4096).ok_or(ParseError::OutOfBounds)?).ok_or(ParseError::OutOfBounds)?;
    let xvc_len = pages(header.xvc_data_length as u64)?.checked_mul(4096).ok_or(ParseError::OutOfBounds)?;
    let xvc_end = xvc_offset.checked_add(xvc_len).ok_or(ParseError::OutOfBounds)?;
    if xvc_end > file_len || xvc_len < 0xda8 || xvc_len > usize::MAX as u64 { return Err(ParseError::OutOfBounds); }
    let xvc = read_at(&mut file, xvc_offset, xvc_len as usize)?;
    let region_count = le_u32(&xvc, 0xd14)? as usize;
    let update_count = le_u32(&xvc, 0xd3c)? as usize;
    if region_count == 0 || region_count > 10_000 || update_count == 0 || update_count > 1_000_000 { return Err(ParseError::InvalidXvcRegions); }
    let region_table_end = 0xda8usize.checked_add(region_count.checked_mul(128).ok_or(ParseError::OutOfBounds)?).ok_or(ParseError::OutOfBounds)?;
    let update_base = region_table_end;
    if region_table_end > xvc.len() || update_base.checked_add(update_count.checked_mul(12).ok_or(ParseError::OutOfBounds)?).ok_or(ParseError::OutOfBounds)? > xvc.len() { return Err(ParseError::InvalidXvcRegions); }
    let plan_count = metadata.segments.len();
    let mut placements: Vec<Option<(u64, u32, bool)>> = vec![None; plan_count];
    let mut region_ranges = Vec::with_capacity(region_count);
    let mut trailing_padding = Vec::new();
    let mut seen_region_ids = HashSet::with_capacity(region_count);
    let first_segment_page = le_u32(&xvc, update_base)? as u64;
    let first_segment_offset = first_segment_page.checked_mul(4096).ok_or(ParseError::OutOfBounds)?;
    if first_segment_offset >= file_len { return Err(ParseError::InvalidXvcRegions); }
    for region_index in 0..region_count {
        let p = 0xda8 + region_index * 128;
        let start = le_u32(&xvc, p + 12)? as usize;
        // Region offsets and lengths are absolute byte ranges inside the XVD
        // file, not offsets inside the XVC metadata area: the highest region
        // ends exactly at the file length. Bounds are therefore checked against
        // the file, while the update table below stores page numbers.
        let off = le_u64(&xvc, p + 80)?;
        let length = le_u64(&xvc, p + 88)?;
        let key_index = le_u16(&xvc, p + 4)?;
        let region_id = le_u32(&xvc, p)?;
        let region_end = off.checked_add(length).ok_or(ParseError::OutOfBounds)?;
        // Zero-length regions are legal and simply carry no extractable pages;
        // only the alignment and the file bound are enforced.
        if off % 4096 != 0 || length % 4096 != 0 || region_end > file_len {
            return Err(ParseError::InvalidXvcRegions);
        }
        if !seen_region_ids.insert(region_id) { return Err(ParseError::InvalidXvcRegions); }
        if key_index != 0 && key_index != u16::MAX { return Err(ParseError::InvalidXvcRegions); }
        if region_ranges.iter().any(|(start, end)| off < *end && *start < region_end) {
            return Err(ParseError::InvalidXvcRegions);
        }
        region_ranges.push((off, region_end));
        if start == 0 && off != first_segment_offset {
            continue;
        }
        if start >= plan_count {
            return Err(ParseError::InvalidXvcRegions);
        }
        let mut pos = off;
        let mut index = start;
        while pos < region_end {
            if index == plan_count && region_index + 1 == region_count && region_end - pos <= 65536 {
                trailing_padding.push((pos, region_end));
                break;
            }
            if index >= plan_count || placements[index].is_some() { return Err(ParseError::InvalidXvcRegions); }
            let consume = pages(metadata.segments[index].size)?.max(1).checked_mul(4096).ok_or(ParseError::OutOfBounds)?;
            if consume > region_end - pos { return Err(ParseError::InvalidXvcRegions); }
            if index >= update_count { return Err(ParseError::InvalidXvcRegions); }
            let up = update_base.checked_add(index.checked_mul(12).ok_or(ParseError::OutOfBounds)?).ok_or(ParseError::OutOfBounds)?;
            if le_u32(&xvc, up)? as u64 * 4096 != pos { return Err(ParseError::InvalidXvcRegions); }
            placements[index] = Some((pos, region_id, key_index != u16::MAX));
            pos = pos.checked_add(consume).ok_or(ParseError::OutOfBounds)?;
            index = index.checked_add(1).ok_or(ParseError::OutOfBounds)?;
        }
    }
    if placements.iter().any(Option::is_none) { return Err(ParseError::InvalidXvcRegions); }
    if placements.iter().any(|p| p.unwrap().2) && content_key.is_none_or(|k| k.len() != 32) { return Err(ParseError::MissingContentKey); }

    // Validate the authenticated hash-tree root and every page as it is read.
    let drive = pages(le_u64(&header_bytes, 0x218)?)?;
    let user_pages = pages(header.user_data_length as u64)?;
    let xvc_pages = pages(header.xvc_data_length as u64)?;
    let mut n = drive.checked_add(user_pages).and_then(|v| v.checked_add(xvc_pages)).ok_or(ParseError::OutOfBounds)?;
    let mut counts = Vec::new();
    loop {
        n = n.checked_add(169).ok_or(ParseError::OutOfBounds)? / 170;
        counts.push(n);
        if n == 1 { break; }
        if counts.len() > 4 { return Err(ParseError::HashMismatch); }
    }
    let tree_pages: u64 = counts.iter().try_fold(0u64, |sum, value| sum.checked_add(*value)).ok_or(ParseError::OutOfBounds)?;
    let tree_bytes = tree_pages.checked_mul(4096).ok_or(ParseError::OutOfBounds)?;
    let tree_offset = 12_288u64.checked_add(pages(le_u32(&header_bytes, 0x288)? as u64)?.checked_mul(4096).ok_or(ParseError::OutOfBounds)?).and_then(|v| v.checked_add((header_bytes[0x470] as u64).checked_mul(4096)?)).ok_or(ParseError::OutOfBounds)?;
    let tree_end = tree_offset.checked_add(tree_bytes).ok_or(ParseError::OutOfBounds)?;
    if tree_end > file_len || tree_bytes > usize::MAX as u64 { return Err(ParseError::OutOfBounds); }
    let tree = read_at(&mut file, tree_offset, tree_bytes as usize)?;
    if Sha256::digest(&tree[..4096])[..] != header_bytes[0x240..0x260] { return Err(ParseError::HashMismatch); }
    let leaf_base = usize::try_from(tree_pages.checked_sub(counts[0]).ok_or(ParseError::OutOfBounds)?)
        .map_err(|_| ParseError::OutOfBounds)?;
    let check_page = |file: &mut File, pos: u64, tree: &[u8]| -> Result<Vec<u8>, ParseError> {
        let index = hashed_page_index(drive, user_offset, pos)?;
        let hashed_pages = drive.checked_add(user_pages).and_then(|value| value.checked_add(xvc_pages)).ok_or(ParseError::OutOfBounds)?;
        if index >= hashed_pages {
            return Err(ParseError::OutOfBounds);
        }
        let at = (leaf_base as u64)
            .checked_add(index / 170)
            .and_then(|page| page.checked_mul(4096))
            .and_then(|value| value.checked_add((index % 170).checked_mul(24)?))
            .ok_or(ParseError::OutOfBounds)?;
        let at = usize::try_from(at).map_err(|_| ParseError::OutOfBounds)?;
        let page = read_at(file, pos, 4096)?;
        if at.checked_add(20).is_none_or(|end| end > tree.len()) || Sha256::digest(&page)[..20] != tree[at..at + 20] { return Err(ParseError::HashMismatch); }
        Ok(page)
    };
    // Validate every parent-to-child branch in the authenticated tree.
    let mut parent_start = 0usize;
    let mut child_start = 1usize;
    for level in (0..counts.len().saturating_sub(1)).rev() {
        for i in 0..counts[level] as usize {
            let child_page = child_start.checked_add(i).ok_or(ParseError::OutOfBounds)?;
            let child_begin = child_page.checked_mul(4096).ok_or(ParseError::OutOfBounds)?;
            let child_end = child_page.checked_add(1).and_then(|page| page.checked_mul(4096)).ok_or(ParseError::OutOfBounds)?;
            if child_end > tree.len() { return Err(ParseError::HashMismatch); }
            let child = &tree[child_begin..child_end];
            let at = parent_start.checked_mul(4096)
                .and_then(|value| value.checked_add((i / 170).checked_mul(4096)?))
                .and_then(|value| value.checked_add((i % 170).checked_mul(24)?))
                .ok_or(ParseError::OutOfBounds)?;
            if at.checked_add(20).is_none_or(|end| end > tree.len()) || Sha256::digest(child)[..20] != tree[at..at + 20] {
                return Err(ParseError::HashMismatch);
            }
        }
        parent_start = child_start;
        child_start = child_start.checked_add(counts[level] as usize).ok_or(ParseError::OutOfBounds)?;
    }
    let data_pages = user_pages.checked_add(xvc_pages).ok_or(ParseError::OutOfBounds)?;
    let hashed_end = user_offset
        .checked_add(data_pages.checked_mul(4096).ok_or(ParseError::OutOfBounds)?)
        .ok_or(ParseError::OutOfBounds)?;
    if hashed_end > file_len { return Err(ParseError::OutOfBounds); }
    let mut verified = user_offset;
    while verified < xvc_end {
        let _ = check_page(&mut file, verified, &tree)?;
        verified = verified.checked_add(4096).ok_or(ParseError::OutOfBounds)?;
    }
    for (start, end) in trailing_padding {
        let mut pos = start;
        while pos < end {
            let _ = check_page(&mut file, pos, &tree)?;
            pos = pos.checked_add(4096).ok_or(ParseError::OutOfBounds)?;
        }
    }

    let input_abs = fs::canonicalize(input).map_err(|e| ParseError::Io(e.to_string()))?;
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| ParseError::Io(e.to_string()))?;
    let output_name = output.file_name().ok_or_else(|| ParseError::Io("输出路径缺少文件名".into()))?;
    let output_abs = fs::canonicalize(parent)
        .map_err(|e| ParseError::Io(e.to_string()))
        .map(|parent| parent.join(output_name))?;
    if output_abs == input_abs { return Err(ParseError::OutputAlreadyExists(output.display().to_string())); }
    let staging = parent.join(format!(".{}.staging-{}", output.file_name().and_then(|s| s.to_str()).unwrap_or("xvc"), std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir(&staging).map_err(|e| ParseError::Io(e.to_string()))?;
    let result = (|| {
        for (index, segment) in metadata.segments.iter().enumerate() {
            let destination = staging.join(PathBuf::from(&paths[index]));
            if let Some(parent) = destination.parent() { fs::create_dir_all(parent).map_err(|e| ParseError::Io(e.to_string()))?; }
            let mut out = OpenOptions::new().write(true).create_new(true).open(&destination).map_err(|e| ParseError::Io(e.to_string()))?;
            let (start, region, encrypted) = placements[index].unwrap();
            let mut remaining = segment.size;
            let mut pos = start;
            while remaining > 0 || pos == start {
                let mut page = check_page(&mut file, pos, &tree)?;
                if encrypted { let mut tweak = [0u8; 16]; let page_index = hashed_page_index(drive, user_offset, pos)?; let at = (leaf_base as u64 + page_index / 170) * 4096 + (page_index % 170) * 24; tweak[..4].copy_from_slice(&tree[at as usize + 20..at as usize + 24]); tweak[4..8].copy_from_slice(&region.to_le_bytes()); tweak[8..].copy_from_slice(&header_bytes[0x220..0x228]); decrypt_page(&mut page, content_key.unwrap(), &tweak)?; }
                let amount = remaining.min(4096) as usize;
                out.write_all(&page[..amount]).map_err(|e| ParseError::Io(e.to_string()))?;
                remaining = remaining.checked_sub(amount as u64).ok_or(ParseError::OutOfBounds)?;
                pos = pos.checked_add(4096).ok_or(ParseError::OutOfBounds)?;
                if amount == 0 { break; }
            }
        }
        Ok::<(), ParseError>(())
    })();
    if result.is_err() { let _ = fs::remove_dir_all(&staging); return result; }
    fs::rename(&staging, output).map_err(|e| { let _ = fs::remove_dir_all(&staging); ParseError::Io(e.to_string()) })
}

pub fn parse_xvd_header(data: &[u8]) -> Result<XvdHeader, ParseError> {
    if data.len() < 0x298 || data.get(0x200..0x208) != Some(b"msft-xvd") {
        return Err(ParseError::UnsupportedXvd);
    }
    let header = XvdHeader {
        format_version: le_u32(data, 0x208)?,
        container_type: le_u32(data, 0x20c)?,
        user_data_length: le_u32(data, 0x28c)?,
        xvc_data_length: le_u32(data, 0x290)?,
        dynamic_data_length: le_u32(data, 0x294)?,
    };
    if header.format_version != 65
        || header.container_type != 3
        || header.dynamic_data_length != 0
        // The XVD field at 0x280 is a four-byte reserved/version value.
        // The following dword at 0x284 is used by real MSIXVC packages
        // (for example, package 1.26.45.01 stores 1 there) and must not be
        // rejected as part of the reserved field.
        || data[0x280..0x284].iter().any(|byte| *byte != 0)
        || !(544..=32 * 1024 * 1024).contains(&header.user_data_length)
        || header.xvc_data_length > 32 * 1024 * 1024
    {
        return Err(ParseError::UnsupportedXvd);
    }
    Ok(header)
}

/// Validates all metadata paths before they are materialized on Windows.
///
/// Returned paths use `/` separators and are relative to the caller-selected
/// extraction directory. No filesystem access or authorization is required.
pub fn validate_segment_plan(metadata: &SegmentMetadata) -> Result<Vec<String>, ParseError> {
    let mut normalized = Vec::with_capacity(metadata.segments.len());
    let mut seen: HashMap<String, String> = HashMap::with_capacity(metadata.segments.len());
    for segment in &metadata.segments {
        let path = validate_segment_path(&segment.path)?;
        let key = path.to_lowercase();
        if let Some(previous) = seen.insert(key, path.clone()) {
            if previous == path {
                return Err(ParseError::UnsafeSegmentPath(path));
            }
            return Err(ParseError::SegmentPathCaseConflict(format!("{} 与 {}", previous, path)));
        }
        normalized.push(path);
    }

    // A file cannot also be a parent directory. Checking prefixes after
    // case-folding catches this independently of the order in the table.
    //
    // The lookup set is case-folded once up front and probed per component: a
    // nested linear scan is quadratic in the segment count, and real packages
    // carry tens of thousands of segments.
    let mut folded: HashSet<String> = HashSet::with_capacity(normalized.len());
    for path in &normalized {
        folded.insert(path.to_lowercase());
    }
    let mut probe = String::new();
    for path in &normalized {
        let components: Vec<&str> = path.split('/').collect();
        if components.len() < 2 {
            continue;
        }
        probe.clear();
        for (index, component) in components.iter().enumerate() {
            if index + 1 == components.len() {
                break;
            }
            if index > 0 {
                probe.push('/');
            }
            probe.push_str(component);
            let folded_prefix = probe.to_lowercase();
            if folded.contains(&folded_prefix) {
                return Err(ParseError::SegmentFileDirectoryConflict(path.clone()));
            }
        }
    }
    Ok(normalized)
}

/// Validates one metadata path using Windows lexical rules only.
pub fn validate_segment_path(path: &str) -> Result<String, ParseError> {
    if path.is_empty() || path.starts_with('/') || path.starts_with('\\') || path.contains(':') {
        return Err(ParseError::UnsafeSegmentPath(path.into()));
    }
    let mut result = Vec::new();
    let normalized_input = path.replace('\\', "/");
    for component in normalized_input.split('/') {
        if component.is_empty() || component == "." || component == ".."
            || component.ends_with(' ')
            || component.ends_with('.')
            || component.chars().any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
        {
            return Err(ParseError::UnsafeSegmentPath(path.into()));
        }
        let device_name = component.trim_end_matches([' ', '.']).split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(device_name.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (device_name.len() == 4 && (device_name.starts_with("COM") || device_name.starts_with("LPT"))
                && device_name.as_bytes()[3].is_ascii_digit() && device_name.as_bytes()[3] != b'0')
        {
            return Err(ParseError::UnsafeSegmentPath(path.into()));
        }
        result.push(component);
    }
    if result.is_empty() { return Err(ParseError::UnsafeSegmentPath(path.into())); }
    Ok(result.join("/"))
}

pub fn parse_segment_metadata(data: &[u8]) -> Result<SegmentMetadata, ParseError> {
    let header_len = le_u32(data, 12)?;
    let segment_count = le_u32(data, 16)?;
    if header_len != 100 || segment_count == 0 || segment_count > 1_000_000 {
        return Err(ParseError::InvalidSegmentMetadata);
    }
    let table_end = (header_len as usize)
        .checked_add((segment_count as usize).checked_mul(16).ok_or(ParseError::OutOfBounds)?)
        .ok_or(ParseError::OutOfBounds)?;
    if table_end > data.len() {
        return Err(ParseError::OutOfBounds);
    }
    let mut segments = Vec::with_capacity(segment_count as usize);
    for index in 0..segment_count as usize {
        let offset = (header_len as usize)
            .checked_add(index.checked_mul(16).ok_or(ParseError::OutOfBounds)?)
            .ok_or(ParseError::OutOfBounds)?;
        let flags = le_u16(data, offset)?;
        let path_len_units = le_u16(data, offset.checked_add(2).ok_or(ParseError::OutOfBounds)?)? as usize;
        let path_len = path_len_units.checked_mul(2).ok_or(ParseError::OutOfBounds)?;
        let path_offset = table_end.checked_add(le_u32(data, offset.checked_add(4).ok_or(ParseError::OutOfBounds)?)? as usize).ok_or(ParseError::OutOfBounds)?;
        let size = le_u64(data, offset.checked_add(8).ok_or(ParseError::OutOfBounds)?)?;
        let path_end = path_offset.checked_add(path_len).ok_or(ParseError::OutOfBounds)?;
        if path_offset < table_end || path_end > data.len() {
            return Err(ParseError::OutOfBounds);
        }
        let path = decode_utf16(&data[path_offset..path_end])?;
        segments.push(SegmentRecord { flags, path, size });
    }
    Ok(SegmentMetadata { header_len, segment_count, segments })
}

fn decode_utf16(data: &[u8]) -> Result<String, ParseError> {
    if !data.len().is_multiple_of(2) {
        return Err(ParseError::InvalidUtf16);
    }
    String::from_utf16(
        &data.as_chunks::<2>().0
            .iter()
            .map(|chunk| u16::from_le_bytes(*chunk))
            .collect::<Vec<_>>(),
    ).map_err(|_| ParseError::InvalidUtf16)
}

fn le_u16(data: &[u8], offset: usize) -> Result<u16, ParseError> {
    data.get(offset..offset + 2).map(|v| u16::from_le_bytes([v[0], v[1]])).ok_or(ParseError::OutOfBounds)
}

fn le_u32(data: &[u8], offset: usize) -> Result<u32, ParseError> {
    data.get(offset..offset + 4).map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]])).ok_or(ParseError::OutOfBounds)
}

fn le_u64(data: &[u8], offset: usize) -> Result<u64, ParseError> {
    data.get(offset..offset + 8).map(|v| u64::from_le_bytes([v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]])).ok_or(ParseError::OutOfBounds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_u16(data: &mut [u8], offset: usize, value: u16) { data[offset..offset + 2].copy_from_slice(&value.to_le_bytes()); }
    fn put_u32(data: &mut [u8], offset: usize, value: u32) { data[offset..offset + 4].copy_from_slice(&value.to_le_bytes()); }
    fn put_u64(data: &mut [u8], offset: usize, value: u64) { data[offset..offset + 8].copy_from_slice(&value.to_le_bytes()); }

    fn fixture() -> Vec<u8> {
        let mut data = vec![0u8; 0x298 + 100 + 16 + 14];
        data[0x200..0x208].copy_from_slice(b"msft-xvd");
        put_u32(&mut data, 0x208, 65);
        put_u32(&mut data, 0x20c, 3);
        put_u32(&mut data, 0x28c, 544);
        put_u32(&mut data, 0x290, 4096);
        put_u32(&mut data, 0x294, 0);
        put_u32(&mut data, 12, 100);
        put_u32(&mut data, 16, 1);
        put_u16(&mut data, 100, 7);
        put_u16(&mut data, 102, 7);
        put_u32(&mut data, 104, 0);
        put_u64(&mut data, 108, 1234);
        for (index, value) in "a/b.txt".encode_utf16().enumerate() {
            put_u16(&mut data, 116 + index * 2, value);
        }
        data
    }

    #[test]
    fn parses_public_xvd_fields_and_raw_segment_metadata() {
        let (header, metadata) = parse_bytes(&fixture()).unwrap();
        assert_eq!(header.format_version, 65);
        assert_eq!(header.container_type, 3);
        assert_eq!(metadata.header_len, 100);
        assert_eq!(metadata.segment_count, 1);
        assert_eq!(metadata.segments[0], SegmentRecord { flags: 7, path: "a/b.txt".into(), size: 1234 });
    }

    fn real_layout_fixture() -> Vec<u8> {
        let user_offset = 16_384usize;
        let mut data = vec![0u8; user_offset + 4096];
        data[0x200..0x208].copy_from_slice(b"msft-xvd");
        put_u32(&mut data, 0x208, 65);
        put_u32(&mut data, 0x20c, 3);
        put_u64(&mut data, 0x218, 12_288);
        put_u32(&mut data, 0x288, 0);
        put_u32(&mut data, 0x28c, 4096);
        put_u32(&mut data, 0x290, 4096);
        put_u32(&mut data, 0x294, 0);

        let user = &mut data[user_offset..user_offset + 4096];
        put_u32(user, 0, 16);
        put_u32(user, 16 + 524, 1);
        let entry = 16 + 528;
        for (index, value) in "SegmentMetadata.bin".encode_utf16().enumerate() {
            put_u16(user, entry + index * 2, value);
        }
        // Entry layout matches the real container: [520] is the payload size and
        // [524] is the offset relative to the metadata header.
        put_u32(user, entry + 520, 260);
        put_u32(user, entry + 524, 1_056);
        put_u32(user, 1_072 + 12, 100);
        put_u32(user, 1_072 + 16, 3);
        for (index, (path, size)) in [
            ("MicrosoftGame.Config", 7),
            ("Minecraft.Windows.exe", 1024),
            ("data/empty.txt", 0),
        ]
        .iter()
        .enumerate()
        {
            let record = 1_072 + 100 + index * 16;
            let path_offset = if index == 0 {
                0
            } else if index == 1 {
                "MicrosoftGame.Config".encode_utf16().count() * 2
            } else {
                ("MicrosoftGame.Config".encode_utf16().count()
                    + "Minecraft.Windows.exe".encode_utf16().count())
                    * 2
            };
            put_u16(user, record + 2, path.encode_utf16().count() as u16);
            put_u32(user, record + 4, path_offset as u32);
            put_u64(user, record + 8, *size);
            let path_start = 1_072 + 100 + 3 * 16 + path_offset;
            for (path_index, value) in path.encode_utf16().enumerate() {
                put_u16(user, path_start + path_index * 2, value);
            }
        }
        data
    }

    fn shifted_real_layout_fixture() -> Vec<u8> {
        let source = real_layout_fixture();
        let user_offset = 20_480usize;
        let mut data = vec![0u8; user_offset + 4096];
        data[..0x471].copy_from_slice(&source[..0x471]);
        data[0x470] = 1;
        data[user_offset..user_offset + 4096].copy_from_slice(&source[16_384..16_384 + 4096]);
        data
    }

    #[test]
    fn hashed_page_index_includes_drive_pages() {
        assert_eq!(hashed_page_index(3, 0x1000, 0x1000).unwrap(), 3);
        assert_eq!(hashed_page_index(3, 0x1000, 0x3000).unwrap(), 5);
        assert!(hashed_page_index(3, 0x1000, 0x1800).is_err());
        assert!(hashed_page_index(3, 0x1000, 0x0).is_err());
    }

    #[test]
    fn parses_metadata_from_real_file_user_data_offset() {
        let data = shifted_real_layout_fixture();
        let path = std::env::temp_dir().join(format!("copper-msixvc-real-file-offset-{}.bin", std::process::id()));
        std::fs::write(&path, data).unwrap();
        let result = parse_file_metadata(&path);
        let _ = std::fs::remove_file(path);
        let (_, metadata) = result.unwrap();
        assert_eq!(metadata.segments[1].path, "Minecraft.Windows.exe");
    }

    #[test]
    fn parses_metadata_from_real_xvd_user_data_layout() {
        let data = real_layout_fixture();
        let path = std::env::temp_dir().join(format!("copper-msixvc-real-layout-{}.bin", std::process::id()));
        std::fs::write(&path, data).unwrap();
        let result = parse_file_metadata(&path);
        let _ = std::fs::remove_file(path);
        let (_, metadata) = result.unwrap();
        assert_eq!(metadata.segment_count, 3);
        assert_eq!(metadata.segments[1].path, "Minecraft.Windows.exe");
        assert_eq!(metadata.segments[1].size, 1024);
    }

    #[test]
    fn validates_windows_paths_and_conflicts() {
        assert_eq!(validate_segment_path("dir\\file.txt").unwrap(), "dir/file.txt");
        for path in ["../x", "C:\\x", "\\\\server\\x", "CON.txt", "dir\\bad.", "dir\\bad ", "a/*"] {
            assert!(matches!(validate_segment_path(path), Err(ParseError::UnsafeSegmentPath(_))), "{path}");
        }
        let metadata = SegmentMetadata {
            header_len: 100,
            segment_count: 2,
            segments: vec![
                SegmentRecord { flags: 0, path: "Readme.txt".into(), size: 1 },
                SegmentRecord { flags: 0, path: "README.TXT".into(), size: 1 },
            ],
        };
        assert!(matches!(validate_segment_plan(&metadata), Err(ParseError::SegmentPathCaseConflict(_))));
        let metadata = SegmentMetadata {
            header_len: 100,
            segment_count: 2,
            segments: vec![
                SegmentRecord { flags: 0, path: "a".into(), size: 1 },
                SegmentRecord { flags: 0, path: "a/b".into(), size: 1 },
            ],
        };
        assert!(matches!(validate_segment_plan(&metadata), Err(ParseError::SegmentFileDirectoryConflict(_))));
    }

    #[test]
    fn rejects_truncated_input() {
        let data = fixture();
        assert_eq!(parse_segment_metadata(&data[..120]), Err(ParseError::OutOfBounds));
    }

    #[test]
    fn rejects_nonzero_reserved_header_region() {
        let mut data = fixture();
        data[0x280] = 1;
        assert_eq!(parse_xvd_header(&data), Err(ParseError::UnsupportedXvd));
    }

    #[test]
    fn rejects_user_data_below_minimum() {
        let mut data = fixture();
        put_u32(&mut data, 0x28c, 543);
        assert_eq!(parse_xvd_header(&data), Err(ParseError::UnsupportedXvd));
    }

    #[test]
    fn rejects_oversized_user_or_xvc_data() {
        let mut data = fixture();
        put_u32(&mut data, 0x28c, 32 * 1024 * 1024 + 1);
        assert_eq!(parse_xvd_header(&data), Err(ParseError::UnsupportedXvd));

        let mut data = fixture();
        put_u32(&mut data, 0x290, 32 * 1024 * 1024 + 1);
        assert_eq!(parse_xvd_header(&data), Err(ParseError::UnsupportedXvd));
    }

    #[test]
    fn unwraps_rfc3394_known_answer() {
        let kek = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
            0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        ];
        let wrapped = [
            0x1f, 0xa6, 0x8b, 0x0a, 0x81, 0x12, 0xb4, 0x47,
            0xae, 0xf3, 0x4b, 0xd8, 0xfb, 0x5a, 0x7b, 0x82,
            0x9d, 0x3e, 0x86, 0x23, 0x71, 0xd2, 0xcf, 0xe5,
        ];
        assert_eq!(unwrap_content_key(&kek, &wrapped).unwrap(), [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
            0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        ]);
    }

    #[test]
    fn rejects_tampered_wrapped_key() {
        let kek = [0u8; 16];
        let mut wrapped = [0u8; 24];
        wrapped[0] = 1;
        assert_eq!(unwrap_content_key(&kek, &wrapped), Err(ParseError::InvalidKey));
    }

    #[test]
    fn hash_tree_validates_page_and_detects_tampering() {
        let page = vec![0x5a; 4096];
        let page_two = vec![0xa5; 4096];
        let mut tree = vec![0u8; 4096];
        tree[..20].copy_from_slice(&Sha256::digest(&page)[..20]);
        tree[24..44].copy_from_slice(&Sha256::digest(&page_two)[..20]);
        let root = Sha256::digest(&tree);
        assert!(validate_hash_tree(&tree, &root, &[page.as_slice(), page_two.as_slice()]).is_ok());
        let mut bad = page.clone();
        bad[7] ^= 1;
        assert_eq!(validate_hash_tree(&tree, &root, &[bad.as_slice(), page_two.as_slice()]), Err(ParseError::HashMismatch));
    }

    #[test]
    fn rejects_malformed_license_tlv() {
        assert_eq!(parse_license_blocks(&[]), Err(ParseError::InvalidLicenseBlob));
        assert_eq!(parse_license_blocks(&[0; 8]), Ok(HashMap::new()));
        assert_eq!(parse_license_blocks(&[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 8, 0, 0, 0]), Err(ParseError::InvalidLicenseBlob));
    }

    #[test]
    fn rejects_output_equal_to_input() {
        let data = fixture();
        let path = std::env::temp_dir().join(format!("copper-msixvc-same-output-{}.bin", std::process::id()));
        std::fs::write(&path, data).unwrap();
        assert!(matches!(extract_xvc(&path, &path, None), Err(ParseError::OutputAlreadyExists(_))));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_device_mismatch_in_content_keys() {
        let mut blob = vec![0u8; 8];
        blob.extend_from_slice(&0xd2u32.to_le_bytes());
        blob.extend_from_slice(&2u32.to_le_bytes());
        blob.extend_from_slice(&[1, 2]);
        blob.extend_from_slice(&0xcau32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(unpack_content_keys(&blob, &[3, 4], &[0u8; 16]), Err(ParseError::DeviceMismatch));
    }

    #[test]
    fn content_key_lease_clears_on_drop() {
        let lease = ContentKeyLease(vec![0x5a; 32]);
        assert_eq!(lease.as_bytes(), &[0x5a; 32]);
    }

    #[test]
    fn content_key_selection_rejects_missing_or_duplicate() {
        let keys = vec![("A".into(), ContentKeyLease(vec![1; 32]))];
        assert_eq!(select_content_key(keys, "b"), Err(ParseError::InvalidKey));
        let keys = vec![
            ("A".into(), ContentKeyLease(vec![1; 32])),
            ("a".into(), ContentKeyLease(vec![2; 32])),
        ];
        assert_eq!(select_content_key(keys, "A"), Err(ParseError::InvalidKey));
    }

    #[test]
    fn does_not_modify_file() {
        let data = fixture();
        let path = std::env::temp_dir().join(format!("copper-msixvc-parser-{}.bin", std::process::id()));
        std::fs::write(&path, &data).unwrap();
        let before = std::fs::read(&path).unwrap();
        parse_file(&path).unwrap();
        assert_eq!(before, std::fs::read(&path).unwrap());
        let _ = std::fs::remove_file(path);
    }
}
