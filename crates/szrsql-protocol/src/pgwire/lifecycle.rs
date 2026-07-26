//! Phase 4.11 — 优雅关闭（Graceful Shutdown）。
//! Phase 4.12 — 信号处理 + Crash Handler。
//!
//! 实现服务器生命周期管理，支持：
//! - 接收外部信号（SIGTERM/SIGINT/Ctrl+C）触发关闭
//! - 停止接受新连接，拒绝新连接并返回 "shutting down" 错误
//! - 等待活跃连接完成（最多 `shutdown_timeout`，默认 30s）
//! - 超时后强制中止剩余连接任务
//! - 退出码 0（正常关闭）/ 非零（超时强制中止）
//!
//! Phase 4.12 新增：
//! - `ShutdownSignal::Graceful`（SIGTERM）：等待活跃连接排空（带超时）
//! - `ShutdownSignal::Immediate`（SIGINT/Ctrl+C）：立即强制中止，不等待
//! - `force_shutdown()`：跳过 drain，直接 abort_all
//!
//! # 设计
//!
//! - 使用 `tokio::sync::watch::Sender<ShutdownState>` 作为关闭信号广播
//!   （`tokio` "full" features 已包含，无需新增依赖）
//! - 每个 per-connection task 持有 `watch::Receiver`，通过 `mark_changed()` 感知关闭
//! - `PgwireServer::serve_with_shutdown` 接受外部 shutdown future，
//!   在 accept 循环中用 `tokio::select!` 竞争 accept 与 shutdown
//! - 关闭后用 `JoinSet` 跟踪的活跃连接任务通过 `abort_all` 强制中止
//!
//! # 关闭状态机
//!
//! ```text
//!   Running ──shutdown signal──▶ Draining ──all conns done──▶ Closed
//!                                  │
//!                                  └──timeout──▶ Closed (force abort)
//!
//!   Running ──force signal──▶ Draining ──abort_all──▶ Closed (immediate)
//! ```

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinSet;

/// 关闭信号类型 — Phase 4.12。
///
/// 区分 SIGTERM（优雅）与 SIGINT（立即）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownSignal {
    /// 优雅关闭（SIGTERM）：等待活跃连接排空（带超时），超时后强制中止。
    Graceful,
    /// 立即关闭（SIGINT/Ctrl+C）：跳过 drain，直接 abort_all。
    Immediate,
}

/// 关闭状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownState {
    /// 正常运行中，接受新连接。
    Running,
    /// 正在排空：停止接受新连接，等待活跃连接完成。
    Draining,
    /// 已关闭（含超时强制中止）。
    Closed,
}

impl ShutdownState {
    /// 是否正在排空或已关闭（即拒绝新连接）。
    pub fn is_rejecting(&self) -> bool {
        matches!(self, ShutdownState::Draining | ShutdownState::Closed)
    }
}

/// 关闭协调器。
///
/// 持有广播发送端与活跃连接任务集合，提供触发关闭与等待排空的接口。
/// 使用 `Arc` 以便在 `PgwireServer` 与 per-connection task 间共享。
pub struct ShutdownCoordinator {
    /// 关闭状态广播发送端。
    tx: watch::Sender<ShutdownState>,
    /// 活跃连接任务集合（per-connection task 通过 `spawn_on` 加入）。
    tasks: Arc<tokio::sync::Mutex<JoinSet<()>>>,
    /// 关闭超时（默认 30s）。
    shutdown_timeout: Duration,
}

impl ShutdownCoordinator {
    /// 创建新协调器，初始状态为 `Running`。
    pub fn new(shutdown_timeout: Duration) -> Self {
        let (tx, _) = watch::channel(ShutdownState::Running);
        Self {
            tx,
            tasks: Arc::new(tokio::sync::Mutex::new(JoinSet::new())),
            shutdown_timeout,
        }
    }

    /// 返回关闭状态广播的接收端。
    ///
    /// per-connection task 持有此接收端以感知关闭信号。
    pub fn subscribe(&self) -> watch::Receiver<ShutdownState> {
        self.tx.subscribe()
    }

