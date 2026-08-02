//! SQLite Record 格式编解码。
//!
//! SQLite Record 由 header 和 body 两部分组成：
//!
//! ```text
//! ┌───────────────────┬───────────────────────────┐
//! │      Header       │           Body            │
//! ├───────────────────┼───────────────────────────┤
//! │ varint(header_sz) │ serial_type varint 序列   │ 值数据序列 │
//! └───────────────────┴───────────────────────────┘
//! ```
//!
//! - **Header**：以 varint 开头表示 header 总长度（含自身），后接各值的 serial type varint
//! - **Body**：按 serial type 顺序排列的值字节串

use szrsql_types::value::Value;

use crate::serial_type::{decode_value, encode_value, serial_type_payload_len};
use crate::varint::{decode_varint, encode_varint};

// =====================================================================
//  编码
// =====================================================================

/// 将一组 `Value` 编码为 SQLite Record 字节序列。
///
/// # 格式
/// `[varint(header_size)] [serial_type varints...] [body...]`
///
/// `header_size` 包含自身的 varint 长度 + 所有 serial type varint 的总长度。
pub fn encode_record(values: &[Value]) -> Vec<u8> {
    // 第一步：编码每个值，得到 (serial_type, payload) 列表
    let encoded: Vec<(u64, Vec<u8>)> = values.iter().map(encode_value).collect();

    // 第二步：编码所有 serial type 为 varint，拼接成 header 的 serial type 部分
    let mut serial_types_buf = Vec::new();
    for (st, _) in &encoded {
        serial_types_buf.extend(encode_varint(*st));
    }

    // 第三步：迭代计算 header_size（含自身 varint 长度）
    // 初始假设 header_size 用 1 字节 varint 表示
    let mut header_size_varint_len = 1usize;
    let mut header_size;
    loop {
        header_size = header_size_varint_len + serial_types_buf.len();
        let actual_varint_len = encode_varint(header_size as u64).len();
        if actual_varint_len == header_size_varint_len {
            break; // 收敛
        }
        header_size_varint_len = actual_varint_len;
    }

    // 第四步：拼接完整 record
    let mut buf =
        Vec::with_capacity(header_size + encoded.iter().map(|(_, p)| p.len()).sum::<usize>());
    buf.extend(encode_varint(header_size as u64));
    buf.extend(&serial_types_buf);
    for (_, payload) in &encoded {
        buf.extend(payload);
    }
    buf
}

// =====================================================================
//  解码
// =====================================================================

