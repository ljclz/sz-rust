// SPDX-License-Identifier: Apache-2.0
use crate::error::AdminError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 配置值类型枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigType {
    String,
    Number,
    Boolean,
    Json,
}

impl ConfigType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Json => "json",
        }
    }
}

/// 系统配置模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigModel {
    pub id: i64,
    pub key: String,
    pub value: serde_json::Value,
    pub value_type: ConfigType,
    pub group: Option<String>,
    pub description: Option<String>,
    pub tenant_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ConfigModel {
    pub fn validate_value_type(
        value: &serde_json::Value,
        value_type: ConfigType,
    ) -> Result<(), AdminError> {
        let valid = match value_type {
            ConfigType::String => value.is_string(),
            ConfigType::Number => value.is_number(),
            ConfigType::Boolean => value.is_boolean(),
            ConfigType::Json => value.is_object() || value.is_array(),
        };
        if valid {
            Ok(())
        } else {
            Err(AdminError::ConfigTypeMismatch)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_value_type_boolean() {
        assert!(
            ConfigModel::validate_value_type(&serde_json::json!(true), ConfigType::Boolean).is_ok()
        );
        assert!(
            ConfigModel::validate_value_type(&serde_json::json!("true"), ConfigType::Boolean)
                .is_err()
        );
    }

    #[test]
    fn test_validate_value_type_number() {
        assert!(
            ConfigModel::validate_value_type(&serde_json::json!(42), ConfigType::Number).is_ok()
        );
        assert!(
            ConfigModel::validate_value_type(&serde_json::json!("42"), ConfigType::Number).is_err()
        );
    }

    #[test]
    fn test_validate_value_type_json() {
        assert!(
            ConfigModel::validate_value_type(&serde_json::json!({"k": "v"}), ConfigType::Json)
                .is_ok()
        );
        assert!(
            ConfigModel::validate_value_type(&serde_json::json!([1, 2]), ConfigType::Json).is_ok()
        );
        assert!(
            ConfigModel::validate_value_type(&serde_json::json!("str"), ConfigType::Json).is_err()
        );
    }

    #[test]
    fn test_validate_value_type_string() {
        assert!(
            ConfigModel::validate_value_type(&serde_json::json!("hello"), ConfigType::String)
                .is_ok()
        );
        assert!(
            ConfigModel::validate_value_type(&serde_json::json!(42), ConfigType::String).is_err()
        );
    }
}
