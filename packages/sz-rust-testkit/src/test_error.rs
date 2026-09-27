//! 测试错误类型（v1.5.0 P1-4）

use std::time::Duration;
use sz_rust_orm_facade::{DbError, PoolError};

/// 测试错误
#[derive(Debug, thiserror::Error)]
pub enum TestError {
    /// 测试超时（spec 5.4.3 异常1）
    #[error("test timeout after {timeout:?}: {reason}")]
    Timeout {
        /// 超时时长
        timeout: Duration,
        /// 疑似原因（锁等待/连接耗尽等）
        reason: String,
    },

    /// 资源泄漏（spec 5.4.2）
    #[error("resource leak detected: {0}")]
    ResourceLeak(String),

    /// 数据库错误
    #[error(transparent)]
    Db(#[from] DbError),

    /// 连接池错误
    #[error(transparent)]
    Pool(#[from] PoolError),
}
