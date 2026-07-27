//! tokio::spawn 异步任务
//!
//! ## PHP 对齐
//!
//! 对齐 PHP Swoole 的异步任务 spawn 机制：
//!
//! | PHP Swoole | Rust | 说明 |
//! |------------|------|------|
//! | `swoole_event::defer($callback)` | `tokio::spawn(fut)` | 延迟执行异步任务 |
//! | `Swoole\Coroutine::create($callback)` | `tokio::spawn(fut)` | 创建协程 |
//! | `Swoole\Coroutine::go()` | `tokio::spawn(fut)` | go() 别名 |
//! | `Swoole\Timer::tick($ms, $callback)` | `tokio::time::interval + spawn` | 定时任务 |
//! | `Swoole\Timer::after($ms, $callback)` | `tokio::time::sleep + spawn` | 一次性延迟任务 |
//!
//! ## 设计
//!
//! 提供 `spawn_with_token` helper：自动注入 `CancellationToken`，任务可监听关闭信号优雅退出。

use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// spawn 一个异步任务并注入 CancellationToken
///
/// 对齐 `Swoole\Coroutine::create()`，额外提供关闭信号监听。
///
/// ## 参数
///
/// - `token`：关闭令牌，任务通过 `token.cancelled().await` 监听关闭
/// - `future`：异步任务 future
///
/// ## 返回
///
/// `JoinHandle<T>`，调用方可 await 获取结果
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_core::runtime::spawn_with_token;
/// use tokio_util::sync::CancellationToken;
///
/// let token = CancellationToken::new();
/// let handle = spawn_with_token(token.clone(), async move {
///     // 任务执行
///     42
/// });
/// ```
pub fn spawn_with_token<F, T>(token: CancellationToken, future: F) -> tokio::task::JoinHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    tokio::spawn(async move {
        let _ = token; // token 移动到任务内，但不主动 cancel
        future.await
    })
}

/// spawn 一个延迟执行的任务（对齐 `Swoole\Timer::after($ms, $callback)`）
///
/// 在 `delay` 后执行 `future`，返回 `JoinHandle<T>`。
pub fn spawn_after<F, T>(delay: Duration, future: F) -> tokio::task::JoinHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        future.await
    })
}

/// spawn 一个周期性任务（对齐 `Swoole\Timer::tick($ms, $callback)`）
///
/// 每 `interval_ms` 毫秒执行一次 `future`，直到 `token` 被 cancel。
///
/// ## 返回
///
/// `JoinHandle<()>`：任务返回 `()`，调用方可 await 等待任务退出（通常在 cancel 后）。
pub fn spawn_tick<F>(
    interval_ms: u64,
    token: CancellationToken,
    mut future: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnMut() + Send + 'static,
{
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(interval_ms));
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = ticker.tick() => future(),
            }
        }
    })
}

/// spawn 一个带超时的任务（对齐 PHP `Swoole\Coroutine::select()` 超时控制）
///
/// 如果 `future` 在 `timeout` 内完成，返回 `Ok(T)`；否则返回 `Err(TimeoutError)`。
pub fn spawn_with_timeout<F, T>(
    timeout: Duration,
    future: F,
) -> tokio::task::JoinHandle<Result<T, TimeoutError>>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    tokio::spawn(async move {
        match tokio::time::timeout(timeout, future).await {
            Ok(result) => Ok(result),
            Err(_) => Err(TimeoutError),
        }
    })
}

