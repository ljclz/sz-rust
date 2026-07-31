//! PostgreSQL 数据类型映射验证模块。
//!
//! 验证 PostgreSQL 数据类型名与 SzRSQL `ColumnType` 的映射关系。
//!
//! # 映射规则
//!
//! | PostgreSQL 类型 | SzRSQL ColumnType | 说明 |
//! |----------------|-------------------|------|
//! | bigint / int8 | Int64 | 64 位整数 |
//! | integer / int / int4 | Int64 | SzRSQL 统一为 Int64 |
//! | smallint / int2 | Int64 | SzRSQL 统一为 Int64 |
//! | serial / bigserial | Int64 | 自增序列统一为 Int64 |
//! | double precision / float8 | Float64 | 64 位浮点 |
//! | real / float4 | Float64 | SzRSQL 统一为 Float64 |
//! | text / varchar / char | Text | 变长字符串统一为 Text |
//! | bytea | Blob | 二进制数据 |
//! | boolean / bool | Bool | 布尔值 |
//! | date | Date | 日期（距纪元天数） |
//! | timestamp | Timestamp | 时间戳 |
//! | timestamptz | Timestamp | 带时区时间戳统一为 Timestamp |
//! | numeric / decimal | Decimal(p, s) | 精确小数 |
//! | json / jsonb | Json | JSON 文档 |
//! | tsvector | TsVector | 全文检索文档向量 |
//! | tsquery | TsQuery | 全文检索查询表达式 |
//! | uuid | Text | UUID 暂存为 Text |
//! | T[] | Array(T) | 数组类型 |

use crate::CompatStatus;
use serde::{Deserialize, Serialize};
use szrsql_types::value::ColumnType;

/// 单项数据类型映射检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataTypeMappingResult {
    /// PostgreSQL 类型名
    pub name: String,
    /// PostgreSQL 类型别名（如 int8 是 bigint 的别名）
    pub pg_aliases: Vec<String>,
    /// 期望的 SzRSQL ColumnType
    pub expected_szrsql_type: String,
    /// 兼容性状态
    pub status: CompatStatus,
    /// 详细说明
    pub detail: String,
}

/// 数据类型映射验证套件
pub struct DataTypeMapping;

impl DataTypeMapping {
    /// 运行全部数据类型映射检查
    pub fn run_all() -> Vec<DataTypeMappingResult> {
        let cases = Self::mapping_table();
        cases
            .into_iter()
            .map(|(pg_type, aliases, expected_type, supported, detail)| DataTypeMappingResult {
                name: pg_type.to_string(),
                pg_aliases: aliases.into_iter().map(String::from).collect(),
                expected_szrsql_type: expected_type.to_string(),
                status: if supported { CompatStatus::Pass } else { CompatStatus::NotImplemented },
                detail,
            })
            .collect()
    }

