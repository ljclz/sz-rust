//! TDS 结果集编码 — ColumnMetaData + Row + Done。
//!
//! TDS 结果集 token 流：
//! ```text
//! 0x81 ColumnMetaData token
//! 0xD1 Row token (重复多次)
//! 0xFD Done token
//! ```
//!
//! ColumnMetaData 格式（TDS 7.2+）：
//! - 0x81 token
//! - TvpResultFlags (1 byte)
//! - NumColumns (2 bytes BE)
//! - 每列：UserType(4 BE) + Flags(2 BE) + TYPE_INFO + ColName(B-VARCHAR UTF-16LE)
//!
//! Row 格式：每列按 TYPE_INFO 中描述的类型编码
//!
//! Done 格式：0xFD + Status(2 LE) + CurCmd(2 LE) + RowCount(8 LE, TDS 7.2+)

use crate::auth::encode_utf16_le;
use crate::types::{IntByteLen, TdsType};
use szrsql_types::value::Value;

/// DONE token 状态标志。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoneStatus(pub u16);

impl DoneStatus {
    /// 最终结果（非中间结果）
    pub const FINAL: DoneStatus = DoneStatus(0x0000);
    /// 更多结果（多结果集）
    pub const MORE: DoneStatus = DoneStatus(0x0001);
    /// 错误
    pub const ERROR: DoneStatus = DoneStatus(0x0002);
    /// 受影响行数有效
    pub const COUNT: DoneStatus = DoneStatus(0x0010);

    /// 创建新状态。
    pub fn new(value: u16) -> Self {
        DoneStatus(value)
    }
}

/// 列元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnMetaData {
    /// 列名
    pub name: String,
    /// TDS 类型
    pub column_type: TdsType,
    /// 列最大长度（字节数；用于变长类型）
    pub max_length: u16,
    /// 整数字节数（仅 INTN 类型使用，1/2/4/8）
    pub int_byte_len: IntByteLen,
    /// 是否可为 NULL
    pub nullable: bool,
}

impl ColumnMetaData {
    /// 创建 NVARCHAR 列。
    pub fn nvarchar(name: impl Into<String>, max_length: u16) -> Self {
        Self {
            name: name.into(),
            column_type: TdsType::NVarChar,
            max_length,
            int_byte_len: IntByteLen::Eight,
            nullable: true,
        }
    }

    /// 创建 INTN 列（根据 Int64 值范围自动选择 1/2/4/8 字节）。
    pub fn integer(name: impl Into<String>, value: i64) -> Self {
        let byte_len = TdsType::int_byte_len(value);
        Self {
            name: name.into(),
            column_type: TdsType::IntN,
            max_length: 8,
            int_byte_len: IntByteLen::from_byte(byte_len).unwrap_or(IntByteLen::Eight),
            nullable: false,
        }
    }

