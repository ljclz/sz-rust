//! 日志系统 — 对齐 PHP `think-logger`
//!
//! ## 设计
//!
//! - 基于 `sz-orm-logger` 的 `StructuredLogger` 提供日志收集
//! - 同时通过 `tracing` 宏输出（与 SZ-ORM-Tracing 协同，未来接入 OpenTelemetry）
//! - 全局单例 `LogFacade`，通过 [`LogFacade::init()`] 初始化、[`LogFacade::instance()`] 获取
//! - 支持多通道（file/console），对齐 PHP `config/log.php` 的 `channels` 配置
//!
//! ## PHP 对齐
//!
//! ```php
//! // PHP think-logger
//! Log::info('hello');
//! Log::error('error occurred', ['exception' => $e]);
//! Log::channel('file')->info('file log');
//! ```
//!
//! ```rust,ignore
//! // SZ-Rust 等价
//! use sz_rust_core::log::LogFacade;
//! LogFacade::instance().unwrap().info("hello");
//! LogFacade::instance().unwrap().error("error occurred");
//! ```

use crate::config::{LogChannel, LogSection};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::OnceLock;

// 重导出 sz-orm-logger 核心类型，方便上层直接使用
pub use sz_orm_logger::{LogEntry, LogLevel, Logger, LoggerFactory, StructuredLogger};

/// 全局日志 facade 单例
static LOG_FACADE: OnceLock<LogFacade> = OnceLock::new();

/// 日志 facade — 持有默认 `StructuredLogger` 和命名通道
///
/// 对齐 PHP `think\facade\Log`，提供全局日志访问点。
pub struct LogFacade {
    /// 默认通道名（对应 PHP `config/log.php` 的 `default`）
    default_channel: String,
    /// 默认 logger 实例
    logger: StructuredLogger,
    /// 命名通道集合（对应 PHP `channels`）
    channels: RwLock<HashMap<String, StructuredLogger>>,
}

impl LogFacade {
    /// 构造 LogFacade 实例（不注册到全局单例）
    pub fn new(section: &LogSection) -> Self {
        let default_channel = section.default.clone();
        let default_log_level = section
            .channels
            .get(&default_channel)
            .map(|c| parse_level(&c.level))
            .unwrap_or(LogLevel::Info);
        let logger = StructuredLogger::with_level(default_log_level);

        let mut channels = HashMap::new();
        for (name, channel_cfg) in &section.channels {
            channels.insert(name.clone(), channel_to_logger(channel_cfg));
        }

        LogFacade {
            default_channel,
            logger,
            channels: RwLock::new(channels),
        }
    }

    /// 初始化全局日志 facade
    ///
    /// 重复调用返回已有实例（不覆盖）。
    pub fn init(section: &LogSection) -> &'static LogFacade {
        LOG_FACADE.get_or_init(|| LogFacade::new(section))
    }

    /// 获取全局日志 facade 实例
    ///
    /// 必须先调用 [`LogFacade::init()`] 初始化，否则返回 `None`。
    pub fn instance() -> Option<&'static LogFacade> {
        LOG_FACADE.get()
    }

    /// 获取默认通道名
    pub fn default_channel(&self) -> &str {
        &self.default_channel
    }

    /// 获取默认 logger 引用
    pub fn logger(&self) -> &StructuredLogger {
        &self.logger
    }

    /// 获取指定通道的 logger 引用
    ///
    /// 对齐 PHP `Log::channel('file')->info(...)`。
    pub fn channel(&self, name: &str) -> Option<ChannelRef<'_>> {
        if self.channels.read().contains_key(name) {
            Some(ChannelRef {
                facade: self,
                name: name.to_string(),
            })
        } else {
            None
        }
    }

    /// 获取所有通道名
    pub fn channel_names(&self) -> Vec<String> {
        self.channels.read().keys().cloned().collect()
    }

    /// 记录日志（同时输出到 StructuredLogger 和 tracing）
    pub fn log(&self, level: LogLevel, msg: &str) {
        self.logger.log(level, msg);
        match level {
            LogLevel::Debug => tracing::debug!("{}", msg),
            LogLevel::Info => tracing::info!("{}", msg),
            LogLevel::Warn => tracing::warn!("{}", msg),
            LogLevel::Error => tracing::error!("{}", msg),
        }
    }

    /// 记录 DEBUG 级别日志
    pub fn debug(&self, msg: &str) {
        self.log(LogLevel::Debug, msg);
    }

    /// 记录 INFO 级别日志
    pub fn info(&self, msg: &str) {
        self.log(LogLevel::Info, msg);
    }

    /// 记录 WARN 级别日志
    pub fn warn(&self, msg: &str) {
        self.log(LogLevel::Warn, msg);
    }

    /// 记录 ERROR 级别日志
    pub fn error(&self, msg: &str) {
        self.log(LogLevel::Error, msg);
    }
}

