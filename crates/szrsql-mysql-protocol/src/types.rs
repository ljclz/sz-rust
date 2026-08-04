//! MySQL 类型系统映射。
//!
//! 将 SzRSQL 内部类型（`szrsql_types::Value`）映射为 MySQL 协议类型。
//!
//! MySQL 协议在 Column Definition 中使用 1 字节类型标识符，
//! 详见 MySQL 文档 "Protocol::ColumnDefinition41"。

use szrsql_types::value::{ColumnType, Value};

/// MySQL 协议列类型标识符（来自 mysql_com.h）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MysqlType {
    /// DECIMAL
    Decimal = 0,
    /// TINYINT
    Tiny = 1,
    /// SMALLINT
    Short = 2,
    /// INT / INTEGER
    Long = 3,
    /// FLOAT
    Float = 4,
    /// DOUBLE
    Double = 5,
    /// NULL
    Null = 6,
    /// TIMESTAMP
    Timestamp = 7,
    /// BIGINT
    LongLong = 8,
    /// MEDIUMINT
    Int24 = 9,
    /// DATE
    Date = 10,
    /// TIME
    Time = 11,
    /// DATETIME
    DateTime = 12,
    /// YEAR(2/4)
    Year = 13,
    /// VARCHAR / VAR_STRING
    VarString = 253,
    /// BIT
    Bit = 16,
    /// JSON
    Json = 245,
    /// NEWDECIMAL（精确小数）
    NewDecimal = 246,
    /// ENUM
    Enum = 247,
    /// SET
    Set = 248,
    /// TINYBLOB / TINYTEXT
    TinyBlob = 249,
    /// MEDIUMBLOB / MEDIUMTEXT
    MediumBlob = 250,
    /// LONGBLOB / LONGTEXT
    LongBlob = 251,
    /// BLOB / TEXT
    Blob = 252,
    /// CHAR / STRING（固定长度）
    String = 254,
    /// GEOMETRY
    Geometry = 255,
}

impl MysqlType {
    /// 根据 SzRSQL Value 推导对应的 MySQL 类型。
    pub fn from_value(value: &Value) -> Self {
        match value {
            Value::Null => MysqlType::Null,
            Value::Bool(_) => MysqlType::Tiny,
            Value::Int64(n) => {
                // 范围在 i8/i16/i32 内用对应小类型，否则 BIGINT
                if *n >= i8::MIN as i64 && *n <= i8::MAX as i64 {
                    MysqlType::Tiny
                } else if *n >= i16::MIN as i64 && *n <= i16::MAX as i64 {
                    MysqlType::Short
                } else if *n >= i32::MIN as i64 && *n <= i32::MAX as i64 {
                    MysqlType::Long
                } else {
                    MysqlType::LongLong
                }
            }
            Value::Float64(_) => MysqlType::Double,
            Value::Decimal(_, _) => MysqlType::NewDecimal,
            Value::Text(_) => MysqlType::VarString,
            Value::Blob(_) => MysqlType::Blob,
            Value::Date(_) => MysqlType::Date,
            Value::Timestamp(_) => MysqlType::DateTime,
            Value::Json(_) => MysqlType::Json,
            Value::Array(_) => MysqlType::Json, // 数组序列化为 JSON
            Value::Enum(_) => MysqlType::Enum,
            Value::Range(_) => MysqlType::String,
            Value::TsVector(_) | Value::TsQuery(_) | Value::Vector(_) | Value::Xml(_) => MysqlType::LongBlob,
        }
    }

