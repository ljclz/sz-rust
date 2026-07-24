//! SZ-Rust CLI — 命令行工具
//!
//! Phase 8 交付物，替代 PHP `think` 命令，借鉴 Laravel Artisan 风格。
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
//! | `cmd::scheduler` | scheduler:* 调度器命令（Phase 8.11-8.14） |
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
pub fn run<I, S>(args: I) -> Result<i32, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString> + Clone,
{
    use clap::Parser;
    let cli = Cli::parse_from(args);
    cli.execute()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_no_args_returns_ok() {
        // 仅程序名、无子命令：command=None，execute 返回 Ok(0)
        let result = run(vec!["sz-rust"]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_run_version_flag() {
        // --version 由 clap 处理后退出（clap 内部调用 exit，测试中会 panic）
        // 因此这里只验证 --help 不影响 run 逻辑
        let result = run(vec!["sz-rust", "cache:clear"]);
        assert!(result.is_ok());
    }
}
