//! sz-rust-upload 错误类型
use thiserror::Error;
#[derive(Debug, Clone, Error)]
pub enum UploadError {
    #[error("内部错误: {0} (code: 17150)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17151)")]
    Config(String),
    #[error("参数无效: {0} (code: 17152)")]
    InvalidParam(String),
    #[error("分片不完整 (code: 17153)")]
    IncompleteChunks,
    #[error("校验失败: {0} (code: 17154)")]
    ChecksumFailed(String),
}
