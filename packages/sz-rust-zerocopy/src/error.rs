//! sz-rust-zerocopy 错误类型
use thiserror::Error;
#[derive(Debug, Clone, Error)]
pub enum ZeroCopyError {
    #[error("内部错误: {0} (code: 17060)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17061)")]
    Config(String),
    #[error("参数无效: {0} (code: 17062)")]
    InvalidParam(String),
    #[error("序列化失败: {0} (code: 17063)")]
    SerializeFailed(String),
    #[error("反序列化失败: {0} (code: 17064)")]
    DeserializeFailed(String),
}