    /// 返回该类型的字面量字符串（用于 Column Definition 中的列类型名）。
    pub fn type_name(self) -> &'static str {
        match self {
            MysqlType::Decimal => "DECIMAL",
            MysqlType::Tiny => "TINYINT",
            MysqlType::Short => "SMALLINT",
            MysqlType::Long => "INT",
            MysqlType::Float => "FLOAT",
            MysqlType::Double => "DOUBLE",
            MysqlType::Null => "NULL",
            MysqlType::Timestamp => "TIMESTAMP",
            MysqlType::LongLong => "BIGINT",
            MysqlType::Int24 => "MEDIUMINT",
            MysqlType::Date => "DATE",
            MysqlType::Time => "TIME",
            MysqlType::DateTime => "DATETIME",
            MysqlType::Year => "YEAR",
            MysqlType::VarString => "VARCHAR",
            MysqlType::Bit => "BIT",
            MysqlType::Json => "JSON",
            MysqlType::NewDecimal => "DECIMAL",
            MysqlType::Enum => "ENUM",
            MysqlType::Set => "SET",
            MysqlType::TinyBlob => "TINYBLOB",
            MysqlType::MediumBlob => "MEDIUMBLOB",
            MysqlType::LongBlob => "LONGBLOB",
            MysqlType::Blob => "BLOB",
            MysqlType::String => "CHAR",
            MysqlType::Geometry => "GEOMETRY",
        }
    }

    /// 返回该类型的二进制标志位（用于 Column Definition flags 字段）。
    pub fn binary_flag(self) -> u16 {
        match self {
            MysqlType::TinyBlob | MysqlType::MediumBlob | MysqlType::LongBlob | MysqlType::Blob => {
                128
            } // BINARY_FLAG
            _ => 0,
        }
    }

    /// 根据 SzRSQL ColumnType 推导对应的 MySQL 类型。
    ///
    /// 用于 ColumnDefinition 构造时根据 ResultColumn.column_type 设置正确的列类型，
    /// 使客户端（如 pymysql）能正确将文本响应转换为 Python int/float/str 等类型。
    pub fn from_column_type(ct: &ColumnType) -> Self {
        match ct {
            ColumnType::Null => MysqlType::Null,
            ColumnType::Int64 => MysqlType::LongLong,
            ColumnType::Float64 => MysqlType::Double,
            ColumnType::Text => MysqlType::VarString,
            ColumnType::Blob => MysqlType::Blob,
            ColumnType::Bool => MysqlType::Tiny,
            ColumnType::Date => MysqlType::Date,
            ColumnType::Timestamp => MysqlType::DateTime,
            ColumnType::Decimal { .. } => MysqlType::NewDecimal,
            ColumnType::Json => MysqlType::Json,
            ColumnType::TsVector | ColumnType::TsQuery | ColumnType::Vector(_)
            | ColumnType::Xml => MysqlType::LongBlob,
            ColumnType::Enum(_) => MysqlType::Enum,
            ColumnType::Array(_) => MysqlType::Json,
            ColumnType::Range(_) => MysqlType::String,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_from_null() {
        assert_eq!(MysqlType::from_value(&Value::Null), MysqlType::Null);
    }

    #[test]
    fn test_type_from_bool() {
        assert_eq!(MysqlType::from_value(&Value::Bool(true)), MysqlType::Tiny);
        assert_eq!(MysqlType::from_value(&Value::Bool(false)), MysqlType::Tiny);
    }

    #[test]
    fn test_type_from_int64_ranges() {
        // i8 范围
        assert_eq!(MysqlType::from_value(&Value::Int64(100)), MysqlType::Tiny);
        assert_eq!(MysqlType::from_value(&Value::Int64(-100)), MysqlType::Tiny);
        // i16 范围
        assert_eq!(MysqlType::from_value(&Value::Int64(1000)), MysqlType::Short);
        // i32 范围
        assert_eq!(
            MysqlType::from_value(&Value::Int64(100000)),
            MysqlType::Long
        );
        // 超出 i32 范围
        assert_eq!(
            MysqlType::from_value(&Value::Int64(i64::MAX)),
            MysqlType::LongLong
        );
    }

    #[test]
    fn test_type_from_float() {
        assert_eq!(
            MysqlType::from_value(&Value::Float64(3.5)),
            MysqlType::Double
        );
    }

    #[test]
    fn test_type_from_text() {
        assert_eq!(
            MysqlType::from_value(&Value::Text("hello".to_string())),
            MysqlType::VarString
        );
    }

    #[test]
    fn test_type_from_blob() {
        assert_eq!(
            MysqlType::from_value(&Value::Blob(vec![1, 2, 3])),
            MysqlType::Blob
        );
    }

    #[test]
    fn test_type_from_date() {
        assert_eq!(MysqlType::from_value(&Value::Date(0)), MysqlType::Date);
    }

    #[test]
    fn test_type_from_timestamp() {
        assert_eq!(
            MysqlType::from_value(&Value::Timestamp(0)),
            MysqlType::DateTime
        );
    }

    #[test]
    fn test_type_from_json() {
        assert_eq!(
            MysqlType::from_value(&Value::Json(serde_json::Value::Null)),
            MysqlType::Json
        );
    }

    #[test]
    fn test_type_from_decimal() {
        assert_eq!(
            MysqlType::from_value(&Value::Decimal(12345, 2)),
            MysqlType::NewDecimal
        );
    }

    #[test]
    fn test_type_name_returns_correct_string() {
        assert_eq!(MysqlType::Long.type_name(), "INT");
        assert_eq!(MysqlType::LongLong.type_name(), "BIGINT");
        assert_eq!(MysqlType::VarString.type_name(), "VARCHAR");
        assert_eq!(MysqlType::Json.type_name(), "JSON");
        assert_eq!(MysqlType::Blob.type_name(), "BLOB");
    }

    #[test]
    fn test_binary_flag_for_blob_types() {
        assert_eq!(MysqlType::Blob.binary_flag(), 128);
        assert_eq!(MysqlType::TinyBlob.binary_flag(), 128);
        assert_eq!(MysqlType::MediumBlob.binary_flag(), 128);
        assert_eq!(MysqlType::LongBlob.binary_flag(), 128);
    }

    #[test]
    fn test_binary_flag_for_non_blob_types() {
        assert_eq!(MysqlType::Long.binary_flag(), 0);
        assert_eq!(MysqlType::VarString.binary_flag(), 0);
        assert_eq!(MysqlType::Json.binary_flag(), 0);
    }
}
