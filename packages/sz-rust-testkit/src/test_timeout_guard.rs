//! 测试超时守护（v1.5.0 P1-4）
//!
//! 用 `tokio::time::timeout` 包裹测试，超时后强制失败并报告挂起位置与疑似原因（spec 5.4.3）。
//!
//! ## 使用示例
//!
//! ```ignore
//! use sz_rust_testkit::test_timeout_guard::TestTimeoutGuard;
//! use std::time::Duration;
//!
//! let result = TestTimeoutGuard::run(Duration::from_secs(5), || async {
//!     // 测试逻辑
//!     Ok(42)
//! }).await?;
//! ```
#![forbid(unsafe_code)]

use std::time::Duration;

use crate::test_error::TestError;
use sz_rust_orm_facade::DbError;

/// 测试超时守护
pub struct TestTimeoutGuard;

impl TestTimeoutGuard {
    /// 用超时包裹执行测试函数（spec 5.4.3）
    ///
    /// 超时后返回 `TestError::Timeout`，包含疑似原因（锁等待/连接耗尽）。
    pub async fn run<F, Fut, T>(timeout: Duration, f: F) -> Result<T, TestError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, DbError>>,
    {
        match tokio::time::timeout(timeout, f()).await {
            Ok(result) => result.map_err(Into::into),
            Err(_) => Err(TestError::Timeout {
                timeout,
                reason: "疑似锁等待或连接池耗尽".to_string(),
            }),
        }
    }

    /// 用超时包裹执行任意 future（不要求 DbError 返回类型）
    pub async fn run_raw<Fut, T>(timeout: Duration, fut: Fut) -> Result<T, TestError>
    where
        Fut: std::future::Future<Output = T>,
    {
        match tokio::time::timeout(timeout, fut).await {
            Ok(result) => Ok(result),
            Err(_) => Err(TestError::Timeout {
                timeout,
                reason: "疑似锁等待或连接池耗尽".to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_timeout_guard_success() {
        let result =
            TestTimeoutGuard::run(Duration::from_secs(5), || async { Ok::<_, DbError>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_timeout_guard_timeout() {
        let result = TestTimeoutGuard::run(Duration::from_millis(10), || async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok::<_, DbError>(42)
        })
        .await;
        assert!(matches!(result, Err(TestError::Timeout { .. })));
    }

    #[tokio::test]

    async fn test_timeout_guard_run_raw_success() {
        let result = TestTimeoutGuard::run_raw(Duration::from_secs(5), async { 100 }).await;
        assert_eq!(result.unwrap(), 100);
    }

    #[tokio::test]
    async fn test_timeout_guard_run_raw_timeout() {
        let result = TestTimeoutGuard::run_raw(Duration::from_millis(10), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            100
        })
        .await;
        assert!(matches!(result, Err(TestError::Timeout { .. })));
    }
}
