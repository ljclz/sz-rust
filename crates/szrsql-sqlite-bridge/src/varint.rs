//! SQLite varint（变长整数）编解码。
//!
//! SQLite 使用大端变长整数编码，最多 9 字节。编码规则：
//! - 前 1-8 字节：高位 bit=1 表示后续还有字节，低 7 bit 为数据
//! - 第 9 字节：全部 8 bit 都为数据（无高位标志）
//!
//! # 容量
//!
//! | 字节数 | 数据位数 | 可表示范围 |
//! |--------|----------|------------|
//! | 1 | 7 | 0..=127 |
//! | 2 | 14 | 0..=16383 |
//! | ... | ... | ... |
//! | 8 | 56 | 0..=2^56-1 |
//! | 9 | 64 | 0..=2^64-1（全量 u64）|

// =====================================================================
//  编码
// =====================================================================

/// 将 `u64` 编码为 SQLite varint 字节序列。
///
/// 选择能表示该值的最短字节数。当值超过 56 位（2^56-1）时使用 9 字节编码。
pub fn encode_varint(value: u64) -> Vec<u8> {
    // 值超过 56 位 → 需要 9 字节编码
    if value >= (1u64 << 56) {
        let mut buf = vec![0u8; 9];
        // 前 8 字节：各取 7 bit，高位置 1
        // 第 9 字节：取低 8 bit
        for i in 0..8usize {
            // shift 依次为 57, 50, 43, 36, 29, 22, 15, 8
            let shift = 8 + 7 * (7 - i);
            buf[i] = ((value >> shift) as u8 & 0x7F) | 0x80;
        }
        buf[8] = value as u8;
        return buf;
    }

    // 1-8 字节：找到最小的 n 使得 value < 2^(7*n)
    let mut n = 1usize;
    while n < 8 && value >= (1u64 << (7 * n)) {
        n += 1;
    }

    let mut buf = vec![0u8; n];
    for i in 0..n {
        // 大端序：最高有效位在前
        let shift = 7 * (n - 1 - i);
        buf[i] = ((value >> shift) as u8) & 0x7F;
        // 除最后一字节外，高位置 1 表示后续还有字节
        if i < n - 1 {
            buf[i] |= 0x80;
        }
    }
    buf
}

// =====================================================================
//  解码
// =====================================================================