    /// 创建 BIT 列。
    pub fn bit(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            column_type: TdsType::Bit,
            max_length: 1,
            int_byte_len: IntByteLen::One,
            nullable: false,
        }
    }

    /// 创建 FLOATN 列（8 字节 = FLOAT）。
    pub fn float8(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            column_type: TdsType::FloatN,
            max_length: 8,
            int_byte_len: IntByteLen::Eight,
            nullable: false,
        }
    }

    /// 创建 DATETIMEN 列（8 字节 = DATETIME）。
    pub fn datetime(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            column_type: TdsType::DateTimeN,
            max_length: 8,
            int_byte_len: IntByteLen::Eight,
            nullable: false,
        }
    }

    /// 创建 DATE 列。
    pub fn date(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            column_type: TdsType::Date,
            max_length: 3,
            int_byte_len: IntByteLen::One,
            nullable: false,
        }
    }

    /// 创建 BIGVARBIN 列。
    pub fn varbinary(name: impl Into<String>, max_length: u16) -> Self {
        Self {
            name: name.into(),
            column_type: TdsType::BigVarBin,
            max_length,
            int_byte_len: IntByteLen::One,
            nullable: true,
        }
    }

    /// 根据 SzRSQL Value 推导列元数据。
    pub fn from_value(name: impl Into<String>, value: &Value) -> Self {
        let name = name.into();
        let column_type = TdsType::from_value(value);
        let int_byte_len = match value {
            Value::Int64(n) => {
                IntByteLen::from_byte(TdsType::int_byte_len(*n)).unwrap_or(IntByteLen::Eight)
            }
            _ => IntByteLen::Eight,
        };
        let max_length = match column_type {
            TdsType::NVarChar => 255,
            TdsType::BigVarChar => 255,
            TdsType::BigVarBin => 255,
            TdsType::IntN => int_byte_len.as_bytes() as u16,
            TdsType::Bit => 1,
            TdsType::FloatN => 8,
            TdsType::DateTimeN => 8,
            TdsType::Date => 3,
            TdsType::Time => 5,
            TdsType::NumericN => 17,
            TdsType::Text => 0,
            TdsType::NChar => 255,
            TdsType::Xml => 0,
        };
        Self {
            name,
            column_type,
            max_length,
            int_byte_len,
            nullable: true,
        }
    }

    /// 编码单列 TYPE_INFO 部分（不含 token、TvpResultFlags、NumColumns）。
    pub fn encode_type_info(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(16);
        // UserType（4 字节 BE，TDS 7.2+）
        buf.extend_from_slice(&0u32.to_be_bytes());
        // Flags（2 字节 BE）：位 0 = nullable
        let flags: u16 = if self.nullable {
            0x0001
        } else {
            0x0000
        };
        buf.extend_from_slice(&flags.to_be_bytes());
        // TYPE_INFO
        buf.push(self.column_type as u8);
        // 变长类型参数
        match self.column_type {
            TdsType::IntN => buf.push(self.int_byte_len.as_bytes()),
            TdsType::FloatN => buf.push(self.max_length as u8),
            TdsType::DateTimeN => buf.push(self.max_length as u8),
            TdsType::BigVarChar | TdsType::BigVarBin | TdsType::NChar | TdsType::NVarChar => {
                // 最大长度（2 字节 LE）
                buf.extend_from_slice(&self.max_length.to_le_bytes());
                // 排序规则（4 字节，固定 0）
                buf.extend_from_slice(&0u32.to_le_bytes());
            }
            TdsType::NumericN => {
                // 精度 + 标度
                buf.push(38); // 最大精度
                buf.push(0); // 标度
            }
            TdsType::Time => {
                // 时间精度（小数秒位数）
                buf.push(7);
            }
            TdsType::Bit | TdsType::Date | TdsType::Text | TdsType::Xml => {}
        }
        // ColName（B-VARCHAR UTF-16LE）
        let name_utf16 = encode_utf16_le(&self.name);
        buf.extend_from_slice(&(name_utf16.len() as u16).to_le_bytes());
        buf.extend_from_slice(&name_utf16);
        buf
    }
}

/// 一行数据（按列编码后的字节序列）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TdsRow {
    /// 列值字节
    pub data: Vec<u8>,
}

