//! SQLite Serial Type 系统（串行类型）编解码。
//!
//! SQLite Record 中每个值前都有一个 serial type 代码，用于标识值的类型和长度。
//!
//! # Serial Type 代码表
//!
//! | 代码 | 类型 | 数据长度 | 说明 |
//! |------|------|----------|------|
//! | 0 | NULL | 0 | 空值 |
//! | 1 | INT8 | 1 | 1 字节有符号整数（大端）|
//! | 2 | INT16 | 2 | 2 字节有符号整数（大端）|
//! | 3 | INT24 | 3 | 3 字节有符号整数（大端）|
//! | 4 | INT32 | 4 | 4 字节有符号整数（大端）|
//! | 5 | INT48 | 6 | 6 字节有符号整数（大端）|
//! | 6 | INT64 | 8 | 8 字节有符号整数（大端）|
//! | 7 | FLOAT64 | 8 | 8 字节 IEEE 754 浮点（大端）|
//! | 8 | INT 0 | 0 | 整数 0（无数据）|
//! | 9 | INT 1 | 0 | 整数 1（无数据）|
//! | ≥12 偶数 | BLOB | (N-12)/2 | 二进制数据 |
//! | ≥13 奇数 | TEXT | (N-13)/2 | UTF-8 文本 |

use szrsql_types::value::Value;

// =====================================================================
//  Serial Type 常量
// =====================================================================

/// NULL（0 字节）
pub const SERIAL_NULL: u64 = 0;
/// 1 字节有符号整数
pub const SERIAL_INT8: u64 = 1;
/// 2 字节有符号整数
pub const SERIAL_INT16: u64 = 2;
/// 3 字节有符号整数
pub const SERIAL_INT24: u64 = 3;
/// 4 字节有符号整数
pub const SERIAL_INT32: u64 = 4;
/// 6 字节有符号整数
pub const SERIAL_INT48: u64 = 5;
/// 8 字节有符号整数
pub const SERIAL_INT64: u64 = 6;
/// 8 字节 IEEE 754 浮点
pub const SERIAL_FLOAT64: u64 = 7;
/// 整数 0（0 字节）
pub const SERIAL_INT_ZERO: u64 = 8;
/// 整数 1（0 字节）
pub const SERIAL_INT_ONE: u64 = 9;
/// BLOB 类型的基数（N ≥ 12 且偶数）
pub const SERIAL_BLOB_BASE: u64 = 12;
/// TEXT 类型的基数（N ≥ 13 且奇数）
pub const SERIAL_TEXT_BASE: u64 = 13;

// =====================================================================
//  编码：Value → (serial_type, payload)
// =====================================================================

