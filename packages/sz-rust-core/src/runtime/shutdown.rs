//! 优雅关闭协调器（Phase 9.8）
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `think-swoole` 的关闭流程：
//!
//! | PHP Swoole | Rust | 说明 |
//! |------------|------|------|
//! | `$server->shutdown()` | `GracefulShutdown::shutdown(timeout)` | 触发关闭 |
//! | `onShutdown` 回调 | `JoinSet` 中所有任务响应 `CancellationToken` | 任务优雅退出 |
//! | `max_wait_time` 配置 | `timeout: Duration` | 超时强制结束 |
//!
//! ## 设计
//!
//! - 使用 `tokio::task::JoinSet` 管理所有后台任务
//! - `CancellationToken` 统一广播关闭信号
//! - `shutdown(timeout)` 先 `cancel()` 通知所有任务，再 `join_all_with_timeout` 等待
//! - 超时后剩余任务被 abort（对齐 Swoole 强制结束）

use std::time::Duration;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// 优雅关闭协调器（Phase 9.8）
///
/// 对齐 PHP `think-swoole` 的关闭流程：管理多个后台任务，触发关闭后等待所有任务
/// 优雅退出，超时则强制 abort。
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_core::runtime::shutdown::GracefulShutdown;
/// use std::time::Duration;
///
/// # #[tokio::main]
/// # async fn main() {
/// let mut gs = GracefulShutdown::new();
/// let token = gs.token();
///
/// // 注册后台任务
/// gs.spawn(async move {
///     token.cancelled().await;
///     // 清理逻辑
/// });
///
/// // 触发关闭，等待最多 5 秒
/// let (success, pending) = gs.shutdown(Duration::from_secs(5)).await;
/// assert!(success);
/// assert_eq!(pending, 0);
/// # }
/// ```
pub struct GracefulShutdown {
    /// 后台任务集合
    tasks: JoinSet<()>,
    /// 关闭令牌：所有任务通过 `token().cancelled().await` 监听关闭
    token: CancellationToken,
}

impl GracefulShutdown {
    /// 创建 GracefulShutdown
    pub fn new() -> Self {
        Self {
            tasks: JoinSet::new(),
            token: CancellationToken::new(),
        }
    }

    /// 获取关闭令牌的克隆
    ///
    /// 后台任务通过 `gs.token().cancelled().await` 监听关闭信号。
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// 注册后台任务到 JoinSet
    ///
    /// 任务应通过 `token().cancelled().await` 监听关闭信号，在收到信号后优雅退出。
    pub fn spawn<F>(&mut self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.tasks.spawn(future);
    }

    /// 当前待完成的任务数
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// 是否没有待完成的任务
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// 触发优雅关闭
    ///
    /// ## 流程
    ///
    /// 1. `token.cancel()` 通知所有任务关闭
    /// 2. 在 `timeout` 内等待所有任务完成
    /// 3. 超时后 abort 剩余任务
    ///
    /// ## 返回
    ///
    /// - `success: bool`：是否所有任务在超时内完成
    /// - `pending: usize`：超时后被 abort 的任务数
    pub async fn shutdown(mut self, timeout: Duration) -> (bool, usize) {
        // 1. 广播关闭信号
        self.token.cancel();

        // 2. 等待所有任务完成，带超时
        let deadline = tokio::time::Instant::now() + timeout;
        let mut success = true;
        let mut aborted = 0usize;

        loop {
            if self.tasks.is_empty() {
                break;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                // 超时，abort 剩余任务
                aborted = self.tasks.len();
                self.tasks.abort_all();
                // 等待 abort 完成
                while self.tasks.join_next().await.is_some() {}
                success = false;
                break;
            }
            tokio::select! {
                biased;
                _ = tokio::time::sleep(remaining) => {
                    // 超时
                    aborted = self.tasks.len();
                    self.tasks.abort_all();
                    while self.tasks.join_next().await.is_some() {}
                    success = false;
                    break;
                }
                res = self.tasks.join_next() => {
                    if res.is_none() {
                        break;
                    }
                    // 任务完成，继续等待下一个
                }
            }
        }

        (success, aborted)
    }

    /// 触发关闭，不等待（立即 abort 所有任务）
    ///
    /// 对齐 Swoole 强制 kill 模式。
    pub fn abort_now(mut self) -> usize {
        let n = self.tasks.len();
        self.tasks.abort_all();
        n
    }
}

impl Default for GracefulShutdown {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_new_empty() {
        let gs = GracefulShutdown::new();
        assert!(gs.is_empty());
        assert_eq!(gs.len(), 0);
    }

    #[tokio::test]
    async fn test_spawn_increments_len() {
        let mut gs = GracefulShutdown::new();
        let token = gs.token();
        gs.spawn(async move {
            token.cancelled().await;
        });
        assert_eq!(gs.len(), 1);
        assert!(!gs.is_empty());
    }

