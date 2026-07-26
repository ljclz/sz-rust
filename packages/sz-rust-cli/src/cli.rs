//! CLI 命令定义 — 基于 clap derive
//!
//! 对齐 PHP ThinkPHP 6 `think` 命令体系，借鉴 Laravel Artisan 风格。
//!
//! ## PHP 对齐
//!
//! PHP `think` 命令使用 Symfony Console 组件，通过 `configure()` 定义参数和选项。
//! Rust 端使用 clap derive 宏，通过结构体字段定义参数和选项。
//!
//! ## 命令对照表
//!
//! | PHP | Rust | 说明 |
//! |-----|------|------|
//! | `php think make:model User` | `sz-rust make:model User` | 生成 Model |
//! | `php think make:controller User` | `sz-rust make:controller User` | 生成 Controller |
//! | `php think make:migration CreateUsers` | `sz-rust make:migration create_users` | 生成迁移文件 |
//! | `php think migrate` | `sz-rust migrate` | 执行迁移 |
//! | `php think migrate:rollback` | `sz-rust migrate --rollback` | 回滚迁移 |
//! | `php think migrate:status` | `sz-rust migrate:status` | 迁移状态 |
//! | `php think route:list` | `sz-rust route:list` | 路由列表 |
//! | `php think cache:clear` | `sz-rust cache:clear` | 清空缓存 |

use clap::{Parser, Subcommand};

use crate::cmd;
use crate::error::CliError;

/// SZ-Rust 命令行工具
///
/// 替代 PHP `think` 命令，提供代码生成、数据库迁移、路由查看、缓存管理等功能。
#[derive(Parser, Debug)]
#[command(
    name = "sz-rust",
    bin_name = "sz-rust",
    version,
    about = "SZ-Rust 命令行工具 — 替代 PHP think 命令",
    long_about = "SZ-Rust CLI 对齐 PHP ThinkPHP 6 think 命令体系，提供 make:migration / make:model / make:controller / migrate / route:list / cache:clear 等命令。"
)]
pub struct Cli {
    /// 子命令
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// 顶层命令枚举
///
/// 对齐 PHP `think` 的命令分组（make / migrate / route / cache）。
#[derive(Subcommand, Debug)]
pub enum Command {
    /// 代码生成命令组（make:migration / make:model / make:controller / make:guard / make:scaffold）
    #[command(name = "make")]
    Make {
        /// make 子命令
        #[command(subcommand)]
        make_command: cmd::make::MakeCommand,
    },

    /// 数据库迁移命令（migrate / migrate:status / migrate:rollback）
    #[command(name = "migrate")]
    Migrate {
        /// 迁移子命令参数
        #[command(flatten)]
        args: cmd::migrate::MigrateArgs,
    },

    /// 迁移状态查询（对齐 PHP `php think migrate:status`）
    #[command(name = "migrate:status")]
    MigrateStatus {
        /// 迁移目录（默认 `migrations`）
        #[arg(short = 'p', long, default_value = "migrations")]
        path: String,

        /// 数据库类型（默认 `postgres`，对齐 sz-orm `DbType`）
        #[arg(long, default_value = "postgres")]
        db_type: String,

        /// 打印每个迁移的 SQL 内容
        #[arg(long)]
        show_sql: bool,
    },

    /// 路由列表（对齐 PHP `php think route:list`）
    #[command(name = "route:list")]
    RouteList {
        /// 输出格式（table / json）
        #[arg(short = 'f', long, default_value = "table")]
        format: String,
    },

    /// 清空缓存（对齐 PHP `php think cache:clear`）
    #[command(name = "cache:clear")]
    CacheClear {
        /// 指定缓存存储名（默认清空所有）
        #[arg(short = 's', long)]
        store: Option<String>,
    },