impl std::fmt::Debug for LogFacade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogFacade")
            .field("default_channel", &self.default_channel)
            .field("channels", &self.channels.read().keys().collect::<Vec<_>>())
            .finish()
    }
}

/// 命名通道引用
///
/// 通过 [`LogFacade::channel()`] 获取，提供与默认 logger 相同的日志方法。
pub struct ChannelRef<'a> {
    facade: &'a LogFacade,
    name: String,
}

impl<'a> ChannelRef<'a> {
    /// 通道名
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 记录日志到指定通道
    pub fn log(&self, level: LogLevel, msg: &str) {
        let guard = self.facade.channels.read();
        if let Some(logger) = guard.get(&self.name) {
            logger.log(level, msg);
        }
        match level {
            LogLevel::Debug => tracing::debug!("[{}] {}", self.name, msg),
            LogLevel::Info => tracing::info!("[{}] {}", self.name, msg),
            LogLevel::Warn => tracing::warn!("[{}] {}", self.name, msg),
            LogLevel::Error => tracing::error!("[{}] {}", self.name, msg),
        }
    }

    /// 记录 debug 级别日志
    pub fn debug(&self, msg: &str) {
        self.log(LogLevel::Debug, msg);
    }

    /// 记录 info 级别日志
    pub fn info(&self, msg: &str) {
        self.log(LogLevel::Info, msg);
    }

    /// 记录 warn 级别日志
    pub fn warn(&self, msg: &str) {
        self.log(LogLevel::Warn, msg);
    }

    /// 记录 error 级别日志
    pub fn error(&self, msg: &str) {
        self.log(LogLevel::Error, msg);
    }
}

/// 从字符串解析日志级别
///
/// 支持大小写不敏感：`"DEBUG"` / `"debug"` / `"Debug"` 均解析为 `LogLevel::Debug`。
/// 未知字符串默认为 `LogLevel::Info`。
pub fn parse_level(s: &str) -> LogLevel {
    match s.to_lowercase().as_str() {
        "debug" => LogLevel::Debug,
        "info" => LogLevel::Info,
        "warn" | "warning" => LogLevel::Warn,
        "error" => LogLevel::Error,
        _ => LogLevel::Info,
    }
}

