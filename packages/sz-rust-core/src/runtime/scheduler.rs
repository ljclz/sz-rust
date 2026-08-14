//! 定时任务调度接入
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `think-swoole` 的 Crontab 模型（注：think-swoole 本身不含 Crontab，
//! 由独立包 `topthink/think-swoole-crontab` 提供）：
//!
//! ```php
//! $cron = new \think\swoole\crontab\Crontab();
//! $cron->add('task-1', '* * * * *', function() { /* ... */ });
//! $cron->run();
//! ```
//!
//! Rust 端复用 `sz_orm_scheduler::CronScheduler`，但**不使用其 `start()` 方法**
//! （因为 `CronScheduler::start()` 内部用 `std::thread::spawn`，与 tokio 不兼容）。
//!
//! ## 设计
//!
//! - `SchedulerRuntime`：封装 `CronScheduler`，用 `tokio::time::interval` + `try_fire_due()` 重写循环
//! - 监听 `CancellationToken` 优雅退出
//! - 保留 `CronScheduler` 原有 API（schedule/cancel/pause/resume/list_tasks）

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::orm::{CronScheduler, JobHandler, ScheduledTask, Scheduler, SchedulerError};

/// 调度器运行时配置
#[derive(Debug, Clone)]
pub struct SchedulerRuntimeConfig {
    /// 调度器 tick 间隔（毫秒，对齐 `CronScheduler::start(tick_ms)`）
    pub tick_ms: u64,
}

impl Default for SchedulerRuntimeConfig {
    fn default() -> Self {
        Self { tick_ms: 1000 }
    }
}

impl SchedulerRuntimeConfig {
    /// 创建新配置
    pub fn new(tick_ms: u64) -> Self {
        Self {
            tick_ms: tick_ms.max(1),
        }
    }
}

/// 调度器运行时
///
/// 封装 `sz_orm_scheduler::CronScheduler`，提供 tokio 兼容的调度循环。
///
/// ## 设计
///
/// - **不调用 `CronScheduler::start()`**（因为它用 `std::thread::spawn`，会占用阻塞线程）
/// - 改用 `tokio::time::interval` + `try_fire_due()` 在 tokio 任务中循环
/// - 监听 `CancellationToken` 优雅退出
/// - 保留 `CronScheduler` 的所有 API（schedule/cancel/pause/resume/list_tasks/register_handler）
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_core::runtime::scheduler::{SchedulerRuntime, SchedulerRuntimeConfig};
/// use sz_orm_scheduler::ScheduledTask;
/// use tokio_util::sync::CancellationToken;
///
/// let runtime = SchedulerRuntime::new(SchedulerRuntimeConfig::default());
/// let task = ScheduledTask::new("task-1", "每分钟任务", "0 * * * *")
///     .with_callback("demo::minute_task");
/// runtime.schedule(task).unwrap();
///
/// let token = CancellationToken::new();
/// let handle = runtime.start(token.clone());
/// // ... 业务运行 ...
/// token.cancel();
/// let _ = handle.await;
/// ```
pub struct SchedulerRuntime {
    config: SchedulerRuntimeConfig,
    scheduler: Arc<CronScheduler>,
}

impl SchedulerRuntime {
    /// 创建调度器运行时
    pub fn new(config: SchedulerRuntimeConfig) -> Self {
        Self {
            config,
            scheduler: Arc::new(CronScheduler::new()),
        }
    }