impl TdsRow {
    /// 编码单值（按列类型）。
    pub fn encode_value(value: &Value, column: &ColumnMetaData) -> Vec<u8> {
        let mut buf = Vec::with_capacity(16);
        match value {
            Value::Null => {
                // 变长类型用长度 0 表示 NULL；定长类型也用 0 长度（TDS 7.2+ NULL 标记）
                if column.column_type.is_variable_length() {
                    // 变长类型：长度前缀 0
                    match column.column_type {
                        TdsType::IntN | TdsType::FloatN | TdsType::DateTimeN | TdsType::Time => {
                            buf.push(0);
                        }
                        TdsType::BigVarChar
                        | TdsType::BigVarBin
                        | TdsType::NChar
                        | TdsType::NVarChar => {
                            buf.extend_from_slice(&0u16.to_le_bytes());
                        }
                        TdsType::NumericN => {
                            buf.push(0);
                        }
                        _ => {}
                    }
                } else {
                    // 定长类型 NULL：变长长度前缀 0（TDS 协议特殊处理）
                    buf.push(0);
                }
            }
            Value::Bool(b) => {
                if column.column_type == TdsType::Bit {
                    buf.push(if *b {
                        1
                    } else {
                        0
                    });
                } else {
                    // 退化为 INTN 1 字节
                    buf.push(1);
                    buf.push(if *b {
                        1
                    } else {
                        0
                    });
                }
            }
            Value::Int64(n) => {
                let byte_len = column.int_byte_len.as_bytes();
                buf.push(byte_len);
                let bytes = match byte_len {
                    1 => (*n as i8).to_le_bytes().to_vec(),
                    2 => (*n as i16).to_le_bytes().to_vec(),
                    4 => (*n as i32).to_le_bytes().to_vec(),
                    _ => n.to_le_bytes().to_vec(),
                };
                buf.extend_from_slice(&bytes);
            }
            Value::Float64(f) => {
                if column.column_type == TdsType::FloatN {
                    buf.push(8);
                    buf.extend_from_slice(&f.to_le_bytes());
                } else {
                    // 退化为字符串
                    let s = format_float(*f);
                    let utf16 = encode_utf16_le(&s);
                    buf.extend_from_slice(&(utf16.len() as u16).to_le_bytes());
                    buf.extend_from_slice(&utf16);
                }
            }
            Value::Text(s) => {
                let utf16 = encode_utf16_le(s);
                buf.extend_from_slice(&(utf16.len() as u16).to_le_bytes());
                buf.extend_from_slice(&utf16);
            }
            Value::Blob(b) => {
                buf.extend_from_slice(&(b.len() as u16).to_le_bytes());
                buf.extend_from_slice(b);
            }
            Value::Date(days) => {
                // DATE：3 字节，自 0001-01-01 起的天数（仅 TDS 7.3+）
                // 1970-01-01 相对 0001-01-01 偏移 719162 天
                let delta = (*days as i64 + 719_162).max(0) as u32;
                buf.push((delta & 0xFF) as u8);
                buf.push(((delta >> 8) & 0xFF) as u8);
                buf.push(((delta >> 16) & 0xFF) as u8);
            }
            Value::Timestamp(micros) => {
                if column.column_type == TdsType::DateTimeN {
                    // DATETIME：4 字节日期（自 1900-01-01 起天数） + 4 字节时间（1/300 秒）
                    // 1970-01-01 相对 1900-01-01 偏移 25569 天
                    let secs = micros / 1_000_000;
                    let days_since_1900 = (secs / 86_400) as i32 + 25_569;
                    let time_part = (*micros % 86_400_000_000) * 300 / 1_000_000;
                    buf.push(8);
                    buf.extend_from_slice(&days_since_1900.to_be_bytes());
                    buf.extend_from_slice(&(time_part as u32).to_be_bytes());
                } else {
                    // 退化为字符串
                    let s = format!("{}", micros);
                    let utf16 = encode_utf16_le(&s);
                    buf.extend_from_slice(&(utf16.len() as u16).to_le_bytes());
                    buf.extend_from_slice(&utf16);
                }
            }
            Value::Decimal(unscaled, scale) => {
                let s = format_decimal(*unscaled, *scale);
                let utf16 = encode_utf16_le(&s);
                buf.extend_from_slice(&(utf16.len() as u16).to_le_bytes());
                buf.extend_from_slice(&utf16);
            }
            Value::Json(v) => {
                let s = serde_json::to_string(v).unwrap_or_default();
                let utf16 = encode_utf16_le(&s);
                buf.extend_from_slice(&(utf16.len() as u16).to_le_bytes());
                buf.extend_from_slice(&utf16);
            }
            Value::Array(arr) => {
                let json_arr: Vec<&Value> = arr.iter().collect();
                let s = serde_json::to_string(&json_arr).unwrap_or_default();
                let utf16 = encode_utf16_le(&s);
                buf.extend_from_slice(&(utf16.len() as u16).to_le_bytes());
                buf.extend_from_slice(&utf16);
            }
            Value::Enum(s) => {
                let utf16 = encode_utf16_le(s);
                buf.extend_from_slice(&(utf16.len() as u16).to_le_bytes());
                buf.extend_from_slice(&utf16);
            }
            Value::Range(_) => {
                buf.extend_from_slice(&0u16.to_le_bytes());
            }
            Value::TsVector(tv) => {
                let s = tv.to_pg_string();
                let utf16 = encode_utf16_le(&s);
                buf.extend_from_slice(&(utf16.len() as u16).to_le_bytes());
                buf.extend_from_slice(&utf16);
            }
            Value::TsQuery(_) => {
                buf.extend_from_slice(&0u16.to_le_bytes());
            }
            // P4-5: 向量以 UTF-16 LE 文本输出
            Value::Vector(v) => {
                let s = v.to_string();
                let utf16 = encode_utf16_le(&s);
                buf.extend_from_slice(&(utf16.len() as u16).to_le_bytes());
                buf.extend_from_slice(&utf16);
            }
            // SQL/XML: XML 以 UTF-16 LE 文本输出
            Value::Xml(x) => {
                let utf16 = encode_utf16_le(x);
                buf.extend_from_slice(&(utf16.len() as u16).to_le_bytes());
                buf.extend_from_slice(&utf16);
            }
        }
        buf
    }

