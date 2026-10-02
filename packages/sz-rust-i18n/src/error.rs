//! sz-rust-i18n 错误类型

use thiserror::Error;

/// sz-rust-i18n 错误（错误码 17030 起）
#[derive(Debug, Clone, Error)]
pub enum I18nError {
    #[error("内部错误: {0} (code: 17030)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17031)")]
    Config(String),
    #[error("参数无效: {0} (code: 17032)")]
    InvalidParam(String),
    #[error("缺失翻译键: {0} (code: 17033)")]
    MissingKey(String),
    #[error("语言不支持: {0} (code: 17034)")]
    UnsupportedLang(String),
}