    /// 启动调度循环（返回 JoinHandle，调用方持有）
    ///
    /// ## 行为
    ///
    /// 1. 每 `tick_ms` 毫秒调用 `scheduler.try_fire_due(now)` 触发到期任务
    /// 2. 监听 `token.cancelled()`，收到信号后停止循环
    /// 3. 不调用 `CronScheduler::stop()`（避免 `JoinHandle::join()` 阻塞）
    pub fn start(&self, token: CancellationToken) -> tokio::task::JoinHandle<()> {
        let scheduler = self.scheduler.clone();
        let tick_interval = Duration::from_millis(self.config.tick_ms);

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(tick_interval);
            // 首次 tick 立即完成（tokio::time::interval 默认行为）
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = ticker.tick() => {
                        let now = chrono::Utc::now();
                        let fired = scheduler.try_fire_due(now);
                        if fired > 0 {
                            tracing::debug!("scheduler fired {} task(s) at {}", fired, now);
                        }
                    }
                }
            }
        })
    }

    /// 注册调度任务（委托给 `CronScheduler::schedule`）
    pub fn schedule(&self, task: ScheduledTask) -> Result<(), SchedulerError> {
        self.scheduler.schedule(task)
    }

    /// 取消任务（委托给 `CronScheduler::cancel`）
    pub fn cancel(&self, task_id: &str) -> Result<(), SchedulerError> {
        self.scheduler.cancel(task_id)
    }

    /// 暂停任务（委托给 `CronScheduler::pause`）
    pub fn pause(&self, task_id: &str) -> Result<(), SchedulerError> {
        self.scheduler.pause(task_id)
    }

    /// 恢复任务（委托给 `CronScheduler::resume`）
    pub fn resume(&self, task_id: &str) -> Result<(), SchedulerError> {
        self.scheduler.resume(task_id)
    }

    /// 列出所有任务（委托给 `CronScheduler::list_tasks`）
    pub fn list_tasks(&self) -> Vec<ScheduledTask> {
        self.scheduler.list_tasks()
    }

    /// 注册任务处理器（委托给 `CronScheduler::register_handler`）
    pub fn register_handler(&self, task_id: impl Into<String>, handler: Arc<dyn JobHandler>) {
        self.scheduler.register_handler(task_id, handler);
    }

    /// 手动触发一次到期任务检查（对齐 `scheduler:run` 命令）
    pub fn try_fire_due(&self) -> usize {
        let now = chrono::Utc::now();
        self.scheduler.try_fire_due(now)
    }

    /// 获取任务数量
    pub fn task_count(&self) -> usize {
        self.scheduler.list_tasks().len()
    }

    /// 获取配置
    pub fn config(&self) -> &SchedulerRuntimeConfig {
        &self.config
    }

    /// 获取内部 CronScheduler 引用（用于直接调用未暴露的方法）
    pub fn scheduler(&self) -> &CronScheduler {
        &self.scheduler
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 测试用 JobHandler：原子计数器
    struct CounterHandler {
        counter: Arc<AtomicUsize>,
    }

    impl CounterHandler {
        fn new() -> (Self, Arc<AtomicUsize>) {
            let counter = Arc::new(AtomicUsize::new(0));
            let handler = Self {
                counter: counter.clone(),
            };
            (handler, counter)
        }
    }

    impl JobHandler for CounterHandler {
        fn handle(&self, _task: &ScheduledTask) -> Result<(), String> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn test_scheduler_runtime_config_default() {
        let config = SchedulerRuntimeConfig::default();
        assert_eq!(config.tick_ms, 1000);
    }

    #[test]
    fn test_scheduler_runtime_config_custom() {
        let config = SchedulerRuntimeConfig::new(500);
        assert_eq!(config.tick_ms, 500);
    }

    #[test]
    fn test_scheduler_runtime_config_zero_clamped() {
        let config = SchedulerRuntimeConfig::new(0);
        assert_eq!(config.tick_ms, 1);
    }

    #[test]
    fn test_schedule_task() {
        let runtime = SchedulerRuntime::new(SchedulerRuntimeConfig::default());
        let task = ScheduledTask::new("task-1", "测试任务", "0 * * * *");
        runtime.schedule(task).unwrap();
        assert_eq!(runtime.task_count(), 1);
    }

    #[test]
    fn test_schedule_multiple_tasks() {
        let runtime = SchedulerRuntime::new(SchedulerRuntimeConfig::default());
        runtime
            .schedule(ScheduledTask::new("t1", "任务1", "0 * * * *"))
            .unwrap();
        runtime
            .schedule(ScheduledTask::new("t2", "任务2", "0 0 * * *"))
            .unwrap();
        runtime
            .schedule(ScheduledTask::new("t3", "任务3", "0 0 0 * *"))
            .unwrap();
        assert_eq!(runtime.task_count(), 3);
    }

    #[test]
    fn test_cancel_task() {
        let runtime = SchedulerRuntime::new(SchedulerRuntimeConfig::default());
        runtime
            .schedule(ScheduledTask::new("task-1", "测试", "0 * * * *"))
            .unwrap();
        assert_eq!(runtime.task_count(), 1);

        runtime.cancel("task-1").unwrap();
        assert_eq!(runtime.task_count(), 0);
    }

    #[test]
    fn test_cancel_nonexistent_task() {
        let runtime = SchedulerRuntime::new(SchedulerRuntimeConfig::default());
        let result = runtime.cancel("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_pause_resume_task() {
        let runtime = SchedulerRuntime::new(SchedulerRuntimeConfig::default());
        runtime
            .schedule(ScheduledTask::new("task-1", "测试", "0 * * * *"))
            .unwrap();

        runtime.pause("task-1").unwrap();
        let tasks = runtime.list_tasks();
        assert!(!tasks[0].enabled);

        runtime.resume("task-1").unwrap();
        let tasks = runtime.list_tasks();
        assert!(tasks[0].enabled);
    }

    #[test]
    fn test_list_tasks() {
        let runtime = SchedulerRuntime::new(SchedulerRuntimeConfig::default());
        runtime
            .schedule(ScheduledTask::new("t1", "任务1", "0 * * * *"))
            .unwrap();
        runtime
            .schedule(ScheduledTask::new("t2", "任务2", "0 0 * * *"))
            .unwrap();

        let tasks = runtime.list_tasks();
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn test_register_handler() {
        let runtime = SchedulerRuntime::new(SchedulerRuntimeConfig::default());
        runtime
            .schedule(ScheduledTask::new("task-1", "测试", "0 * * * *"))
            .unwrap();

        let (handler, _counter) = CounterHandler::new();
        runtime.register_handler("task-1", Arc::new(handler));
        // 注册成功即可，不验证内部状态
    }

    #[tokio::test]
    async fn test_start_and_cancel() {
        let runtime = SchedulerRuntime::new(SchedulerRuntimeConfig::new(10));
        let token = CancellationToken::new();
        let handle = runtime.start(token.clone());

        // 等待几次 tick
        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();

        // 任务应该退出
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "scheduler task should stop on cancel");
    }

    #[tokio::test]
    async fn test_scheduler_fires_due_task() {
        let runtime = SchedulerRuntime::new(SchedulerRuntimeConfig::new(10));
        // 注册一个每秒触发的任务（5 字段 cron：second minute hour day month）
        // "*" 表示每秒都触发
        runtime
            .schedule(ScheduledTask::new("every-second", "每秒任务", "* * * * *"))
            .unwrap();

        let (handler, counter) = CounterHandler::new();
        runtime.register_handler("every-second", Arc::new(handler));

        let token = CancellationToken::new();
        let handle = runtime.start(token.clone());

        // 等待足够时间让任务触发
        tokio::time::sleep(Duration::from_millis(100)).await;
        token.cancel();
        let _ = handle.await;

        // 至少触发 1 次
        assert!(
            counter.load(Ordering::SeqCst) >= 1,
            "task should have fired at least once"
        );
    }

    #[test]
    fn test_try_fire_due_manual() {
        let runtime = SchedulerRuntime::new(SchedulerRuntimeConfig::default());
        // 注册一个每秒触发的任务
        runtime
            .schedule(ScheduledTask::new("every-second", "每秒任务", "* * * * *"))
            .unwrap();

        let (handler, counter) = CounterHandler::new();
        runtime.register_handler("every-second", Arc::new(handler));

        // 手动触发
        let fired = runtime.try_fire_due();
        assert!(fired >= 1);
        assert!(counter.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn test_try_fire_due_no_tasks() {
        let runtime = SchedulerRuntime::new(SchedulerRuntimeConfig::default());
        let fired = runtime.try_fire_due();
        assert_eq!(fired, 0);
    }

    #[test]
    fn test_config_accessor() {
        let runtime = SchedulerRuntime::new(SchedulerRuntimeConfig::new(250));
        assert_eq!(runtime.config().tick_ms, 250);
    }

    #[test]
    fn test_scheduler_accessor() {
        let runtime = SchedulerRuntime::new(SchedulerRuntimeConfig::default());
        let scheduler = runtime.scheduler();
        // 验证可以获取内部引用且状态可查询（初始无任务、可触发零个到期任务）
        assert!(scheduler.list_tasks().is_empty(), "初始任务列表应为空");
        let fired = scheduler.try_fire_due(chrono::Utc::now());
        assert_eq!(fired, 0, "无任务时不应触发任何项");
    }
}
