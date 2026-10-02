//! sz-rust-alert-engine 错误类型
use thiserror::Error;
#[derive(Debug, Clone, Error)]
pub enum AlertEngineError {
    #[error("内部错误: {0} (code: 17080)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17081)")]
    Config(String),
    #[error("参数无效: {0} (code: 17082)")]
    InvalidParam(String),
    #[error("规则评估失败: {0} (code: 17083)")]
    RuleEvalFailed(String),
    #[error("通知发送失败: {0} (code: 17084)")]
    NotifyFailed(String),
}
