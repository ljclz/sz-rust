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