    /// 编码一行（按列顺序）。
    pub fn encode(values: &[Value], columns: &[ColumnMetaData]) -> Self {
        let mut data = Vec::with_capacity(64);
        for (i, value) in values.iter().enumerate() {
            if let Some(col) = columns.get(i) {
                data.extend_from_slice(&Self::encode_value(value, col));
            }
        }
        Self { data }
    }
}

/// 结果集编码器。
pub struct ResultSetEncoder;

impl ResultSetEncoder {
    /// 编码 ColumnMetaData token（0x81）。
    pub fn encode_column_metadata(columns: &[ColumnMetaData]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        // 0x81 = COLMETADATA token
        buf.push(0x81);
        // TvpResultFlags（1 字节）
        buf.push(0x00);
        // NumColumns（2 字节 BE）
        buf.extend_from_slice(&(columns.len() as u16).to_be_bytes());
        // 每列 TYPE_INFO
        for col in columns {
            buf.extend_from_slice(&col.encode_type_info());
        }
        buf
    }

    /// 编码 Row token（0xD1）+ 行数据。
    pub fn encode_row(row: &TdsRow) -> Vec<u8> {
        let mut buf = Vec::with_capacity(1 + row.data.len());
        buf.push(0xD1);
        buf.extend_from_slice(&row.data);
        buf
    }

    /// 编码 Done token（0xFD）。
    ///
    /// TDS 7.2+ Done 包含 8 字节行数。
    pub fn encode_done(status: DoneStatus, command_type: u16, row_count: u64) -> Vec<u8> {
        let mut buf = Vec::with_capacity(13);
        buf.push(0xFD);
        buf.extend_from_slice(&status.0.to_le_bytes());
        buf.extend_from_slice(&command_type.to_le_bytes());
        buf.extend_from_slice(&row_count.to_le_bytes());
        buf
    }

    /// 编码 DONE token 表示命令完成（无结果集）。
    pub fn encode_done_final(row_count: u64) -> Vec<u8> {
        Self::encode_done(DoneStatus::FINAL, 0, row_count)
    }

    /// 编码完整结果集：ColumnMetaData + Row* + Done。
    pub fn encode_result_set(columns: &[ColumnMetaData], rows: &[Vec<Value>]) -> Vec<Vec<u8>> {
        let mut packets = Vec::with_capacity(2 + rows.len());
        packets.push(Self::encode_column_metadata(columns));
        for row in rows {
            let tds_row = TdsRow::encode(row, columns);
            packets.push(Self::encode_row(&tds_row));
        }
        packets.push(Self::encode_done(DoneStatus::FINAL, 0, rows.len() as u64));
        packets
    }
}

