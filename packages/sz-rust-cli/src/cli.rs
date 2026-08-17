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
//! | `php think make:seeder UserSeeder` | `sz-rust make:seeder user_seeder` | 生成填充文件 |
//! | `php think make:validate User` | `sz-rust make:validate User` | 生成验证器 |
//! | `php think make:event User` | `sz-rust make:event User` | 生成事件 |
//! | `php think make:listener UserListener` | `sz-rust make:listener UserListener` | 生成监听器 |
//! | `php think make:command Hello` | `sz-rust make:command Hello` | 生成命令 |
//! | `php think make:service UserService` | `sz-rust make:service UserService` | 生成服务 |
//! | `php think migrate` | `sz-rust migrate` | 执行迁移 |
//! | `php think migrate:rollback` | `sz-rust migrate --rollback` | 回滚迁移 |
//! | `php think migrate:status` | `sz-rust migrate:status` | 迁移状态 |
//! | `php think db:seed` | `sz-rust db:seed` | 数据填充 |
//! | `php think route:list` | `sz-rust route:list` | 路由列表 |
//! | `php think cache:clear` | `sz-rust cache:clear` | 清空缓存 |
//! | `php think optimize:route` | `sz-rust optimize:route` | 路由缓存 |
//! | `php think optimize:config` | `sz-rust optimize:config` | 配置缓存 |
//! | `php think optimize:schema` | `sz-rust optimize:schema` | 数据表字段缓存 |
//! | `php think route:clear` | `sz-rust route:clear` | 清除路由缓存 |

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

        /// 数据库连接 URL（启用在线模式，查询真实状态）
        ///
        /// 省略时为离线模式，所有迁移状态显示为 `Pending*`。
        #[arg(long)]
        url: Option<String>,
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

    /// 数据填充（对齐 PHP `php think db:seed`）
    ///
    /// 从 `seeds/` 目录加载 `.sql` 文件并执行。提供 `--url` 时连接数据库真实执行，
    /// 否则为离线模式（仅打印待执行内容）。
    #[command(name = "db:seed")]
    Seed {
        /// 填充目录（默认 `seeds`）
        #[arg(short = 'p', long, default_value = "seeds")]
        path: String,

        /// 数据库类型（默认 `postgres`，对齐 sz-orm `DbType`）
        #[arg(long, default_value = "postgres")]
        db_type: String,

        /// 打印每个填充文件的 SQL 内容
        #[arg(long)]
        show_sql: bool,

        /// 数据库连接 URL（启用在线模式）
        ///
        /// 省略时为离线模式，仅打印待执行的 SQL。
        #[arg(long)]
        url: Option<String>,

        /// 指定填充器文件名（不含扩展名，如 `001_users_seed`）
        ///
        /// 省略时执行目录下所有 `.sql` 文件。
        #[arg(short = 'c', long)]
        class: Option<String>,
    },

    /// 调度器命令组（scheduler:list / scheduler:run / scheduler:start）
    #[command(name = "scheduler")]
    Scheduler {
        /// scheduler 子命令
        #[command(subcommand)]
        scheduler_command: cmd::scheduler::SchedulerCommand,
    },

    /// 生成路由缓存（对齐 PHP `php think optimize:route`）
    ///
    /// 收集路由元数据，序列化为 JSON 写入 `runtime/cache/route_cache.json`。
    #[command(name = "optimize:route")]
    OptimizeRoute,

    /// 生成配置缓存（对齐 PHP `php think optimize:config`）
    ///
    /// 扫描 `config/` 目录，合并所有配置，序列化为 JSON 写入 `runtime/cache/config_cache.json`。
    #[command(name = "optimize:config")]
    OptimizeConfig,

    /// 生成数据表字段缓存（对齐 PHP `php think optimize:schema`）
    ///
    /// 读取 `config/database.yml` 数据库连接配置，生成 schema 缓存索引文件
    /// （`runtime/schema_cache.json` + `runtime/schema_cache.php`）。
    /// 业务方运行时通过 `SchemaCache::remember_schema()` 填充具体字段信息。
    #[command(name = "optimize:schema")]
    OptimizeSchema,

    /// 清除路由缓存（对齐 PHP `php think route:clear`）
    ///
    /// 删除 `runtime/cache/route_cache.json` 文件。
    #[command(name = "route:clear")]
    RouteClear,

    /// 插件市场命令组（plugin search/install/publish/uninstall/update/list/login）
    #[command(name = "plugin")]
    Plugin {
        /// plugin 子命令
        #[command(subcommand)]
        plugin_command: cmd::plugin::PluginCommand,
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
    pub async fn execute(&self) -> Result<i32, CliError> {
        match &self.command {
            None => {
                // 无子命令，打印帮助
                println!("SZ-Rust CLI — 使用 --help 查看可用命令");
                Ok(0)
            }
            Some(Command::Make { make_command }) => {
                cmd::make::execute(make_command).await.map(|_| 0)
            }
            Some(Command::Migrate { args }) => cmd::migrate::execute_migrate(args).map(|_| 0),
            Some(Command::MigrateStatus {
                path,
                db_type,
                show_sql,
                url,
            }) => cmd::migrate::execute_status_full(path, db_type, *show_sql, url.as_deref())
                .map(|_| 0),
            Some(Command::RouteList { format }) => {
                cmd::route::execute_route_list(format).map(|_| 0)
            }
            Some(Command::CacheClear { store }) => {
                cmd::cache::execute_cache_clear(store.as_deref()).map(|_| 0)
            }
            Some(Command::Seed {
                path,
                db_type,
                show_sql,
                url,
                class,
            }) => {
                let args = cmd::seed::SeedArgs {
                    path: path.clone(),
                    db_type: db_type.clone(),
                    show_sql: *show_sql,
                    url: url.clone(),
                    class: class.clone(),
                };
                cmd::seed::execute_seed(&args).map(|_| 0)
            }
            Some(Command::Scheduler { scheduler_command }) => {
                cmd::scheduler::execute(scheduler_command).map(|_| 0)
            }
            Some(Command::OptimizeRoute) => {
                cmd::optimize::execute_optimize_route().await.map(|_| 0)
            }
            Some(Command::OptimizeConfig) => {
                cmd::optimize::execute_optimize_config().await.map(|_| 0)
            }
            Some(Command::OptimizeSchema) => {
                cmd::optimize::execute_optimize_schema().await.map(|_| 0)
            }
            Some(Command::RouteClear) => cmd::optimize::execute_route_clear().await.map(|_| 0),
            Some(Command::Plugin { plugin_command }) => cmd::plugin::execute(plugin_command).await,
        }
    }
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
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
    fn test_parse_optimize_schema() {
        let cli = Cli::parse_from(["sz-rust", "optimize:schema"]);
        assert!(matches!(cli.command, Some(Command::OptimizeSchema)));
    }

    #[test]
    fn test_parse_make_validate() {
        let cli = Cli::parse_from(["sz-rust", "make", "validate", "User"]);
        match cli.command {
            Some(Command::Make { make_command }) => {
                assert!(matches!(
                    make_command,
                    cmd::make::MakeCommand::Validate { .. }
                ));
            }
            _ => panic!("expected Make command"),
        }
    }

    #[test]
    fn test_parse_make_seeder() {
        let cli = Cli::parse_from(["sz-rust", "make", "seeder", "001_users"]);
        match cli.command {
            Some(Command::Make { make_command }) => {
                assert!(matches!(
                    make_command,
                    cmd::make::MakeCommand::Seeder { .. }
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
    fn test_parse_db_seed() {
        let cli = Cli::parse_from(["sz-rust", "db:seed"]);
        assert!(matches!(cli.command, Some(Command::Seed { .. })));
    }

    #[test]
    fn test_parse_db_seed_with_options() {
        let cli = Cli::parse_from([
            "sz-rust",
            "db:seed",
            "--path",
            "custom_seeds",
            "--db-type",
            "mysql",
            "--show-sql",
            "--url",
            "mysql://user:pass@host:3306/db",
            "--class",
            "001_users",
        ]);
        match cli.command {
            Some(Command::Seed {
                path,
                db_type,
                show_sql,
                url,
                class,
            }) => {
                assert_eq!(path, "custom_seeds");
                assert_eq!(db_type, "mysql");
                assert!(show_sql);
                assert_eq!(url.as_deref(), Some("mysql://user:pass@host:3306/db"));
                assert_eq!(class.as_deref(), Some("001_users"));
            }
            _ => panic!("expected Seed command"),
        }
    }

    #[tokio::test]
    async fn test_execute_no_command_returns_ok() {
        let cli = Cli { command: None };
        let result = cli.execute().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_execute_make_model() {
        let _lock = crate::cmd::test_support::acquire_global_lock();
        let temp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let cli = Cli {
            command: Some(Command::Make {
                make_command: cmd::make::MakeCommand::Model {
                    name: "User".to_string(),
                },
            }),
        };
        let result = cli.execute().await;
        std::env::set_current_dir(&original).unwrap();
        assert!(result.is_ok());
        assert!(temp.path().join("app/model/User.rs").exists());
    }

    #[tokio::test]
    async fn test_execute_migrate_offline() {
        let _lock = crate::cmd::test_support::acquire_global_lock();
        let temp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let cli = Cli {
            command: Some(Command::Migrate {
                args: cmd::migrate::MigrateArgs {
                    rollback: false,
                    path: temp.path().to_string_lossy().to_string(),
                    db_type: "postgres".to_string(),
                    show_sql: false,
                    url: None,
                },
            }),
        };
        let result = cli.execute().await;
        std::env::set_current_dir(&original).unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_migrate_status_offline() {
        let _lock = crate::cmd::test_support::acquire_global_lock();
        let temp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let cli = Cli {
            command: Some(Command::MigrateStatus {
                path: temp.path().to_string_lossy().to_string(),
                db_type: "postgres".to_string(),
                show_sql: false,
                url: None,
            }),
        };
        let result = cli.execute().await;
        std::env::set_current_dir(&original).unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_route_list() {
        let cli = Cli {
            command: Some(Command::RouteList {
                format: "table".to_string(),
            }),
        };
        let result = cli.execute().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_cache_clear() {
        let _lock = crate::cmd::test_support::acquire_global_lock();
        let temp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let cli = Cli {
            command: Some(Command::CacheClear { store: None }),
        };
        let result = cli.execute().await;
        std::env::set_current_dir(&original).unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_seed_offline() {
        let _lock = crate::cmd::test_support::acquire_global_lock();
        let temp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let cli = Cli {
            command: Some(Command::Seed {
                path: temp.path().to_string_lossy().to_string(),
                db_type: "postgres".to_string(),
                show_sql: false,
                url: None,
                class: None,
            }),
        };
        let result = cli.execute().await;
        std::env::set_current_dir(&original).unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_scheduler_list() {
        let cli = Cli {
            command: Some(Command::Scheduler {
                scheduler_command: cmd::scheduler::SchedulerCommand::List {
                    config: std::path::PathBuf::from("/nonexistent/scheduler.toml"),
                },
            }),
        };
        let result = cli.execute().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_optimize_route() {
        let _lock = crate::cmd::test_support::acquire_global_lock();
        let temp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let cli = Cli {
            command: Some(Command::OptimizeRoute),
        };
        let result = cli.execute().await;
        std::env::set_current_dir(&original).unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_optimize_config() {
        let _lock = crate::cmd::test_support::acquire_global_lock();
        let temp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        // execute_optimize_config 需要 config 目录存在
        std::fs::create_dir_all(temp.path().join("config")).unwrap();

        let cli = Cli {
            command: Some(Command::OptimizeConfig),
        };
        let result = cli.execute().await;
        std::env::set_current_dir(&original).unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_optimize_schema() {
        let _lock = crate::cmd::test_support::acquire_global_lock();
        let temp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let cli = Cli {
            command: Some(Command::OptimizeSchema),
        };
        let result = cli.execute().await;
        std::env::set_current_dir(&original).unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_route_clear() {
        let _lock = crate::cmd::test_support::acquire_global_lock();
        let temp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let cli = Cli {
            command: Some(Command::RouteClear),
        };
        let result = cli.execute().await;
        std::env::set_current_dir(&original).unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_plugin_list() {
        let cli = Cli {
            command: Some(Command::Plugin {
                plugin_command: cmd::plugin::PluginCommand::List,
            }),
        };
        let result = cli.execute().await;
        assert!(result.is_ok());
    }
}