/// 从字节切片解码 SQLite Record 为 `Vec<Value>`。
///
/// # 参数
/// - `buf`：record 字节切片
///
/// # 返回
/// - `Some(Vec<Value>)`：解码成功
/// - `None`：数据不完整或格式错误
pub fn decode_record(buf: &[u8]) -> Option<Vec<Value>> {
    // 读取 header_size varint
    let (header_size, header_size_len) = decode_varint(buf)?;
    let header_size = header_size as usize;
    if header_size < header_size_len || buf.len() < header_size {
        return None;
    }

    // 读取所有 serial type
    let mut serial_types: Vec<u64> = Vec::new();
    let mut pos = header_size_len;
    while pos < header_size {
        let (st, len) = decode_varint(&buf[pos..])?;
        serial_types.push(st);
        pos += len;
        if pos > header_size {
            return None; // serial type 越界
        }
    }

    // 读取 body：按 serial type 顺序解码每个值
    let mut values = Vec::with_capacity(serial_types.len());
    let mut body_pos = header_size;
    for st in &serial_types {
        let payload_len = serial_type_payload_len(*st);
        if body_pos + payload_len > buf.len() {
            return None; // body 不足
        }
        let value = decode_value(*st, &buf[body_pos..body_pos + payload_len]);
        values.push(value);
        body_pos += payload_len;
    }

    Some(values)
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  空记录
    // -----------------------------------------------------------------

    #[test]
    fn encode_empty_record() {
        let buf = encode_record(&[]);
        // header_size = 1（仅 header_size varint 自身，1 字节）
        assert_eq!(buf, vec![0x01]);
    }

    #[test]
    fn decode_empty_record() {
        let values = decode_record(&[0x01]).expect("decode should succeed");
        assert!(values.is_empty());
    }

    // -----------------------------------------------------------------
    //  单值记录
    // -----------------------------------------------------------------

    #[test]
    fn encode_single_null() {
        let buf = encode_record(&[Value::Null]);
        // header: varint(2) + varint(0) = [0x02, 0x00]
        // body: 空
        assert_eq!(buf, vec![0x02, 0x00]);
    }

    #[test]
    fn roundtrip_single_null() {
        let buf = encode_record(&[Value::Null]);
        let values = decode_record(&buf).expect("decode should succeed");
        assert_eq!(values, vec![Value::Null]);
    }

    #[test]
    fn roundtrip_single_int() {
        let buf = encode_record(&[Value::Int64(42)]);
        let values = decode_record(&buf).expect("decode should succeed");
        assert_eq!(values, vec![Value::Int64(42)]);
    }

    #[test]
    fn roundtrip_single_text() {
        let buf = encode_record(&[Value::Text("hello".to_string())]);
        let values = decode_record(&buf).expect("decode should succeed");
        assert_eq!(values, vec![Value::Text("hello".to_string())]);
    }

    // -----------------------------------------------------------------
    //  多值记录
    // -----------------------------------------------------------------

    #[test]
    fn roundtrip_mixed_values() {
        let values = vec![
            Value::Int64(42),
            Value::Text("hello".to_string()),
            Value::Float64(3.5),
            Value::Null,
            Value::Bool(true),
            Value::Blob(vec![0xDE, 0xAD]),
        ];
        let buf = encode_record(&values);
        let decoded = decode_record(&buf).expect("decode should succeed");
        assert_eq!(decoded.len(), values.len());
        assert_eq!(decoded[0], Value::Int64(42));
        assert_eq!(decoded[1], Value::Text("hello".to_string()));
        assert_eq!(decoded[2], Value::Float64(3.5));
        assert_eq!(decoded[3], Value::Null);
        assert_eq!(decoded[4], Value::Int64(1)); // Bool(true) → Int64(1)
        assert_eq!(decoded[5], Value::Blob(vec![0xDE, 0xAD]));
    }

    // -----------------------------------------------------------------
    //  header_size 自引用测试
    // -----------------------------------------------------------------

    #[test]
    fn header_size_includes_itself() {
        // 编码一个单 NULL 记录
        let buf = encode_record(&[Value::Null]);
        // header_size 应该 = 1(自身 varint) + 1(serial_type varint) = 2
        let (header_size, len) = decode_varint(&buf).expect("decode header_size");
        assert_eq!(header_size, 2);
        assert_eq!(len, 1);
        // header_size varint 占 1 字节，serial type varint 占 1 字节
    }

    #[test]
    fn header_size_large_record() {
        // 构造一个需要多字节 header_size 的记录
        // 需要足够多的值使得 serial_types 部分超过 127 字节
        let values: Vec<Value> = (0..100).map(|i| Value::Int64(i)).collect();
        let buf = encode_record(&values);
        let (header_size, _) = decode_varint(&buf).expect("decode header_size");
        // 100 个 Int64 值，每个 serial_type 至少 1 字节 varint
        // header_size > 100
        assert!(header_size > 100);
        // 解码应成功
        let decoded = decode_record(&buf).expect("decode should succeed");
        assert_eq!(decoded.len(), 100);
    }

    // -----------------------------------------------------------------
    //  错误处理
    // -----------------------------------------------------------------

    #[test]
    fn decode_empty_buffer_returns_none() {
        assert_eq!(decode_record(&[]), None);
    }

    #[test]
    fn decode_truncated_header_returns_none() {
        // header_size 声明为 10，但缓冲区只有 2 字节
        assert_eq!(decode_record(&[0x0A, 0x00]), None);
    }

    // -----------------------------------------------------------------
    //  往返测试（所有 Value 类型）
    // -----------------------------------------------------------------

    #[test]
    fn roundtrip_all_scalar_types() {
        let values = vec![
            Value::Null,
            Value::Int64(0),
            Value::Int64(1),
            Value::Int64(-1),
            Value::Int64(i64::MAX),
            Value::Int64(i64::MIN),
            Value::Float64(0.0),
            Value::Float64(3.5),
            Value::Float64(-1e100),
            Value::Text(String::new()),
            Value::Text("hello world".to_string()),
            Value::Text("中文测试".to_string()),
            Value::Blob(vec![]),
            Value::Blob(vec![0x00, 0xFF]),
            Value::Bool(true),
            Value::Bool(false),
            Value::Date(0),
            Value::Date(20454),
            Value::Timestamp(0),
            Value::Timestamp(1_700_000_000_000_000),
            Value::Decimal(42, 0),
            Value::Decimal(12345, 2),
            Value::Enum("active".to_string()),
        ];
        let buf = encode_record(&values);
        let decoded = decode_record(&buf).expect("decode should succeed");
        assert_eq!(decoded.len(), values.len());
        // 逐个比较（注意 Bool 和 Date/Timestamp/Decimal 的编码后类型可能变化）
        assert_eq!(decoded[0], Value::Null);
        assert_eq!(decoded[1], Value::Int64(0));
        assert_eq!(decoded[2], Value::Int64(1));
        assert_eq!(decoded[3], Value::Int64(-1));
        assert_eq!(decoded[4], Value::Int64(i64::MAX));
        assert_eq!(decoded[5], Value::Int64(i64::MIN));
        assert_eq!(decoded[6], Value::Float64(0.0));
        assert_eq!(decoded[7], Value::Float64(3.5));
        assert_eq!(decoded[8], Value::Float64(-1e100));
        assert_eq!(decoded[9], Value::Text(String::new()));
        assert_eq!(decoded[10], Value::Text("hello world".to_string()));
        assert_eq!(decoded[11], Value::Text("中文测试".to_string()));
        assert_eq!(decoded[12], Value::Blob(vec![]));
        assert_eq!(decoded[13], Value::Blob(vec![0x00, 0xFF]));
        // Bool(true) → Int64(1), Bool(false) → Int64(0)
        assert_eq!(decoded[14], Value::Int64(1));
        assert_eq!(decoded[15], Value::Int64(0));
        // Date/Timestamp → Int64
        assert_eq!(decoded[16], Value::Int64(0));
        assert_eq!(decoded[17], Value::Int64(20454));
        assert_eq!(decoded[18], Value::Int64(0));
        assert_eq!(decoded[19], Value::Int64(1_700_000_000_000_000));
        // Decimal(42, 0) → Int64(42)
        assert_eq!(decoded[20], Value::Int64(42));
        // Decimal(12345, 2) → Float64(123.45)
        assert!(matches!(decoded[21], Value::Float64(_)));
        // Enum → Text
        assert_eq!(decoded[22], Value::Text("active".to_string()));
    }

    // -----------------------------------------------------------------
    //  大记录测试
    // -----------------------------------------------------------------

    #[test]
    fn roundtrip_large_text_record() {
        // 1KB 文本
        let large_text = "x".repeat(1024);
        let values = vec![Value::Text(large_text.clone())];
        let buf = encode_record(&values);
        let decoded = decode_record(&buf).expect("decode should succeed");
        assert_eq!(decoded, vec![Value::Text(large_text)]);
    }

    #[test]
    fn roundtrip_many_values_record() {
        // 1000 个整数
        let values: Vec<Value> = (0..1000).map(Value::Int64).collect();
        let buf = encode_record(&values);
        let decoded = decode_record(&buf).expect("decode should succeed");
        assert_eq!(decoded.len(), 1000);
        for (i, v) in decoded.iter().enumerate() {
            assert_eq!(*v, Value::Int64(i as i64));
        }
    }

    // -----------------------------------------------------------------
    //  body 紧跟 header 测试
    // -----------------------------------------------------------------

    #[test]
    fn body_starts_immediately_after_header() {
        // 编码 [Int64(42), Text("hi")]
        let buf = encode_record(&[Value::Int64(42), Value::Text("hi".to_string())]);
        let (header_size, _header_size_len) = decode_varint(&buf).unwrap();
        // body 从 header_size 开始
        let body = &buf[header_size as usize..];
        // Int64(42) → serial type 1, payload [42]
        assert_eq!(body[0], 42);
        // Text("hi") → payload b"hi"
        assert_eq!(&body[1..3], b"hi");
    }
}
