//! SZ-Rust Runtime — Swoole/Worker 适配主入口
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `topthink/think-swoole` 的运行时模型：
//!
//! | PHP Swoole | Rust SZ-Rust | 说明 |
//! |------------|--------------|------|
//! | `Swoole\Runtime::enableCoroutine()` | `SzRuntime::block_on(fut)` | 协程入口 |
//! | `swoole_cpu_num()` | `num_cpus::get()` | 默认 worker 数 |
//! | `worker_num` 配置项 | `SzRuntime::with_worker_threads(n)` | 自定义 worker 数 |
//! | `Swoole\Process::signal()` | `tokio::signal` + `CancellationToken` | 信号处理 |
//! | `Swoole\Timer::tick()` | `tokio::time::interval` + `tokio::select!` | 定时任务 |
//! | `swoole_event::defer()` | `SzRuntime::spawn(fut)` | 异步任务 |
//!
//! ## 模块结构
//!
//! | 模块 | 对齐 PHP | 子任务 |
//! |------|---------|--------|
//! | `runtime::worker` | `worker_num` 配置 | 9.2 |
//! | `runtime::spawn` | `swoole_event::defer` | 9.3 |
//! | `runtime::queue` | `think-queue` 消费者 | 9.4 |
//! | `runtime::mqtt` | 长连接 | 9.5 |
//! | `runtime::websocket` | `think-worker` | 9.6 |
//! | `runtime::scheduler` | `Crontab` | 9.7 |
//! | `runtime::shutdown` | 优雅关闭 | 9.8 |
//! | `runtime::signal` | `SIGTERM/SIGINT` | 9.9 |
//!
//! ## 关键决策
//!
//! - **不修改 `sz-orm-scheduler` 内部 API**：保持 `CronScheduler::start()`/`stop()` 向后兼容，
//!   在 sz-rust 侧用 `tokio::time::interval` + `try_fire_due()` 重写循环。
//! - **使用 `CancellationToken` 统一关闭广播**：替代 `AtomicBool` + `oneshot`，支持父子层级。
//! - **双平台信号处理**：Unix 用 `SignalKind::terminate/interrupt`，Windows 用 `ctrl_c/ctrl_close`。

pub mod mqtt;
pub mod queue;
pub mod scheduler;
pub mod shutdown;
pub mod signal;
pub mod spawn;
pub mod websocket;
pub mod worker;

// P2: Addon 热加载探索（可选 feature: hot-reload）
#[cfg(feature = "hot-reload")]
pub mod hot_reload;

pub use mqtt::{MqttRuntime, MqttRuntimeConfig};
pub use queue::{QueueConsumer, QueueRuntime, QueueRuntimeConfig};
pub use scheduler::SchedulerRuntime;
pub use shutdown::GracefulShutdown;
pub use signal::shutdown_signal;
pub use spawn::spawn_with_token;
pub use websocket::{WebSocketRuntime, WebSocketRuntimeConfig};
pub use worker::WorkerConfig;

use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// SZ-Rust 异步运行时
///
/// 对齐 PHP `think-swoole` 的运行时模型：基于 tokio multi_thread runtime，
/// worker 数量默认 = CPU 核数（对齐 `swoole_cpu_num()`）。
///
/// ## 设计
///
/// - 封装 `tokio::runtime::Runtime`，对外暴露 `block_on` / `spawn` 入口
/// - 持有 `CancellationToken`，所有后台任务通过 `shutdown_token()` 监听关闭信号
/// - `shutdown_timeout` 触发关闭流程：先 `cancel()` 通知所有任务，等待超时后 drop runtime
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_core::runtime::SzRuntime;
/// use std::time::Duration;
///
/// let rt = SzRuntime::new();
/// assert!(rt.worker_threads() > 0);
///
/// // spawn 后台任务
/// let handle = rt.spawn(async { 42 });
/// assert_eq!(rt.block_on(handle).unwrap(), 42);
///
/// // 优雅关闭
/// assert!(rt.shutdown_timeout(Duration::from_millis(100)));
/// ```
pub struct SzRuntime {
    /// 内部 tokio runtime（multi_thread）
    runtime: tokio::runtime::Runtime,
    /// worker 线程数（对齐 swoole `worker_num`）
    worker_threads: usize,
    /// 关闭令牌：所有后台任务通过 `shutdown_token()` 获取子 token 监听关闭
    shutdown_token: CancellationToken,
}

impl SzRuntime {
    /// 创建 SzRuntime，worker_threads = `num_cpus::get()`（对齐 `swoole_cpu_num()`）
    pub fn new() -> Self {
        Self::with_worker_threads(num_cpus::get())
    }

