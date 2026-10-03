// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 日志聚合核心逻辑（spec §5.13）
//!
//! 结构化日志 + 采样 + trace 关联 + 日志查询 API。

use std::collections::HashMap;
use std::time::Instant;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::LogAggregatorError;

/// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LogLevel {
    /// 跟踪
    Trace,
    /// 调试
    Debug,
    /// 信息
    Info,
    /// 警告
    Warn,
    /// 错误
    Error,
    /// 致命
    Fatal,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Trace => write!(f, "TRACE"),
            LogLevel::Debug => write!(f, "DEBUG"),
            LogLevel::Info => write!(f, "INFO"),
            LogLevel::Warn => write!(f, "WARN"),
            LogLevel::Error => write!(f, "ERROR"),
            LogLevel::Fatal => write!(f, "FATAL"),
        }
    }
}

/// 结构化日志条目（spec §5.13 规则 1）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogEntry {
    /// 时间戳
    pub timestamp: DateTime<Utc>,
    /// 级别
    pub level: LogLevel,
    /// 消息
    pub message: String,
    /// 字段
    pub fields: HashMap<String, Value>,
    /// trace_id（spec §5.13 规则 3）
    pub trace_id: Option<String>,
    /// span_id
    pub span_id: Option<String>,
}

impl LogEntry {
    /// 创建新日志条目
    pub fn new(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            level,
            message: message.into(),
            fields: HashMap::new(),
            trace_id: None,
            span_id: None,
        }
    }

    /// 设置 trace_id
    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    /// 设置 span_id
    pub fn with_span_id(mut self, span_id: impl Into<String>) -> Self {
        self.span_id = Some(span_id.into());
        self
    }

    /// 添加字段
    pub fn with_field(mut self, key: impl Into<String>, value: Value) -> Self {
        self.fields.insert(key.into(), value);
        self
    }
}

/// 日志采样配置（spec §5.13 规则 2）
#[derive(Debug, Clone)]
pub struct SamplingConfig {
    /// 采样率（0~1，ERROR 级别 100% 保留，spec §6.13 规则 2）
    pub rate: f64,
    /// 是否保留所有 ERROR
    pub keep_error: bool,
}

impl Default for SamplingConfig {
    fn default() -> Self {
        Self {
            rate: 1.0,
            keep_error: true,
        }
    }
}

impl SamplingConfig {
    /// 验证采样配置
    pub fn validate(&self) -> Result<(), LogAggregatorError> {
        if self.rate < 0.0 || self.rate > 1.0 {
            return Err(LogAggregatorError::Config(format!(
                "采样率必须在 0~1 之间，当前: {}",
                self.rate
            )));
        }
        Ok(())
    }

    /// 判断日志是否应该采样保留
    pub fn should_sample(&self, level: LogLevel) -> bool {
        if self.keep_error && level >= LogLevel::Error {
            return true;
        }
        if self.rate >= 1.0 {
            return true;
        }
        if self.rate <= 0.0 {
            return false;
        }
        // 确定性采样：基于时间戳的伪随机
        let now = Instant::now().elapsed().as_nanos() as f64;
        let frac = (now % 10000.0) / 10000.0;
        frac < self.rate
    }
}

/// 日志查询参数（spec §5.13 规则 4）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogQuery {
    /// trace_id 过滤
    pub trace_id: Option<String>,
    /// 时间范围
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    /// 级别过滤（>= 此级别）
    pub level: Option<LogLevel>,
    /// 关键字过滤
    pub keyword: Option<String>,
    /// 分页
    pub limit: usize,
    /// 偏移
    pub offset: usize,
}

impl Default for LogQuery {
    fn default() -> Self {
        Self {
            trace_id: None,
            time_range: None,
            level: None,
            keyword: None,
            limit: 100,
            offset: 0,
        }
    }
}

/// 日志聚合服务
pub struct LogAggregator {
    sampling: SamplingConfig,
    entries: RwLock<Vec<LogEntry>>,
}

impl LogAggregator {
    /// 创建日志聚合服务
    pub fn new(sampling: SamplingConfig) -> Result<Self, LogAggregatorError> {
        sampling.validate()?;
        Ok(Self {
            sampling,
            entries: RwLock::new(Vec::new()),
        })
    }

    /// 写日志（采样过滤，spec §5.13 规则 2）
    pub async fn write(&self, entry: LogEntry) -> Result<bool, LogAggregatorError> {
        if !self.sampling.should_sample(entry.level) {
            return Ok(false);
        }
        self.entries.write().push(entry);
        Ok(true)
    }

    /// 查询日志（spec §5.13 规则 4，多维度过滤）
    pub async fn query(&self, q: &LogQuery) -> Result<Vec<LogEntry>, LogAggregatorError> {
        let entries = self.entries.read();
        let filtered: Vec<LogEntry> = entries
            .iter()
            .filter(|e| {
                if let Some(ref tid) = q.trace_id {
                    if e.trace_id.as_ref() != Some(tid) {
                        return false;
                    }
                }
                if let Some((start, end)) = q.time_range {
                    if e.timestamp < start || e.timestamp > end {
                        return false;
                    }
                }
                if let Some(level) = q.level {
                    if e.level < level {
                        return false;
                    }
                }
                if let Some(ref kw) = q.keyword {
                    if !e.message.contains(kw) {
                        return false;
                    }
                }
                true
            })
            .skip(q.offset)
            .take(q.limit)
            .cloned()
            .collect();
        Ok(filtered)
    }

