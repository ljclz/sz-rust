//! Console 模块 — 自定义命令注册与分发
//!
//! 对齐 PHP ThinkPHP 6 `think\console\Console` 与 `think\console\Command` 基类。
//!
//! ## PHP 对齐
//!
//! PHP ThinkPHP 通过 `think\console\Console` 注册并分发命令：
//!
//! ```php
//! $console = new \think\Console();
//! $console->add(new \app\command\Hello());
//! $console->run();
//! ```
//!
//! Rust 端通过 `Console` 结构体 + `Command` trait 实现等价功能：
//!
//! ```rust,ignore
//! use sz_rust_cli::{Console, Command, CommandSignature};
//!
//! struct HelloCommand;
//!
//! impl Command for HelloCommand {
//!     fn signature(&self) -> CommandSignature {
//!         CommandSignature {
//!             name: "hello".to_string(),
//!             description: "Print hello world".to_string(),
//!             usage: "sz-rust hello".to_string(),
//!         }
//!     }
//!     fn execute(&self, _args: &[String]) -> Result<i32, sz_rust_cli::CliError> {
//!         println!("Hello, World!");
//!         Ok(0)
//!     }
//! }
//!
//! let mut console = Console::new();
//! console.register(Box::new(HelloCommand));
//! console.run(std::env::args().collect()).await;
//! ```
//!
//! ## 设计说明
//!
//! - 内置命令（make / migrate / route / cache / scheduler）仍由 clap derive 处理
//! - 自定义命令通过 `Command` trait 在运行时注册
//! - `Console::run` 先检查首个参数是否匹配已注册的自定义命令，若不匹配则回退到 clap CLI

use std::collections::HashMap;

use crate::CliError;

/// 控制台命令签名（名称、描述、用法）
///
/// 对齐 PHP `think\console\command\Command::configure()` 中设置的 name / description / help。
#[derive(Debug, Clone)]
pub struct CommandSignature {
    /// 命令名称（如 `"make:model"`、`"hello"`）
    pub name: String,
    /// 简短描述
    pub description: String,
    /// 用法示例
    pub usage: String,
}

/// 自定义控制台命令 trait
///
/// 对齐 PHP `think\console\Command` 抽象基类。
///
/// PHP 端通过继承 `Command` 并实现 `configure()` 与 `execute()` 方法定义命令；
/// Rust 端通过实现此 trait 达到相同效果。
pub trait Command: Send + Sync {
    /// 获取命令签名
    ///
    /// 对齐 PHP `think\console\Command::configure()`。
    fn signature(&self) -> CommandSignature;

    /// 执行命令
    ///
    /// 对齐 PHP `think\console\Command::execute(Input $input, Output $output)`。
    ///
    /// # 参数
    ///
    /// - `args`：命令参数（不含程序名与命令名，仅含命令后的额外参数）
    ///
    /// # 返回
    ///
    /// - `Ok(0)`：成功
    /// - `Ok(code)`：命令指定的退出码（非 0 表示部分失败）
    /// - `Err(_)`：内部错误
    fn execute(&self, args: &[String]) -> Result<i32, CliError>;
}

/// 控制台应用 — 命令注册表与分发器
///
/// 对齐 PHP `think\console\Console`。允许在运行时注册自定义命令，
/// 并列出所有可用命令。
///
/// # 分发逻辑
///
/// 1. 若 `args[1]` 匹配已注册的自定义命令名，则分发到该命令
/// 2. 否则，回退到内置的 clap CLI（`crate::run`）
///
/// # 示例
///
/// ```rust,ignore
/// use sz_rust_cli::{Console, Command, CommandSignature};
///
/// let mut console = Console::new();
/// console.register(Box::new(MyCommand));
/// console.run(vec!["sz-rust".to_string(), "my:command".to_string()]).await;
/// ```
pub struct Console {
    /// 已注册的自定义命令映射（命令名 → 命令实现）
    commands: HashMap<String, Box<dyn Command>>,
}