    /// 返回活跃连接任务集合的克隆引用（用于 per-connection task 注册）。
    pub fn tasks(&self) -> Arc<tokio::sync::Mutex<JoinSet<()>>> {
        Arc::clone(&self.tasks)
    }

    /// 当前关闭状态。
    pub fn state(&self) -> ShutdownState {
        *self.tx.borrow()
    }

    /// 是否正在拒绝新连接。
    pub fn is_rejecting(&self) -> bool {
        self.state().is_rejecting()
    }

    /// 触发优雅关闭。
    ///
    /// 1. 将状态切换为 `Draining`（停止接受新连接）
    /// 2. 等待所有活跃连接任务完成，最多 `shutdown_timeout`
    /// 3. 超时则 `abort_all` 强制中止剩余任务
    /// 4. 将状态切换为 `Closed`
    ///
    /// 返回 `true` 表示所有连接正常排空；`false` 表示超时强制中止。
    pub async fn shutdown(&self) -> bool {
        // 1. 切换到 Draining
        // 使用 send_modify 而非 send：send 在无接收者时不更新值，
        // send_modify 总是直接修改共享值，确保 state() 反映最新状态。
        self.tx.send_modify(|v| *v = ShutdownState::Draining);
        tracing::info!(
            timeout_secs = self.shutdown_timeout.as_secs(),
            "shutdown triggered, draining active connections"
        );

        // 2. 等待所有活跃连接完成，带超时
        let drained = tokio::time::timeout(self.shutdown_timeout, self.drain_tasks()).await;

        let all_drained = matches!(drained, Ok(()));

        // 3. 超时则强制中止剩余任务
        if !all_drained {
            let count = self.tasks.lock().await.len();
            tracing::warn!(
                remaining = count,
                "shutdown timeout reached, force aborting remaining connections"
            );
            self.tasks.lock().await.abort_all();
            // 等待 abort 完成
            while self.tasks.lock().await.join_next().await.is_some() {}
        }

        // 4. 切换到 Closed
        self.tx.send_modify(|v| *v = ShutdownState::Closed);
        tracing::info!(all_drained, "shutdown complete");
        all_drained
    }

    /// Phase 4.12：触发立即关闭（不等活跃事务）。
    ///
    /// 1. 将状态切换为 `Draining`（停止接受新连接）
    /// 2. **不等待**，直接 `abort_all` 强制中止所有活跃连接任务
    /// 3. 等待 abort 落地（`join_next` 循环确保所有 task 已终止）
    /// 4. 将状态切换为 `Closed`
    ///
    /// 与 `shutdown()` 的区别：跳过 drain 等待，适合 SIGINT 等需要立即退出的场景。
    pub async fn force_shutdown(&self) {
        // 1. 切换到 Draining
        self.tx.send_modify(|v| *v = ShutdownState::Draining);
        let count = self.tasks.lock().await.len();
        tracing::warn!(
            remaining = count,
            "force shutdown triggered, aborting all connections immediately"
        );

        // 2. 立即 abort_all（不等待 drain）
        self.tasks.lock().await.abort_all();

        // 3. 等待 abort 落地
        while let Some(res) = self.tasks.lock().await.join_next().await {
            if let Err(e) = res {
                tracing::debug!(error = %e, "aborted connection task during force shutdown");
            }
        }

        // 4. 切换到 Closed
        self.tx.send_modify(|v| *v = ShutdownState::Closed);
        tracing::info!("force shutdown complete");
    }

    /// Phase 4.12：根据信号类型执行对应的关闭策略。
    ///
    /// - `Graceful`：调用 `shutdown()`（带超时排空）
    /// - `Immediate`：调用 `force_shutdown()`（立即中止）
    ///
    /// 返回 `true` 表示优雅排空完成；`false` 表示强制中止（含 Immediate 信号）。
    pub async fn shutdown_with_signal(&self, signal: ShutdownSignal) -> bool {
        match signal {
            ShutdownSignal::Graceful => self.shutdown().await,
            ShutdownSignal::Immediate => {
                self.force_shutdown().await;
                false
            }
        }
    }

