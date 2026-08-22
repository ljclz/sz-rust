//! 字段定义解析器
//!
//! 对应 design.md 第 1.1.3.7 节，解析 `"name:Type,age:i32"` 格式的字段定义，
//! 并提供 Rust 类型 → SQL 类型映射。

use crate::error::CliError;

/// 字段定义
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// 字段名（Rust 标识符）
    pub name: String,
    /// Rust 类型名（如 `String`、`i32`、`Option<String>`）
    pub rust_type: String,
    /// SQL 类型（如 `VARCHAR(255)`、`INT`）
    pub sql_type: String,
    /// 是否可空
    pub is_nullable: bool,
    /// 是否主键
    pub is_primary_key: bool,
    /// 是否索引
    pub is_indexed: bool,
}

/// 字段定义解析器
pub struct FieldParser;

/// Rust 类型 → SQL 类型映射
const TYPE_MAP: &[(&str, &str)] = &[
    ("String", "VARCHAR(255)"),
    ("i32", "INT"),
    ("i64", "BIGINT"),
    ("f64", "DOUBLE"),
    ("bool", "BOOLEAN"),
    ("DateTime", "DATETIME"),
    ("Json", "JSON"),
];

/// 已知的 Rust 类型列表
const KNOWN_TYPES: &[&str] = &["String", "i32", "i64", "f64", "bool", "DateTime", "Json"];

/// 字段修饰符
const MODIFIER_PK: &str = "pk";
const MODIFIER_INDEX: &str = "index";

impl FieldParser {
    /// 解析字段定义字符串
    ///
    /// 格式：`"name:Type,name2:Type2,..."`
    ///
    /// 支持修饰符：
    /// - `name:Type:pk` — 标记为主键
    /// - `name:Type:index` — 标记为索引
    /// - `name:Type?` — 标记为可空（语法糖，等价于 `Option<Type>`）
    ///
    /// # 错误
    ///
    /// - `CliError::FieldParseError`：格式错误、未知类型、非法标识符、注入字符
    pub fn parse(input: &str) -> Result<Vec<Field>, CliError> {
        let input = input.trim();

        if input.is_empty() {
            return Err(CliError::FieldParseError(
                "field definition is empty".to_string(),
            ));
        }

        if input.ends_with(',') {
            return Err(CliError::FieldParseError(format!(
                "trailing comma at end of field definition: '{input}'"
            )));
        }

        let mut fields = Vec::new();
        for (idx, part) in input.split(',').enumerate() {
            let part = part.trim();
            if part.is_empty() {
                return Err(CliError::FieldParseError(format!(
                    "empty field at position {idx}"
                )));
            }
            let field = Self::parse_single(part, idx)?;
            fields.push(field);
        }

        Ok(fields)
    }

    /// 解析单个字段定义
    fn parse_single(part: &str, idx: usize) -> Result<Field, CliError> {
        let tokens: Vec<&str> = part.split(':').collect();
        if tokens.len() < 2 {
            return Err(CliError::FieldParseError(format!(
                "missing ':' in field definition '{part}' at position {idx}"
            )));
        }

        let name = tokens[0].trim();
        let mut rust_type = tokens[1].trim().to_string();
        let mut is_nullable = false;
        let mut is_primary_key = false;
        let mut is_indexed = false;

        if rust_type.ends_with('?') {
            is_nullable = true;
            rust_type = rust_type[..rust_type.len() - 1].to_string();
        }

        for modifier in tokens.iter().skip(2) {
            let modifier = modifier.trim();
            match modifier {
                MODIFIER_PK => is_primary_key = true,
                MODIFIER_INDEX => is_indexed = true,
                other => {
                    return Err(CliError::FieldParseError(format!(
                        "unknown modifier '{other}' in field definition '{part}' at position {idx}"
                    )));
                }
            }
        }

        Self::validate_name(name, idx)?;
        Self::validate_type(&rust_type, idx)?;

        let sql_type = Self::rust_type_to_sql(&rust_type)?;

        Ok(Field {
            name: name.to_string(),
            rust_type,
            sql_type,
            is_nullable,
            is_primary_key,
            is_indexed,
        })
    }