    /// 调度器命令组（scheduler:list / scheduler:run / scheduler:start）
    #[command(name = "scheduler")]
    Scheduler {
        /// scheduler 子命令
        #[command(subcommand)]
        scheduler_command: cmd::scheduler::SchedulerCommand,
    },
}

impl Cli {
    /// 执行命令
    ///
    /// 根据 `command` 字段分发到对应的命令处理器。
    ///
    /// # 返回
    ///
    /// - `Ok(0)`：成功
    /// - `Ok(code)`：命令指定的退出码（非 0 表示部分失败）
    /// - `Err(_)`：内部错误
    pub fn execute(&self) -> Result<i32, CliError> {
        match &self.command {
            None => {
                // 无子命令，打印帮助
                println!("SZ-Rust CLI — 使用 --help 查看可用命令");
                Ok(0)
            }
            Some(Command::Make { make_command }) => cmd::make::execute(make_command).map(|_| 0),
            Some(Command::Migrate { args }) => cmd::migrate::execute_migrate(args).map(|_| 0),
            Some(Command::MigrateStatus {
                path,
                db_type,
                show_sql,
            }) => cmd::migrate::execute_status_with(path, db_type, *show_sql).map(|_| 0),
            Some(Command::RouteList { format }) => {
                cmd::route::execute_route_list(format).map(|_| 0)
            }
            Some(Command::CacheClear { store }) => {
                cmd::cache::execute_cache_clear(store.as_deref()).map(|_| 0)
            }
            Some(Command::Scheduler { scheduler_command }) => {
                cmd::scheduler::execute(scheduler_command).map(|_| 0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_parse_make_model() {
        let cli = Cli::parse_from(["sz-rust", "make", "model", "User"]);
        match cli.command {
            Some(Command::Make { make_command }) => {
                assert!(matches!(make_command, cmd::make::MakeCommand::Model { .. }));
            }
            _ => panic!("expected Make command"),
        }
    }

    #[test]
    fn test_parse_make_controller() {
        let cli = Cli::parse_from(["sz-rust", "make", "controller", "User"]);
        match cli.command {
            Some(Command::Make { make_command }) => {
                assert!(matches!(
                    make_command,
                    cmd::make::MakeCommand::Controller { .. }
                ));
            }
            _ => panic!("expected Make command"),
        }
    }

    #[test]
    fn test_parse_make_migration() {
        let cli = Cli::parse_from(["sz-rust", "make", "migration", "create_users"]);
        match cli.command {
            Some(Command::Make { make_command }) => {
                assert!(matches!(
                    make_command,
                    cmd::make::MakeCommand::Migration { .. }
                ));
            }
            _ => panic!("expected Make command"),
        }
    }

    #[test]
    fn test_parse_migrate() {
        let cli = Cli::parse_from(["sz-rust", "migrate"]);
        assert!(matches!(cli.command, Some(Command::Migrate { .. })));
    }

    #[test]
    fn test_parse_migrate_status() {
        let cli = Cli::parse_from(["sz-rust", "migrate:status"]);
        assert!(matches!(cli.command, Some(Command::MigrateStatus { .. })));
    }

    #[test]
    fn test_parse_route_list() {
        let cli = Cli::parse_from(["sz-rust", "route:list"]);
        assert!(matches!(cli.command, Some(Command::RouteList { .. })));
    }

    #[test]
    fn test_parse_cache_clear() {
        let cli = Cli::parse_from(["sz-rust", "cache:clear"]);
        assert!(matches!(cli.command, Some(Command::CacheClear { .. })));
    }

    #[test]
    fn test_parse_cache_clear_with_store() {
        let cli = Cli::parse_from(["sz-rust", "cache:clear", "--store", "redis"]);
        match cli.command {
            Some(Command::CacheClear { store }) => {
                assert_eq!(store.as_deref(), Some("redis"));
            }
            _ => panic!("expected CacheClear command"),
        }
    }

    #[test]
    fn test_parse_scheduler() {
        let cli = Cli::parse_from(["sz-rust", "scheduler", "list"]);
        assert!(matches!(cli.command, Some(Command::Scheduler { .. })));
    }

    #[test]
    fn test_execute_no_command_returns_ok() {
        let cli = Cli { command: None };
        let result = cli.execute();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }
}
