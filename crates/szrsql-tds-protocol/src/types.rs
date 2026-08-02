//! TDS 类型系统映射。
//!
//! 将 SzRSQL 内部类型（`szrsql_types::Value`）映射为 TDS 协议类型。
//!
//! TDS 协议在 ColumnMetaData token 中使用 1 字节类型标识符。
//! 详见 MS-TDS 文档 "Data Type Definitions"。
//!
//! 主要类型：
//! - `BIT` (0x68)：1 字节布尔
//! - `INTN` (0x26)：变长整数（1/2/4/8 字节）
//! - `FLOATN` (0x6E)：变长浮点（4/8 字节）
//! - `BIGVARCHAR` (0xA7)：变长 ANSI 字符串
//! - `BIGVARBIN` (0xA5)：变长二进制
//! - `NCHAR` (0xE6) / `NVARCHAR` (0xE7)：Unicode 字符串（UTF-16LE）
//! - `DATETIMN` (0x6D)：日期时间
//! - `DATEN` (0x28) / `TIMEN` (0x29)：日期 / 时间（TDS 7.3+）
//! - `TEXT` (0x23)：长文本

use szrsql_types::value::Value;

/// TDS 协议列类型标识符（来自 MS-TDS 规范）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TdsType {
    /// TEXT（长文本，ANSI）
    Text = 0x23,
    /// DATE（TDS 7.3+，3 字节）
    Date = 0x28,
    /// TIME（TDS 7.3+，变长）
    Time = 0x29,
    /// INTN（变长整数，1/2/4/8 字节）
    IntN = 0x26,
    /// BIGVARCHAR（变长 ANSI 字符串）
    BigVarChar = 0xA7,
    /// BIGVARBIN（变长二进制）
    BigVarBin = 0xA5,
    /// BIT（1 字节布尔）
    Bit = 0x68,
    /// FLOATN（变长浮点，4/8 字节）
    FloatN = 0x6E,
    /// DATETIMN（变长日期时间，4/8 字节）
    DateTimeN = 0x6D,
    /// NUMERICN / DECIMALN（定点数）
    NumericN = 0x6C,
    /// NCHAR（固定长度 Unicode）
    NChar = 0xE6,
    /// NVARCHAR（变长 Unicode，UTF-16LE）
    NVarChar = 0xE7,
    /// XML（TDS 9.0+）
    Xml = 0xF1,
}

impl TdsType {
    /// 从字节解析类型。
    pub fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            0x23 => TdsType::Text,
            0x28 => TdsType::Date,
            0x29 => TdsType::Time,
            0x26 => TdsType::IntN,
            0xA7 => TdsType::BigVarChar,
            0xA5 => TdsType::BigVarBin,
            0x68 => TdsType::Bit,
            0x6E => TdsType::FloatN,
            0x6D => TdsType::DateTimeN,
            0x6C => TdsType::NumericN,
            0xE6 => TdsType::NChar,
            0xE7 => TdsType::NVarChar,
            0xF1 => TdsType::Xml,
            _ => return None,
        })
    }

    /// 根据 SzRSQL Value 推导对应的 TDS 类型。
    ///
    /// 数值类型统一映射为 INTN/FLOATN，具体字节数由实际值范围决定。
    pub fn from_value(value: &Value) -> Self {
        match value {
            Value::Null => TdsType::IntN,
            Value::Bool(_) => TdsType::Bit,
            Value::Int64(_) => TdsType::IntN,
            Value::Float64(_) => TdsType::FloatN,
            Value::Decimal(_, _) => TdsType::NumericN,
            Value::Text(_) => TdsType::NVarChar,
            Value::Blob(_) => TdsType::BigVarBin,
            Value::Date(_) => TdsType::Date,
            Value::Timestamp(_) => TdsType::DateTimeN,
            Value::Json(_) => TdsType::NVarChar,
            Value::Array(_) => TdsType::NVarChar,
            Value::Enum(_) => TdsType::NVarChar,
            Value::Range(_) => TdsType::NVarChar,
            Value::TsVector(_) | Value::TsQuery(_) => TdsType::Text,
        }
    }

    /// 返回该类型的字面量字符串（用于错误显示）。
    pub fn type_name(self) -> &'static str {
        match self {
            TdsType::Text => "TEXT",
            TdsType::Date => "DATE",
            TdsType::Time => "TIME",
            TdsType::IntN => "INTN",
            TdsType::BigVarChar => "VARCHAR",
            TdsType::BigVarBin => "VARBINARY",
            TdsType::Bit => "BIT",
            TdsType::FloatN => "FLOATN",
            TdsType::DateTimeN => "DATETIME",
            TdsType::NumericN => "NUMERIC",
            TdsType::NChar => "NCHAR",
            TdsType::NVarChar => "NVARCHAR",
            TdsType::Xml => "XML",
        }
    }

    /// 返回该类型是否为变长类型（需要长度前缀）。
    pub fn is_variable_length(self) -> bool {
        matches!(
            self,
            TdsType::IntN
                | TdsType::BigVarChar
                | TdsType::BigVarBin
                | TdsType::FloatN
                | TdsType::DateTimeN
                | TdsType::NumericN
                | TdsType::NChar
                | TdsType::NVarChar
                | TdsType::Time
        )
    }

    /// 返回该类型是否为 Unicode 字符串类型。
    pub fn is_unicode(self) -> bool {
        matches!(self, TdsType::NChar | TdsType::NVarChar)
    }

    /// 返回该类型是否为整数类型。
    pub fn is_integer(self) -> bool {
        matches!(self, TdsType::IntN | TdsType::Bit)
    }

    /// 返回该类型是否为数值类型（整数或浮点）。
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            TdsType::IntN | TdsType::Bit | TdsType::FloatN | TdsType::NumericN
        )
    }

    /// 根据 Int64 范围推导 INTN 字段最大字节数（1/2/4/8）。
    pub fn int_byte_len(n: i64) -> u8 {
        if n >= i8::MIN as i64 && n <= i8::MAX as i64 {
            1
        } else if n >= i16::MIN as i64 && n <= i16::MAX as i64 {
            2
        } else if n >= i32::MIN as i64 && n <= i32::MAX as i64 {
            4
        } else {
            8
        }
    }
}

