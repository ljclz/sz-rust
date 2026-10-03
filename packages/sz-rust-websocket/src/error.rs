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
    /// 连接空闲超时（spec §5.21 异常 1）
    #[error("连接空闲超时 (code: 17145)")]
    IdleTimeout,
    /// 未认证连接拒绝（spec §5.21 规则 5）
    #[error("未认证连接拒绝 (code: 17146)")]
    Unauthenticated,
    /// 并发连接超限（spec §6.21 规则 3）
    #[error("并发连接超限 (code: 17147)")]
    ConnectionLimitExceeded,
    /// 广播过载（spec §5.21 异常 2）
    #[error("广播过载 (code: 17148)")]
    BroadcastOverload,
    /// 房间不存在
    #[error("房间不存在: {0} (code: 17149)")]
    RoomNotFound(String),
}