/// ENVCHANGE token 环境类型枚举（MS-TDS 规范）。
///
/// 表示服务器环境上下文发生变化的类型，登录成功后通常会发送
/// PacketSize / Database / Collation 三种 ENVCHANGE。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum EnvChangeType {
    /// PacketSize（1）：协商的包大小变化
    PacketSize = 1,
    /// BeginTxn（4）：事务开始
    BeginTxn = 4,
    /// CommitTxn（5）：事务提交
    CommitTxn = 5,
    /// RollbackTxn（6）：事务回滚
    RollbackTxn = 6,
    /// Database（7）：当前数据库上下文变化
    Database = 7,
    /// Language（8）：会话语言变化
    Language = 8,
    /// Charset（9）：字符集变化
    Charset = 9,
    /// Collation（10）：排序规则变化
    Collation = 10,
}

impl EnvChangeType {
    /// 从字节解析环境类型。
    pub fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            1 => EnvChangeType::PacketSize,
            4 => EnvChangeType::BeginTxn,
            5 => EnvChangeType::CommitTxn,
            6 => EnvChangeType::RollbackTxn,
            7 => EnvChangeType::Database,
            8 => EnvChangeType::Language,
            9 => EnvChangeType::Charset,
            10 => EnvChangeType::Collation,
            _ => return None,
        })
    }
}

