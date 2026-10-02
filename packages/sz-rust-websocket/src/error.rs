//! sz-rust-websocket 错误类型
use thiserror::Error;
#[derive(Debug, Clone, Error)]
pub enum WebSocketError {
    #[error("内部错误: {0} (code: 17140)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17141)")]
    Config(String),
    #[error("参数无效: {0} (code: 17142)")]
    InvalidParam(String),
    #[error("连接不存在: {0} (code: 17143)")]
    ConnectionNotFound(String),
    #[error("心跳超时 (code: 17144)")]
    HeartbeatTimeout,
}
