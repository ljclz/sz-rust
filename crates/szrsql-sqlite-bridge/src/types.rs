//! SQLite 类型系统映射。
//!
//! 本模块定义 SQLite 的存储类型枚举，并提供从 SzRSQL `Value` 推导
//! 对应 SQLite 类型的能力。
//!
//! # SQLite 存储类
//!
//! SQLite 采用动态类型系统，每个值属于以下 5 种存储类之一：
//!
//! | 存储类 | 数字代码 | 说明 |
//! |--------|----------|------|
//! | INTEGER | 1 | 8 字节有符号整数 |
//! | FLOAT | 2 | 8 字节 IEEE 754 浮点数 |
//! | TEXT | 3 | UTF-8/UTF-16 编码字符串 |
//! | BLOB | 4 | 任意二进制数据 |
//! | NULL | 5 | 空值 |
//!
//! # 类型映射策略
//!
//! 由于 SQLite 是动态类型系统，而 SzRSQL 是强类型系统，映射时遵循
//! "最小损失"原则：
//!
//! | SzRSQL Value | SQLite 类型 |
//! |--------------|-------------|
//! | Null | NULL |
//! | Int64 / Bool / Date / Timestamp / Decimal | INTEGER |
//! | Float64 | FLOAT |
//! | Text / Enum | TEXT |
//! | Blob | BLOB |
//! | Array / Range / Json / TsVector / TsQuery | TEXT（序列化后存储） |

use szrsql_types::value::Value;

// =====================================================================
//  SQLite 类型枚举
// =====================================================================

/// SQLite 存储类枚举
///
/// 数字代码与 SQLite 文件格式规范中的"串行类型"前 5 个值对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SqliteType {
    /// INTEGER 存储类（串行类型代码 1）
    Integer = 1,
    /// FLOAT 存储类（串行类型代码 2）
    Float = 2,
    /// TEXT 存储类（串行类型代码 3）
    Text = 3,
    /// BLOB 存储类（串行类型代码 4）
    Blob = 4,
    /// NULL 存储类（串行类型代码 5）
    Null = 5,
}

impl SqliteType {
    /// 从 SzRSQL `Value` 推导对应的 SQLite 存储类。
    ///
    /// 映射规则遵循"最小损失"原则：
    /// - 数值类（Int64/Bool/Date/Timestamp/Decimal）→ INTEGER
    /// - 浮点类（Float64）→ FLOAT
    /// - 文本类（Text/Enum/Array/Range/Json/TsVector/TsQuery）→ TEXT
    /// - 二进制类（Blob）→ BLOB
    /// - 空值（Null）→ NULL
    pub fn from_value(value: &Value) -> Self {
        match value {
            Value::Null => SqliteType::Null,
            // 整数类：直接存储为 INTEGER
            Value::Int64(_) | Value::Bool(_) | Value::Date(_) | Value::Timestamp(_) => {
                SqliteType::Integer
            }
            // 定点数：scale=0 时为整数，否则为 FLOAT（SQLite 无原生 Decimal）
            Value::Decimal(_, scale) => {
                if *scale == 0 {
                    SqliteType::Integer
                } else {
                    SqliteType::Float
                }
            }
            // 浮点类
            Value::Float64(_) => SqliteType::Float,
            // 文本类
            Value::Text(_) | Value::Enum(_) => SqliteType::Text,
            // 复合类型序列化为 JSON 文本存储
            Value::Array(_)
            | Value::Range(_)
            | Value::Json(_)
            | Value::TsVector(_)
            | Value::TsQuery(_) => SqliteType::Text,
            // 二进制
            Value::Blob(_) => SqliteType::Blob,
        }
    }

    /// 返回该类型的标准名称字符串（用于错误信息和调试输出）。
    pub fn type_name(self) -> &'static str {
        match self {
            SqliteType::Integer => "INTEGER",
            SqliteType::Float => "FLOAT",
            SqliteType::Text => "TEXT",
            SqliteType::Blob => "BLOB",
            SqliteType::Null => "NULL",
        }
    }

    /// 返回 SQLite 文件格式中的数字代码（1..=5）。
    pub fn code(self) -> u8 {
        self as u8
    }
}