    /// 校验字段名合法性
    fn validate_name(name: &str, idx: usize) -> Result<(), CliError> {
        if name.is_empty() {
            return Err(CliError::FieldParseError(format!(
                "empty field name at position {idx}"
            )));
        }

        if name
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            return Err(CliError::FieldParseError(format!(
                "field name '{name}' at position {idx} starts with a digit"
            )));
        }

        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(CliError::FieldParseError(format!(
                "field name '{name}' at position {idx} contains invalid characters (only letters, digits, and underscores are allowed)"
            )));
        }

        Ok(())
    }

    /// 校验类型合法性
    fn validate_type(rust_type: &str, idx: usize) -> Result<(), CliError> {
        if !KNOWN_TYPES.contains(&rust_type) {
            return Err(CliError::FieldParseError(format!(
                "unknown type '{rust_type}' at position {idx}. Known types: {}",
                KNOWN_TYPES.join(", ")
            )));
        }
        Ok(())
    }

    /// Rust 类型 → SQL 类型映射
    pub fn rust_type_to_sql(rust_type: &str) -> Result<String, CliError> {
        for (rust, sql) in TYPE_MAP {
            if *rust == rust_type {
                return Ok(sql.to_string());
            }
        }
        Err(CliError::FieldParseError(format!(
            "unknown Rust type '{rust_type}'. Known types: {}",
            KNOWN_TYPES.join(", ")
        )))
    }

    /// 返回全部已知 Rust 类型
    pub fn known_types() -> &'static [&'static str] {
        KNOWN_TYPES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic() {
        let fields = FieldParser::parse("id:i32,name:String,age:i32").unwrap();
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[0].name, "id");
        assert_eq!(fields[0].rust_type, "i32");
        assert_eq!(fields[0].sql_type, "INT");
        assert_eq!(fields[1].name, "name");
        assert_eq!(fields[1].rust_type, "String");
        assert_eq!(fields[1].sql_type, "VARCHAR(255)");
    }

    #[test]
    fn test_parse_with_datetime() {
        let fields = FieldParser::parse("id:i32,name:String,created_at:DateTime").unwrap();
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[2].name, "created_at");
        assert_eq!(fields[2].rust_type, "DateTime");
        assert_eq!(fields[2].sql_type, "DATETIME");
    }

    #[test]
    fn test_parse_all_types() {
        let fields =
            FieldParser::parse("a:String,b:i32,c:i64,d:f64,e:bool,f:DateTime,g:Json").unwrap();
        assert_eq!(fields.len(), 7);
        assert_eq!(fields[0].sql_type, "VARCHAR(255)");
        assert_eq!(fields[1].sql_type, "INT");
        assert_eq!(fields[2].sql_type, "BIGINT");
        assert_eq!(fields[3].sql_type, "DOUBLE");
        assert_eq!(fields[4].sql_type, "BOOLEAN");
        assert_eq!(fields[5].sql_type, "DATETIME");
        assert_eq!(fields[6].sql_type, "JSON");
    }

    #[test]
    fn test_parse_nullable() {
        let fields = FieldParser::parse("id:i32,name:String?").unwrap();
        assert!(!fields[0].is_nullable);
        assert!(fields[1].is_nullable);
        assert_eq!(fields[1].rust_type, "String");
    }

    #[test]
    fn test_parse_primary_key() {
        let fields = FieldParser::parse("id:i32:pk,name:String").unwrap();
        assert!(fields[0].is_primary_key);
        assert!(!fields[1].is_primary_key);
    }

    #[test]
    fn test_parse_indexed() {
        let fields = FieldParser::parse("id:i32:pk,email:String:index").unwrap();
        assert!(fields[0].is_primary_key);
        assert!(fields[1].is_indexed);
    }

    #[test]
    fn test_parse_empty_string() {
        let result = FieldParser::parse("");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CliError::FieldParseError(_)));
    }

    #[test]
    fn test_parse_trailing_comma() {
        let result = FieldParser::parse("id:i32,name:String,");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CliError::FieldParseError(_)));
        assert!(err.to_string().contains("trailing comma"));
    }

    #[test]
    fn test_parse_unknown_type() {
        let result = FieldParser::parse("id:UnknownType");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CliError::FieldParseError(_)));
        assert!(err.to_string().contains("unknown type"));
    }

    #[test]
    fn test_parse_missing_colon() {
        let result = FieldParser::parse("id_i32");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("missing ':'"));
    }

    #[test]
    fn test_parse_name_starts_with_digit() {
        let result = FieldParser::parse("1id:i32");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("starts with a digit"));
    }

    #[test]
    fn test_parse_name_with_special_chars() {
        let result = FieldParser::parse("na;me:i32");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }

    #[test]
    fn test_parse_injection_semicolon() {
        let result = FieldParser::parse("name:String;rm -rf /");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_injection_pipe() {
        let result = FieldParser::parse("name:String|cat /etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_unknown_modifier() {
        let result = FieldParser::parse("id:i32:foobar");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("unknown modifier"));
    }

    #[test]
    fn test_parse_whitespace_trimming() {
        let fields = FieldParser::parse(" id : i32 , name : String ").unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "id");
        assert_eq!(fields[1].name, "name");
    }

    #[test]
    fn test_rust_type_to_sql() {
        assert_eq!(
            FieldParser::rust_type_to_sql("String").unwrap(),
            "VARCHAR(255)"
        );
        assert_eq!(FieldParser::rust_type_to_sql("i32").unwrap(), "INT");
        assert_eq!(FieldParser::rust_type_to_sql("i64").unwrap(), "BIGINT");
        assert_eq!(FieldParser::rust_type_to_sql("f64").unwrap(), "DOUBLE");
        assert_eq!(FieldParser::rust_type_to_sql("bool").unwrap(), "BOOLEAN");
        assert_eq!(
            FieldParser::rust_type_to_sql("DateTime").unwrap(),
            "DATETIME"
        );
        assert_eq!(FieldParser::rust_type_to_sql("Json").unwrap(), "JSON");
    }

    #[test]
    fn test_rust_type_to_sql_unknown() {
        let result = FieldParser::rust_type_to_sql("Unknown");
        assert!(result.is_err());
    }

    #[test]
    fn test_known_types() {
        let types = FieldParser::known_types();
        assert_eq!(types.len(), 7);
        assert!(types.contains(&"String"));
        assert!(types.contains(&"Json"));
    }
}