    /// 日志总数
    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 获取采样配置
    pub fn sampling(&self) -> &SamplingConfig {
        &self.sampling
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_sampling_config_validate() {
        let config = SamplingConfig {
            rate: 0.5,
            keep_error: true,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_sampling_config_invalid_rate() {
        let config = SamplingConfig {
            rate: 1.5,
            keep_error: true,
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_sampling_should_keep_error() {
        let config = SamplingConfig {
            rate: 0.0,
            keep_error: true,
        };
        assert!(config.should_sample(LogLevel::Error));
        assert!(config.should_sample(LogLevel::Fatal));
    }

    #[test]
    fn test_sampling_full_rate() {
        let config = SamplingConfig {
            rate: 1.0,
            keep_error: false,
        };
        assert!(config.should_sample(LogLevel::Info));
    }

    #[test]
    fn test_sampling_zero_rate_non_error() {
        let config = SamplingConfig {
            rate: 0.0,
            keep_error: false,
        };
        assert!(!config.should_sample(LogLevel::Info));
    }

    #[tokio::test]
    async fn test_write_and_query() {
        let agg = LogAggregator::new(SamplingConfig::default()).unwrap();
        let entry = LogEntry::new(LogLevel::Info, "test message")
            .with_trace_id("trace-123")
            .with_span_id("span-456")
            .with_field("key", json!("value"));
        agg.write(entry).await.unwrap();
        assert_eq!(agg.len(), 1);
        let results = agg.query(&LogQuery::default()).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message, "test message");
    }

    #[tokio::test]
    async fn test_query_by_trace_id() {
        let agg = LogAggregator::new(SamplingConfig::default()).unwrap();
        agg.write(LogEntry::new(LogLevel::Info, "msg1").with_trace_id("trace-1"))
            .await
            .unwrap();
        agg.write(LogEntry::new(LogLevel::Info, "msg2").with_trace_id("trace-2"))
            .await
            .unwrap();
        let q = LogQuery {
            trace_id: Some("trace-1".to_string()),
            ..Default::default()
        };
        let results = agg.query(&q).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message, "msg1");
    }

    #[tokio::test]
    async fn test_query_by_level() {
        let agg = LogAggregator::new(SamplingConfig::default()).unwrap();
        agg.write(LogEntry::new(LogLevel::Info, "info"))
            .await
            .unwrap();
        agg.write(LogEntry::new(LogLevel::Warn, "warn"))
            .await
            .unwrap();
        agg.write(LogEntry::new(LogLevel::Error, "error"))
            .await
            .unwrap();
        let q = LogQuery {
            level: Some(LogLevel::Warn),
            ..Default::default()
        };
        let results = agg.query(&q).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_query_by_keyword() {
        let agg = LogAggregator::new(SamplingConfig::default()).unwrap();
        agg.write(LogEntry::new(LogLevel::Info, "database error"))
            .await
            .unwrap();
        agg.write(LogEntry::new(LogLevel::Info, "cache miss"))
            .await
            .unwrap();
        let q = LogQuery {
            keyword: Some("error".to_string()),
            ..Default::default()
        };
        let results = agg.query(&q).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message, "database error");
    }

    #[tokio::test]
    async fn test_query_by_time_range() {
        let agg = LogAggregator::new(SamplingConfig::default()).unwrap();
        let now = Utc::now();
        let mut entry = LogEntry::new(LogLevel::Info, "old");
        entry.timestamp = now - chrono::Duration::hours(2);
        agg.write(entry).await.unwrap();
        agg.write(LogEntry::new(LogLevel::Info, "recent"))
            .await
            .unwrap();
        let q = LogQuery {
            time_range: Some((
                now - chrono::Duration::minutes(5),
                now + chrono::Duration::minutes(5),
            )),
            ..Default::default()
        };
        let results = agg.query(&q).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message, "recent");
    }

    #[tokio::test]
    async fn test_query_pagination() {
        let agg = LogAggregator::new(SamplingConfig::default()).unwrap();
        for i in 0..10 {
            agg.write(LogEntry::new(LogLevel::Info, format!("msg{i}")))
                .await
                .unwrap();
        }
        let q = LogQuery {
            limit: 3,
            offset: 2,
            ..Default::default()
        };
        let results = agg.query(&q).await.unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].message, "msg2");
    }

    #[tokio::test]
    async fn test_sampling_drops_non_error() {
        let agg = LogAggregator::new(SamplingConfig {
            rate: 0.0,
            keep_error: true,
        })
        .unwrap();
        agg.write(LogEntry::new(LogLevel::Info, "info"))
            .await
            .unwrap();
        agg.write(LogEntry::new(LogLevel::Error, "error"))
            .await
            .unwrap();
        assert_eq!(agg.len(), 1);
    }

    #[test]
    fn test_log_level_display() {
        assert_eq!(LogLevel::Info.to_string(), "INFO");
        assert_eq!(LogLevel::Error.to_string(), "ERROR");
    }

    #[test]
    fn test_log_entry_builder() {
        let entry = LogEntry::new(LogLevel::Warn, "test")
            .with_trace_id("t1")
            .with_span_id("s1")
            .with_field("count", json!(42));
        assert_eq!(entry.trace_id, Some("t1".to_string()));
        assert_eq!(entry.span_id, Some("s1".to_string()));
        assert_eq!(entry.fields.get("count"), Some(&json!(42)));
    }
}