impl From<&Value> for SqliteType {
    fn from(value: &Value) -> Self {
        Self::from_value(value)
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_types::value::{RangeType, RangeValue, TsQuery, TsVector};

    // -----------------------------------------------------------------
    //  基础映射测试
    // -----------------------------------------------------------------

    #[test]
    fn from_value_null_maps_to_null() {
        // Null → NULL
        assert_eq!(SqliteType::from_value(&Value::Null), SqliteType::Null);
    }

    #[test]
    fn from_value_int64_maps_to_integer() {
        // Int64 → INTEGER
        assert_eq!(
            SqliteType::from_value(&Value::Int64(42)),
            SqliteType::Integer
        );
        assert_eq!(
            SqliteType::from_value(&Value::Int64(i64::MIN)),
            SqliteType::Integer
        );
        assert_eq!(
            SqliteType::from_value(&Value::Int64(i64::MAX)),
            SqliteType::Integer
        );
    }

    #[test]
    fn from_value_float64_maps_to_float() {
        // Float64 → FLOAT
        assert_eq!(
            SqliteType::from_value(&Value::Float64(3.5)),
            SqliteType::Float
        );
        // 边界值：NaN / Infinity 仍属于 FLOAT
        assert_eq!(
            SqliteType::from_value(&Value::Float64(f64::NAN)),
            SqliteType::Float
        );
        assert_eq!(
            SqliteType::from_value(&Value::Float64(f64::INFINITY)),
            SqliteType::Float
        );
    }

    #[test]
    fn from_value_text_maps_to_text() {
        // Text → TEXT
        assert_eq!(
            SqliteType::from_value(&Value::Text("hello".to_string())),
            SqliteType::Text
        );
        // 空字符串仍属于 TEXT
        assert_eq!(
            SqliteType::from_value(&Value::Text(String::new())),
            SqliteType::Text
        );
    }

    #[test]
    fn from_value_blob_maps_to_blob() {
        // Blob → BLOB
        assert_eq!(
            SqliteType::from_value(&Value::Blob(vec![0xDE, 0xAD])),
            SqliteType::Blob
        );
        // 空字节串仍属于 BLOB
        assert_eq!(
            SqliteType::from_value(&Value::Blob(Vec::new())),
            SqliteType::Blob
        );
    }

    // -----------------------------------------------------------------
    //  数值类转换测试（Bool/Date/Timestamp/Decimal）
    // -----------------------------------------------------------------

    #[test]
    fn from_value_bool_maps_to_integer() {
        // Bool → INTEGER（SQLite 无独立 BOOLEAN，按整数 0/1 存储）
        assert_eq!(
            SqliteType::from_value(&Value::Bool(true)),
            SqliteType::Integer
        );
        assert_eq!(
            SqliteType::from_value(&Value::Bool(false)),
            SqliteType::Integer
        );
    }

    #[test]
    fn from_value_date_and_timestamp_map_to_integer() {
        // Date → INTEGER（自 epoch 起的天数）
        assert_eq!(SqliteType::from_value(&Value::Date(0)), SqliteType::Integer);
        assert_eq!(
            SqliteType::from_value(&Value::Date(20454)),
            SqliteType::Integer
        );
        // Timestamp → INTEGER（微秒时间戳）
        assert_eq!(
            SqliteType::from_value(&Value::Timestamp(1_700_000_000_000_000)),
            SqliteType::Integer
        );
    }

    #[test]
    fn from_value_decimal_scale_dependent() {
        // scale=0 → INTEGER（无小数部分）
        assert_eq!(
            SqliteType::from_value(&Value::Decimal(42, 0)),
            SqliteType::Integer
        );
        // scale>0 → FLOAT（SQLite 无原生 Decimal）
        assert_eq!(
            SqliteType::from_value(&Value::Decimal(12345, 2)),
            SqliteType::Float
        );
        assert_eq!(
            SqliteType::from_value(&Value::Decimal(-5, 3)),
            SqliteType::Float
        );
    }

    // -----------------------------------------------------------------
    //  复合类型映射测试
    // -----------------------------------------------------------------

    #[test]
    fn from_value_enum_maps_to_text() {
        // Enum → TEXT（存储枚举字面量）
        assert_eq!(
            SqliteType::from_value(&Value::Enum("active".to_string())),
            SqliteType::Text
        );
    }

    #[test]
    fn from_value_compound_types_map_to_text() {
        // Array → TEXT（序列化为 JSON 存储）
        let arr = Value::Array(vec![Value::Int64(1), Value::Int64(2)]);
        assert_eq!(SqliteType::from_value(&arr), SqliteType::Text);

        // Range → TEXT
        let range = Value::Range(RangeValue {
            lower: Some(Box::new(Value::Int64(1))),
            upper: Some(Box::new(Value::Int64(10))),
            lower_inc: true,
            upper_inc: false,
            range_type: RangeType::Int4Range,
        });
        assert_eq!(SqliteType::from_value(&range), SqliteType::Text);

        // Json → TEXT
        let json = Value::Json(serde_json::json!({"key": "value"}));
        assert_eq!(SqliteType::from_value(&json), SqliteType::Text);

        // TsVector → TEXT
        let ts = Value::TsVector(TsVector::from_lexemes(["hello", "world"]));
        assert_eq!(SqliteType::from_value(&ts), SqliteType::Text);

        // TsQuery → TEXT
        let tq = Value::TsQuery(TsQuery::lexeme("hello"));
        assert_eq!(SqliteType::from_value(&tq), SqliteType::Text);
    }

    // -----------------------------------------------------------------
    //  type_name / code / From<&Value> 测试
    // -----------------------------------------------------------------

    #[test]
    fn type_name_returns_canonical_string() {
        // 验证每个变体的标准名称字符串
        assert_eq!(SqliteType::Integer.type_name(), "INTEGER");
        assert_eq!(SqliteType::Float.type_name(), "FLOAT");
        assert_eq!(SqliteType::Text.type_name(), "TEXT");
        assert_eq!(SqliteType::Blob.type_name(), "BLOB");
        assert_eq!(SqliteType::Null.type_name(), "NULL");
    }

    #[test]
    fn code_returns_serial_type_value() {
        // 数字代码与 SQLite 文件格式规范一致
        assert_eq!(SqliteType::Integer.code(), 1);
        assert_eq!(SqliteType::Float.code(), 2);
        assert_eq!(SqliteType::Text.code(), 3);
        assert_eq!(SqliteType::Blob.code(), 4);
        assert_eq!(SqliteType::Null.code(), 5);
    }

    #[test]
    fn from_trait_delegates_to_from_value() {
        // From<&Value> 实现应与 from_value 行为一致
        let v = Value::Int64(42);
        let t1 = SqliteType::from_value(&v);
        let t2: SqliteType = SqliteType::from(&v);
        assert_eq!(t1, t2);
    }

    #[test]
    fn all_value_variants_have_defined_mapping() {
        // 穷举所有 Value 变体，确保映射无遗漏（编译期保证 match 穷尽）
        let samples: Vec<Value> = vec![
            Value::Null,
            Value::Int64(1),
            Value::Float64(1.0),
            Value::Text("a".to_string()),
            Value::Blob(vec![1]),
            Value::Bool(true),
            Value::Date(0),
            Value::Timestamp(0),
            Value::Decimal(1, 0),
            Value::Decimal(1, 2),
            Value::Array(vec![]),
            Value::Enum("x".to_string()),
            Value::Range(RangeValue {
                lower: None,
                upper: None,
                lower_inc: false,
                upper_inc: false,
                range_type: RangeType::Int4Range,
            }),
            Value::Json(serde_json::json!({})),
            Value::TsVector(TsVector::new()),
            Value::TsQuery(TsQuery::Empty),
        ];
        for v in &samples {
            // 不应 panic —— 所有变体都必须有定义好的映射
            let _ty = SqliteType::from_value(v);
        }
    }
}