/// 将 SzRSQL `Value` 编码为 SQLite serial type 和 payload 字节。
///
/// # 返回
/// `(serial_type, payload_bytes)`：
/// - `serial_type`：SQLite serial type 代码
/// - `payload_bytes`：值的数据字节（NULL/INT0/INT1 时为空 Vec）
pub fn encode_value(value: &Value) -> (u64, Vec<u8>) {
    match value {
        Value::Null => (SERIAL_NULL, Vec::new()),

        // 整数：选择最短编码
        Value::Int64(v) => encode_int64(*v),

        // 浮点：始终使用 serial type 7
        Value::Float64(v) => (SERIAL_FLOAT64, v.to_bits().to_be_bytes().to_vec()),

        // 文本：UTF-8 编码
        Value::Text(s) => {
            let bytes = s.as_bytes();
            let serial = SERIAL_TEXT_BASE + (bytes.len() as u64) * 2;
            (serial, bytes.to_vec())
        }

        // 二进制
        Value::Blob(b) => {
            let serial = SERIAL_BLOB_BASE + (b.len() as u64) * 2;
            (serial, b.clone())
        }

        // Bool：true → 1, false → 0
        Value::Bool(b) => {
            if *b {
                (SERIAL_INT_ONE, Vec::new())
            } else {
                (SERIAL_INT_ZERO, Vec::new())
            }
        }

        // Date/Timestamp：按 i64 整数编码
        Value::Date(d) => encode_int64(i64::from(*d)),
        Value::Timestamp(t) => encode_int64(*t),

        // Decimal：scale=0 按整数，scale>0 按浮点
        Value::Decimal(v, scale) => {
            if *scale == 0 {
                // scale=0 → 整数；若溢出 i64 则降级为浮点
                match i64::try_from(*v) {
                    Ok(i) => encode_int64(i),
                    Err(_) => {
                        let f = *v as f64;
                        (SERIAL_FLOAT64, f.to_bits().to_be_bytes().to_vec())
                    }
                }
            } else {
                // scale>0 → 浮点（SQLite 无原生 Decimal）
                let divisor = 10_f64.powi(i32::from(*scale));
                let f = *v as f64 / divisor;
                (SERIAL_FLOAT64, f.to_bits().to_be_bytes().to_vec())
            }
        }

        // 复合类型：序列化为 JSON 文本
        Value::Enum(s) => {
            let bytes = s.as_bytes();
            let serial = SERIAL_TEXT_BASE + (bytes.len() as u64) * 2;
            (serial, bytes.to_vec())
        }
        Value::Array(_)
        | Value::Range(_)
        | Value::Json(_)
        | Value::TsVector(_)
        | Value::TsQuery(_)
        | Value::Vector(_)
        | Value::Xml(_) => {
            // 使用 Value 的 cast_explicit 转为 Text，再编码
            let text = value_to_text(value);
            let bytes = text.as_bytes();
            let serial = SERIAL_TEXT_BASE + (bytes.len() as u64) * 2;
            (serial, bytes.to_vec())
        }
    }
}

/// 将 i64 编码为最短 serial type + payload。
fn encode_int64(v: i64) -> (u64, Vec<u8>) {
    // 特殊值 0 和 1 使用 0 字节编码
    if v == 0 {
        return (SERIAL_INT_ZERO, Vec::new());
    }
    if v == 1 {
        return (SERIAL_INT_ONE, Vec::new());
    }

    // 根据值域选择最短编码（有符号数）
    if (-128..=127).contains(&v) {
        (SERIAL_INT8, vec![v as i8 as u8])
    } else if (-32768..=32767).contains(&v) {
        (SERIAL_INT16, (v as i16).to_be_bytes().to_vec())
    } else if (-8388608..=8388607).contains(&v) {
        // 3 字节：取低 24 bit，大端
        let bytes = (v as i32).to_be_bytes();
        (SERIAL_INT24, bytes[1..4].to_vec())
    } else if (-2147483648..=2147483647).contains(&v) {
        (SERIAL_INT32, (v as i32).to_be_bytes().to_vec())
    } else if (-(1i64 << 47)..=(1i64 << 47) - 1).contains(&v) {
        // 6 字节：取低 48 bit，大端
        let bytes = v.to_be_bytes();
        (SERIAL_INT48, bytes[2..8].to_vec())
    } else {
        (SERIAL_INT64, v.to_be_bytes().to_vec())
    }
}

/// 将复合类型 Value 转为文本表示（用于序列化存储）。
fn value_to_text(value: &Value) -> String {
    match value {
        Value::Array(arr) => {
            // 数组序列化为 JSON
            let json_arr: Vec<serde_json::Value> = arr.iter().map(value_to_json).collect();
            serde_json::to_string(&json_arr).unwrap_or_else(|_| "[]".to_string())
        }
        Value::Range(r) => {
            // 范围序列化为文本
            let lower = r.lower.as_ref().map(|v| value_to_json(v));
            let upper = r.upper.as_ref().map(|v| value_to_json(v));
            let l_bracket = if r.lower_inc {
                '['
            } else {
                '('
            };
            let r_bracket = if r.upper_inc {
                ']'
            } else {
                ')'
            };
            format!(
                "{}{},{}{}",
                l_bracket,
                lower.map(|v| v.to_string()).unwrap_or_default(),
                upper.map(|v| v.to_string()).unwrap_or_default(),
                r_bracket
            )
        }
        Value::Json(j) => serde_json::to_string(j).unwrap_or_default(),
        Value::TsVector(ts) => ts.to_pg_string(),
        Value::TsQuery(q) => q.to_pg_string(),
        // 其他类型不应到达此函数，但提供 fallback
        _ => format!("{value:?}"),
    }
}

