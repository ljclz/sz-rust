// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 运行时日志级别动态切换模块
//!
//! 提供 [`LogLevel`] 枚举与 [`LogLevelManager`] 管理器，支持在运行时
//! 通过信号（如 SIGUSR1/SIGUSR2）或管理 API 动态切换 `tracing` 日志级别，
//! 无需重启进程。
//!
//! # 线程安全
//!
//! [`LogLevelManager`] 内部使用 [`parking_lot::RwLock`] 保护级别状态，
//! 可在多线程环境中安全共享。
//!
//! # 快速入门
//!
//! ```
//! use sz_rust_observability::{LogLevel, LogLevelManager};
//!
//! let manager = LogLevelManager::new();
//! assert_eq!(manager.current_level(), LogLevel::Info);
//!
//! manager.set_level(LogLevel::Debug).unwrap();
//! assert_eq!(manager.current_level(), LogLevel::Debug);
//!
//! let level = LogLevelManager::from_str("warn").unwrap();
//! manager.set_level(level).unwrap();
//! assert_eq!(manager.current_level(), LogLevel::Warn);
//! ```

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use tracing::level_filters::LevelFilter;

/// 日志级别枚举
///
/// 与 `tracing::Level` 一一对应，支持序列化/反序列化与字符串解析。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LogLevel {
    /// 最详细级别，包含所有事件
    Trace,
    /// 调试级别
    Debug,
    /// 信息级别（默认）
    Info,
    /// 警告级别
    Warn,
    /// 错误级别
    Error,
}

#[allow(clippy::derivable_impls)]
impl Default for LogLevel {
    fn default() -> Self {
        LogLevel::Info
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        };
        f.write_str(s)
    }
}

impl FromStr for LogLevel {
    type Err = String;

    /// 从字符串解析日志级别（大小写不敏感）
    ///
    /// ```
    /// use std::str::FromStr;
    /// use sz_rust_observability::LogLevel;
    ///
    /// assert_eq!(LogLevel::from_str("INFO").unwrap(), LogLevel::Info);
    /// assert_eq!(LogLevel::from_str("Debug").unwrap(), LogLevel::Debug);
    /// assert!(LogLevel::from_str("invalid").is_err());
    /// ```
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "trace" => Ok(LogLevel::Trace),
            "debug" => Ok(LogLevel::Debug),
            "info" => Ok(LogLevel::Info),
            "warn" => Ok(LogLevel::Warn),
            "error" => Ok(LogLevel::Error),
            other => Err(format!("unknown log level: {other}")),
        }
    }
}

impl LogLevel {
    /// 转换为 `tracing::Level`
    pub fn to_tracing(self) -> tracing::Level {
        match self {
            LogLevel::Trace => tracing::Level::TRACE,
            LogLevel::Debug => tracing::Level::DEBUG,
            LogLevel::Info => tracing::Level::INFO,
            LogLevel::Warn => tracing::Level::WARN,
            LogLevel::Error => tracing::Level::ERROR,
        }
    }

    /// 转换为 `tracing::level_filters::LevelFilter`
    pub fn to_level_filter(self) -> LevelFilter {
        LevelFilter::from_level(self.to_tracing())
    }
}

/// 日志级别管理器
///
/// 维护运行时日志级别状态，线程安全（内部使用 [`parking_lot::RwLock`]）。
/// 同时维护对应的 [`LevelFilter`]，供外部通过
/// [`current_filter`](Self::current_filter) 获取后应用到
/// `tracing` subscriber（如 `tracing_subscriber` 的 reload handle）。
pub struct LogLevelManager {
    level: RwLock<LogLevel>,
    filter: RwLock<LevelFilter>,
}

impl Default for LogLevelManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LogLevelManager {
    /// 创建管理器，默认级别为 [`LogLevel::Info`]
    pub fn new() -> Self {
        Self {
            level: RwLock::new(LogLevel::Info),
            filter: RwLock::new(LogLevel::Info.to_level_filter()),
        }
    }

    /// 动态切换日志级别
    ///
    /// 更新内部状态（[`LogLevel`] 与 [`LevelFilter`]）。实际生效需由应用
    /// 在初始化时使用 `tracing_subscriber` 的 reload handle，或通过
    /// [`current_filter`](Self::current_filter) 获取最新过滤器后重新安装 subscriber。
    ///
    /// ```
    /// use sz_rust_observability::{LogLevel, LogLevelManager};
    ///
    /// let manager = LogLevelManager::new();
    /// manager.set_level(LogLevel::Error).unwrap();
    /// assert_eq!(manager.current_level(), LogLevel::Error);
    /// ```
    pub fn set_level(&self, level: LogLevel) -> Result<(), String> {
        let new_filter = level.to_level_filter();
        *self.level.write() = level;
        *self.filter.write() = new_filter;
        Ok(())
    }

