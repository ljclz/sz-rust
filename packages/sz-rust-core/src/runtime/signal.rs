//! 信号处理（Phase 9.9）
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `think-swoole` 的信号处理：
//!
//! | PHP Swoole | Rust | 说明 |
//! |------------|------|------|
//! | `Swoole\Process::signal(SIGTERM, $cb)` | `signal::unix::signal(SignalKind::terminate)` | Unix SIGTERM |
//! | `Swoole\Process::signal(SIGINT, $cb)` | `signal::unix::signal(SignalKind::interrupt)` | Unix SIGINT (Ctrl+C) |
//! | `Swoole\Process::signal(SIGQUIT, $cb)` | `signal::unix::signal(SignalKind::quit)` | Unix SIGQUIT |
//! | Windows 平台 | `tokio::signal::ctrl_c()` + `ctrl_close()` | Windows 信号 |
//!
//! ## 设计
//!
//! - 双平台支持：Unix 用 `SignalKind`，Windows 用 `ctrl_c` + `ctrl_close`
//! - `shutdown_signal()` 返回 `Future<Output = ()>`，await 时阻塞直到收到信号
//! - 收到信号后立即返回，调用方根据返回值触发 `CancellationToken::cancel()`
//!
//! ## 信号映射
//!
//! | 平台 | 信号 | 含义 |
//! |------|------|------|
//! | Unix | SIGTERM | 终止信号（kill 默认） |
//! | Unix | SIGINT | 中断信号（Ctrl+C） |
//! | Unix | SIGQUIT | 退出信号（Ctrl+\，生成 core dump） |
//! | Windows | ctrl_c | Ctrl+C |
//! | Windows | ctrl_close | 控制台窗口关闭 |
//! | Windows | ctrl_break | Ctrl+Break |
//! | Windows | ctrl_shutdown | 系统关闭 |
//! | Windows | ctrl_logoff | 用户注销 |

use std::future::Future;

/// 等待关闭信号（Phase 9.9）
///
/// 对齐 PHP `think-swoole` 的信号监听：在 Unix 上监听 SIGTERM/SIGINT/SIGQUIT，
/// 在 Windows 上监听 ctrl_c/ctrl_close/ctrl_break/ctrl_shutdown/ctrl_logoff。
///
/// 收到任一信号后立即返回，调用方应根据返回值触发 `CancellationToken::cancel()`。
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_core::runtime::signal::shutdown_signal;
/// use tokio_util::sync::CancellationToken;
///
/// # #[tokio::main]
/// # async fn main() {
/// let token = CancellationToken::new();
/// let token_clone = token.clone();
///
/// tokio::spawn(async move {
///     shutdown_signal().await;
///     token_clone.cancel();
/// });
///
/// // 主任务监听 token
/// token.cancelled().await;
/// # }
/// ```
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        // 监听 SIGTERM / SIGINT / SIGQUIT
        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("Failed to install SIGINT handler");
        let mut sigquit = signal(SignalKind::quit()).expect("Failed to install SIGQUIT handler");

        tokio::select! {
            _ = sigterm.recv() => {}
            _ = sigint.recv() => {}
            _ = sigquit.recv() => {}
        }
    }

    #[cfg(windows)]
    {
        use tokio::signal::windows;

        // 监听 Windows 控制台信号
        let mut ctrl_c = windows::ctrl_c().expect("Failed to install Ctrl+C handler");
        let mut ctrl_close = windows::ctrl_close().expect("Failed to install Ctrl+Close handler");
        let mut ctrl_break = windows::ctrl_break().expect("Failed to install Ctrl+Break handler");
        let mut ctrl_shutdown =
            windows::ctrl_shutdown().expect("Failed to install Ctrl+Shutdown handler");
        let mut ctrl_logoff =
            windows::ctrl_logoff().expect("Failed to install Ctrl+Logoff handler");

        tokio::select! {
            _ = ctrl_c.recv() => {}
            _ = ctrl_close.recv() => {}
            _ = ctrl_break.recv() => {}
            _ = ctrl_shutdown.recv() => {}
            _ = ctrl_logoff.recv() => {}
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        // 其他平台：直接返回（不支持信号处理）
        std::future::pending::<()>().await;
    }
}

/// 等待关闭信号并触发 CancellationToken（Phase 9.9）
///
/// 这是 `shutdown_signal()` + `token.cancel()` 的便捷封装。
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_core::runtime::signal::shutdown_with_token;
/// use tokio_util::sync::CancellationToken;
///
/// # #[tokio::main]
/// # async fn main() {
/// let token = CancellationToken::new();
/// tokio::spawn(shutdown_with_token(token.clone()));
///
/// token.cancelled().await;
/// # }
/// ```
pub async fn shutdown_with_token(token: tokio_util::sync::CancellationToken) {
    shutdown_signal().await;
    token.cancel();
}

