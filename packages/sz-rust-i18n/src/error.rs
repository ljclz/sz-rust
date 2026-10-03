//! sz-rust-i18n 错误类型

use thiserror::Error;

/// i18n 错误（错误码 17030 起）
#[derive(Debug, Clone, Error)]
pub enum I18nError {
    /// 资源文件格式错误（错误码 17030）
    #[error("资源文件格式错误: {0} (code: 17030)")]
    InvalidFormat(String),

    /// 资源加载失败（错误码 17031）
    #[error("资源加载失败: {0} (code: 17031)")]
    LoadFailed(String),

    /// 语言代码不合法（非 BCP 47）（错误码 17032）
    #[error("语言代码不合法: {0} (code: 17032)")]
    InvalidLanguageCode(String),

    /// 缺失翻译键（错误码 17033）
    #[error("缺失翻译键: {0} (code: 17033)")]
    MissingKey(String),

    /// 参数插值失败（错误码 17034）
    #[error("参数插值失败: {0} (code: 17034)")]
    InterpolationFailed(String),
}
