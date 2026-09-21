// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! `scheduler:*` 命令 — 接入 sz-orm-scheduler，支持配置文件加载
//!
//! ## 命令列表
//!
//! - `scheduler:list` — 列出配置文件中定义的调度任务
//! - `scheduler:run` — 立即执行一次到期任务
//! - `scheduler:start` — 启动调度器（持续运行）
//!
//! ## 配置文件
//!
//! 默认读取当前目录下的 `scheduler.toml`，格式：
//!
//! ```toml
//! [[tasks]]
//! id = "cleanup-logs"
//! name = "清理日志"
//! cron = "0 3 * * *"
//! callback = "demo::cleanup_logs"
//! enabled = true
//!
//! [[tasks]]
//! id = "sync-data"
//! name = "数据同步"
//! cron = "*/30 * * * *"
//! callback = "demo::sync_data"
//! ```
//!
//! ## PHP 对齐
//!
//! PHP ThinkPHP 6 通过 `think\swoole\crontab\Crontab` 注册定时任务。
//! Rust 端复用 `sz-orm_scheduler::CronScheduler`。

use std::path::PathBuf;
use std::sync::Arc;

use clap::Subcommand;
use serde::{Deserialize, Serialize};

use sz_rust_core::orm::scheduler::Scheduler;

use crate::error::CliError;

/// 默认配置文件路径
const DEFAULT_CONFIG_PATH: &str = "scheduler.toml";

/// `scheduler` 子命令枚举
#[derive(Subcommand, Debug)]
pub enum SchedulerCommand {
    /// 列出配置文件中定义的调度任务
    #[command(name = "list")]
    List {
        /// 配置文件路径（默认 scheduler.toml）
        #[arg(short = 'c', long, default_value = DEFAULT_CONFIG_PATH)]
        config: PathBuf,
    },

    /// 立即执行一次到期任务（对齐 `php think scheduler:run`）
    #[command(name = "run")]
    Run {
        /// 配置文件路径（默认 scheduler.toml）
        #[arg(short = 'c', long, default_value = DEFAULT_CONFIG_PATH)]
        config: PathBuf,
    },

    /// 启动调度器（持续运行，对齐 `php think scheduler:start`）
    #[command(name = "start")]
    Start {
        /// 调度器 tick 间隔（毫秒，默认 1000）
        #[arg(short = 't', long, default_value = "1000")]
        tick_ms: u64,

        /// 配置文件路径（默认 scheduler.toml）
        #[arg(short = 'c', long, default_value = DEFAULT_CONFIG_PATH)]
        config: PathBuf,
    },
}

/// 调度任务配置项（对应 `scheduler.toml` 中的 `[[tasks]]` 段）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskConfig {
    /// 任务 ID（唯一标识）
    pub id: String,
    /// 任务名称
    pub name: String,
    /// cron 表达式（5 字段：second minute hour day month）
    pub cron: String,
    /// 回调标识（用于 handler 注册查找）
    #[serde(default)]
    pub callback: String,
    /// 是否启用（默认 true）
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// 调度器配置文件（`scheduler.toml`）
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SchedulerConfig {
    /// 任务列表
    #[serde(default)]
    pub tasks: Vec<TaskConfig>,
}

