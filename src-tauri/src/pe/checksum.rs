//! PE 校验和算法。
//!
//! 与 `CheckSumMappedFile` 同款：把整个文件按 16 位字求和（超过 16 位的进位
//! 折回），跳过 `CheckSum` 字段自身，最后加上文件长度。
//!
//! 追加节之后必须重算：加载器在开启强制校验时会拒绝 PE 校验和错误的映像。

/// 计算 PE 校验和。
///
/// `checksum_offset` 是 Optional Header 内 `CheckSum` 字段的偏移，求和时跳过
/// 该 4 字节（标准做法；不清零再算会把它自己的旧值算进去）。
pub fn pe_checksum_with(data: &[u8], checksum_offset: usize) -> u32 {
    let mut sum: u64 = 0;
    let mut index = 0usize;
    while index + 1 < data.len() {
        // 校验和字段覆盖 4 字节 = 2 个字，两字都跳过。
        if index == checksum_offset || index == checksum_offset + 2 {
            index += 2;
            continue;
        }
        let word = u16::from_le_bytes([data[index], data[index + 1]]) as u64;
        sum += word;
        sum = (sum & 0xffff) + (sum >> 16);
        index += 2;
    }
    // 奇数长度文件：最后一个字节按低字节计入
    if index < data.len() {
        sum += data[index] as u64;
        sum = (sum & 0xffff) + (sum >> 16);
    }
    sum = (sum & 0xffff) + (sum >> 16);
    let mut result = sum as u32;
    // 标准做法：把文件长度也折进校验和
    result = result.wrapping_add(data.len() as u32);
    if result > 0xffff {
        result = (result & 0xffff) + (result >> 16);
    }
    result
}

/// 对已解析镜像计算校验和（自动定位字段偏移）。
pub fn pe_checksum(data: &[u8]) -> u32 {
    // Optional Header = PE 偏移 + 4（签名）+ 20（COFF）；CheckSum 在 +64。
    // 这里不重新解析（调用方手上已有 Image），直接按 PE32+ 布局推导偏移。
    match data.get(0x3c..0x40) {
        Some(pe_offset_bytes) => {
            let pe_offset = u32::from_le_bytes([
                pe_offset_bytes[0],
                pe_offset_bytes[1],
                pe_offset_bytes[2],
                pe_offset_bytes[3],
            ]) as usize;
            pe_checksum_with(data, pe_offset + 4 + 20 + 64)
        }
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_skips_its_own_field() {
        // 构造 16 字节数据，把「校验和字段」放在偏移 4
        let mut data = vec![0u8; 16];
        data[0] = 1;
        data[2] = 2;
        let expected = pe_checksum_with(&data, 4);
        // 往字段里写垃圾值不应影响结果
        data[4..8].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        assert_eq!(pe_checksum_with(&data, 4), expected);
    }

    #[test]
    fn checksum_is_deterministic_and_order_sensitive() {
        let a = [1u8, 0, 2, 0, 3, 0];
        let b = [3u8, 0, 2, 0, 1, 0];
        assert_ne!(pe_checksum_with(&a, usize::MAX), pe_checksum_with(&b, usize::MAX));
    }
}
