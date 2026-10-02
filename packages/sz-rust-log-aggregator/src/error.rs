//! sz-rust-log-aggregator 错误类型
use thiserror::Error;
#[derive(Debug, Clone, Error)]
pub enum LogAggregatorError {
    #[error("内部错误: {0} (code: 17090)")]
    Internal(String),
    #[error("配置错误: {0} (code: 17091)")]
    Config(String),
    #[error("参数无效: {0} (code: 17092)")]
    InvalidParam(String),
    #[error("日志查询失败: {0} (code: 17093)")]
    QueryFailed(String),
}
