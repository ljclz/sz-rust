//! `scheduler:*` 命令 — 接入 sz-orm-scheduler
//!
//! ## 命令列表
//!
//! - `scheduler:list` — 列出已注册的调度任务
//! - `scheduler:run` — 立即执行一次到期任务
//! - `scheduler:start` — 启动调度器（持续运行）
//!
//! ## PHP 对齐
//!
//! PHP ThinkPHP 6 通过 `think\swoole\crontab\Crontab` 注册定时任务。
//! Rust 端复用 `sz-orm-scheduler::CronScheduler`。

use std::sync::Arc;

use clap::Subcommand;

use sz_orm_scheduler::Scheduler;

use crate::error::CliError;

/// `scheduler` 子命令枚举
#[derive(Subcommand, Debug)]
pub enum SchedulerCommand {
    /// 列出已注册的调度任务
    #[command(name = "list")]
    List,

    /// 立即执行一次到期任务（对齐 `php think scheduler:run`）
    #[command(name = "run")]
    Run,

    /// 启动调度器（持续运行，对齐 `php think scheduler:start`）
    #[command(name = "start")]
    Start {
        /// 调度器 tick 间隔（毫秒，默认 1000）
        #[arg(short = 't', long, default_value = "1000")]
        tick_ms: u64,
    },
}

/// 执行 scheduler 子命令
pub fn execute(cmd: &SchedulerCommand) -> Result<(), CliError> {
    let scheduler = sz_orm_scheduler::CronScheduler::new();

    match cmd {
        SchedulerCommand::List => execute_list(&scheduler),
        SchedulerCommand::Run => execute_run(&scheduler),
        SchedulerCommand::Start { tick_ms } => execute_start(&scheduler, *tick_ms),
    }
}

/// 执行 scheduler:list
fn execute_list(scheduler: &sz_orm_scheduler::CronScheduler) -> Result<(), CliError> {
    let tasks = scheduler.list_tasks();

    if tasks.is_empty() {
        println!("No scheduled tasks registered.");
        println!();
        println!("To register tasks, create a scheduler configuration file");
        println!("or use the scheduler API in your application code.");
        return Ok(());
    }

    println!(
        "{:<15} {:<20} {:<25} {:<8}",
        "ID", "Name", "Cron", "Enabled"
    );
    println!("{}", "-".repeat(70));

    for task in &tasks {
        println!(
            "{:<15} {:<20} {:<25} {:<8}",
            task.id, task.name, task.cron_expr, task.enabled
        );
    }

    println!();
    println!("Total: {} task(s)", tasks.len());
    Ok(())
}

/// 执行 scheduler:run
///
/// 触发一次到期任务的执行（对齐 PHP `scheduler:run`）
fn execute_run(scheduler: &sz_orm_scheduler::CronScheduler) -> Result<(), CliError> {
    let now = chrono::Utc::now();
    let fired = scheduler.try_fire_due(now);
    println!("Scheduler run: {} task(s) fired at {}", fired, now);
    Ok(())
}

/// 执行 scheduler:start
///
/// 启动调度器并持续运行（对齐 PHP `scheduler:start`）
fn execute_start(
    scheduler: &sz_orm_scheduler::CronScheduler,
    tick_ms: u64,
) -> Result<(), CliError> {
    println!("Starting scheduler with tick interval: {}ms", tick_ms);
    println!("Press Ctrl+C to stop.");

    // 注册示例任务（演示用途）
    register_demo_tasks(scheduler)?;

    // 启动调度器
    scheduler
        .start(tick_ms)
        .map_err(|e| CliError::Scheduler(e.to_string()))?;

    println!("Scheduler started. Waiting for tasks to fire...");

    // 阻塞主线程，直到收到 Ctrl+C 信号
    // 注意：这是简化实现，生产环境应使用 tokio::signal::ctrl_c()
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// 注册演示任务
///
/// 展示调度器功能，实际应用中应由用户代码注册。
fn register_demo_tasks(scheduler: &sz_orm_scheduler::CronScheduler) -> Result<(), CliError> {
    // 示例任务 1：每分钟执行（5 字段 cron：second minute hour day month）
    let task1 = sz_orm_scheduler::ScheduledTask::new("demo-1", "每分钟任务", "0 * * * *")
        .with_callback("demo::minute_task");
    scheduler
        .schedule(task1)
        .map_err(|e| CliError::Scheduler(e.to_string()))?;

    // 示例任务 2：每小时执行
    let task2 = sz_orm_scheduler::ScheduledTask::new("demo-2", "每小时任务", "0 0 * * *")
        .with_callback("demo::hourly_task");
    scheduler
        .schedule(task2)
        .map_err(|e| CliError::Scheduler(e.to_string()))?;

    // 注册处理器
    let handler: Arc<dyn sz_orm_scheduler::JobHandler> = Arc::new(DemoJobHandler);
    scheduler.register_handler("demo-1", handler.clone());
    scheduler.register_handler("demo-2", handler);

    println!("Registered 2 demo tasks (demo-1, demo-2)");
    Ok(())
}

/// 演示任务处理器
struct DemoJobHandler;

impl sz_orm_scheduler::JobHandler for DemoJobHandler {
    fn handle(&self, task: &sz_orm_scheduler::ScheduledTask) -> Result<(), String> {
        println!(
            "[{}] Task fired: {} ({})",
            chrono::Utc::now(),
            task.name,
            task.id
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_orm_scheduler::JobHandler;
    use sz_orm_scheduler::Scheduler;

    #[test]
    fn test_execute_list_empty() {
        let scheduler = sz_orm_scheduler::CronScheduler::new();
        let result = execute_list(&scheduler);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_list_with_tasks() {
        let scheduler = sz_orm_scheduler::CronScheduler::new();
        let task = sz_orm_scheduler::ScheduledTask::new("test-1", "测试任务", "0 * * * *");
        scheduler.schedule(task).unwrap();

        let result = execute_list(&scheduler);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_run() {
        let scheduler = sz_orm_scheduler::CronScheduler::new();
        let result = execute_run(&scheduler);
        assert!(result.is_ok());
    }

    #[test]
    fn test_register_demo_tasks() {
        let scheduler = sz_orm_scheduler::CronScheduler::new();
        let result = register_demo_tasks(&scheduler);
        assert!(result.is_ok());

        let tasks = scheduler.list_tasks();
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn test_register_demo_tasks_with_invalid_cron() {
        let scheduler = sz_orm_scheduler::CronScheduler::new();
        // 直接测试无效 cron 表达式的 schedule 调用
        let task = sz_orm_scheduler::ScheduledTask::new("bad", "bad", "");
        let result = scheduler.schedule(task);
        assert!(result.is_err());
    }

    #[test]
    fn test_demo_job_handler() {
        let handler = DemoJobHandler;
        let task = sz_orm_scheduler::ScheduledTask::new("test", "测试", "0 * * * *");
        let result = handler.handle(&task);
        assert!(result.is_ok());
    }
}
