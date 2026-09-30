//! Binary AXML (`AndroidManifest.xml`) reader.
//!
//! Why this exists: the launcher's Android path must know the *real*
//! `package` / `versionName` / `versionCode` of an imported MCBE APK.
//! `versionName` is not cosmetic — it selects which native library set the
//! game runtime loads (see `CopperGameRuntimePreparer`). Trusting the
//! frontend for those values would let a wrong value reach `System.load`
//! ordering and crash with no actionable error.
//!
//! Only the subset needed for the manifest `<manifest>` element is parsed:
//! the string pool plus a linear scan of start-element chunks. Attribute
//! values are read from the typed value, so `versionCode` is an integer and
//! `versionName` a string without guessing from a raw index.

use crate::error::KernelError;

const RES_STRING_POOL_TYPE: u16 = 0x0001;
const RES_XML_START_ELEMENT_TYPE: u16 = 0x0102;
const RES_XML_END_ELEMENT_TYPE: u16 = 0x0103;

const POOL_FLAG_UTF8: u32 = 1 << 8;

const ANDROID_NAMESPACE: &str = "http://schemas.android.com/apk/res/android";

const TYPE_STRING: u8 = 0x03;
const TYPE_INT_DEC: u8 = 0x10;
const TYPE_INT_HEX: u8 = 0x11;
const TYPE_INT_BOOLEAN: u8 = 0x12;

/// The identity fields CopperGolem needs from a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidManifestIdentity {
    pub package_name: String,
    pub version_name: String,
    pub version_code: u64,
}

fn invalid(message: impl Into<String>) -> KernelError {
    KernelError::InvalidArgument(message.into())
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], KernelError> {
        if self.remaining() < len {
            return Err(invalid("AndroidManifest.xml 意外截断"));
        }
        let slice = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16, KernelError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, KernelError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn peek_u16(&self) -> Result<u16, KernelError> {
        if self.remaining() < 2 {
            return Err(invalid("AndroidManifest.xml 意外截断"));
        }
        Ok(u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]))
    }

    fn peek_u8(&self) -> Result<u8, KernelError> {
        if self.remaining() < 1 {
            return Err(invalid("AndroidManifest.xml 意外截断"));
        }
        Ok(self.data[self.pos])
    }
}

struct StringPool {
    strings: Vec<String>,
    utf8: bool,
}

impl StringPool {
    fn parse(chunk: &[u8]) -> Result<Self, KernelError> {
        if chunk.len() < 28 {
            return Err(invalid("AndroidManifest.xml 字符串池头部损坏"));
        }
        let string_count = u32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]) as usize;
        let flags = u32::from_le_bytes([chunk[16], chunk[17], chunk[18], chunk[19]]);
        let strings_start = u32::from_le_bytes([chunk[20], chunk[21], chunk[22], chunk[23]]) as usize;
        let utf8 = flags & POOL_FLAG_UTF8 != 0;

        if string_count == 0 {
            return Ok(Self { strings: Vec::new(), utf8 });
        }
        if strings_start < 28 || strings_start > chunk.len() {
            return Err(invalid("AndroidManifest.xml 字符串池偏移越界"));
        }

        let mut strings = Vec::with_capacity(string_count);
        for index in 0..string_count {
            let off_pos = 28 + index * 4;
            if off_pos + 4 > chunk.len() {
                return Err(invalid("AndroidManifest.xml 字符串偏移表越界"));
            }
            let offset = u32::from_le_bytes([
                chunk[off_pos],
                chunk[off_pos + 1],
                chunk[off_pos + 2],
                chunk[off_pos + 3],
            ]) as usize;
            let abs = strings_start
                .checked_add(offset)
                .ok_or_else(|| invalid("AndroidManifest.xml 字符串偏移溢出"))?;
            if abs >= chunk.len() {
                return Err(invalid("AndroidManifest.xml 字符串数据越界"));
            }
            strings.push(read_pool_string(chunk, abs, utf8)?);
        }
        Ok(Self { strings, utf8 })
    }

    fn get(&self, index: u32) -> Result<&str, KernelError> {
        self.strings
            .get(index as usize)
            .map(|s| s.as_str())
            .ok_or_else(|| invalid(format!("AndroidManifest.xml 字符串索引越界: {index}")))
    }

    /// Byte length of one encoded entry, used to walk style/spacing tables.
    #[allow(dead_code)]
    fn is_utf8(&self) -> bool {
        self.utf8
    }
}

fn read_pool_string(chunk: &[u8], offset: usize, utf8: bool) -> Result<String, KernelError> {
    let mut reader = Reader::new(&chunk[offset..]);
    if utf8 {
        read_utf8_string(&mut reader)
    } else {
        read_utf16_string(&mut reader)
    }
}

