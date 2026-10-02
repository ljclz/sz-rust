//! sz-rust-codegen-ui 错误类型
use thiserror::Error;
#[derive(Debug, Clone, Error)]
pub enum CodegenUiError {
    #[error("内部错误: {0} (code: 17190)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17191)")]
    Config(String),
    #[error("参数无效: {0} (code: 17192)")]
    InvalidParam(String),
    #[error("模板不存在: {0} (code: 17193)")]
    TemplateNotFound(String),
}