/// 整数类型字节数枚举（用于 INTN 类型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntByteLen {
    /// 1 字节（TINYINT / i8）
    One = 1,
    /// 2 字节（SMALLINT / i16）
    Two = 2,
    /// 4 字节（INT / i32）
    Four = 4,
    /// 8 字节（BIGINT / i64）
    Eight = 8,
}

impl IntByteLen {
    /// 从字节数构造。
    pub fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            1 => IntByteLen::One,
            2 => IntByteLen::Two,
            4 => IntByteLen::Four,
            8 => IntByteLen::Eight,
            _ => return None,
        })
    }

    /// 转换为字节数。
    pub fn as_bytes(self) -> u8 {
        self as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_from_byte_known() {
        assert_eq!(TdsType::from_byte(0x68), Some(TdsType::Bit));
        assert_eq!(TdsType::from_byte(0x26), Some(TdsType::IntN));
        assert_eq!(TdsType::from_byte(0x6E), Some(TdsType::FloatN));
        assert_eq!(TdsType::from_byte(0xE7), Some(TdsType::NVarChar));
        assert_eq!(TdsType::from_byte(0xA7), Some(TdsType::BigVarChar));
        assert_eq!(TdsType::from_byte(0xA5), Some(TdsType::BigVarBin));
        assert_eq!(TdsType::from_byte(0x23), Some(TdsType::Text));
        assert_eq!(TdsType::from_byte(0x6D), Some(TdsType::DateTimeN));
        assert_eq!(TdsType::from_byte(0x6C), Some(TdsType::NumericN));
        assert_eq!(TdsType::from_byte(0x28), Some(TdsType::Date));
        assert_eq!(TdsType::from_byte(0x29), Some(TdsType::Time));
        assert_eq!(TdsType::from_byte(0xE6), Some(TdsType::NChar));
        assert_eq!(TdsType::from_byte(0xF1), Some(TdsType::Xml));
    }

    #[test]
    fn test_type_from_byte_unknown() {
        assert_eq!(TdsType::from_byte(0xFF), None);
        assert_eq!(TdsType::from_byte(0x00), None);
        assert_eq!(TdsType::from_byte(0x99), None);
    }

    #[test]
    fn test_type_from_value_null() {
        assert_eq!(TdsType::from_value(&Value::Null), TdsType::IntN);
    }

    #[test]
    fn test_type_from_value_bool() {
        assert_eq!(TdsType::from_value(&Value::Bool(true)), TdsType::Bit);
        assert_eq!(TdsType::from_value(&Value::Bool(false)), TdsType::Bit);
    }

    #[test]
    fn test_type_from_value_int() {
        assert_eq!(TdsType::from_value(&Value::Int64(0)), TdsType::IntN);
        assert_eq!(TdsType::from_value(&Value::Int64(i64::MAX)), TdsType::IntN);
    }

    #[test]
    fn test_type_from_value_float_and_decimal() {
        assert_eq!(TdsType::from_value(&Value::Float64(3.5)), TdsType::FloatN);
        assert_eq!(
            TdsType::from_value(&Value::Decimal(12345, 2)),
            TdsType::NumericN
        );
    }

    #[test]
    fn test_type_from_value_text_and_blob() {
        assert_eq!(
            TdsType::from_value(&Value::Text("hello".to_string())),
            TdsType::NVarChar
        );
        assert_eq!(
            TdsType::from_value(&Value::Blob(vec![1, 2, 3])),
            TdsType::BigVarBin
        );
    }

    #[test]
    fn test_type_from_value_date_and_timestamp() {
        assert_eq!(TdsType::from_value(&Value::Date(0)), TdsType::Date);
        assert_eq!(
            TdsType::from_value(&Value::Timestamp(0)),
            TdsType::DateTimeN
        );
    }

    #[test]
    fn test_type_from_value_json_and_others() {
        assert_eq!(
            TdsType::from_value(&Value::Json(serde_json::Value::Null)),
            TdsType::NVarChar
        );
        assert_eq!(
            TdsType::from_value(&Value::Enum("a".to_string())),
            TdsType::NVarChar
        );
    }

    #[test]
    fn test_type_name_returns_correct_string() {
        assert_eq!(TdsType::Bit.type_name(), "BIT");
        assert_eq!(TdsType::IntN.type_name(), "INTN");
        assert_eq!(TdsType::NVarChar.type_name(), "NVARCHAR");
        assert_eq!(TdsType::FloatN.type_name(), "FLOATN");
        assert_eq!(TdsType::DateTimeN.type_name(), "DATETIME");
        assert_eq!(TdsType::BigVarBin.type_name(), "VARBINARY");
    }

    #[test]
    fn test_is_variable_length() {
        assert!(TdsType::IntN.is_variable_length());
        assert!(TdsType::NVarChar.is_variable_length());
        assert!(TdsType::FloatN.is_variable_length());
        // BIT 是固定长度（1 字节）
        assert!(!TdsType::Bit.is_variable_length());
        // DATE 是固定长度（3 字节）
        assert!(!TdsType::Date.is_variable_length());
    }

    #[test]
    fn test_is_unicode_and_is_integer() {
        assert!(TdsType::NVarChar.is_unicode());
        assert!(TdsType::NChar.is_unicode());
        assert!(!TdsType::BigVarChar.is_unicode());

        assert!(TdsType::IntN.is_integer());
        assert!(TdsType::Bit.is_integer());
        assert!(!TdsType::FloatN.is_integer());
    }

    #[test]
    fn test_is_numeric() {
        assert!(TdsType::IntN.is_numeric());
        assert!(TdsType::FloatN.is_numeric());
        assert!(TdsType::NumericN.is_numeric());
        assert!(TdsType::Bit.is_numeric());
        assert!(!TdsType::NVarChar.is_numeric());
    }

    #[test]
    fn test_int_byte_len_ranges() {
        assert_eq!(TdsType::int_byte_len(0), 1);
        assert_eq!(TdsType::int_byte_len(127), 1);
        assert_eq!(TdsType::int_byte_len(-128), 1);
        assert_eq!(TdsType::int_byte_len(128), 2);
        assert_eq!(TdsType::int_byte_len(32767), 2);
        assert_eq!(TdsType::int_byte_len(-32768), 2);
        assert_eq!(TdsType::int_byte_len(32768), 4);
        assert_eq!(TdsType::int_byte_len(2_147_483_647), 4);
        assert_eq!(TdsType::int_byte_len(2_147_483_648), 8);
        assert_eq!(TdsType::int_byte_len(i64::MAX), 8);
        assert_eq!(TdsType::int_byte_len(i64::MIN), 8);
    }

    #[test]
    fn test_int_byte_len_enum_from_byte() {
        assert_eq!(IntByteLen::from_byte(1), Some(IntByteLen::One));
        assert_eq!(IntByteLen::from_byte(2), Some(IntByteLen::Two));
        assert_eq!(IntByteLen::from_byte(4), Some(IntByteLen::Four));
        assert_eq!(IntByteLen::from_byte(8), Some(IntByteLen::Eight));
        assert_eq!(IntByteLen::from_byte(3), None);
        assert_eq!(IntByteLen::from_byte(16), None);
    }

    #[test]
    fn test_int_byte_len_enum_as_bytes() {
        assert_eq!(IntByteLen::One.as_bytes(), 1);
        assert_eq!(IntByteLen::Two.as_bytes(), 2);
        assert_eq!(IntByteLen::Four.as_bytes(), 4);
        assert_eq!(IntByteLen::Eight.as_bytes(), 8);
    }
}