/// UTF-8 pool entry: char count (1–2 bytes), byte count (1–2 bytes), bytes, NUL.
fn read_utf8_string(reader: &mut Reader<'_>) -> Result<String, KernelError> {
    let _char_len = read_utf8_length(reader)?;
    let byte_len = read_utf8_length(reader)?;
    let bytes = reader.take(byte_len)?;
    String::from_utf8(bytes.to_vec()).map_err(|_| invalid("AndroidManifest.xml 字符串不是合法 UTF-8"))
}

/// UTF-16 pool entry: unit count (1–2 units), UTF-16LE units, NUL unit.
fn read_utf16_string(reader: &mut Reader<'_>) -> Result<String, KernelError> {
    let first = reader.u16()?;
    let unit_len = if first & 0x8000 != 0 {
        let second = reader.u16()?;
        (((first & 0x7FFF) as usize) << 16) | second as usize
    } else {
        first as usize
    };
    let bytes = reader.take(unit_len * 2)?;
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).map_err(|_| invalid("AndroidManifest.xml 字符串不是合法 UTF-16"))
}

fn read_utf8_length(reader: &mut Reader<'_>) -> Result<usize, KernelError> {
    let first = reader.peek_u8()?;
    if first & 0x80 == 0 {
        reader.take(1)?;
        Ok(first as usize)
    } else {
        let second = reader.peek_u16()? as u8;
        reader.take(2)?;
        Ok((((first & 0x7F) as usize) << 8) | second as usize)
    }
}

/// Read `package` / `versionName` / `versionCode` from a binary manifest.
pub fn read_identity(manifest_xml: &[u8]) -> Result<AndroidManifestIdentity, KernelError> {
    if manifest_xml.len() < 8 {
        return Err(invalid("AndroidManifest.xml 为空"));
    }
    let file_type = u16::from_le_bytes([manifest_xml[0], manifest_xml[1]]);
    if file_type != RES_STRING_POOL_TYPE && file_type != 0x0003 {
        return Err(invalid("AndroidManifest.xml 不是二进制 AXML 格式"));
    }

    let mut pool: Option<StringPool> = None;
    let mut namespaces: Vec<(u32, String)> = Vec::new();
    let mut pos = 8usize;

    while pos + 8 <= manifest_xml.len() {
        let header = &manifest_xml[pos..pos + 8];
        let chunk_type = u16::from_le_bytes([header[0], header[1]]);
        let header_size = u16::from_le_bytes([header[2], header[3]]) as usize;
        let chunk_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        if chunk_size < 8 || pos + chunk_size > manifest_xml.len() {
            break;
        }
        let body = &manifest_xml[pos..pos + chunk_size];
        let body_start = header_size.min(chunk_size);

        match chunk_type {
            RES_STRING_POOL_TYPE => {
                pool = Some(StringPool::parse(body)?);
            }
            // start-namespace: uri string index, prefix string index
            0x0100 => {
                if body.len() >= body_start + 8 {
                    let uri = u32::from_le_bytes([
                        body[body_start],
                        body[body_start + 1],
                        body[body_start + 2],
                        body[body_start + 3],
                    ]);
                    let prefix = u32::from_le_bytes([
                        body[body_start + 4],
                        body[body_start + 5],
                        body[body_start + 6],
                        body[body_start + 7],
                    ]);
                    if let Some(pool) = pool.as_ref() {
                        namespaces.push((prefix, pool.get(uri)?.to_string()));
                    }
                }
            }
            RES_XML_START_ELEMENT_TYPE => {
                let pool_ref = pool
                    .as_ref()
                    .ok_or_else(|| invalid("AndroidManifest.xml 缺少字符串池"))?;
                let element = parse_start_element(body, body_start, pool_ref, &namespaces)?;
                if let Some(identity) = element {
                    return Ok(identity);
                }
            }
            RES_XML_END_ELEMENT_TYPE => {
                // A well-formed manifest always yields `manifest` before this.
                if !namespaces.is_empty() {
                    break;
                }
            }
            _ => {}
        }
        pos += chunk_size;
    }

    Err(invalid("AndroidManifest.xml 中未找到 <manifest> 元素"))
}

/// Attributes of one start-element chunk.
struct StartElement {
    name: String,
    attributes: Vec<(Option<String>, String, u8, u32)>,
}

