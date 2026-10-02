//! sz-rust-plugin-sdk 错误类型
use thiserror::Error;
#[derive(Debug, Clone, Error)]
pub enum SdkError {
    #[error("内部错误: {0} (code: 17040)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17041)")]
    Config(String),
    #[error("参数无效: {0} (code: 17042)")]
    InvalidParam(String),
    #[error("版本不兼容: {0} (code: 17043)")]
    VersionIncompatible(String),
}