    /// PostgreSQL 类型 → SzRSQL ColumnType 映射表
    ///
    /// 返回值：(PG 主类型名, 别名列表, SzRSQL ColumnType 名称, 是否支持, 说明)
    fn mapping_table() -> Vec<(&'static str, Vec<&'static str>, &'static str, bool, String)> {
        vec![
            (
                "bigint",
                vec!["int8", "int64"],
                "Int64",
                true,
                "64 位整数，SzRSQL 原生支持".to_string(),
            ),
            (
                "integer",
                vec!["int", "int4"],
                "Int64",
                true,
                "32 位整数，SzRSQL 统一映射为 Int64".to_string(),
            ),
            (
                "smallint",
                vec!["int2"],
                "Int64",
                true,
                "16 位整数，SzRSQL 统一映射为 Int64".to_string(),
            ),
            (
                "serial",
                vec!["serial4"],
                "Int64",
                true,
                "自增整数，解析为 Int64（序列由 DDL 层处理）".to_string(),
            ),
            (
                "bigserial",
                vec!["serial8"],
                "Int64",
                true,
                "自增大整数，解析为 Int64（序列由 DDL 层处理）".to_string(),
            ),
            (
                "double precision",
                vec!["float8", "double"],
                "Float64",
                true,
                "64 位浮点数，SzRSQL 原生支持".to_string(),
            ),
            (
                "real",
                vec!["float4", "float"],
                "Float64",
                true,
                "32 位浮点数，SzRSQL 统一映射为 Float64".to_string(),
            ),
            (
                "text",
                vec!["varchar", "character varying", "char", "character"],
                "Text",
                true,
                "变长字符串，SzRSQL 统一为 Text（长度限制由 DDL 层处理）".to_string(),
            ),
            (
                "bytea",
                vec!["binary", "varbinary"],
                "Blob",
                true,
                "二进制数据，SzRSQL 原生支持".to_string(),
            ),
            (
                "boolean",
                vec!["bool"],
                "Bool",
                true,
                "布尔值，SzRSQL 原生支持".to_string(),
            ),
            (
                "date",
                vec![],
                "Date",
                true,
                "日期（距纪元天数），SzRSQL 原生支持".to_string(),
            ),
            (
                "timestamp",
                vec!["timestamp without time zone"],
                "Timestamp",
                true,
                "时间戳，SzRSQL 原生支持".to_string(),
            ),
            (
                "timestamptz",
                vec!["timestamp with time zone"],
                "Timestamp",
                true,
                "带时区时间戳，SzRSQL 统一为 Timestamp（时区由客户端处理）".to_string(),
            ),
            (
                "numeric",
                vec!["decimal"],
                "Decimal(p, s)",
                true,
                "精确小数，SzRSQL 支持 Decimal(precision, scale)".to_string(),
            ),
            (
                "json",
                vec![],
                "Json",
                true,
                "JSON 文档，SzRSQL 原生支持".to_string(),
            ),
            (
                "jsonb",
                vec![],
                "Json",
                true,
                "二进制 JSON，SzRSQL 统一为 Json（存储格式内部处理）".to_string(),
            ),
            (
                "tsvector",
                vec![],
                "TsVector",
                true,
                "全文检索文档向量，SzRSQL 原生支持".to_string(),
            ),
            (
                "tsquery",
                vec![],
                "TsQuery",
                true,
                "全文检索查询表达式，SzRSQL 原生支持".to_string(),
            ),
            (
                "uuid",
                vec![],
                "Text",
                true,
                "UUID，SzRSQL 暂存为 Text（128 位值以字符串表示）".to_string(),
            ),
            (
                "array",
                vec!["T[]"],
                "Array(T)",
                true,
                "数组类型，SzRSQL 支持 Array(Box<ColumnType>)".to_string(),
            ),
            (
                "enum",
                vec!["ENUM"],
                "Enum(Vec<String>)",
                true,
                "枚举类型，SzRSQL 支持 Enum(Vec<String>)".to_string(),
            ),
            (
                "range",
                vec!["int4range", "numrange", "tsrange"],
                "Range(RangeType)",
                true,
                "范围类型，SzRSQL 支持 Range(RangeType)".to_string(),
            ),
            (
                "money",
                vec![],
                "Decimal(19, 2)",
                true,
                "货币类型，映射为 Decimal(19, 2)".to_string(),
            ),
            (
                "interval",
                vec![],
                "Text",
                true,
                "时间间隔类型，SzRSQL 暂存为 Text（如 '1 day 2 hours'）".to_string(),
            ),
            (
                "bit",
                vec!["bit varying", "varbit"],
                "Text",
                true,
                "位串类型，SzRSQL 暂存为 Text（0/1 字符串表示）".to_string(),
            ),
            (
                "cidr",
                vec!["inet", "macaddr"],
                "Text",
                true,
                "网络地址类型，SzRSQL 暂存为 Text（字符串表示）".to_string(),
            ),
            (
                "point",
                vec!["line", "lseg", "box", "path", "polygon", "circle"],
                "Text",
                true,
                "几何类型，SzRSQL 暂存为 Text（无 PostGIS 支持）".to_string(),
            ),
            (
                "xml",
                vec![],
                "Text",
                true,
                "XML 类型，SzRSQL 暂存为 Text（XML 文档字符串）".to_string(),
            ),
        ]
    }

    /// 根据 PostgreSQL 类型名查询 SzRSQL 映射
    ///
    /// 支持别名查询（如 "int8" 等价于 "bigint"）
    pub fn lookup(pg_type: &str) -> Option<&'static str> {
        let table = Self::mapping_table();
        let normalized = pg_type.to_lowercase();

