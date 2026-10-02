//! sz-rust-key-rotation 错误类型
use thiserror::Error;
#[derive(Debug, Clone, Error)]
pub enum KeyRotationError {
    #[error("内部错误: {0} (code: 17130)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17131)")]
    Config(String),
    #[error("参数无效: {0} (code: 17132)")]
    InvalidParam(String),
    #[error("密钥不存在: {0} (code: 17133)")]
    KeyNotFound(String),
    #[error("轮换失败: {0} (code: 17134)")]
    RotationFailed(String),
}