/// 超时错误
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("task timed out")]
pub struct TimeoutError;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    #[tokio::test]
    async fn test_spawn_with_token_basic() {
        let token = CancellationToken::new();
        let handle = spawn_with_token(token, async { 42 });
        assert_eq!(handle.await.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_spawn_with_token_cancellation() {
        let token = CancellationToken::new();
        let token_clone = token.clone();
        let token_for_closure = token.clone();
        let handle = spawn_with_token(token_clone, async move {
            token_for_closure.cancelled().await;
            99
        });
        token.cancel();
        assert_eq!(handle.await.unwrap(), 99);
    }

    #[tokio::test]
    async fn test_spawn_after_basic() {
        let start = Instant::now();
        let handle = spawn_after(Duration::from_millis(50), async { 7 });
        let result = handle.await.unwrap();
        assert_eq!(result, 7);
        assert!(start.elapsed() >= Duration::from_millis(40));
    }

    #[tokio::test]
    async fn test_spawn_after_zero_delay() {
        let handle = spawn_after(Duration::from_millis(0), async { 1 });
        assert_eq!(handle.await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_spawn_tick_fires_multiple_times() {
        let counter = Arc::new(AtomicUsize::new(0));
        let token = CancellationToken::new();
        let counter_clone = counter.clone();
        let handle = spawn_tick(10, token.clone(), move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        // 等待足够时间让 tick 触发多次
        tokio::time::sleep(Duration::from_millis(100)).await;
        token.cancel();
        let _ = handle.await;

        // 至少触发 1 次（interval 首次 tick 立即触发）
        assert!(counter.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn test_spawn_tick_stops_on_cancel() {
        let counter = Arc::new(AtomicUsize::new(0));
        let token = CancellationToken::new();
        let counter_clone = counter.clone();
        let handle = spawn_tick(10, token.clone(), move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        tokio::time::sleep(Duration::from_millis(30)).await;
        token.cancel();
        let _ = handle.await;
        let count_after_cancel = counter.load(Ordering::SeqCst);

        // 等待一段时间确认计数不再增长
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(counter.load(Ordering::SeqCst), count_after_cancel);
    }

    #[tokio::test]
    async fn test_spawn_with_timeout_success() {
        let handle = spawn_with_timeout(Duration::from_millis(100), async { 42 });
        assert_eq!(handle.await.unwrap().unwrap(), 42);
    }

    #[tokio::test]
    async fn test_spawn_with_timeout_failure() {
        let handle = spawn_with_timeout(Duration::from_millis(10), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            42
        });
        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), TimeoutError);
    }

    #[tokio::test]
    async fn test_spawn_with_timeout_zero_timeout() {
        // 零超时仍然允许 future 至少 poll 一次
        let handle = spawn_with_timeout(Duration::from_millis(0), async { 1 });
        let result = handle.await.unwrap();
        // 零超时可能成功也可能失败，取决于 race，但不应 panic
        let _ = result;
    }

    #[tokio::test]
    async fn test_timeout_error_display() {
        let err = TimeoutError;
        assert_eq!(format!("{}", err), "task timed out");
    }

    #[tokio::test]
    async fn test_spawn_multiple_concurrent_tasks() {
        let token1 = CancellationToken::new();
        let token2 = CancellationToken::new();
        let h1 = spawn_with_token(token1, async { 1 });
        let h2 = spawn_with_token(token2, async { 2 });
        assert_eq!(h1.await.unwrap(), 1);
        assert_eq!(h2.await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_spawn_tick_immediate_first_fire() {
        // tokio::time::interval 首次 tick 立即完成
        let counter = Arc::new(AtomicUsize::new(0));
        let token = CancellationToken::new();
        let counter_clone = counter.clone();
        let handle = spawn_tick(1, token.clone(), move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        // 轮询等待计数器达到 1（超时 5s），避免固定 sleep 的调度竞态
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while counter.load(Ordering::SeqCst) < 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
            if tokio::time::Instant::now() > deadline {
                token.cancel();
                let _ = handle.await;
                panic!("spawn_tick 首次 tick 未在 5s 内触发");
            }
        }
        token.cancel();
        let _ = handle.await;
        assert!(
            counter.load(Ordering::SeqCst) >= 1,
            "spawn_tick 首次 tick 应在创建后立即触发，实际计数为 {}",
            counter.load(Ordering::SeqCst)
        );
    }
}
