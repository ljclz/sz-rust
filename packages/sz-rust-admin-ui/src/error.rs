//! sz-rust-admin-ui 错误类型
use thiserror::Error;
#[derive(Debug, Clone, Error)]
pub enum AdminUiError {
    #[error("内部错误: {0} (code: 17170)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17171)")]
    Config(String),
    #[error("参数无效: {0} (code: 17172)")]
    InvalidParam(String),
    #[error("权限不足: {0} (code: 17173)")]
    PermissionDenied(String),
}
