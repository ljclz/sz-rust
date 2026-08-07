//! SZ-Rust CLI — 命令行工具
//!
//! 替代 PHP `think` 命令，借鉴 Laravel Artisan 风格。
//!
//! ## PHP 对齐
//!
//! 本包对齐 PHP ThinkPHP 6 `think` 命令体系：
//!
//! - `php think make:model` → `sz-rust make:model`
//! - `php think make:controller` → `sz-rust make:controller`
//! - `php think make:migration` → `sz-rust make:migration`（Phinx 风格）
//! - `php think migrate` → `sz-rust migrate`
//! - `php think migrate:status` → `sz-rust migrate:status`
//! - `php think route:list` → `sz-rust route:list`
//! - `php think cache:clear` → `sz-rust cache:clear`
//!
//! ## 模块结构
//!
//! | 模块 | 功能 |
//! |------|------|
//! | `cli` | clap 命令定义（Cli / Commands / Options） |
//! | `console` | 自定义命令注册与分发（对齐 PHP `think\console\Console`） |
//! | `cmd::make` | make:* 代码生成命令 |
//! | `cmd::migrate` | migrate / migrate:status 迁移命令 |
//! | `cmd::route` | route:list 路由列表命令 |
//! | `cmd::cache` | cache:clear 缓存清理命令 |
//! | `cmd::scheduler` | scheduler:* 调度器命令 |
//! | `error` | CLI 错误类型 |
//! | `stubs` | 代码生成模板（对齐 PHP make/stubs） |
//!
//! ## R5 硬约束
//!
//! - R5-48：`make:model` 生成 Model 骨架代码对齐 PHP `think\console\command\make\Model`
//! - R5-49：`make:controller` 生成 Controller 骨架代码对齐 PHP `think\console\command\make\Controller`
//! - R5-50：`migrate:status` 显示迁移进度对齐 PHP `think migrate:status`
//! - R5-51：`cache:clear` 清空缓存对齐 PHP `think cache:clear`

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cli;
pub mod cmd;
pub mod console;
pub mod error;
pub mod stubs;

pub use cli::{Cli, Command as CliCommand};
pub use console::{Command, CommandSignature, Console};
pub use error::CliError;

/// 运行 CLI（入口函数）
///
/// 解析命令行参数并执行对应命令，返回退出码。
///
/// # 参数
///
/// - `args`：命令行参数（含程序名，如 `["sz-rust", "make", "model", "User"]`）
///
/// # 返回
///
/// - `Ok(0)`：成功
/// - `Ok(code)`：命令指定的退出码
/// - `Err(_)`：内部错误
pub async fn run<I, S>(args: I) -> Result<i32, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString> + Clone,
{
    use clap::Parser;
    let cli = Cli::parse_from(args);
    cli.execute().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_no_args_returns_ok() {
        // 仅程序名、无子命令：command=None，execute 返回 Ok(0)
        let result = run(vec!["sz-rust"]).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_run_cache_clear_command() {
        // 通过 run() 分发执行 cache:clear 命令。
        // cache:clear 读写进程级工作目录下的 runtime/cache，
        // 必须持有全局互斥锁并隔离到临时目录，避免与 make/optimize
        // 模块的 set_current_dir 测试并行竞态。
        // clippy::await_holding_lock: 本测试运行在 current_thread runtime，
        // std::sync::MutexGuard 跨 await 不会跨线程，安全。
        let _lock = crate::cmd::test_support::acquire_global_lock();
        let temp = tempfile::tempdir().expect("tempdir failed");
        let original = std::env::current_dir().expect("current_dir failed");
        std::env::set_current_dir(temp.path()).expect("set_current_dir failed");
        let result = run(vec!["sz-rust", "cache:clear"]).await;
        let restore = std::env::set_current_dir(&original);
        assert!(restore.is_ok(), "恢复工作目录失败");
        assert!(result.is_ok());
    }
}