        // 精确匹配主类型名
        for (main_type, _, szrsql_type, supported, _) in &table {
            if *main_type == normalized && *supported {
                return Some(szrsql_type);
            }
        }

        // 匹配别名
        for (_, aliases, szrsql_type, supported, _) in &table {
            if *supported && aliases.iter().any(|a| *a == normalized) {
                return Some(*szrsql_type);
            }
        }

        None
    }

    /// 将 PostgreSQL 类型名转换为 SzRSQL ColumnType 实例
    ///
    /// 对于需要参数的类型（如 NUMERIC(p,s)），使用默认参数。
    /// 返回 `None` 表示该类型尚未实现。
    ///
    /// # 示例
    ///
    /// ```ignore
    /// use szrsql_pgcompat::DataTypeMapping;
    /// use szrsql_types::value::ColumnType;
    ///
    /// assert_eq!(DataTypeMapping::to_column_type("bigint"), Some(ColumnType::Int64));
    /// assert_eq!(DataTypeMapping::to_column_type("text"), Some(ColumnType::Text));
    /// assert_eq!(DataTypeMapping::to_column_type("interval"), Some(ColumnType::Text));
    /// ```
    pub fn to_column_type(pg_type: &str) -> Option<ColumnType> {
        let normalized = pg_type.to_lowercase();
        match normalized.as_str() {
            "bigint" | "int8" | "int64" | "integer" | "int" | "int4" | "smallint" | "int2"
            | "serial" | "serial4" | "bigserial" | "serial8" => Some(ColumnType::Int64),
            "double precision" | "float8" | "double" | "real" | "float4" | "float" => {
                Some(ColumnType::Float64)
            }
            "text" | "varchar" | "character varying" | "char" | "character" | "uuid" => {
                Some(ColumnType::Text)
            }
            "bytea" | "binary" | "varbinary" => Some(ColumnType::Blob),
            "boolean" | "bool" => Some(ColumnType::Bool),
            "date" => Some(ColumnType::Date),
            "timestamp" | "timestamp without time zone" | "timestamptz"
            | "timestamp with time zone" => Some(ColumnType::Timestamp),
            "numeric" | "decimal" => Some(ColumnType::Decimal {
                precision: 38,
                scale: 10,
            }),
            "money" => Some(ColumnType::Decimal {
                precision: 19,
                scale: 2,
            }),
            "json" | "jsonb" => Some(ColumnType::Json),
            "tsvector" => Some(ColumnType::TsVector),
            "tsquery" => Some(ColumnType::TsQuery),
            // Phase F-10: 兼容性类型 — 暂存为 Text
            "interval" | "bit" | "bit varying" | "varbit" | "cidr" | "inet" | "macaddr"
            | "macaddr8" | "point" | "line" | "lseg" | "box" | "path" | "polygon" | "circle"
            | "xml" => Some(ColumnType::Text),
            _ => None,
        }
    }

    /// 将 PostgreSQL 数组类型名（如 "TEXT[]"）转换为 SzRSQL Array(ColumnType)
    ///
    /// 返回 `None` 表示元素类型未实现或格式非法。
    pub fn to_array_column_type(pg_type: &str) -> Option<ColumnType> {
        let trimmed = pg_type.trim();
        if !trimmed.ends_with("[]") {
            return None;
        }
        let element = trimmed.trim_end_matches("[]");
        let inner = Self::to_column_type(element)?;
        Some(ColumnType::Array(Box::new(inner)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_returns_nonempty() {
        let results = DataTypeMapping::run_all();
        assert!(!results.is_empty(), "应返回至少一项检查结果");
    }

    #[test]
    fn core_types_are_supported() {
        let results = DataTypeMapping::run_all();
        for name in &["bigint", "integer", "text", "boolean", "timestamp", "numeric"] {
            let r = results.iter().find(|r| r.name == *name)
                .unwrap_or_else(|| panic!("应包含类型 {name}"));
            assert_eq!(r.status, CompatStatus::Pass, "核心类型 {name} 应支持");
        }
    }

    #[test]
    fn lookup_by_main_type() {
        assert_eq!(DataTypeMapping::lookup("bigint"), Some("Int64"));
        assert_eq!(DataTypeMapping::lookup("text"), Some("Text"));
        assert_eq!(DataTypeMapping::lookup("boolean"), Some("Bool"));
    }

    #[test]
    fn lookup_by_alias() {
        assert_eq!(DataTypeMapping::lookup("int8"), Some("Int64"));
        assert_eq!(DataTypeMapping::lookup("int4"), Some("Int64"));
        assert_eq!(DataTypeMapping::lookup("bool"), Some("Bool"));
        assert_eq!(DataTypeMapping::lookup("float8"), Some("Float64"));
    }

    #[test]
    fn lookup_case_insensitive() {
        assert_eq!(DataTypeMapping::lookup("BIGINT"), Some("Int64"));
        assert_eq!(DataTypeMapping::lookup("Text"), Some("Text"));
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert_eq!(DataTypeMapping::lookup("nonexistent_type"), None);
    }

    #[test]
    fn unsupported_types_marked_not_implemented() {
        let results = DataTypeMapping::run_all();
        // Phase F-10: interval 现已支持（存为 Text）
        let interval = results.iter().find(|r| r.name == "interval")
            .expect("应包含 interval 类型");
        assert_eq!(interval.status, CompatStatus::Pass);
    }

    #[test]
    fn array_type_mapping_present() {
        let results = DataTypeMapping::run_all();
        let array = results.iter().find(|r| r.name == "array")
            .expect("应包含 array 类型");
        assert_eq!(array.expected_szrsql_type, "Array(T)");
        assert_eq!(array.status, CompatStatus::Pass);
    }

    #[test]
    fn to_column_type_basic() {
        assert_eq!(DataTypeMapping::to_column_type("bigint"), Some(ColumnType::Int64));
        assert_eq!(DataTypeMapping::to_column_type("int8"), Some(ColumnType::Int64));
        assert_eq!(DataTypeMapping::to_column_type("text"), Some(ColumnType::Text));
        assert_eq!(DataTypeMapping::to_column_type("boolean"), Some(ColumnType::Bool));
        assert_eq!(DataTypeMapping::to_column_type("bytea"), Some(ColumnType::Blob));
        assert_eq!(DataTypeMapping::to_column_type("date"), Some(ColumnType::Date));
        assert_eq!(DataTypeMapping::to_column_type("timestamp"), Some(ColumnType::Timestamp));
        assert_eq!(DataTypeMapping::to_column_type("json"), Some(ColumnType::Json));
        assert_eq!(DataTypeMapping::to_column_type("tsvector"), Some(ColumnType::TsVector));
    }

    #[test]
    fn to_column_type_decimal_default() {
        let ct = DataTypeMapping::to_column_type("numeric").expect("numeric 应可转换");
        match ct {
            ColumnType::Decimal { precision, scale } => {
                assert_eq!(precision, 38);
                assert_eq!(scale, 10);
            }
            other => panic!("numeric 应映射为 Decimal，实际: {other:?}"),
        }
    }

    #[test]
    fn to_column_type_unsupported_returns_none() {
        // Phase F-10: interval/point/xml 现已支持（存为 Text）
        assert_eq!(DataTypeMapping::to_column_type("interval"), Some(ColumnType::Text));
        assert_eq!(DataTypeMapping::to_column_type("point"), Some(ColumnType::Text));
        assert_eq!(DataTypeMapping::to_column_type("xml"), Some(ColumnType::Text));
        // 真正未实现的类型仍返回 None
        assert_eq!(DataTypeMapping::to_column_type("nonexistent"), None);
    }

    #[test]
    fn to_array_column_type_basic() {
        let ct = DataTypeMapping::to_array_column_type("TEXT[]").expect("TEXT[] 应可转换");
        match ct {
            ColumnType::Array(inner) => assert_eq!(*inner, ColumnType::Text),
            other => panic!("应映射为 Array，实际: {other:?}"),
        }
    }

    #[test]
    fn to_array_column_type_invalid_returns_none() {
        assert_eq!(DataTypeMapping::to_array_column_type("TEXT"), None);
        // Phase F-10: INTERVAL 现已支持，INTERVAL[] 应可转换
        let interval_arr = DataTypeMapping::to_array_column_type("INTERVAL[]");
        assert!(interval_arr.is_some(), "INTERVAL[] 应可转换");
    }
}