impl SchedulerConfig {
    /// 从 TOML 字符串解析
    ///
    /// # 错误
    ///
    /// - TOML 解析失败
    /// - 任务 ID 重复
    pub fn from_toml_str(s: &str) -> Result<Self, CliError> {
        let config: SchedulerConfig = toml::from_str(s)
            .map_err(|e| CliError::Scheduler(format!("config parse error: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    /// 从文件加载
    ///
    /// # 错误
    ///
    /// - 文件读取失败
    /// - TOML 解析失败
    /// - 任务 ID 重复
    pub fn from_file(path: &std::path::Path) -> Result<Self, CliError> {
        let content = std::fs::read_to_string(path)?;
        Self::from_toml_str(&content)
    }

    /// 校验配置：检测重复 ID
    fn validate(&self) -> Result<(), CliError> {
        let mut seen = std::collections::HashSet::new();
        for task in &self.tasks {
            if !seen.insert(&task.id) {
                return Err(CliError::Scheduler(format!(
                    "duplicate task id: {}",
                    task.id
                )));
            }
        }
        Ok(())
    }

    /// 将配置中的任务注册到调度器，并注册默认的打印 handler
    ///
    /// 返回注册的任务数量。
    pub fn register_to(
        &self,
        scheduler: &sz_rust_core::orm::scheduler::CronScheduler,
    ) -> Result<usize, CliError> {
        let handler: Arc<dyn sz_rust_core::orm::scheduler::JobHandler> = Arc::new(PrintJobHandler);
        let mut count = 0;
        for task in &self.tasks {
            let scheduled =
                sz_rust_core::orm::scheduler::ScheduledTask::new(&task.id, &task.name, &task.cron)
                    .with_callback(&task.callback);
            let scheduled = if task.enabled {
                scheduled
            } else {
                scheduled.disable()
            };
            scheduler.schedule(scheduled).map_err(|e| {
                CliError::Scheduler(format!("schedule task {} failed: {e}", task.id))
            })?;
            scheduler.register_handler(&task.id, handler.clone());
            count += 1;
        }
        Ok(count)
    }
}

/// 执行 scheduler 子命令
pub fn execute(cmd: &SchedulerCommand) -> Result<(), CliError> {
    match cmd {
        SchedulerCommand::List { config } => execute_list(config),
        SchedulerCommand::Run { config } => execute_run(config),
        SchedulerCommand::Start { tick_ms, config } => execute_start(*tick_ms, config),
    }
}

/// 加载配置文件；若文件不存在则返回空配置（而非报错），便于在无配置时使用 demo 任务
fn load_config_or_empty(path: &std::path::Path) -> Result<SchedulerConfig, CliError> {
    if !path.exists() {
        return Ok(SchedulerConfig::default());
    }
    SchedulerConfig::from_file(path)
}

/// 构建调度器并加载配置
fn build_scheduler(
    config: &SchedulerConfig,
) -> Result<sz_rust_core::orm::scheduler::CronScheduler, CliError> {
    let scheduler = sz_rust_core::orm::scheduler::CronScheduler::new();
    config.register_to(&scheduler)?;
    Ok(scheduler)
}

/// 执行 scheduler:list
fn execute_list(config_path: &std::path::Path) -> Result<(), CliError> {
    let config = load_config_or_empty(config_path)?;

    if config.tasks.is_empty() {
        println!("No scheduled tasks registered.");
        println!();
        println!("To register tasks, create a scheduler configuration file at:");
        println!("  {}", config_path.display());
        println!();
        println!("Example scheduler.toml:");
        println!();
        println!("  [[tasks]]");
        println!("  id = \"cleanup-logs\"");
        println!("  name = \"清理日志\"");
        println!("  cron = \"0 3 * * *\"");
        println!("  callback = \"demo::cleanup_logs\"");
        println!("  enabled = true");
        return Ok(());
    }

    let scheduler = build_scheduler(&config)?;
    let tasks = scheduler.list_tasks();

    println!(
        "{:<20} {:<25} {:<25} {:<8}",
        "ID", "Name", "Cron", "Enabled"
    );
    println!("{}", "-".repeat(80));

    for task in &tasks {
        println!(
            "{:<20} {:<25} {:<25} {:<8}",
            task.id, task.name, task.cron_expr, task.enabled
        );
    }

    println!();
    println!("Total: {} task(s)", tasks.len());
    println!("Config: {}", config_path.display());
    Ok(())
}

/// 执行 scheduler:run
///
/// 触发一次到期任务的执行（对齐 PHP `scheduler:run`）
fn execute_run(config_path: &std::path::Path) -> Result<(), CliError> {
    let config = load_config_or_empty(config_path)?;

    if config.tasks.is_empty() {
        println!("No scheduled tasks registered. Nothing to run.");
        println!("Config: {}", config_path.display());
        return Ok(());
    }

    let scheduler = build_scheduler(&config)?;
    let now = chrono::Utc::now();

    // 输出每个任务的下次运行时间，便于调试
    println!("Scheduler run at {} (UTC)", now);
    println!();
    for task in &config.tasks {
        match scheduler.next_run_time(&task.cron, now) {
            Ok(next) => {
                println!(
                    "  {:<20} {:<25} cron={:<20} next={}",
                    task.id, task.name, task.cron, next
                );
            }
            Err(e) => {
                println!(
                    "  {:<20} {:<25} cron={:<20} next=N/A ({})",
                    task.id, task.name, task.cron, e
                );
            }
        }
    }
    println!();

    let fired = scheduler.try_fire_due(now);
    println!("Fired {} task(s) matching current time.", fired);
    println!("Config: {}", config_path.display());
    Ok(())
}

/// 执行 scheduler:start
///
/// 启动调度器并持续运行（对齐 PHP `scheduler:start`）
fn execute_start(tick_ms: u64, config_path: &std::path::Path) -> Result<(), CliError> {
    let config = load_config_or_empty(config_path)?;

    if config.tasks.is_empty() {
        return Err(CliError::Scheduler(format!(
            "no tasks to schedule. Please create config file at {}",
            config_path.display()
        )));
    }

    let scheduler = build_scheduler(&config)?;
    let task_count = config.tasks.len();

    println!("Starting scheduler with tick interval: {}ms", tick_ms);
    println!(
        "Loaded {} task(s) from {}",
        task_count,
        config_path.display()
    );
    println!("Press Ctrl+C to stop.");
    println!();

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

/// 默认任务处理器：打印任务触发信息
///
/// 实际应用中应通过 `CronScheduler::register_handler` 注册自定义 handler。
struct PrintJobHandler;

impl sz_rust_core::orm::scheduler::JobHandler for PrintJobHandler {
    fn handle(&self, task: &sz_rust_core::orm::scheduler::ScheduledTask) -> Result<(), String> {
        println!(
            "[{}] Task fired: {} ({}) callback={}",
            chrono::Utc::now(),
            task.name,
            task.id,
            if task.callback.is_empty() {
                "(none)"
            } else {
                &task.callback
            }
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_core::orm::scheduler::JobHandler;
    use tempfile::NamedTempFile;

    const SAMPLE_TOML: &str = r#"
[[tasks]]
id = "cleanup-logs"
name = "清理日志"
cron = "0 3 * * *"
callback = "demo::cleanup_logs"
enabled = true

[[tasks]]
id = "sync-data"
name = "数据同步"
cron = "*/30 * * * *"
callback = "demo::sync_data"

[[tasks]]
id = "disabled-task"
name = "已禁用任务"
cron = "0 0 * * *"
callback = "demo::disabled"
enabled = false
"#;

    #[test]
    fn test_config_from_toml_str() {
        let config = SchedulerConfig::from_toml_str(SAMPLE_TOML).unwrap();
        assert_eq!(config.tasks.len(), 3);

        assert_eq!(config.tasks[0].id, "cleanup-logs");
        assert_eq!(config.tasks[0].name, "清理日志");
        assert_eq!(config.tasks[0].cron, "0 3 * * *");
        assert_eq!(config.tasks[0].callback, "demo::cleanup_logs");
        assert!(config.tasks[0].enabled);

        // 未指定 enabled 时默认为 true
        assert!(config.tasks[1].enabled);

        // 显式禁用
        assert!(!config.tasks[2].enabled);
    }

    #[test]
    fn test_config_from_toml_str_empty() {
        let config = SchedulerConfig::from_toml_str("").unwrap();
        assert!(config.tasks.is_empty());
    }

    #[test]
    fn test_config_duplicate_id_rejected() {
        let toml_str = r#"
[[tasks]]
id = "dup"
name = "任务1"
cron = "0 * * * *"

[[tasks]]
id = "dup"
name = "任务2"
cron = "0 * * * *"
"#;
        let result = SchedulerConfig::from_toml_str(toml_str);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("duplicate task id"), "got: {err}");
    }

    #[test]
    fn test_config_invalid_toml_rejected() {
        let result = SchedulerConfig::from_toml_str("not valid toml [[[[");
        assert!(result.is_err());
    }

    #[test]
    fn test_config_from_file() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), SAMPLE_TOML).unwrap();

        let config = SchedulerConfig::from_file(tmp.path()).unwrap();
        assert_eq!(config.tasks.len(), 3);
    }

    #[test]
    fn test_config_from_file_not_found() {
        let result = SchedulerConfig::from_file(std::path::Path::new("/nonexistent/path.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_register_to_scheduler() {
        let config = SchedulerConfig::from_toml_str(SAMPLE_TOML).unwrap();
        let scheduler = sz_rust_core::orm::scheduler::CronScheduler::new();

        let count = config.register_to(&scheduler).unwrap();
        assert_eq!(count, 3);

        let tasks = scheduler.list_tasks();
        assert_eq!(tasks.len(), 3);

        // 验证 disabled 任务确实被禁用
        let disabled = tasks.iter().find(|t| t.id == "disabled-task").unwrap();
        assert!(!disabled.enabled);
    }

    #[test]
    fn test_register_to_invalid_cron_rejected() {
        let toml_str = r#"
[[tasks]]
id = "bad"
name = "坏任务"
cron = "not a cron"
"#;
        let config = SchedulerConfig::from_toml_str(toml_str).unwrap();
        let scheduler = sz_rust_core::orm::scheduler::CronScheduler::new();

        let result = config.register_to(&scheduler);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("schedule task"), "got: {err}");
    }

    #[test]
    fn test_load_config_or_empty_missing_file() {
        let config =
            load_config_or_empty(std::path::Path::new("/nonexistent/scheduler.toml")).unwrap();
        assert!(config.tasks.is_empty());
    }

    #[test]
    fn test_load_config_or_empty_existing_file() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), SAMPLE_TOML).unwrap();

        let config = load_config_or_empty(tmp.path()).unwrap();
        assert_eq!(config.tasks.len(), 3);
    }

    #[test]
    fn test_build_scheduler() {
        let config = SchedulerConfig::from_toml_str(SAMPLE_TOML).unwrap();
        let scheduler = build_scheduler(&config).unwrap();
        let tasks = scheduler.list_tasks();
        assert_eq!(tasks.len(), 3);
    }

    #[test]
    fn test_execute_list_empty_config() {
        // 使用不存在的路径，应返回空配置并打印帮助信息
        let result = execute_list(std::path::Path::new("/nonexistent/scheduler.toml"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_list_with_config() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), SAMPLE_TOML).unwrap();

        let result = execute_list(tmp.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_run_empty_config() {
        let result = execute_run(std::path::Path::new("/nonexistent/scheduler.toml"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_run_with_config() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), SAMPLE_TOML).unwrap();

        let result = execute_run(tmp.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_start_empty_config_errors() {
        let result = execute_start(1000, std::path::Path::new("/nonexistent/scheduler.toml"));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no tasks to schedule"), "got: {err}");
    }

    #[test]
    fn test_print_job_handler() {
        let handler = PrintJobHandler;
        let task = sz_rust_core::orm::scheduler::ScheduledTask::new("test", "测试", "0 * * * *")
            .with_callback("demo::test");
        let result = handler.handle(&task);
        assert!(result.is_ok());
    }

    #[test]
    fn test_print_job_handler_empty_callback() {
        let handler = PrintJobHandler;
        let task = sz_rust_core::orm::scheduler::ScheduledTask::new("test", "测试", "0 * * * *");
        let result = handler.handle(&task);
        assert!(result.is_ok());
    }

    #[test]
    fn test_task_config_default_enabled() {
        let toml_str = r#"
[[tasks]]
id = "t1"
name = "任务"
cron = "0 * * * *"
"#;
        let config = SchedulerConfig::from_toml_str(toml_str).unwrap();
        assert!(config.tasks[0].enabled, "enabled should default to true");
    }

    #[test]
    fn test_config_serialize_roundtrip() {
        let config = SchedulerConfig::from_toml_str(SAMPLE_TOML).unwrap();
        let toml_str = toml::to_string(&config).unwrap();
        let config2 = SchedulerConfig::from_toml_str(&toml_str).unwrap();
        assert_eq!(config, config2);
    }

    #[test]
    fn test_execute_dispatch_list() {
        let cmd = SchedulerCommand::List {
            config: PathBuf::from("/nonexistent/scheduler.toml"),
        };
        let result = execute(&cmd);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_dispatch_run() {
        let cmd = SchedulerCommand::Run {
            config: PathBuf::from("/nonexistent/scheduler.toml"),
        };
        let result = execute(&cmd);
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_dispatch_start_empty_config() {
        let cmd = SchedulerCommand::Start {
            tick_ms: 1000,
            config: PathBuf::from("/nonexistent/scheduler.toml"),
        };
        let result = execute(&cmd);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no tasks to schedule"));
    }

    #[test]
    fn test_execute_run_with_invalid_cron() {
        // 包含无效 cron 的配置应触发 next_run_time 错误分支
        let tmp = NamedTempFile::new().unwrap();
        let toml_str = r#"
[[tasks]]
id = "bad-cron"
name = "坏 cron"
cron = "not a valid cron"
callback = "demo::bad"
"#;
        std::fs::write(tmp.path(), toml_str).unwrap();

        let result = execute_run(tmp.path());
        // 无效 cron 在 schedule 时就会报错，build_scheduler 返回 Err
        assert!(result.is_err());
    }
}