/// 从 `LogChannel` 配置构造 `StructuredLogger`
fn channel_to_logger(channel: &LogChannel) -> StructuredLogger {
    StructuredLogger::with_level(parse_level(&channel.level))
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LogChannel, LogSection};

    /// 构造测试用的 LogSection（含 file + console 两个通道）
    fn make_log_section() -> LogSection {
        let mut channels = HashMap::new();
        channels.insert(
            "file".to_string(),
            LogChannel {
                r#type: "file".to_string(),
                path: "runtime/logs".to_string(),
                level: "info".to_string(),
                max_files: 30,
                format: "%{time} [%{level}] %{message}".to_string(),
            },
        );
        channels.insert(
            "console".to_string(),
            LogChannel {
                r#type: "console".to_string(),
                path: String::new(),
                level: "debug".to_string(),
                max_files: 0,
                format: "%{time} [%{level}] %{message}".to_string(),
            },
        );
        LogSection {
            default: "file".to_string(),
            channels,
        }
    }

    /// 测试 parse_level 各种输入
    #[test]
    fn test_parse_level() {
        assert_eq!(parse_level("debug"), LogLevel::Debug);
        assert_eq!(parse_level("DEBUG"), LogLevel::Debug);
        assert_eq!(parse_level("Debug"), LogLevel::Debug);
        assert_eq!(parse_level("info"), LogLevel::Info);
        assert_eq!(parse_level("INFO"), LogLevel::Info);
        assert_eq!(parse_level("warn"), LogLevel::Warn);
        assert_eq!(parse_level("warning"), LogLevel::Warn);
        assert_eq!(parse_level("WARN"), LogLevel::Warn);
        assert_eq!(parse_level("error"), LogLevel::Error);
        assert_eq!(parse_level("ERROR"), LogLevel::Error);
        // 未知字符串默认 Info
        assert_eq!(parse_level("unknown"), LogLevel::Info);
        assert_eq!(parse_level(""), LogLevel::Info);
    }

    /// 测试 LogFacade 构造和默认通道
    #[test]
    fn test_log_facade_new() {
        let section = make_log_section();
        let facade = LogFacade::new(&section);

        assert_eq!(facade.default_channel(), "file");
        let names = facade.channel_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"file".to_string()));
        assert!(names.contains(&"console".to_string()));
    }

    /// 测试默认 logger 级别取自 default 通道
    #[test]
    fn test_default_logger_level() {
        let section = make_log_section();
        let facade = LogFacade::new(&section);

        // file 通道 level=info，所以默认 logger 级别为 Info
        assert_eq!(facade.logger().level(), LogLevel::Info);

        // Debug 级别应被过滤
        facade.debug("debug msg - should be filtered");
        let entries = facade.logger().entries();
        assert!(entries.iter().all(|e| e.level != LogLevel::Debug));
    }

    /// 测试日志记录到默认 logger
    #[test]
    fn test_log_to_default_logger() {
        let section = make_log_section();
        let facade = LogFacade::new(&section);

        facade.info("test info message");
        facade.warn("test warn message");
        facade.error("test error message");

        let entries = facade.logger().entries();
        assert!(entries.iter().any(|e| e.message == "test info message"));
        assert!(entries.iter().any(|e| e.message == "test warn message"));
        assert!(entries.iter().any(|e| e.message == "test error message"));
    }

    /// 测试通过 ChannelRef 访问命名通道
    #[test]
    fn test_channel_access() {
        let section = make_log_section();
        let facade = LogFacade::new(&section);

        // file 通道存在
        let file_channel = facade.channel("file");
        assert!(file_channel.is_some());
        let file_channel = file_channel.unwrap();
        assert_eq!(file_channel.name(), "file");

        // console 通道存在
        let console_channel = facade.channel("console");
        assert!(console_channel.is_some());

        // 不存在的通道返回 None
        assert!(facade.channel("nonexistent").is_none());
    }

    /// 测试 console 通道（level=debug）能记录所有级别
    #[test]
    fn test_console_channel_debug_level() {
        let section = make_log_section();
        let facade = LogFacade::new(&section);

        let console = facade.channel("console").unwrap();
        console.debug("debug msg");
        console.info("info msg");
        console.warn("warn msg");
        console.error("error msg");

        // console 通道 level=debug，所有级别都应记录
        let guard = facade.channels.read();
        let console_logger = guard.get("console").unwrap();
        let entries = console_logger.entries();
        assert_eq!(entries.len(), 4);
    }

    /// 测试 LogFacade init 全局单例
    #[test]
    fn test_log_facade_init_singleton() {
        let section = make_log_section();
        let facade = LogFacade::init(&section);

        // instance() 应返回同一实例
        let facade2 = LogFacade::instance();
        assert!(facade2.is_some());
        assert!(std::ptr::eq(facade, facade2.unwrap()));

        // 再次 init 应返回同一实例（不覆盖）
        let section2 = make_log_section();
        let facade3 = LogFacade::init(&section2);
        assert!(std::ptr::eq(facade, facade3));
    }

    /// 测试从实际配置文件加载日志配置
    #[test]
    fn test_load_from_config_file() {
        // 查找 config 目录
        let config_dir = std::env::current_dir().ok().and_then(|d| {
            let mut current = d.clone();
            for _ in 0..5 {
                if current.join("config").exists() {
                    return Some(current.join("config"));
                }
                if let Some(parent) = current.parent() {
                    current = parent.to_path_buf();
                } else {
                    break;
                }
            }
            None
        });

        let Some(config_dir) = config_dir else {
            eprintln!("跳过：未找到 config 目录");
            return;
        };

        let log_path = config_dir.join("log.yml");
        if !log_path.exists() {
            eprintln!("跳过：未找到 log.yml");
            return;
        }

        let content = std::fs::read_to_string(&log_path).unwrap();
        let section: LogSection = serde_yml::from_str(&content).unwrap();

        // 验证默认通道为 file
        assert_eq!(section.default, "file");

        // 验证有 file 和 console 两个通道
        assert!(section.channels.contains_key("file"));
        assert!(section.channels.contains_key("console"));

        // 验证 file 通道配置
        let file_channel = section.channels.get("file").unwrap();
        assert_eq!(file_channel.r#type, "file");
        assert_eq!(file_channel.level, "info");
        assert_eq!(file_channel.max_files, 30);

        // 验证 console 通道配置
        let console_channel = section.channels.get("console").unwrap();
        assert_eq!(console_channel.r#type, "console");
        assert_eq!(console_channel.level, "debug");
    }

    /// 测试 LogFacade::new 处理空 channels（默认通道不存在时用 Info 级别）
    #[test]
    fn test_log_facade_with_empty_channels() {
        let section = LogSection::default();
        let facade = LogFacade::new(&section);

        // 默认通道为空，logger 级别应为 Info（fallback）
        assert_eq!(facade.logger().level(), LogLevel::Info);
        assert_eq!(facade.default_channel(), "");
    }

    /// 测试 LogFacade::Debug 输出
    #[test]
    fn test_log_facade_debug_format() {
        let section = make_log_section();
        let facade = LogFacade::new(&section);

        let debug_str = format!("{:?}", facade);
        assert!(debug_str.contains("LogFacade"));
        assert!(debug_str.contains("file"));
    }
}
