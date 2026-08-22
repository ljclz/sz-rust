//! 输入合法性校验器
//!
//! 对应 design.md 第 2.2.2.6 节，校验用户输入的插件名、表名、字段定义等。
//! 包含路径遍历防护与代码注入防护。

use crate::error::CliError;
use crate::field_parser::Field;

/// 输入校验器
pub struct InputValidator;

/// 危险字符（代码注入防护）
const DANGEROUS_CHARS: &[char] = &[';', '|', '&', '$', '`', '!', '\n', '\r', '<', '>'];

impl InputValidator {
    /// 校验插件名称（Rust crate 命名规范）
    ///
    /// 规则：小写字母、数字、下划线、连字符，不以数字开头
    pub fn validate_plugin_name(name: &str) -> Result<(), CliError> {
        if name.is_empty() {
            return Err(CliError::InvalidPluginName(
                "plugin name is empty".to_string(),
            ));
        }

        if name.len() > 64 {
            return Err(CliError::InvalidPluginName(format!(
                "plugin name '{name}' exceeds 64 characters"
            )));
        }

        if name
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            return Err(CliError::InvalidPluginName(format!(
                "plugin name '{name}' starts with a digit"
            )));
        }

        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Err(CliError::InvalidPluginName(format!(
                "plugin name '{name}' contains invalid characters (only lowercase letters, digits, underscores, and hyphens are allowed)"
            )));
        }

        Ok(())
    }

    /// 校验表名（SQL 标识符规范 + 路径遍历防护）
    pub fn validate_table_name(name: &str) -> Result<(), CliError> {
        if name.is_empty() {
            return Err(CliError::FieldParseError("table name is empty".to_string()));
        }

        if name.contains("..") {
            return Err(CliError::FieldParseError(format!(
                "table name '{name}' contains path traversal sequence '..'"
            )));
        }

        if name.starts_with('/') || name.starts_with('\\') {
            return Err(CliError::FieldParseError(format!(
                "table name '{name}' is an absolute path"
            )));
        }

        if name
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            return Err(CliError::FieldParseError(format!(
                "table name '{name}' starts with a digit"
            )));
        }

        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(CliError::FieldParseError(format!(
                "table name '{name}' contains invalid characters (only letters, digits, and underscores are allowed)"
            )));
        }

        Ok(())
    }

    /// 校验字段定义格式 + 注入防护
    pub fn validate_fields(fields: &str) -> Result<(), CliError> {
        if fields.is_empty() {
            return Err(CliError::FieldParseError(
                "fields definition is empty".to_string(),
            ));
        }

        for ch in DANGEROUS_CHARS {
            if fields.contains(*ch) {
                return Err(CliError::FieldParseError(format!(
                    "fields definition contains dangerous character '{ch}'"
                )));
            }
        }

        crate::field_parser::FieldParser::parse(fields)?;
        Ok(())
    }

    /// 校验外键字段存在于从表字段定义中
    pub fn validate_foreign_key(fk: &str, slave_fields: &[Field]) -> Result<(), CliError> {
        if fk.is_empty() {
            return Err(CliError::ForeignKeyNotFound(
                "foreign key is empty".to_string(),
            ));
        }

        if !slave_fields.iter().any(|f| f.name == fk) {
            return Err(CliError::ForeignKeyNotFound(fk.to_string()));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_plugin_name_valid() {
        assert!(InputValidator::validate_plugin_name("my-plugin").is_ok());
        assert!(InputValidator::validate_plugin_name("my_plugin").is_ok());
        assert!(InputValidator::validate_plugin_name("myplugin123").is_ok());
        assert!(InputValidator::validate_plugin_name("a").is_ok());
    }

    #[test]
    fn test_validate_plugin_name_empty() {
        assert!(InputValidator::validate_plugin_name("").is_err());
    }

    #[test]
    fn test_validate_plugin_name_with_space() {
        let result = InputValidator::validate_plugin_name("my plugin");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CliError::InvalidPluginName(_)
        ));
    }

    #[test]
    fn test_validate_plugin_name_starts_with_digit() {
        let result = InputValidator::validate_plugin_name("1plugin");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_plugin_name_uppercase() {
        let result = InputValidator::validate_plugin_name("MyPlugin");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_table_name_valid() {
        assert!(InputValidator::validate_table_name("users").is_ok());
        assert!(InputValidator::validate_table_name("user_orders").is_ok());
        assert!(InputValidator::validate_table_name("table123").is_ok());
    }

    #[test]
    fn test_validate_table_name_path_traversal() {
        let result = InputValidator::validate_table_name("../etc/evil");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("path traversal"));
    }

    #[test]
    fn test_validate_table_name_absolute_path() {
        let result = InputValidator::validate_table_name("/etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_table_name_starts_with_digit() {
        let result = InputValidator::validate_table_name("123table");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_fields_valid() {
        let result = InputValidator::validate_fields("id:i32,name:String,age:i32");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_fields_empty() {
        let result = InputValidator::validate_fields("");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_fields_injection_semicolon() {
        let result = InputValidator::validate_fields("name:String;rm -rf /");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_fields_injection_pipe() {
        let result = InputValidator::validate_fields("name:String|cat /etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_fields_injection_ampersand() {
        let result = InputValidator::validate_fields("name:String&whoami");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_fields_injection_backtick() {
        let result = InputValidator::validate_fields("name:String`whoami`");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_foreign_key_exists() {
        let fields = vec![
            Field {
                name: "id".to_string(),
                rust_type: "i32".to_string(),
                sql_type: "INT".to_string(),
                is_nullable: false,
                is_primary_key: true,
                is_indexed: false,
            },
            Field {
                name: "user_id".to_string(),
                rust_type: "i32".to_string(),
                sql_type: "INT".to_string(),
                is_nullable: false,
                is_primary_key: false,
                is_indexed: false,
            },
        ];
        assert!(InputValidator::validate_foreign_key("user_id", &fields).is_ok());
    }

    #[test]
    fn test_validate_foreign_key_not_exists() {
        let fields = vec![Field {
            name: "id".to_string(),
            rust_type: "i32".to_string(),
            sql_type: "INT".to_string(),
            is_nullable: false,
            is_primary_key: true,
            is_indexed: false,
        }];
        let result = InputValidator::validate_foreign_key("user_id", &fields);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CliError::ForeignKeyNotFound(_)
        ));
    }

    #[test]
    fn test_validate_foreign_key_empty() {
        let fields = vec![];
        let result = InputValidator::validate_foreign_key("", &fields);
        assert!(result.is_err());
    }
}