/// 将 Value 转为 serde_json::Value（用于数组/范围的嵌套序列化）。
fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Int64(v) => serde_json::json!(v),
        Value::Float64(v) => serde_json::json!(v),
        Value::Text(s) => serde_json::json!(s),
        Value::Bool(b) => serde_json::json!(b),
        Value::Blob(b) => serde_json::json!(b),
        Value::Date(d) => serde_json::json!(d),
        Value::Timestamp(t) => serde_json::json!(t),
        Value::Decimal(v, scale) => {
            if *scale == 0 {
                serde_json::json!(v)
            } else {
                let divisor = 10_f64.powi(i32::from(*scale));
                serde_json::json!(*v as f64 / divisor)
            }
        }
        Value::Enum(s) => serde_json::json!(s),
        Value::Array(_)
        | Value::Range(_)
        | Value::Json(_)
        | Value::TsVector(_)
        | Value::TsQuery(_)
        | Value::Vector(_)
        | Value::Xml(_) => {
            serde_json::json!(value_to_text(value))
        }
    }
}

// =====================================================================
//  解码：serial_type + payload → Value
// =====================================================================

/// 根据 serial type 和 payload 字节解码为 SzRSQL `Value`。
///
/// # 参数
/// - `serial_type`：SQLite serial type 代码
/// - `buf`：payload 字节切片（长度必须与 serial type 要求一致）
///
/// # 返回
/// 解码后的 `Value`。若 serial type 不合法则返回 `Value::Null`。
pub fn decode_value(serial_type: u64, buf: &[u8]) -> Value {
    match serial_type {
        SERIAL_NULL => Value::Null,

        SERIAL_INT8 => {
            if buf.is_empty() {
                return Value::Null;
            }
            Value::Int64(i8::from_be_bytes([buf[0]]) as i64)
        }

        SERIAL_INT16 => {
            if buf.len() < 2 {
                return Value::Null;
            }
            Value::Int64(i16::from_be_bytes([buf[0], buf[1]]) as i64)
        }

        SERIAL_INT24 => {
            if buf.len() < 3 {
                return Value::Null;
            }
            // 3 字节有符号整数：拼成 4 字节后做符号扩展
            let mut bytes = [0u8; 4];
            bytes[1] = buf[0];
            bytes[2] = buf[1];
            bytes[3] = buf[2];
            // 符号扩展：若最高位（bit 23）为 1，则高字节补 0xFF
            if buf[0] & 0x80 != 0 {
                bytes[0] = 0xFF;
            }
            Value::Int64(i32::from_be_bytes(bytes) as i64)
        }

        SERIAL_INT32 => {
            if buf.len() < 4 {
                return Value::Null;
            }
            Value::Int64(i32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as i64)
        }

        SERIAL_INT48 => {
            if buf.len() < 6 {
                return Value::Null;
            }
            // 6 字节有符号整数：拼成 8 字节后做符号扩展
            let mut bytes = [0u8; 8];
            bytes[2] = buf[0];
            bytes[3] = buf[1];
            bytes[4] = buf[2];
            bytes[5] = buf[3];
            bytes[6] = buf[4];
            bytes[7] = buf[5];
            // 符号扩展：若最高位（bit 47）为 1，则高 2 字节补 0xFF
            if buf[0] & 0x80 != 0 {
                bytes[0] = 0xFF;
                bytes[1] = 0xFF;
            }
            Value::Int64(i64::from_be_bytes(bytes))
        }

        SERIAL_INT64 => {
            if buf.len() < 8 {
                return Value::Null;
            }
            Value::Int64(i64::from_be_bytes([
                buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
            ]))
        }

        SERIAL_FLOAT64 => {
            if buf.len() < 8 {
                return Value::Null;
            }
            let bits = u64::from_be_bytes([
                buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
            ]);
            Value::Float64(f64::from_bits(bits))
        }

        SERIAL_INT_ZERO => Value::Int64(0),

        SERIAL_INT_ONE => Value::Int64(1),

        // N ≥ 12 且偶数 → BLOB
        n if n >= SERIAL_BLOB_BASE && n % 2 == 0 => {
            let len = ((n - SERIAL_BLOB_BASE) / 2) as usize;
            if buf.len() < len {
                return Value::Null;
            }
            Value::Blob(buf[..len].to_vec())
        }

        // N ≥ 13 且奇数 → TEXT（UTF-8）
        n if n >= SERIAL_TEXT_BASE && n % 2 == 1 => {
            let len = ((n - SERIAL_TEXT_BASE) / 2) as usize;
            if buf.len() < len {
                return Value::Null;
            }
            match String::from_utf8(buf[..len].to_vec()) {
                Ok(s) => Value::Text(s),
                Err(_) => {
                    // 非 UTF-8 数据，降级为 BLOB
                    Value::Blob(buf[..len].to_vec())
                }
            }
        }

        // 10, 11 为保留值（未使用），返回 Null
        _ => Value::Null,
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 根据 serial type 计算其 payload 的字节长度。
///
/// 用于 Record 解码时确定每个值占用的字节数。
pub fn serial_type_payload_len(serial_type: u64) -> usize {
    match serial_type {
        SERIAL_NULL | SERIAL_INT_ZERO | SERIAL_INT_ONE => 0,
        SERIAL_INT8 => 1,
        SERIAL_INT16 => 2,
        SERIAL_INT24 => 3,
        SERIAL_INT32 => 4,
        SERIAL_INT48 => 6,
        SERIAL_INT64 | SERIAL_FLOAT64 => 8,
        n if n >= SERIAL_BLOB_BASE && n % 2 == 0 => ((n - SERIAL_BLOB_BASE) / 2) as usize,
        n if n >= SERIAL_TEXT_BASE && n % 2 == 1 => ((n - SERIAL_TEXT_BASE) / 2) as usize,
        _ => 0,
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  NULL 编解码
    // -----------------------------------------------------------------

    #[test]
    fn encode_null() {
        let (st, payload) = encode_value(&Value::Null);
        assert_eq!(st, SERIAL_NULL);
        assert!(payload.is_empty());
    }

    #[test]
    fn decode_null() {
        assert_eq!(decode_value(SERIAL_NULL, &[]), Value::Null);
    }

    // -----------------------------------------------------------------
    //  整数编解码（各 serial type）
    // -----------------------------------------------------------------

    #[test]
    fn encode_int_zero_uses_serial_8() {
        let (st, payload) = encode_value(&Value::Int64(0));
        assert_eq!(st, SERIAL_INT_ZERO);
        assert!(payload.is_empty());
    }

    #[test]
    fn encode_int_one_uses_serial_9() {
        let (st, payload) = encode_value(&Value::Int64(1));
        assert_eq!(st, SERIAL_INT_ONE);
        assert!(payload.is_empty());
    }

    #[test]
    fn encode_small_positive_int_uses_serial_1() {
        let (st, payload) = encode_value(&Value::Int64(42));
        assert_eq!(st, SERIAL_INT8);
        assert_eq!(payload, vec![42]);
    }

    #[test]
    fn encode_small_negative_int_uses_serial_1() {
        let (st, payload) = encode_value(&Value::Int64(-1));
        assert_eq!(st, SERIAL_INT8);
        assert_eq!(payload, vec![0xFF]); // -1 as i8
    }

    #[test]
    fn encode_int_128_uses_serial_2() {
        let (st, payload) = encode_value(&Value::Int64(128));
        assert_eq!(st, SERIAL_INT16);
        assert_eq!(payload, vec![0x00, 0x80]);
    }

    #[test]
    fn encode_int_32767_uses_serial_2() {
        let (st, payload) = encode_value(&Value::Int64(32767));
        assert_eq!(st, SERIAL_INT16);
        assert_eq!(payload, vec![0x7F, 0xFF]);
    }

    #[test]
    fn encode_int_32768_uses_serial_3() {
        let (st, payload) = encode_value(&Value::Int64(32768));
        assert_eq!(st, SERIAL_INT24);
        assert_eq!(payload, vec![0x00, 0x80, 0x00]);
    }

    #[test]
    fn encode_int_i32_max_uses_serial_4() {
        let (st, payload) = encode_value(&Value::Int64(i32::MAX as i64));
        assert_eq!(st, SERIAL_INT32);
        assert_eq!(payload, vec![0x7F, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn encode_large_int_uses_serial_6() {
        let (st, payload) = encode_value(&Value::Int64(i64::MAX));
        assert_eq!(st, SERIAL_INT64);
        assert_eq!(payload.len(), 8);
    }

    #[test]
    fn encode_i64_min_uses_serial_6() {
        let (st, payload) = encode_value(&Value::Int64(i64::MIN));
        assert_eq!(st, SERIAL_INT64);
        assert_eq!(payload, i64::MIN.to_be_bytes().to_vec());
    }

    // -----------------------------------------------------------------
    //  整数往返测试
    // -----------------------------------------------------------------

    #[test]
    fn roundtrip_integers() {
        let test_values: &[i64] = &[
            0,
            1,
            -1,
            42,
            -42,
            127,
            -128,
            128,
            -129,
            32767,
            -32768,
            32768,
            8388607,
            -8388608,
            8388608,
            2147483647,
            -2147483648,
            2147483648,
            140737488355327,
            -140737488355328,
            i64::MAX,
            i64::MIN,
        ];
        for &v in test_values {
            let (st, payload) = encode_value(&Value::Int64(v));
            let decoded = decode_value(st, &payload);
            assert_eq!(
                decoded,
                Value::Int64(v),
                "roundtrip failed for Int64({v}): st={st}, payload={payload:?}"
            );
        }
    }

    // -----------------------------------------------------------------
    //  浮点数编解码
    // -----------------------------------------------------------------

    #[test]
    fn encode_float64() {
        let (st, payload) = encode_value(&Value::Float64(3.5));
        assert_eq!(st, SERIAL_FLOAT64);
        assert_eq!(payload.len(), 8);
    }

    #[test]
    fn roundtrip_float64() {
        let test_values: &[f64] = &[
            0.0,
            -0.0,
            1.0,
            -1.0,
            3.5,
            -3.5,
            f64::INFINITY,
            f64::NEG_INFINITY,
            1e308,
            -1e308,
            1e-308,
        ];
        for &v in test_values {
            let (st, payload) = encode_value(&Value::Float64(v));
            let decoded = decode_value(st, &payload);
            assert_eq!(
                decoded,
                Value::Float64(v),
                "roundtrip failed for Float64({v})"
            );
        }
    }

    // -----------------------------------------------------------------
    //  文本编解码
    // -----------------------------------------------------------------

    #[test]
    fn encode_empty_text() {
        let (st, payload) = encode_value(&Value::Text(String::new()));
        assert_eq!(st, SERIAL_TEXT_BASE); // 13
        assert!(payload.is_empty());
    }

    #[test]
    fn encode_text_hello() {
        let (st, payload) = encode_value(&Value::Text("hello".to_string()));
        assert_eq!(st, SERIAL_TEXT_BASE + 10); // 13 + 2*5 = 23
        assert_eq!(payload, b"hello");
    }

    #[test]
    fn roundtrip_text() {
        let test_values: &[&str] = &["", "a", "hello", "世界", "🎉", "multi\nline\ntext"];
        for &s in test_values {
            let (st, payload) = encode_value(&Value::Text(s.to_string()));
            let decoded = decode_value(st, &payload);
            assert_eq!(
                decoded,
                Value::Text(s.to_string()),
                "roundtrip failed for Text({s:?})"
            );
        }
    }

    // -----------------------------------------------------------------
    //  BLOB 编解码
    // -----------------------------------------------------------------

    #[test]
    fn encode_empty_blob() {
        let (st, payload) = encode_value(&Value::Blob(Vec::new()));
        assert_eq!(st, SERIAL_BLOB_BASE); // 12
        assert!(payload.is_empty());
    }

    #[test]
    fn roundtrip_blob() {
        let test_values: &[Vec<u8>] = &[
            vec![],
            vec![0x00],
            vec![0xFF],
            vec![0xDE, 0xAD, 0xBE, 0xEF],
            vec![0; 100],
        ];
        for v in test_values {
            let (st, payload) = encode_value(&Value::Blob(v.clone()));
            let decoded = decode_value(st, &payload);
            assert_eq!(
                decoded,
                Value::Blob(v.clone()),
                "roundtrip failed for Blob({v:?})"
            );
        }
    }

    // -----------------------------------------------------------------
    //  Bool 编解码
    // -----------------------------------------------------------------

    #[test]
    fn encode_bool_true() {
        let (st, payload) = encode_value(&Value::Bool(true));
        assert_eq!(st, SERIAL_INT_ONE);
        assert!(payload.is_empty());
    }

    #[test]
    fn encode_bool_false() {
        let (st, payload) = encode_value(&Value::Bool(false));
        assert_eq!(st, SERIAL_INT_ZERO);
        assert!(payload.is_empty());
    }

    #[test]
    fn decode_bool_values() {
        // serial type 8 → Int64(0) ≈ false
        assert_eq!(decode_value(SERIAL_INT_ZERO, &[]), Value::Int64(0));
        // serial type 9 → Int64(1) ≈ true
        assert_eq!(decode_value(SERIAL_INT_ONE, &[]), Value::Int64(1));
    }

    // -----------------------------------------------------------------
    //  Date/Timestamp 编解码
    // -----------------------------------------------------------------

    #[test]
    fn roundtrip_date() {
        let test_values: &[i32] = &[0, 1, -1, 20454, -365, i32::MAX, i32::MIN];
        for &d in test_values {
            let (st, payload) = encode_value(&Value::Date(d));
            let decoded = decode_value(st, &payload);
            assert_eq!(
                decoded,
                Value::Int64(i64::from(d)),
                "roundtrip failed for Date({d})"
            );
        }
    }

    #[test]
    fn roundtrip_timestamp() {
        let test_values: &[i64] = &[0, 1, -1, 1_700_000_000_000_000, i64::MAX, i64::MIN];
        for &t in test_values {
            let (st, payload) = encode_value(&Value::Timestamp(t));
            let decoded = decode_value(st, &payload);
            assert_eq!(
                decoded,
                Value::Int64(t),
                "roundtrip failed for Timestamp({t})"
            );
        }
    }

    // -----------------------------------------------------------------
    //  Decimal 编解码
    // -----------------------------------------------------------------

    #[test]
    fn encode_decimal_scale_zero_as_integer() {
        let (st, _) = encode_value(&Value::Decimal(42, 0));
        // scale=0 → 按整数编码
        assert!(matches!(st, SERIAL_INT8 | SERIAL_INT_ZERO | SERIAL_INT_ONE));
    }

    #[test]
    fn encode_decimal_scale_positive_as_float() {
        let (st, payload) = encode_value(&Value::Decimal(12345, 2));
        assert_eq!(st, SERIAL_FLOAT64);
        assert_eq!(payload.len(), 8);
    }

    // -----------------------------------------------------------------
    //  payload_len 测试
    // -----------------------------------------------------------------

    #[test]
    fn payload_len_null_is_zero() {
        assert_eq!(serial_type_payload_len(SERIAL_NULL), 0);
    }

    #[test]
    fn payload_len_int_zero_one_is_zero() {
        assert_eq!(serial_type_payload_len(SERIAL_INT_ZERO), 0);
        assert_eq!(serial_type_payload_len(SERIAL_INT_ONE), 0);
    }

    #[test]
    fn payload_len_int_sizes() {
        assert_eq!(serial_type_payload_len(SERIAL_INT8), 1);
        assert_eq!(serial_type_payload_len(SERIAL_INT16), 2);
        assert_eq!(serial_type_payload_len(SERIAL_INT24), 3);
        assert_eq!(serial_type_payload_len(SERIAL_INT32), 4);
        assert_eq!(serial_type_payload_len(SERIAL_INT48), 6);
        assert_eq!(serial_type_payload_len(SERIAL_INT64), 8);
        assert_eq!(serial_type_payload_len(SERIAL_FLOAT64), 8);
    }

    #[test]
    fn payload_len_blob_and_text() {
        // BLOB: (N - 12) / 2
        assert_eq!(serial_type_payload_len(12), 0); // 空 BLOB
        assert_eq!(serial_type_payload_len(14), 1); // 1 字节 BLOB
        assert_eq!(serial_type_payload_len(16), 2); // 2 字节 BLOB

        // TEXT: (N - 13) / 2
        assert_eq!(serial_type_payload_len(13), 0); // 空 TEXT
        assert_eq!(serial_type_payload_len(15), 1); // 1 字节 TEXT
        assert_eq!(serial_type_payload_len(17), 2); // 2 字节 TEXT
    }

    // -----------------------------------------------------------------
    //  24 位整数符号扩展测试
    // -----------------------------------------------------------------

    #[test]
    fn decode_int24_positive() {
        // 8388607 = 0x7FFFFF（最大正数）
        let v = decode_value(SERIAL_INT24, &[0x7F, 0xFF, 0xFF]);
        assert_eq!(v, Value::Int64(8388607));
    }

    #[test]
    fn decode_int24_negative() {
        // -1 = 0xFFFFFF（符号扩展）
        let v = decode_value(SERIAL_INT24, &[0xFF, 0xFF, 0xFF]);
        assert_eq!(v, Value::Int64(-1));
    }

    #[test]
    fn decode_int24_min() {
        // -8388608 = 0x800000
        let v = decode_value(SERIAL_INT24, &[0x80, 0x00, 0x00]);
        assert_eq!(v, Value::Int64(-8388608));
    }

    // -----------------------------------------------------------------
    //  48 位整数符号扩展测试
    // -----------------------------------------------------------------

    #[test]
    fn decode_int48_positive() {
        // 140737488355327 = 0x7FFFFFFFFFFF（最大正数）
        let v = decode_value(SERIAL_INT48, &[0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(v, Value::Int64(140737488355327));
    }

    #[test]
    fn decode_int48_negative() {
        // -1 = 0xFFFFFFFFFFFF
        let v = decode_value(SERIAL_INT48, &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(v, Value::Int64(-1));
    }

    // -----------------------------------------------------------------
    //  Enum 编解码
    // -----------------------------------------------------------------

    #[test]
    fn roundtrip_enum() {
        let (st, payload) = encode_value(&Value::Enum("active".to_string()));
        assert_eq!(st, SERIAL_TEXT_BASE + 12); // 13 + 2*6 = 25
        assert_eq!(payload, b"active");
        let decoded = decode_value(st, &payload);
        assert_eq!(decoded, Value::Text("active".to_string()));
    }

    // -----------------------------------------------------------------
    //  非 UTF-8 文本降级为 BLOB
    // -----------------------------------------------------------------

    #[test]
    fn decode_invalid_utf8_text_falls_back_to_blob() {
        // 构造一个 serial type 表示 2 字节 TEXT，但 payload 不是合法 UTF-8
        let st = SERIAL_TEXT_BASE + 4; // 13 + 2*2 = 17 → 2 字节 TEXT
        let payload = vec![0xFF, 0xFE]; // 非 UTF-8
        let decoded = decode_value(st, &payload);
        assert_eq!(decoded, Value::Blob(vec![0xFF, 0xFE]));
    }

    // -----------------------------------------------------------------
    //  保留值 10/11 测试
    // -----------------------------------------------------------------

    #[test]
    fn decode_reserved_values_return_null() {
        assert_eq!(decode_value(10, &[]), Value::Null);
        assert_eq!(decode_value(11, &[]), Value::Null);
    }
}