    /// 创建 SzRuntime，自定义 worker_threads（对齐 `worker_num` 配置项）
    ///
    /// - `worker_threads = 0` 会被强制为 1
    pub fn with_worker_threads(worker_threads: usize) -> Self {
        let n = worker_threads.max(1);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(n)
            .enable_all()
            .thread_name("sz-rust-worker")
            .build()
            .expect("Failed to create tokio runtime");
        Self {
            runtime,
            worker_threads: n,
            shutdown_token: CancellationToken::new(),
        }
    }

    /// 获取 worker 线程数
    pub fn worker_threads(&self) -> usize {
        self.worker_threads
    }

    /// 获取关闭令牌的克隆
    ///
    /// 后台任务通过 `rt.shutdown_token().cancelled().await` 监听关闭信号。
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown_token.clone()
    }

    /// 在 runtime 上 spawn 异步任务（对齐 `swoole_event::defer`）
    ///
    /// 返回 `JoinHandle`，可 await 获取结果。
    pub fn spawn<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime.spawn(future)
    }

    /// 在 runtime 上阻塞运行 future（对齐 `Swoole\Runtime::enableCoroutine` 入口）
    ///
    /// 调用线程会阻塞直到 future 完成。
    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future,
    {
        self.runtime.block_on(future)
    }

    /// 触发优雅关闭：`cancel()` 通知所有后台任务，等待 `timeout` 后 drop runtime
    ///
    /// - 返回 `true`：runtime 已关闭
    /// - 注意：Drop runtime 时 tokio 会等待所有任务完成，超时由调用方控制
    pub fn shutdown_timeout(self, timeout: Duration) -> bool {
        self.shutdown_token.cancel();
        // 给后台任务响应关闭信号的时间
        self.runtime.block_on(async {
            let _ = tokio::time::timeout(timeout, async {
                // 等待一小段时间让任务响应 cancel
                tokio::time::sleep(Duration::from_millis(10)).await;
            })
            .await;
        });
        // Drop runtime 会等待所有任务完成（可能阻塞）
        drop(self.runtime);
        true
    }

    /// 获取内部 runtime 句柄（用于在不持有 SzRuntime 时 spawn 任务）
    pub fn handle(&self) -> tokio::runtime::Handle {
        self.runtime.handle().clone()
    }
}

impl Default for SzRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_default_worker_threads() {
        let rt = SzRuntime::new();
        assert_eq!(rt.worker_threads(), num_cpus::get());
    }

    #[test]
    fn test_with_worker_threads_custom() {
        let rt = SzRuntime::with_worker_threads(2);
        assert_eq!(rt.worker_threads(), 2);
    }

    #[test]
    fn test_with_worker_threads_zero_falls_back_to_one() {
        let rt = SzRuntime::with_worker_threads(0);
        assert_eq!(rt.worker_threads(), 1);
    }

    #[test]
    fn test_spawn_and_block_on() {
        let rt = SzRuntime::with_worker_threads(1);
        let handle = rt.spawn(async { 42 });
        let result = rt.block_on(handle).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_block_on_directly() {
        let rt = SzRuntime::with_worker_threads(1);
        let result = rt.block_on(async { 100 });
        assert_eq!(result, 100);
    }

    #[test]
    fn test_shutdown_token_cancellation() {
        let rt = SzRuntime::with_worker_threads(1);
        let token = rt.shutdown_token();
        assert!(!token.is_cancelled());
        assert!(rt.shutdown_timeout(Duration::from_millis(50)));
        // shutdown_timeout 已消费 rt，token 已 cancel
        assert!(token.is_cancelled());
    }

    #[test]
    fn test_spawn_with_token_cancellation() {
        let rt = SzRuntime::with_worker_threads(1);
        let token = rt.shutdown_token();
        let handle = rt.spawn(async move {
            // 模拟后台任务：监听 cancel
            token.cancelled().await;
            99
        });
        // 触发关闭
        let token2 = rt.shutdown_token();
        token2.cancel();
        let result = rt.block_on(handle).unwrap();
        assert_eq!(result, 99);
    }

    #[test]
    fn test_handle_can_spawn() {
        let rt = SzRuntime::with_worker_threads(1);
        let handle = rt.handle();
        let task = handle.spawn(async { 7 });
        let result = rt.block_on(task).unwrap();
        assert_eq!(result, 7);
    }

    #[test]
    fn test_default_impl_equals_new() {
        let rt1 = SzRuntime::default();
        let rt2 = SzRuntime::new();
        assert_eq!(rt1.worker_threads(), rt2.worker_threads());
    }

    #[test]
    fn test_multiple_runtime_instances() {
        // 验证可以创建多个独立的 runtime 实例
        let rt1 = SzRuntime::with_worker_threads(1);
        let rt2 = SzRuntime::with_worker_threads(1);
        let h1 = rt1.spawn(async { 1 });
        let h2 = rt2.spawn(async { 2 });
        assert_eq!(rt1.block_on(h1).unwrap(), 1);
        assert_eq!(rt2.block_on(h2).unwrap(), 2);
    }
}