fn parse_start_element(
    body: &[u8],
    body_start: usize,
    pool: &StringPool,
    namespaces: &[(u32, String)],
) -> Result<Option<AndroidManifestIdentity>, KernelError> {
    if body.len() < body_start + 20 {
        return Err(invalid("AndroidManifest.xml 元素节点损坏"));
    }
    let name_index = u32::from_le_bytes([
        body[body_start + 4],
        body[body_start + 5],
        body[body_start + 6],
        body[body_start + 7],
    ]);
    let attribute_start = u16::from_le_bytes([
        body[body_start + 8],
        body[body_start + 9],
    ]) as usize;
    let attribute_size = u16::from_le_bytes([
        body[body_start + 10],
        body[body_start + 11],
    ]) as usize;
    let attribute_count = u16::from_le_bytes([
        body[body_start + 12],
        body[body_start + 13],
    ]) as usize;

    let element = read_element(body, body_start, attribute_start, attribute_size, attribute_count, pool, namespaces)?;
    if element.name != "manifest" {
        return Ok(None);
    }

    let mut identity = AndroidManifestIdentity {
        package_name: String::new(),
        version_name: String::new(),
        version_code: 0,
    };
    for (ns, attr_name, data_type, data) in element.attributes {
        let is_android = ns.as_deref() == Some(ANDROID_NAMESPACE) || ns.is_none();
        if !is_android {
            continue;
        }
        match attr_name.as_str() {
            "package" if data_type == TYPE_STRING => {
                identity.package_name = pool.get(data)?.to_string();
            }
            "versionName" if data_type == TYPE_STRING => {
                identity.version_name = pool.get(data)?.to_string();
            }
            "versionCode" if matches!(data_type, TYPE_INT_DEC | TYPE_INT_HEX) => {
                identity.version_code = data as u64;
            }
            _ => {}
        }
    }
    if identity.package_name.is_empty() {
        return Err(invalid("AndroidManifest.xml 的 <manifest> 缺少 package"));
    }
    Ok(Some(identity))
}

#[allow(clippy::too_many_arguments)]
fn read_element(
    body: &[u8],
    body_start: usize,
    attribute_start: usize,
    attribute_size: usize,
    attribute_count: usize,
    pool: &StringPool,
    namespaces: &[(u32, String)],
) -> Result<StartElement, KernelError> {
    let name = pool
        .get(u32::from_le_bytes([
            body[body_start + 4],
            body[body_start + 5],
            body[body_start + 6],
            body[body_start + 7],
        ]))?
        .to_string();

    if attribute_count == 0 || attribute_size == 0 {
        return Ok(StartElement { name, attributes: Vec::new() });
    }

    let base = body_start
        .checked_add(attribute_start)
        .ok_or_else(|| invalid("AndroidManifest.xml 属性偏移溢出"))?;
    if base + attribute_count * attribute_size > body.len() {
        return Err(invalid("AndroidManifest.xml 属性区越界"));
    }

    let mut attributes = Vec::with_capacity(attribute_count as usize);
    for index in 0..attribute_count as usize {
        let at = base + index * attribute_size;
        let ns_index = u32::from_le_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]);
        let name_index = u32::from_le_bytes([body[at + 4], body[at + 5], body[at + 6], body[at + 7]]);
        // raw_value at +8 is intentionally unused: typed values are authoritative.
        let data_type = body[at + 15];
        let data = u32::from_le_bytes([body[at + 16], body[at + 17], body[at + 18], body[at + 19]]);

        let ns = if ns_index == 0xFFFFFFFF {
            None
        } else {
            namespaces
                .iter()
                .find(|(prefix, _)| *prefix == ns_index)
                .map(|(_, uri)| uri.clone())
        };
        attributes.push((ns, pool.get(name_index)?.to_string(), data_type, data));
    }
    Ok(StartElement { name, attributes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_non_axml() {
        assert!(read_identity(&[]).is_err());
        assert!(read_identity(&[0x02, 0x00, 0x0C, 0x00, 0, 0, 0, 0]).is_err());
        assert!(read_identity(&[0x03, 0x00, 0x08, 0x00, 0x08, 0, 0, 0]).is_err());
    }

    #[test]
    fn reads_utf8_length_encoding() {
        // single byte length 5
        let mut reader = Reader::new(&[5, 1, 2, 3, 4, 5]);
        assert_eq!(read_utf8_length(&mut reader).unwrap(), 5);
        // two byte length: 0x80|1, 0x02 -> (1 << 8) | 2
        let mut reader = Reader::new(&[0x81, 0x02]);
        assert_eq!(read_utf8_length(&mut reader).unwrap(), 258);
    }

    #[test]
    fn reads_utf16_length_encoding() {
        let bytes: Vec<u8> = "ab".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let mut input = vec![2, 0];
        input.extend_from_slice(&bytes);
        input.extend_from_slice(&[0, 0]);
        let mut reader = Reader::new(&input);
        assert_eq!(read_utf16_string(&mut reader).unwrap(), "ab");
    }
}
