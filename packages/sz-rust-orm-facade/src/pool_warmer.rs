//! PoolWarmer — 连接池预热（P3 L3 调优）
//!
//! 启动时并发建立 N 个连接放入连接池，消除首次请求冷启动延迟。
//! 预热失败时降级到懒加载（首次请求时建立连接）。
//!
//! ## 用法
//!
//! ```rust,ignore
//! use sz_rust_orm_facade::pool_warmer::PoolWarmer;
//!
//! let warmer = PoolWarmer::new(10, || async {
//!     // 建立连接的逻辑
//!     Ok(())
//! });
//! warmer.warm().await?;
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// 预热错误
#[derive(Debug, thiserror::Error)]
pub enum WarmupError {
    /// 连接建立失败
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    /// 预热超时
    #[error("warmup timeout after {0:?}")]
    Timeout(Duration),
    /// 部分预热失败
    #[error("partial warmup failure: {succeeded}/{total} succeeded")]
    PartialFailure { succeeded: u32, total: u32 },
}

/// 连接建立工厂（async 闭包，返回连接或错误）
pub type ConnectFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<(), WarmupError>> + Send>> + Send + Sync>;

/// 连接池预热器
///
/// 启动时并发建立 `warmup_count` 个连接，消除首次请求冷启动。
/// 预热失败时降级到懒加载，不阻塞启动。
pub struct PoolWarmer {
    /// 预热连接数
    warmup_count: u32,
    /// 连接建立工厂
    connect_fn: ConnectFn,
    /// 单连接超时
    connect_timeout: Duration,
}

impl PoolWarmer {
    /// 创建 PoolWarmer
    ///
    /// - `warmup_count`：预热连接数
    /// - `connect_fn`：连接建立闭包（每次调用建立一个连接）
    pub fn new<F, Fut>(warmup_count: u32, connect_fn: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), WarmupError>> + Send + 'static,
    {
        Self {
            warmup_count,
            connect_fn: Arc::new(move || Box::pin(connect_fn())),
            connect_timeout: Duration::from_secs(10),
        }
    }

    /// 设置单连接超时
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// 预热连接数
    pub fn warmup_count(&self) -> u32 {
        self.warmup_count
    }

    /// 执行预热（并发建立 N 个连接）
    ///
    /// 成功返回 `Ok(())`，部分失败返回 `WarmupError::PartialFailure`。
    /// 全部失败返回 `WarmupError::ConnectionFailed`。
    pub async fn warm(&self) -> Result<(), WarmupError> {
        if self.warmup_count == 0 {
            return Ok(());
        }

        let mut handles = Vec::with_capacity(self.warmup_count as usize);
        for _ in 0..self.warmup_count {
            let connect_fn = self.connect_fn.clone();
            let timeout = self.connect_timeout;
            handles.push(tokio::spawn(async move {
                tokio::time::timeout(timeout, connect_fn()).await
            }));
        }

        let mut succeeded = 0u32;
        let mut last_error = String::new();
        for handle in handles {
            match handle.await {
                Ok(Ok(Ok(()))) => succeeded += 1,
                Ok(Ok(Err(e))) => last_error = e.to_string(),
                Ok(Err(_)) => last_error = "timeout".to_string(),
                Err(e) => last_error = e.to_string(),
            }
        }

        if succeeded == self.warmup_count {
            Ok(())
        } else if succeeded > 0 {
            Err(WarmupError::PartialFailure {
                succeeded,
                total: self.warmup_count,
            })
        } else {
            Err(WarmupError::ConnectionFailed(last_error))
        }
    }
}

impl std::fmt::Debug for PoolWarmer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PoolWarmer {{ warmup_count: {}, timeout: {:?} }}",
            self.warmup_count, self.connect_timeout
        )
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_pool_warmer_success() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();
        let warmer = PoolWarmer::new(5, move || {
            let c = counter_clone.clone();
            async move {
                c.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
        });
        let result = warmer.warm().await;
        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::Relaxed), 5);
    }

    #[tokio::test]
    async fn test_pool_warmer_zero_count() {
        let warmer = PoolWarmer::new(0, || async { Ok(()) });
        let result = warmer.warm().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_pool_warmer_partial_failure() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();
        let warmer = PoolWarmer::new(4, move || {
            let c = counter_clone.clone();
            async move {
                let n = c.fetch_add(1, Ordering::Relaxed);
                if n < 2 {
                    Ok(())
                } else {
                    Err(WarmupError::ConnectionFailed("mock fail".to_string()))
                }
            }
        });
        let result = warmer.warm().await;
        assert!(matches!(
            result,
            Err(WarmupError::PartialFailure {
                succeeded: 2,
                total: 4
            })
        ));
    }

    #[tokio::test]
    async fn test_pool_warmer_all_fail() {
        let warmer = PoolWarmer::new(3, || async {
            Err(WarmupError::ConnectionFailed("mock fail".to_string()))
        });
        let result = warmer.warm().await;
        assert!(matches!(result, Err(WarmupError::ConnectionFailed(_))));
    }

    #[tokio::test]
    async fn test_pool_warmer_timeout() {
        let warmer = PoolWarmer::new(1, || async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(())
        })
        .with_timeout(Duration::from_millis(50));
        let result = warmer.warm().await;
        assert!(result.is_err());
    }

    #[test]
    fn test_pool_warmer_warmup_count() {
        let warmer = PoolWarmer::new(10, || async { Ok(()) });
        assert_eq!(warmer.warmup_count(), 10);
    }

    #[test]
    fn test_pool_warmer_debug() {
        let warmer = PoolWarmer::new(5, || async { Ok(()) });
        let s = format!("{warmer:?}");
        assert!(s.contains("warmup_count: 5"));
    }
}
