// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 插件上下文接口（spec §5.5 禁止项）
//!
//! 禁止 SDK API 直接访问宿主内部状态，仅通过提供的接口访问。

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::error::SdkError;

/// 插件上下文（只读接口，禁止直接访问宿主内部状态）
#[async_trait::async_trait]
pub trait PluginContext: Send + Sync {
    /// 读取配置值
    async fn get_config(&self, key: &str) -> Result<Option<serde_json::Value>, SdkError>;

    /// 读取插件自身状态
    async fn get_state(&self, key: &str) -> Result<Option<serde_json::Value>, SdkError>;

    /// 写入插件自身状态
    async fn set_state(&self, key: &str, value: serde_json::Value) -> Result<(), SdkError>;

    /// 记录日志
    async fn log(&self, level: LogLevel, message: &str) -> Result<(), SdkError>;
}

/// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// 调试
    Debug,
    /// 信息
    Info,
    /// 警告
    Warn,
    /// 错误
    Error,
}

impl LogLevel {
    /// 转为字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// 默认插件上下文实现（基于内存存储）
pub struct DefaultPluginContext {
    /// 宿主配置（只读）
    config: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    /// 插件状态（读写）
    state: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    /// 日志收集
    logs: Arc<RwLock<Vec<(LogLevel, String)>>>,
}

impl DefaultPluginContext {
    /// 创建默认上下文
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(HashMap::new())),
            state: Arc::new(RwLock::new(HashMap::new())),
            logs: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 设置宿主配置（仅宿主可调用）
    pub fn set_config(&self, key: impl Into<String>, value: serde_json::Value) {
        self.config.write().insert(key.into(), value);
    }

    /// 获取日志快照
    pub fn logs(&self) -> Vec<(LogLevel, String)> {
        self.logs.read().clone()
    }
}

impl Default for DefaultPluginContext {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl PluginContext for DefaultPluginContext {
    async fn get_config(&self, key: &str) -> Result<Option<serde_json::Value>, SdkError> {
        Ok(self.config.read().get(key).cloned())
    }

    async fn get_state(&self, key: &str) -> Result<Option<serde_json::Value>, SdkError> {
        Ok(self.state.read().get(key).cloned())
    }

    async fn set_state(&self, key: &str, value: serde_json::Value) -> Result<(), SdkError> {
        self.state.write().insert(key.to_string(), value);
        Ok(())
    }

    async fn log(&self, level: LogLevel, message: &str) -> Result<(), SdkError> {
        self.logs.write().push((level, message.to_string()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_config_read() {
        let ctx = DefaultPluginContext::new();
        ctx.set_config("theme", serde_json::json!("dark"));

        let result = ctx.get_config("theme").await.unwrap();
        assert_eq!(result, Some(serde_json::json!("dark")));

        let missing = ctx.get_config("nonexistent").await.unwrap();
        assert_eq!(missing, None);
    }

    #[tokio::test]
    async fn test_state_read_write() {
        let ctx = DefaultPluginContext::new();

        ctx.set_state("counter", serde_json::json!(42))
            .await
            .unwrap();
        let value = ctx.get_state("counter").await.unwrap();
        assert_eq!(value, Some(serde_json::json!(42)));

        ctx.set_state("counter", serde_json::json!(43))
            .await
            .unwrap();
        let updated = ctx.get_state("counter").await.unwrap();
        assert_eq!(updated, Some(serde_json::json!(43)));
    }

    #[tokio::test]
    async fn test_logging() {
        let ctx = DefaultPluginContext::new();
        ctx.log(LogLevel::Info, "插件启动").await.unwrap();
        ctx.log(LogLevel::Warn, "配置缺失").await.unwrap();

        let logs = ctx.logs();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].0, LogLevel::Info);
        assert_eq!(logs[0].1, "插件启动");
    }

    #[test]
    fn test_log_level_as_str() {
        assert_eq!(LogLevel::Debug.as_str(), "debug");
        assert_eq!(LogLevel::Info.as_str(), "info");
        assert_eq!(LogLevel::Warn.as_str(), "warn");
        assert_eq!(LogLevel::Error.as_str(), "error");
    }
}