    /// 获取当前日志级别
    pub fn current_level(&self) -> LogLevel {
        *self.level.read()
    }

    /// 获取当前日志级别对应的 [`LevelFilter`]
    ///
    /// 外部可据此通过 `tracing_subscriber::reload::Handle::set_new_filter`
    /// 动态更新全局 subscriber 的过滤级别。
    pub fn current_filter(&self) -> LevelFilter {
        *self.filter.read()
    }

    /// 从字符串解析日志级别
    ///
    /// ```
    /// use sz_rust_observability::{LogLevel, LogLevelManager};
    ///
    /// let level = LogLevelManager::from_str("trace").unwrap();
    /// assert_eq!(level, LogLevel::Trace);
    /// ```
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<LogLevel, String> {
        LogLevel::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_level_is_info() {
        let manager = LogLevelManager::new();
        assert_eq!(manager.current_level(), LogLevel::Info);
    }

    #[test]
    fn test_set_and_get_level() {
        let manager = LogLevelManager::new();
        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
        ] {
            manager.set_level(level).unwrap();
            assert_eq!(manager.current_level(), level);
        }
    }

    #[test]
    fn test_from_str_valid() {
        assert_eq!(LogLevel::from_str("trace").unwrap(), LogLevel::Trace);
        assert_eq!(LogLevel::from_str("debug").unwrap(), LogLevel::Debug);
        assert_eq!(LogLevel::from_str("info").unwrap(), LogLevel::Info);
        assert_eq!(LogLevel::from_str("warn").unwrap(), LogLevel::Warn);
        assert_eq!(LogLevel::from_str("error").unwrap(), LogLevel::Error);
    }

    #[test]
    fn test_from_str_case_insensitive() {
        assert_eq!(LogLevel::from_str("TRACE").unwrap(), LogLevel::Trace);
        assert_eq!(LogLevel::from_str("Info").unwrap(), LogLevel::Info);
        assert_eq!(LogLevel::from_str("ERROR").unwrap(), LogLevel::Error);
    }

    #[test]
    fn test_from_str_invalid() {
        assert!(LogLevel::from_str("verbose").is_err());
        assert!(LogLevel::from_str("").is_err());
        assert!(LogLevel::from_str("123").is_err());
    }

    #[test]
    fn test_display() {
        assert_eq!(LogLevel::Trace.to_string(), "trace");
        assert_eq!(LogLevel::Debug.to_string(), "debug");
        assert_eq!(LogLevel::Info.to_string(), "info");
        assert_eq!(LogLevel::Warn.to_string(), "warn");
        assert_eq!(LogLevel::Error.to_string(), "error");
    }

    #[test]
    fn test_serde_roundtrip() {
        let level = LogLevel::Warn;
        let json = serde_json::to_string(&level).unwrap();
        assert_eq!(json, "\"Warn\"");
        let deserialized: LogLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, level);
    }

    #[test]
    fn test_to_tracing_level() {
        assert_eq!(LogLevel::Trace.to_tracing(), tracing::Level::TRACE);
        assert_eq!(LogLevel::Debug.to_tracing(), tracing::Level::DEBUG);
        assert_eq!(LogLevel::Info.to_tracing(), tracing::Level::INFO);
        assert_eq!(LogLevel::Warn.to_tracing(), tracing::Level::WARN);
        assert_eq!(LogLevel::Error.to_tracing(), tracing::Level::ERROR);
    }

    #[test]
    fn test_to_level_filter() {
        assert_eq!(LogLevel::Trace.to_level_filter(), LevelFilter::TRACE);
        assert_eq!(LogLevel::Debug.to_level_filter(), LevelFilter::DEBUG);
        assert_eq!(LogLevel::Info.to_level_filter(), LevelFilter::INFO);
        assert_eq!(LogLevel::Warn.to_level_filter(), LevelFilter::WARN);
        assert_eq!(LogLevel::Error.to_level_filter(), LevelFilter::ERROR);
    }

    #[test]
    fn test_manager_from_str() {
        let level = LogLevelManager::from_str("debug").unwrap();
        assert_eq!(level, LogLevel::Debug);
        assert!(LogLevelManager::from_str("invalid").is_err());
    }

    #[test]
    fn test_current_filter_updates_with_level() {
        let manager = LogLevelManager::new();
        assert_eq!(manager.current_filter(), LevelFilter::INFO);
        manager.set_level(LogLevel::Trace).unwrap();
        assert_eq!(manager.current_filter(), LevelFilter::TRACE);
        manager.set_level(LogLevel::Error).unwrap();
        assert_eq!(manager.current_filter(), LevelFilter::ERROR);
    }
}
