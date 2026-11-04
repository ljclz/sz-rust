// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 分布式事务错误类型

use thiserror::Error;

/// 分布式事务错误
#[derive(Debug, Error)]
pub enum DtxError {
    /// 正向操作失败
    #[error("Forward action failed at step {step_id}: {source}")]
    ForwardFailed {
        step_id: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// 补偿操作失败
    #[error("Compensate failed at step {step_id}: {source}")]
    CompensateFailed {
        step_id: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// 补偿重试耗尽
    #[error("Compensate retry exhausted for step {step_id} after {retries} attempts")]
    CompensateRetryExhausted { step_id: String, retries: u32 },

    /// 事务超时
    #[error("Transaction timed out after {0:?}")]
    Timeout(std::time::Duration),

    /// 状态持久化失败
    #[error("Persistence failed: {0}")]
    Persistence(String),

    /// 事务状态非法
    #[error("Invalid transaction state: {0}")]
    InvalidState(String),

    /// 事务不存在
    #[error("Transaction not found: {0}")]
    NotFound(String),

    /// 序列化失败
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// 通用错误
    #[error("{0}")]
    Generic(String),
}

impl DtxError {
    /// 从字符串创建通用错误
    pub fn generic(msg: impl Into<String>) -> Self {
        Self::Generic(msg.into())
    }

    /// 创建正向操作失败
    pub fn forward_failed(
        step_id: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::ForwardFailed {
            step_id: step_id.into(),
            source: Box::new(source),
        }
    }

    /// 创建补偿失败
    pub fn compensate_failed(
        step_id: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::CompensateFailed {
            step_id: step_id.into(),
            source: Box::new(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dtx_error_display() {
        let e = DtxError::generic("test error");
        assert_eq!(e.to_string(), "test error");

        let e = DtxError::Timeout(std::time::Duration::from_secs(30));
        assert!(e.to_string().contains("30s"));
    }

    #[test]
    fn test_forward_failed() {
        let e = DtxError::forward_failed("step-1", DtxError::generic("inner"));
        let s = e.to_string();
        assert!(s.contains("step-1"));
        assert!(s.contains("inner"));
    }

    #[test]
    fn test_compensate_retry_exhausted() {
        let e = DtxError::CompensateRetryExhausted {
            step_id: "s1".into(),
            retries: 3,
        };
        assert!(e.to_string().contains("3 attempts"));
    }
}