    #[tokio::test]
    async fn test_shutdown_success_all_tasks_complete() {
        let mut gs = GracefulShutdown::new();
        let token = gs.token();
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        gs.spawn(async move {
            // 等待关闭信号后设置 flag
            token.cancelled().await;
            flag_clone.store(true, Ordering::SeqCst);
        });

        let (success, aborted) = gs.shutdown(Duration::from_secs(1)).await;
        assert!(success);
        assert_eq!(aborted, 0);
        assert!(flag.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_shutdown_multiple_tasks() {
        let mut gs = GracefulShutdown::new();
        let token1 = gs.token();
        let token2 = gs.token();

        gs.spawn(async move {
            token1.cancelled().await;
        });
        gs.spawn(async move {
            token2.cancelled().await;
        });

        let (success, aborted) = gs.shutdown(Duration::from_secs(1)).await;
        assert!(success);
        assert_eq!(aborted, 0);
    }

    #[tokio::test]
    async fn test_shutdown_timeout_aborts_tasks() {
        let mut gs = GracefulShutdown::new();
        // 不监听 token 的任务，会一直运行直到 abort
        gs.spawn(async {
            // 模拟不响应关闭信号的任务
            std::future::pending::<()>().await;
        });

        let (success, aborted) = gs.shutdown(Duration::from_millis(50)).await;
        assert!(!success);
        assert_eq!(aborted, 1);
    }

    #[tokio::test]
    async fn test_shutdown_with_mixed_tasks() {
        let mut gs = GracefulShutdown::new();
        let token = gs.token();

        // 任务1：响应关闭信号
        gs.spawn(async move {
            token.cancelled().await;
        });
        // 任务2：不响应关闭信号
        gs.spawn(async {
            std::future::pending::<()>().await;
        });

        let (success, aborted) = gs.shutdown(Duration::from_millis(50)).await;
        assert!(!success);
        assert_eq!(aborted, 1);
    }

    #[tokio::test]
    async fn test_shutdown_no_tasks() {
        let gs = GracefulShutdown::new();
        let (success, aborted) = gs.shutdown(Duration::from_millis(100)).await;
        assert!(success);
        assert_eq!(aborted, 0);
    }

    #[tokio::test]
    async fn test_abort_now() {
        let mut gs = GracefulShutdown::new();
        gs.spawn(async {
            std::future::pending::<()>().await;
        });
        gs.spawn(async {
            std::future::pending::<()>().await;
        });
        let aborted = gs.abort_now();
        assert_eq!(aborted, 2);
    }

    #[tokio::test]
    async fn test_abort_now_empty() {
        let gs = GracefulShutdown::new();
        assert_eq!(gs.abort_now(), 0);
    }

    #[tokio::test]
    async fn test_token_cancellation_propagates() {
        let mut gs = GracefulShutdown::new();
        let token = gs.token();
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        for _ in 0..5 {
            let t = token.clone();
            let c = counter.clone();
            gs.spawn(async move {
                t.cancelled().await;
                c.fetch_add(1, Ordering::SeqCst);
            });
        }

        let (success, aborted) = gs.shutdown(Duration::from_secs(1)).await;
        assert!(success);
        assert_eq!(aborted, 0);
        assert_eq!(counter.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn test_default_impl() {
        let gs = GracefulShutdown::default();
        assert!(gs.is_empty());
    }

    #[tokio::test]
    async fn test_shutdown_zero_timeout() {
        let mut gs = GracefulShutdown::new();
        let token = gs.token();
        // 即使任务响应 token，0 超时也会 abort
        gs.spawn(async move {
            token.cancelled().await;
        });
        let (success, aborted) = gs.shutdown(Duration::from_millis(0)).await;
        // 0 超时可能成功也可能失败，取决于任务响应速度
        // 主要验证不 panic
        let _ = (success, aborted);
    }

    #[tokio::test]
    async fn test_shutdown_returns_correct_aborted_count() {
        let mut gs = GracefulShutdown::new();
        // 3 个不响应的任务
        gs.spawn(async {
            std::future::pending::<()>().await;
        });
        gs.spawn(async {
            std::future::pending::<()>().await;
        });
        gs.spawn(async {
            std::future::pending::<()>().await;
        });

        let (success, aborted) = gs.shutdown(Duration::from_millis(10)).await;
        assert!(!success);
        assert_eq!(aborted, 3);
    }

    #[tokio::test]
    async fn test_task_completes_before_shutdown() {
        let mut gs = GracefulShutdown::new();
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        gs.spawn(async move {
            flag_clone.store(true, Ordering::SeqCst);
        });

        // 等待任务自然完成
        tokio::time::sleep(Duration::from_millis(50)).await;

        let (success, aborted) = gs.shutdown(Duration::from_secs(1)).await;
        assert!(success);
        assert_eq!(aborted, 0);
        assert!(flag.load(Ordering::SeqCst));
    }
}