/// 编码 ENVCHANGE token (0xE3)。
///
/// 格式（MS-TDS 2.2.7.4）：
/// ```text
/// token_type(1B = 0xE3)
/// length(2B LE)        —— 后续 payload 字节数
/// env_type(1B)         —— EnvChangeType
/// old_value(B-VARCHAR) —— 2B LE 长度 + 字节
/// new_value(B-VARCHAR) —— 2B LE 长度 + 字节
/// ```
///
/// 对于 PacketSize / Database / Language / Charset，old/new 使用 ANSI 字节。
/// 对于 Collation，old/new 为 5 字节排序规则（任务约定仍按 B-VARCHAR 编码）。
/// 对于事务 token，old/new 为 8 字节二进制（任务约定仍按 B-VARCHAR 编码）。
pub fn encode_envchange(env_type: u8, old: &str, new: &str) -> Vec<u8> {
    let old_bytes = old.as_bytes();
    let new_bytes = new.as_bytes();
    // payload = env_type(1) + old_len(2) + old + new_len(2) + new
    let payload_len = 1u16
        .saturating_add(2)
        .saturating_add(old_bytes.len() as u16)
        .saturating_add(2)
        .saturating_add(new_bytes.len() as u16);
    let mut buf = Vec::with_capacity(3 + payload_len as usize);
    // token 标识
    buf.push(0xE3);
    // length（LE）
    buf.extend_from_slice(&payload_len.to_le_bytes());
    // env_type
    buf.push(env_type);
    // old_value (B-VARCHAR)
    buf.extend_from_slice(&(old_bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(old_bytes);
    // new_value (B-VARCHAR)
    buf.extend_from_slice(&(new_bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(new_bytes);
    buf
}

/// 格式化 f64 为字符串（与 mysql-protocol 保持一致）。
fn format_float(f: f64) -> String {
    if f.is_nan() || f.is_infinite() {
        "NULL".to_string()
    } else if f.fract() == 0.0 {
        format!("{:.1}", f)
    } else {
        f.to_string()
    }
}

/// 格式化定点数。
fn format_decimal(unscaled: i128, scale: u8) -> String {
    if scale == 0 {
        return unscaled.to_string();
    }
    let scale = scale as u32;
    let abs = unscaled.unsigned_abs();
    let abs_str = abs.to_string();
    let int_part_len = abs_str.len().saturating_sub(scale as usize);
    let int_part = &abs_str[..int_part_len];
    let frac_part = &abs_str[int_part_len..];

    let mut result = String::new();
    if unscaled < 0 {
        result.push('-');
    }
    if int_part.is_empty() {
        result.push('0');
    } else {
        result.push_str(int_part);
    }
    result.push('.');
    let frac_padded = format!("{:0>width$}", frac_part, width = scale as usize);
    result.push_str(&frac_padded);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_column_metadata_nvarchar() {
        let col = ColumnMetaData::nvarchar("name", 255);
        assert_eq!(col.column_type, TdsType::NVarChar);
        assert_eq!(col.max_length, 255);
        assert!(col.nullable);
    }

    #[test]
    fn test_column_metadata_integer_byte_lengths() {
        let col1 = ColumnMetaData::integer("c1", 100);
        assert_eq!(col1.int_byte_len, IntByteLen::One);
        let col2 = ColumnMetaData::integer("c2", 1000);
        assert_eq!(col2.int_byte_len, IntByteLen::Two);
        let col4 = ColumnMetaData::integer("c4", 100_000);
        assert_eq!(col4.int_byte_len, IntByteLen::Four);
        let col8 = ColumnMetaData::integer("c8", i64::MAX);
        assert_eq!(col8.int_byte_len, IntByteLen::Eight);
    }

    #[test]
    fn test_column_metadata_from_value_text() {
        let col = ColumnMetaData::from_value("name", &Value::Text("hello".to_string()));
        assert_eq!(col.column_type, TdsType::NVarChar);
        assert!(col.nullable);
    }

    #[test]
    fn test_column_metadata_from_value_int() {
        let col = ColumnMetaData::from_value("id", &Value::Int64(42));
        assert_eq!(col.column_type, TdsType::IntN);
        assert_eq!(col.int_byte_len, IntByteLen::One);
    }

    #[test]
    fn test_encode_type_info_nvarchar() {
        let col = ColumnMetaData::nvarchar("name", 100);
        let bytes = col.encode_type_info();
        // UserType (4) + Flags (2) + TYPE_INFO (1 + 2 + 4) + ColName length (2) + UTF-16 "name" (8)
        assert!(bytes.len() > 13);
        // 类型字节应为 0xE7
        assert_eq!(bytes[6], 0xE7);
    }

    #[test]
    fn test_encode_value_null_nvarchar() {
        let col = ColumnMetaData::nvarchar("name", 100);
        let bytes = TdsRow::encode_value(&Value::Null, &col);
        assert_eq!(bytes, vec![0, 0]); // 2 字节 LE 长度 = 0
    }

    #[test]
    fn test_encode_value_int64_byte_lengths() {
        let col = ColumnMetaData::integer("id", 0);
        let bytes = TdsRow::encode_value(&Value::Int64(5), &col);
        assert_eq!(bytes, vec![1, 5]); // 长度 1 + 1 字节值

        // 列元数据应根据所存储值的范围选择字节长度
        let col2 = ColumnMetaData::integer("id", 300);
        let bytes2 = TdsRow::encode_value(&Value::Int64(300), &col2);
        assert_eq!(bytes2[0], 2);
        assert_eq!(bytes2.len(), 3);
    }

    #[test]
    fn test_encode_value_text() {
        let col = ColumnMetaData::nvarchar("name", 255);
        let bytes = TdsRow::encode_value(&Value::Text("hi".to_string()), &col);
        assert_eq!(bytes[0..2], 4u16.to_le_bytes()); // UTF-16LE 2 字符 = 4 字节
        assert_eq!(bytes.len(), 6);
    }

    #[test]
    fn test_encode_value_bool_bit() {
        let col = ColumnMetaData::bit("flag");
        let bytes = TdsRow::encode_value(&Value::Bool(true), &col);
        assert_eq!(bytes, vec![1]);
        let bytes2 = TdsRow::encode_value(&Value::Bool(false), &col);
        assert_eq!(bytes2, vec![0]);
    }

    #[test]
    fn test_encode_value_float() {
        let col = ColumnMetaData::float8("value");
        let bytes = TdsRow::encode_value(&Value::Float64(3.5), &col);
        assert_eq!(bytes[0], 8);
        assert_eq!(bytes.len(), 9);
    }

    #[test]
    fn test_encode_row_multiple_columns() {
        let cols = vec![
            ColumnMetaData::integer("id", 0),
            ColumnMetaData::nvarchar("name", 100),
        ];
        let row = vec![Value::Int64(1), Value::Text("Alice".to_string())];
        let tds_row = TdsRow::encode(&row, &cols);
        // 第 1 列：长度 1 + 1 字节值；第 2 列：2 字节长度 + 10 字节 UTF-16
        assert!(!tds_row.data.is_empty());
        assert_eq!(tds_row.data[0], 1);
        assert_eq!(tds_row.data[1], 1);
    }

    #[test]
    fn test_encode_column_metadata_token() {
        let cols = vec![
            ColumnMetaData::integer("id", 0),
            ColumnMetaData::nvarchar("name", 100),
        ];
        let bytes = ResultSetEncoder::encode_column_metadata(&cols);
        assert_eq!(bytes[0], 0x81); // COLMETADATA token
        assert_eq!(bytes[1], 0x00); // TvpResultFlags
        assert_eq!(&bytes[2..4], &2u16.to_be_bytes()); // NumColumns
    }

    #[test]
    fn test_encode_row_token() {
        let row = TdsRow {
            data: vec![1, 2, 3],
        };
        let bytes = ResultSetEncoder::encode_row(&row);
        assert_eq!(bytes[0], 0xD1);
        assert_eq!(&bytes[1..], &[1, 2, 3]);
    }

    #[test]
    fn test_encode_done_token() {
        let bytes = ResultSetEncoder::encode_done(DoneStatus::FINAL, 0, 5);
        assert_eq!(bytes[0], 0xFD);
        assert_eq!(&bytes[1..3], &0u16.to_le_bytes());
        assert_eq!(&bytes[3..5], &0u16.to_le_bytes());
        assert_eq!(&bytes[5..13], &5u64.to_le_bytes());
    }

    #[test]
    fn test_encode_result_set_structure() {
        let cols = vec![
            ColumnMetaData::integer("id", 0),
            ColumnMetaData::nvarchar("name", 100),
        ];
        let rows = vec![
            vec![Value::Int64(1), Value::Text("Alice".to_string())],
            vec![Value::Int64(2), Value::Text("Bob".to_string())],
        ];
        let packets = ResultSetEncoder::encode_result_set(&cols, &rows);
        assert_eq!(packets.len(), 4); // 1 metadata + 2 rows + 1 done
        assert_eq!(packets[0][0], 0x81);
        assert_eq!(packets[1][0], 0xD1);
        assert_eq!(packets[2][0], 0xD1);
        assert_eq!(packets[3][0], 0xFD);
    }

    #[test]
    fn test_done_status_constants() {
        assert_eq!(DoneStatus::FINAL.0, 0x0000);
        assert_eq!(DoneStatus::MORE.0, 0x0001);
        assert_eq!(DoneStatus::ERROR.0, 0x0002);
        assert_eq!(DoneStatus::COUNT.0, 0x0010);
    }

    #[test]
    fn test_format_decimal_basic() {
        assert_eq!(format_decimal(12345, 0), "12345");
        assert_eq!(format_decimal(12345, 2), "123.45");
        assert_eq!(format_decimal(-12345, 2), "-123.45");
        assert_eq!(format_decimal(5, 2), "0.05");
        assert_eq!(format_decimal(0, 2), "0.00");
    }

    #[test]
    fn test_format_float_basic() {
        assert_eq!(format_float(3.5), "3.5");
        assert_eq!(format_float(2.0), "2.0");
        assert_eq!(format_float(f64::NAN), "NULL");
        assert_eq!(format_float(f64::INFINITY), "NULL");
    }

    #[test]
    fn test_encode_value_blob() {
        let col = ColumnMetaData::varbinary("data", 255);
        let bytes = TdsRow::encode_value(&Value::Blob(vec![0xDE, 0xAD]), &col);
        assert_eq!(&bytes[0..2], &2u16.to_le_bytes());
        assert_eq!(&bytes[2..], &[0xDE, 0xAD]);
    }

    #[test]
    fn test_encode_value_date_epoch() {
        let col = ColumnMetaData::date("d");
        let bytes = TdsRow::encode_value(&Value::Date(0), &col);
        assert_eq!(bytes.len(), 3);
    }

    #[test]
    fn test_encode_done_final_helper() {
        let bytes = ResultSetEncoder::encode_done_final(10);
        assert_eq!(bytes[0], 0xFD);
        let row_count = u64::from_le_bytes([
            bytes[5], bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11], bytes[12],
        ]);
        assert_eq!(row_count, 10);
    }

    #[test]
    fn test_env_change_type_from_byte_known() {
        assert_eq!(EnvChangeType::from_byte(1), Some(EnvChangeType::PacketSize));
        assert_eq!(EnvChangeType::from_byte(4), Some(EnvChangeType::BeginTxn));
        assert_eq!(EnvChangeType::from_byte(5), Some(EnvChangeType::CommitTxn));
        assert_eq!(
            EnvChangeType::from_byte(6),
            Some(EnvChangeType::RollbackTxn)
        );
        assert_eq!(EnvChangeType::from_byte(7), Some(EnvChangeType::Database));
        assert_eq!(EnvChangeType::from_byte(8), Some(EnvChangeType::Language));
        assert_eq!(EnvChangeType::from_byte(9), Some(EnvChangeType::Charset));
        assert_eq!(EnvChangeType::from_byte(10), Some(EnvChangeType::Collation));
    }

    #[test]
    fn test_env_change_type_from_byte_unknown() {
        assert_eq!(EnvChangeType::from_byte(0), None);
        assert_eq!(EnvChangeType::from_byte(2), None);
        assert_eq!(EnvChangeType::from_byte(255), None);
    }

    #[test]
    fn test_encode_envchange_packet_size() {
        let bytes = encode_envchange(EnvChangeType::PacketSize as u8, "4096", "4096");
        // token + length(2) + env_type(1) + old_len(2) + old(4) + new_len(2) + new(4)
        assert_eq!(bytes[0], 0xE3);
        let payload_len = u16::from_le_bytes([bytes[1], bytes[2]]) as usize;
        assert_eq!(payload_len, 1 + 2 + 4 + 2 + 4);
        assert_eq!(bytes[3], EnvChangeType::PacketSize as u8);
        let old_len = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        assert_eq!(old_len, 4);
        assert_eq!(&bytes[6..10], b"4096");
        let new_len = u16::from_le_bytes([bytes[10], bytes[11]]) as usize;
        assert_eq!(new_len, 4);
        assert_eq!(&bytes[12..16], b"4096");
        // 整体长度 = 3 + payload_len
        assert_eq!(bytes.len(), 3 + payload_len);
    }

    #[test]
    fn test_encode_envchange_database() {
        let bytes = encode_envchange(EnvChangeType::Database as u8, "", "master");
        assert_eq!(bytes[0], 0xE3);
        assert_eq!(bytes[3], EnvChangeType::Database as u8);
        // old 为空字符串
        let old_len = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        assert_eq!(old_len, 0);
        // new = "master"
        let new_len = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        assert_eq!(new_len, 6);
        assert_eq!(&bytes[8..14], b"master");
    }

    #[test]
    fn test_encode_envchange_collation() {
        // Collation：old/new 任意字符串（任务约定按 B-VARCHAR 编码）
        let bytes = encode_envchange(EnvChangeType::Collation as u8, "", "0x0904d000");
        assert_eq!(bytes[0], 0xE3);
        assert_eq!(bytes[3], EnvChangeType::Collation as u8);
        let new_len = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        assert_eq!(new_len, 10);
        assert_eq!(&bytes[8..18], b"0x0904d000");
    }

    #[test]
    fn test_encode_envchange_empty_values() {
        let bytes = encode_envchange(EnvChangeType::BeginTxn as u8, "", "");
        // payload = env_type(1) + old_len(2) + new_len(2) = 5
        assert_eq!(bytes[0], 0xE3);
        let payload_len = u16::from_le_bytes([bytes[1], bytes[2]]) as usize;
        assert_eq!(payload_len, 5);
        assert_eq!(bytes.len(), 3 + 5);
    }
}