/// 从字节切片解码 SQLite varint。
///
/// # 返回
/// - `Some((value, bytes_consumed))`：解码成功
/// - `None`：缓冲区不足或数据不完整
pub fn decode_varint(buf: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    for i in 0..9usize {
        if i >= buf.len() {
            return None; // 缓冲区不足
        }
        let byte = buf[i];
        if i == 8 {
            // 第 9 字节：全部 8 bit 为数据
            result = (result << 8) | (byte as u64);
            return Some((result, 9));
        } else {
            // 前 8 字节：取低 7 bit
            result = (result << 7) | ((byte & 0x7F) as u64);
            if byte & 0x80 == 0 {
                // 高位为 0，结束
                return Some((result, i + 1));
            }
        }
    }
    // 理论上不可达：循环最多到 i=8 时已返回
    None
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  编码测试
    // -----------------------------------------------------------------

    #[test]
    fn encode_zero_uses_one_byte() {
        // 0 → 单字节 0x00
        assert_eq!(encode_varint(0), vec![0x00]);
    }

    #[test]
    fn encode_small_value_one_byte() {
        // 0..=127 → 单字节
        assert_eq!(encode_varint(1), vec![0x01]);
        assert_eq!(encode_varint(127), vec![0x7F]);
    }

    #[test]
    fn encode_128_uses_two_bytes() {
        // 128 → 2 字节：0x81 0x00
        assert_eq!(encode_varint(128), vec![0x81, 0x00]);
    }

    #[test]
    fn encode_16383_uses_two_bytes() {
        // 16383 = 2^14 - 1 → 2 字节最大值
        assert_eq!(encode_varint(16383), vec![0xFF, 0x7F]);
    }

    #[test]
    fn encode_16384_uses_three_bytes() {
        // 16384 = 2^14 → 3 字节
        assert_eq!(encode_varint(16384), vec![0x81, 0x80, 0x00]);
    }

    #[test]
    fn encode_56_bit_value_uses_eight_bytes() {
        // 2^56 - 1 → 8 字节（前7字节高位为1，第8字节高位为0）
        let val = (1u64 << 56) - 1;
        let encoded = encode_varint(val);
        assert_eq!(encoded.len(), 8);
        // 前 7 字节高位为 1
        for &b in &encoded[0..7] {
            assert_eq!(b & 0x80, 0x80);
        }
        // 第 8 字节高位为 0
        assert_eq!(encoded[7] & 0x80, 0x00);
    }

    #[test]
    fn encode_2_pow_56_uses_nine_bytes() {
        // 2^56 → 9 字节
        let val = 1u64 << 56;
        let encoded = encode_varint(val);
        assert_eq!(encoded.len(), 9);
        // 前 8 字节高位为 1
        for &b in &encoded[0..8] {
            assert_eq!(b & 0x80, 0x80);
        }
    }

    #[test]
    fn encode_u64_max_uses_nine_bytes() {
        // u64::MAX → 9 字节
        let encoded = encode_varint(u64::MAX);
        assert_eq!(encoded.len(), 9);
        // 前 8 字节均为 0xFF
        for &b in &encoded[0..8] {
            assert_eq!(b, 0xFF);
        }
        // 第 9 字节为 0xFF
        assert_eq!(encoded[8], 0xFF);
    }

    // -----------------------------------------------------------------
    //  解码测试
    // -----------------------------------------------------------------

    #[test]
    fn decode_zero_one_byte() {
        assert_eq!(decode_varint(&[0x00]), Some((0, 1)));
    }

    #[test]
    fn decode_small_value_one_byte() {
        assert_eq!(decode_varint(&[0x01]), Some((1, 1)));
        assert_eq!(decode_varint(&[0x7F]), Some((127, 1)));
    }

    #[test]
    fn decode_128_two_bytes() {
        assert_eq!(decode_varint(&[0x81, 0x00]), Some((128, 2)));
    }

    #[test]
    fn decode_16383_two_bytes() {
        assert_eq!(decode_varint(&[0xFF, 0x7F]), Some((16383, 2)));
    }

    #[test]
    fn decode_u64_max_nine_bytes() {
        let encoded = encode_varint(u64::MAX);
        let (val, len) = decode_varint(&encoded).expect("decode should succeed");
        assert_eq!(val, u64::MAX);
        assert_eq!(len, 9);
    }

    #[test]
    fn decode_empty_buffer_returns_none() {
        assert_eq!(decode_varint(&[]), None);
    }

    #[test]
    fn decode_truncated_multi_byte_returns_none() {
        // 0x81 表示后续还有字节，但缓冲区只有 1 字节
        assert_eq!(decode_varint(&[0x81]), None);
    }

    // -----------------------------------------------------------------
    //  往返测试
    // -----------------------------------------------------------------

    #[test]
    fn roundtrip_all_byte_lengths() {
        // 测试 1-9 字节边界值
        let test_values: &[u64] = &[
            0,
            1,
            127,
            128,
            16383,
            16384,
            2097151,
            2097152,
            268435455,
            268435456,
            34359738367,
            34359738368,
            4398046511103,
            4398046511104,
            562949953421311,
            562949953421312,
            u64::MAX,
        ];
        for &val in test_values {
            let encoded = encode_varint(val);
            let (decoded, len) = decode_varint(&encoded)
                .unwrap_or_else(|| panic!("decode failed for value {val}, encoded={encoded:?}"));
            assert_eq!(val, decoded, "roundtrip mismatch for value {val}");
            assert_eq!(len, encoded.len(), "length mismatch for value {val}");
        }
    }

    #[test]
    fn roundtrip_random_values() {
        // 测试一些任意值
        let test_values: &[u64] = &[
            42,
            1000,
            1000000,
            1000000000,
            1000000000000,
            1000000000000000,
            12345678901234567,
            0xDEAD_BEEF_CAFE_BABE,
        ];
        for &val in test_values {
            let encoded = encode_varint(val);
            let (decoded, _) = decode_varint(&encoded).expect("decode should succeed");
            assert_eq!(val, decoded);
        }
    }

    // -----------------------------------------------------------------
    //  解码后剩余字节测试
    // -----------------------------------------------------------------

    #[test]
    fn decode_consumes_correct_bytes_leaving_rest() {
        // 编码 300 后拼接额外字节，解码应只消费 2 字节
        let encoded = encode_varint(300);
        let mut buf = encoded.clone();
        buf.extend_from_slice(&[0xFF, 0xFF]);
        let (val, len) = decode_varint(&buf).expect("decode should succeed");
        assert_eq!(val, 300);
        assert_eq!(len, encoded.len());
        // 剩余字节未被消费
        assert_eq!(&buf[len..], &[0xFF, 0xFF]);
    }

    // -----------------------------------------------------------------
    //  SQLite 规范一致性测试
    // -----------------------------------------------------------------

    #[test]
    fn sqlite_spec_example_values() {
        // SQLite 文档中的示例值
        // 0x00 → 0
        assert_eq!(decode_varint(&[0x00]), Some((0, 1)));
        // 0x7F → 127
        assert_eq!(decode_varint(&[0x7F]), Some((127, 1)));
        // 0x81 0x00 → 128
        assert_eq!(decode_varint(&[0x81, 0x00]), Some((128, 2)));
        // 0x82 0x00 → 256
        assert_eq!(decode_varint(&[0x82, 0x00]), Some((256, 2)));
        // 0x81 0x01 → 129
        assert_eq!(decode_varint(&[0x81, 0x01]), Some((129, 2)));
    }
}