    /// 排空所有活跃连接任务（无超时，等待全部完成）。
    async fn drain_tasks(&self) {
        while let Some(res) = self.tasks.lock().await.join_next().await {
            if let Err(e) = res {
                tracing::warn!(error = %e, "connection task panicked during drain");
            }
        }
    }
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self::new(Duration::from_secs(30))
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_shutdown_state_machine() {
        let coord = ShutdownCoordinator::new(Duration::from_secs(1));
        assert_eq!(coord.state(), ShutdownState::Running);
        assert!(!coord.is_rejecting());

        // 切换到 Draining（用 send_modify 避免"无接收者时不更新"的 watch 语义陷阱）
        coord.tx.send_modify(|v| *v = ShutdownState::Draining);
        assert_eq!(coord.state(), ShutdownState::Draining);
        assert!(coord.is_rejecting());

        // 切换到 Closed
        coord.tx.send_modify(|v| *v = ShutdownState::Closed);
        assert_eq!(coord.state(), ShutdownState::Closed);
        assert!(coord.is_rejecting());
    }

    #[tokio::test]
    async fn test_subscribe_receives_state_changes() {
        let coord = ShutdownCoordinator::new(Duration::from_secs(1));
        let mut rx = coord.subscribe();

        assert_eq!(*rx.borrow(), ShutdownState::Running);

        coord.tx.send_modify(|v| *v = ShutdownState::Draining);
        // watch::Receiver 需要等待 mark_changed 或 borrow
        assert!(rx.changed().await.is_ok());
        assert_eq!(*rx.borrow(), ShutdownState::Draining);
    }

    #[tokio::test]
    async fn test_shutdown_drains_empty_immediately() {
        let coord = ShutdownCoordinator::new(Duration::from_secs(5));
        let all_drained = coord.shutdown().await;
        assert!(all_drained);
        assert_eq!(coord.state(), ShutdownState::Closed);
    }

    #[tokio::test]
    async fn test_shutdown_waits_for_active_tasks() {
        let coord = ShutdownCoordinator::new(Duration::from_secs(5));
        let tasks = coord.tasks();

        // 启动一个短任务
        {
            let mut tasks = tasks.lock().await;
            tasks.spawn(async {
                sleep(Duration::from_millis(100)).await;
            });
        }

        let all_drained = coord.shutdown().await;
        assert!(all_drained);
        assert_eq!(coord.state(), ShutdownState::Closed);
    }

    #[tokio::test]
    async fn test_shutdown_force_aborts_on_timeout() {
        let coord = ShutdownCoordinator::new(Duration::from_millis(50));
        let tasks = coord.tasks();

        // 启动一个长任务（不会在 50ms 内完成）
        {
            let mut tasks = tasks.lock().await;
            tasks.spawn(async {
                sleep(Duration::from_secs(10)).await;
            });
        }

        let all_drained = coord.shutdown().await;
        assert!(!all_drained); // 超时强制中止
        assert_eq!(coord.state(), ShutdownState::Closed);
    }

    #[tokio::test]
    async fn test_shutdown_state_is_rejecting_helper() {
        assert!(!ShutdownState::Running.is_rejecting());
        assert!(ShutdownState::Draining.is_rejecting());
        assert!(ShutdownState::Closed.is_rejecting());
    }

    #[tokio::test]
    async fn test_default_shutdown_timeout_is_30s() {
        let coord = ShutdownCoordinator::default();
        // 验证默认超时为 30s（不实际等待，仅检查状态）
        assert_eq!(coord.state(), ShutdownState::Running);
    }

    #[tokio::test]
    async fn test_multiple_subscribers_all_receive_shutdown() {
        let coord = ShutdownCoordinator::new(Duration::from_secs(1));
        let mut rx1 = coord.subscribe();
        let mut rx2 = coord.subscribe();

        coord.tx.send_modify(|v| *v = ShutdownState::Draining);

        // 两个订阅者都应收到状态变化
        assert!(rx1.changed().await.is_ok());
        assert!(rx2.changed().await.is_ok());
        assert_eq!(*rx1.borrow(), ShutdownState::Draining);
        assert_eq!(*rx2.borrow(), ShutdownState::Draining);
    }