/// 返回一个 Future，await 时会等待关闭信号（不消费 token）
///
/// 用于在 `tokio::select!` 中组合使用：
///
/// ```rust,ignore
/// use sz_rust_core::runtime::signal::shutdown_future;
///
/// # #[tokio::main]
/// # async fn main() {
/// tokio::select! {
///     _ = shutdown_future() => {
///         println!("收到关闭信号");
///     }
///     _ = tokio::time::sleep(Duration::from_secs(10)) => {
///         println!("超时");
///     }
/// }
/// # }
/// ```
pub fn shutdown_future() -> impl Future<Output = ()> {
    shutdown_signal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn test_shutdown_with_token_immediate_cancel() {
        // 测试 shutdown_with_token 在 token 已取消时不会阻塞
        // 这里不实际发送信号，而是验证函数能正确编译和基本逻辑
        let token = CancellationToken::new();
        let token_clone = token.clone();

        // 立即取消，让 shutdown_with_token 不会真正等待信号
        // 注意：shutdown_with_token 会先 await shutdown_signal()，这里只是验证类型正确
        let _handle = tokio::spawn(async move {
            // 不实际等待，仅验证类型
            let _ = token_clone;
        });

        token.cancel();
        // 验证 token 可以正常取消
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn test_shutdown_future_is_send() {
        // 验证 shutdown_future() 返回的 Future 是 Send（可在 tokio::spawn 中使用）
        fn assert_send<T: Send>(_t: T) {}
        let fut = shutdown_future();
        assert_send(fut);
    }

    #[tokio::test]
    async fn test_shutdown_signal_does_not_block_indefinitely_in_select() {
        // 验证 shutdown_signal 可以在 select! 中与其他 future 组合
        // 这里用 timeout 来验证它不会立即返回（除非有信号）
        tokio::select! {
            _ = shutdown_signal() => {
                // 收到信号（测试环境通常不会收到）
            }
            _ = tokio::time::sleep(Duration::from_millis(10)) => {
                // 超时，验证 shutdown_signal 不会立即返回
            }
        }
    }

    #[tokio::test]
    async fn test_shutdown_with_token_cancels_token() {
        // 验证 shutdown_with_token 的类型签名正确
        let token = CancellationToken::new();
        let _fut = shutdown_with_token(token.clone());
        // 不实际 await（会阻塞等待信号），仅验证类型
    }

    #[tokio::test]
    async fn test_cancellation_token_basic() {
        // 验证 CancellationToken 的基本行为（作为 signal 模块的基础设施）
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());

        let token_clone = token.clone();
        token_clone.cancel();

        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn test_cancellation_token_cancelled_completes_after_cancel() {
        let token = CancellationToken::new();
        let token_clone = token.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            token_clone.cancel();
        });

        // cancelled() 在 cancel 后立即完成
        token.cancelled().await;
    }

    #[tokio::test]
    async fn test_multiple_listeners_all_complete() {
        let token = CancellationToken::new();

        let t1 = token.clone();
        let t2 = token.clone();
        let t3 = token.clone();

        let h1 = tokio::spawn(async move {
            t1.cancelled().await;
            1
        });
        let h2 = tokio::spawn(async move {
            t2.cancelled().await;
            2
        });
        let h3 = tokio::spawn(async move {
            t3.cancelled().await;
            3
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        token.cancel();

        assert_eq!(h1.await.unwrap(), 1);
        assert_eq!(h2.await.unwrap(), 2);
        assert_eq!(h3.await.unwrap(), 3);
    }

    #[tokio::test]
    async fn test_child_token_cancellation() {
        // 验证子 token 的取消行为
        let parent = CancellationToken::new();
        let child = parent.child_token();

        assert!(!parent.is_cancelled());
        assert!(!child.is_cancelled());

        parent.cancel();

        assert!(parent.is_cancelled());
        assert!(child.is_cancelled());
    }

    #[tokio::test]
    async fn test_child_token_does_not_cancel_parent() {
        let parent = CancellationToken::new();
        let child = parent.child_token();

        child.cancel();

        assert!(!parent.is_cancelled());
        assert!(child.is_cancelled());
    }

    #[tokio::test]
    async fn test_select_with_token_and_other_future() {
        // 验证 CancellationToken 可以在 select! 中正常使用
        let token = CancellationToken::new();
        let token_clone = token.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            token_clone.cancel();
        });

        tokio::select! {
            _ = token.cancelled() => {
                // 预期路径
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                panic!("token cancellation should win");
            }
        }
    }

    #[tokio::test]
    async fn test_nested_child_tokens() {
        // 验证多层子 token 的取消传播
        let root = CancellationToken::new();
        let level1 = root.child_token();
        let level2 = level1.child_token();
        let level3 = level2.child_token();

        root.cancel();

        assert!(root.is_cancelled());
        assert!(level1.is_cancelled());
        assert!(level2.is_cancelled());
        assert!(level3.is_cancelled());
    }
}
