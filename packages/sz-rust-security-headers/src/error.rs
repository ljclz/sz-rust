//! sz-rust-security-headers 错误类型（复用 axum 错误）

use thiserror::Error;

/// 安全头错误（错误码 17120 起）
#[derive(Debug, Clone, Error)]
pub enum SecurityHeaderError {
    /// 头设置失败（错误码 17120）
    #[error("头设置失败: {0} (code: 17120)")]
    HeaderSetFailed(String),

    /// 配置无效（错误码 17121）
    #[error("配置无效: {0} (code: 17121)")]
    InvalidConfig(String),
}
