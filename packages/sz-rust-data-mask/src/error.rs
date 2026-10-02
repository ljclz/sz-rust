//! sz-rust-data-mask 错误类型
use thiserror::Error;
#[derive(Debug, Clone, Error)]
pub enum MaskError {
    #[error("内部错误: {0} (code: 17110)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17111)")]
    Config(String),
    #[error("参数无效: {0} (code: 17112)")]
    InvalidParam(String),
    #[error("脱敏规则未找到: {0} (code: 17113)")]
    RuleNotFound(String),
}
