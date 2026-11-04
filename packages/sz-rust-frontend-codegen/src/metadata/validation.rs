// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 验证规则

use serde::{Deserialize, Serialize};

/// 验证规则类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationRuleType {
    /// 必填
    Required,
    /// 邮箱
    Email,
    /// URL
    Url,
    /// 最大长度
    MaxLength,
    /// 最小长度
    MinLength,
    /// 正则
    Regex,
    /// 数值
    Numeric,
    /// 整数
    Integer,
    /// 日期
    Date,
}

/// 验证规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationRule {
    /// 规则类型
    pub rule_type: ValidationRuleType,
    /// 参数
    pub param: Option<String>,
    /// 错误消息
    pub message: Option<String>,
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validation_rule_type_serde_required() {
        let json = "\"required\"";
        let rule: ValidationRuleType = serde_json::from_str(json).unwrap();
        assert_eq!(rule, ValidationRuleType::Required);
        let serialized = serde_json::to_string(&rule).unwrap();
        assert_eq!(serialized, "\"required\"");
    }

    #[test]
    fn test_validation_rule_type_serde_email() {
        let json = "\"email\"";
        let rule: ValidationRuleType = serde_json::from_str(json).unwrap();
        assert_eq!(rule, ValidationRuleType::Email);
    }

    #[test]
    fn test_validation_rule_type_serde_url() {
        let json = "\"url\"";
        let rule: ValidationRuleType = serde_json::from_str(json).unwrap();
        assert_eq!(rule, ValidationRuleType::Url);
    }

    #[test]
    fn test_validation_rule_type_serde_max_length() {
        let json = "\"max_length\"";
        let rule: ValidationRuleType = serde_json::from_str(json).unwrap();
        assert_eq!(rule, ValidationRuleType::MaxLength);
    }

    #[test]
    fn test_validation_rule_type_serde_min_length() {
        let json = "\"min_length\"";
        let rule: ValidationRuleType = serde_json::from_str(json).unwrap();
        assert_eq!(rule, ValidationRuleType::MinLength);
    }

    #[test]
    fn test_validation_rule_type_serde_regex() {
        let json = "\"regex\"";
        let rule: ValidationRuleType = serde_json::from_str(json).unwrap();
        assert_eq!(rule, ValidationRuleType::Regex);
    }

    #[test]
    fn test_validation_rule_type_serde_numeric() {
        let json = "\"numeric\"";
        let rule: ValidationRuleType = serde_json::from_str(json).unwrap();
        assert_eq!(rule, ValidationRuleType::Numeric);
    }

    #[test]
    fn test_validation_rule_type_serde_integer() {
        let json = "\"integer\"";
        let rule: ValidationRuleType = serde_json::from_str(json).unwrap();
        assert_eq!(rule, ValidationRuleType::Integer);
    }

    #[test]
    fn test_validation_rule_type_serde_date() {
        let json = "\"date\"";
        let rule: ValidationRuleType = serde_json::from_str(json).unwrap();
        assert_eq!(rule, ValidationRuleType::Date);
    }

    #[test]
    fn test_validation_rule_type_invalid_serde() {
        let json = "\"nonexistent\"";
        let result: Result<ValidationRuleType, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_rule_full_serde() {
        let rule = ValidationRule {
            rule_type: ValidationRuleType::MaxLength,
            param: Some("255".to_string()),
            message: Some("不能超过255个字符".to_string()),
        };
        let json = serde_json::to_string(&rule).unwrap();
        let deserialized: ValidationRule = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.rule_type, ValidationRuleType::MaxLength);
        assert_eq!(deserialized.param, Some("255".to_string()));
        assert_eq!(deserialized.message, Some("不能超过255个字符".to_string()));
    }

    #[test]
    fn test_validation_rule_minimal() {
        let rule = ValidationRule {
            rule_type: ValidationRuleType::Required,
            param: None,
            message: None,
        };
        let json = serde_json::to_string(&rule).unwrap();
        let deserialized: ValidationRule = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.rule_type, ValidationRuleType::Required);
        assert!(deserialized.param.is_none());
        assert!(deserialized.message.is_none());
    }

    #[test]
    fn test_validation_rule_type_all_variants_roundtrip() {
        let variants = [
            ValidationRuleType::Required,
            ValidationRuleType::Email,
            ValidationRuleType::Url,
            ValidationRuleType::MaxLength,
            ValidationRuleType::MinLength,
            ValidationRuleType::Regex,
            ValidationRuleType::Numeric,
            ValidationRuleType::Integer,
            ValidationRuleType::Date,
        ];
        for variant in variants {
            let json = serde_json::to_string(&variant).unwrap();
            let back: ValidationRuleType = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
        }
    }
}