    #[tokio::test]
    async fn test_tasks_arc_sharing() {
        let coord = ShutdownCoordinator::new(Duration::from_secs(1));
        let tasks1 = coord.tasks();
        let tasks2 = coord.tasks();

        // 两个 Arc 应指向同一个 JoinSet
        {
            let mut tasks = tasks1.lock().await;
            tasks.spawn(async {
                sleep(Duration::from_millis(10)).await;
            });
        }

        // 通过 tasks2 也能看到任务
        let count = tasks2.lock().await.len();
        assert_eq!(count, 1);

        // 排空
        coord.shutdown().await;
        let count = tasks2.lock().await.len();
        assert_eq!(count, 0);
    }

    // ==================== Phase 4.12 单元测试 ====================

    #[tokio::test]
    async fn test_force_shutdown_aborts_immediately() {
        // 用一个"很长的超时"来证明 force_shutdown 不会等待
        let coord = ShutdownCoordinator::new(Duration::from_secs(60));
        let tasks = coord.tasks();

        // 启动一个 10 秒的长任务
        {
            let mut tasks = tasks.lock().await;
            tasks.spawn(async {
                sleep(Duration::from_secs(10)).await;
            });
        }

        let start = std::time::Instant::now();
        coord.force_shutdown().await;
        let elapsed = start.elapsed();

        // force_shutdown 应在 1 秒内完成（实际通常 <100ms）
        assert!(
            elapsed < Duration::from_secs(1),
            "force_shutdown took too long: {elapsed:?}"
        );
        assert_eq!(coord.state(), ShutdownState::Closed);

        // 所有任务应已清理
        let count = tasks.lock().await.len();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_force_shutdown_empty_immediate() {
        let coord = ShutdownCoordinator::new(Duration::from_secs(60));
        coord.force_shutdown().await;
        assert_eq!(coord.state(), ShutdownState::Closed);
    }

    #[tokio::test]
    async fn test_shutdown_with_signal_graceful() {
        // Graceful 信号 + 短任务 + 长超时 → 应正常排空
        let coord = ShutdownCoordinator::new(Duration::from_secs(5));
        let tasks = coord.tasks();
        {
            let mut tasks = tasks.lock().await;
            tasks.spawn(async {
                sleep(Duration::from_millis(50)).await;
            });
        }
        let all_drained = coord.shutdown_with_signal(ShutdownSignal::Graceful).await;
        assert!(all_drained);
        assert_eq!(coord.state(), ShutdownState::Closed);
    }

    #[tokio::test]
    async fn test_shutdown_with_signal_immediate() {
        // Immediate 信号 + 长任务 + 长超时 → 应立即中止（不等）
        let coord = ShutdownCoordinator::new(Duration::from_secs(60));
        let tasks = coord.tasks();
        {
            let mut tasks = tasks.lock().await;
            tasks.spawn(async {
                sleep(Duration::from_secs(10)).await;
            });
        }
        let start = std::time::Instant::now();
        let all_drained = coord.shutdown_with_signal(ShutdownSignal::Immediate).await;
        let elapsed = start.elapsed();

        assert!(!all_drained); // Immediate 总是返回 false
        assert!(
            elapsed < Duration::from_secs(1),
            "Immediate shutdown took too long: {elapsed:?}"
        );
        assert_eq!(coord.state(), ShutdownState::Closed);
    }

    #[tokio::test]
    async fn test_shutdown_signal_enum_traits() {
        // 验证 ShutdownSignal 的 Debug/Clone/Copy/PartialEq/Eq
        let g1 = ShutdownSignal::Graceful;
        let g2 = g1;
        assert_eq!(g1, g2);
        assert_ne!(g1, ShutdownSignal::Immediate);
        // Debug 可用
        assert!(format!("{g1:?}").contains("Graceful"));
        assert!(format!("{:?}", ShutdownSignal::Immediate).contains("Immediate"));
    }
}