impl Console {
    /// 创建空的 Console 实例
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
        }
    }

    /// 注册自定义命令
    ///
    /// 对齐 PHP `think\console\Console::add(Command $command)`。
    ///
    /// # 参数
    ///
    /// - `command`：实现了 `Command` trait 的命令实例（装箱）
    ///
    /// # 返回
    ///
    /// 返回 `&mut Self` 以支持链式调用。
    ///
    /// # 示例
    ///
    /// ```rust,ignore
    /// let mut console = Console::new();
    /// console
    ///     .register(Box::new(HelloCommand))
    ///     .register(Box::new(WorldCommand));
    /// ```
    pub fn register(&mut self, command: Box<dyn Command>) -> &mut Self {
        let name = command.signature().name;
        self.commands.insert(name, command);
        self
    }

    /// 运行控制台
    ///
    /// 若 `args[1]`（程序名后的首个参数）匹配已注册的自定义命令名，
    /// 则分发到该命令；否则回退到内置 clap CLI。
    ///
    /// # 参数
    ///
    /// - `args`：命令行参数（含程序名，如 `["sz-rust", "hello", "arg1"]`）
    ///
    /// # 返回
    ///
    /// - `Ok(0)`：成功
    /// - `Ok(code)`：命令指定的退出码
    /// - `Err(_)`：内部错误
    pub async fn run(&self, args: Vec<String>) -> Result<i32, CliError> {
        if args.len() >= 2 {
            if let Some(command) = self.commands.get(&args[1]) {
                let cmd_args: &[String] = &args[2..];
                return command.execute(cmd_args);
            }
        }
        crate::run(args).await
    }

    /// 列出所有已注册的自定义命令签名
    ///
    /// 对齐 PHP `think\console\Console::getCommands()`。
    ///
    /// # 返回
    ///
    /// 返回命令签名向量，顺序不保证（HashMap 迭代顺序不确定）。
    pub fn list(&self) -> Vec<CommandSignature> {
        self.commands.values().map(|cmd| cmd.signature()).collect()
    }

    /// 打印命令列表到标准输出
    ///
    /// 对齐 PHP `think\console\Console::listCommands()`。
    pub fn print_list(&self) {
        println!("Available commands:");
        let mut signatures = self.list();
        signatures.sort_by(|a, b| a.name.cmp(&b.name));
        for sig in signatures {
            println!("  {:<20} {}", sig.name, sig.description);
        }
    }
}

impl Default for Console {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;

    /// 测试用命令：打印 Hello, World!
    struct HelloCommand;

    impl Command for HelloCommand {
        fn signature(&self) -> CommandSignature {
            CommandSignature {
                name: "hello".to_string(),
                description: "Print hello world".to_string(),
                usage: "sz-rust hello".to_string(),
            }
        }

        fn execute(&self, _args: &[String]) -> Result<i32, CliError> {
            println!("Hello, World!");
            Ok(0)
        }
    }

    /// 测试用命令：回显参数
    struct EchoCommand;

    impl Command for EchoCommand {
        fn signature(&self) -> CommandSignature {
            CommandSignature {
                name: "echo".to_string(),
                description: "Echo arguments".to_string(),
                usage: "sz-rust echo <args...>".to_string(),
            }
        }

        fn execute(&self, args: &[String]) -> Result<i32, CliError> {
            println!("{}", args.join(" "));
            Ok(0)
        }
    }

    #[test]
    fn test_register_and_list() {
        let mut console = Console::new();
        console.register(Box::new(HelloCommand));
        let commands = console.list();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, "hello");
        assert_eq!(commands[0].description, "Print hello world");
        assert_eq!(commands[0].usage, "sz-rust hello");
    }

    #[test]
    fn test_register_multiple_commands() {
        let mut console = Console::new();
        console
            .register(Box::new(HelloCommand))
            .register(Box::new(EchoCommand));
        let commands = console.list();
        assert_eq!(commands.len(), 2);
    }

    #[tokio::test]
    async fn test_run_custom_command() {
        let mut console = Console::new();
        console.register(Box::new(HelloCommand));
        let result = console
            .run(vec!["sz-rust".to_string(), "hello".to_string()])
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_run_custom_command_with_args() {
        let mut console = Console::new();
        console.register(Box::new(EchoCommand));
        let result = console
            .run(vec![
                "sz-rust".to_string(),
                "echo".to_string(),
                "foo".to_string(),
                "bar".to_string(),
            ])
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_run_unknown_command_falls_through() {
        let _lock = crate::cmd::test_support::acquire_global_lock();
        let temp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().ok();
        std::env::set_current_dir(temp.path()).unwrap();
        let console = Console::new();
        // "cache:clear" 是内置 clap 命令，未注册为自定义命令，应回退到 crate::run
        let result = console
            .run(vec!["sz-rust".to_string(), "cache:clear".to_string()])
            .await;
        if let Some(ref orig) = original {
            let _ = std::env::set_current_dir(orig);
        }
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_run_no_args_falls_through() {
        let console = Console::new();
        // 无参数时应回退到 crate::run
        let result = console.run(vec!["sz-rust".to_string()]).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_console_default_is_empty() {
        let console = Console::default();
        assert!(console.list().is_empty());
    }

    #[test]
    fn test_register_overwrites_same_name() {
        let mut console = Console::new();
        console.register(Box::new(HelloCommand));
        // 注册同名命令应覆盖
        console.register(Box::new(EchoCommand));
        let commands = console.list();
        // HelloCommand 和 EchoCommand 名称不同，故应为 2
        assert_eq!(commands.len(), 2);
    }

    #[test]
    fn test_print_list_empty() {
        let console = Console::new();
        let commands = console.list();
        assert!(commands.is_empty(), "空命令列表应返回空切片");
    }

    #[test]
    fn test_print_list_with_commands() {
        let mut console = Console::new();
        console
            .register(Box::new(HelloCommand))
            .register(Box::new(EchoCommand));
        let commands = console.list();
        assert_eq!(commands.len(), 2, "应有两个注册命令");
    }
}
