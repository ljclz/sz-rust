// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 指标查询（spec §5.26 规则 5）
//!
//! 按时间范围/指标名/标签查询指标。

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::error::MonitorPanelError;
use crate::realtime::MetricPoint;

/// 实时指标查询（spec §5.26 规则 3/5）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetricQuery {
    /// 指标名称
    pub metric_name: String,
    /// 时间范围
    pub time_range: (DateTime<Utc>, DateTime<Utc>),
    /// 标签过滤
    pub labels: HashMap<String, String>,
    /// 采样间隔
    pub step: Duration,
}

impl MetricQuery {
    /// 创建查询
    pub fn new(metric_name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            metric_name: metric_name.into(),
            time_range: (now - chrono::Duration::hours(1), now),
            labels: HashMap::new(),
            step: Duration::from_secs(15),
        }
    }

    /// 设置时间范围
    pub fn with_time_range(mut self, from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        self.time_range = (from, to);
        self
    }

    /// 添加标签
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    /// 设置采样间隔
    pub fn with_step(mut self, step: Duration) -> Self {
        self.step = step;
        self
    }

    /// 校验查询
    pub fn validate(&self) -> Result<(), MonitorPanelError> {
        if self.metric_name.is_empty() {
            return Err(MonitorPanelError::Internal("指标名称不能为空".into()));
        }
        if self.time_range.0 > self.time_range.1 {
            return Err(MonitorPanelError::Internal(
                "时间范围无效: from > to".into(),
            ));
        }
        if self.step == Duration::ZERO {
            return Err(MonitorPanelError::Internal("采样间隔不能为零".into()));
        }
        Ok(())
    }

    /// 生成 PromQL 查询语句
    pub fn to_promql(&self) -> String {
        if self.labels.is_empty() {
            return self.metric_name.clone();
        }
        let labels: Vec<String> = self
            .labels
            .iter()
            .map(|(k, v)| format!("{k}=\"{v}\""))
            .collect();
        format!("{}{{{}}}", self.metric_name, labels.join(","))
    }
}

/// 指标查询服务（spec §5.26 规则 5）
pub struct QueryService;

impl QueryService {
    /// 敏感指标名集合（spec §5.26 禁止项）
    const SENSITIVE_METRICS: &'static [&'static str] =
        &["secret", "password", "token", "api_key", "private_key"];

    /// 检查指标名是否敏感
    pub fn is_sensitive(metric_name: &str) -> bool {
        let lower = metric_name.to_lowercase();
        Self::SENSITIVE_METRICS.iter().any(|s| lower.contains(s))
    }

    /// 脱敏指标名（spec §5.26 禁止项）
    pub fn desensitize_metric_name(metric_name: &str) -> String {
        if Self::is_sensitive(metric_name) {
            "***".to_string()
        } else {
            metric_name.to_string()
        }
    }

    /// 脱敏指标值（spec §5.26 禁止项）
    pub fn desensitize_points(points: Vec<MetricPoint>, metric_name: &str) -> Vec<MetricPoint> {
        if Self::is_sensitive(metric_name) {
            points
                .into_iter()
                .map(|p| MetricPoint::new(p.timestamp, 0.0))
                .collect()
        } else {
            points
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_query_validate() {
        let query = MetricQuery::new("cpu_usage");
        assert!(query.validate().is_ok());
    }

    #[test]
    fn test_metric_query_empty_name() {
        let query = MetricQuery::new("");
        assert!(query.validate().is_err());
    }

    #[test]
    fn test_metric_query_invalid_time_range() {
        let now = Utc::now();
        let query = MetricQuery::new("cpu").with_time_range(now, now - chrono::Duration::hours(1));
        assert!(query.validate().is_err());
    }

    #[test]
    fn test_metric_query_to_promql() {
        let query = MetricQuery::new("cpu_usage")
            .with_label("host", "server1")
            .with_label("env", "prod");
        let promql = query.to_promql();
        assert!(promql.contains("cpu_usage{"));
        assert!(promql.contains("host=\"server1\""));
        assert!(promql.contains("env=\"prod\""));
    }

    #[test]
    fn test_metric_query_to_promql_no_labels() {
        let query = MetricQuery::new("cpu_usage");
        assert_eq!(query.to_promql(), "cpu_usage");
    }

    #[test]
    fn test_is_sensitive() {
        assert!(QueryService::is_sensitive("api_key"));
        assert!(QueryService::is_sensitive("user_password"));
        assert!(!QueryService::is_sensitive("cpu_usage"));
    }

    #[test]
    fn test_desensitize_metric_name() {
        assert_eq!(QueryService::desensitize_metric_name("api_key"), "***");
        assert_eq!(
            QueryService::desensitize_metric_name("cpu_usage"),
            "cpu_usage"
        );
    }

    #[test]
    fn test_desensitize_points() {
        let points = vec![
            MetricPoint::new(Utc::now(), 42.0),
            MetricPoint::new(Utc::now(), 99.0),
        ];
        let desensitized = QueryService::desensitize_points(points, "secret_value");
        assert_eq!(desensitized[0].value, 0.0);
        assert_eq!(desensitized[1].value, 0.0);
    }
}
