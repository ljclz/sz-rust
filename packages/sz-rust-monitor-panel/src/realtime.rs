// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 实时指标展示（spec §5.26 规则 3，§6.26 规则 3）
//!
//! 定时刷新（刷新周期为正数秒）。

use std::time::Duration;

use chrono::{DateTime, Utc};

/// 指标数据点
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricPoint {
    /// 时间戳
    pub timestamp: DateTime<Utc>,
    /// 值
    pub value: f64,
}

impl MetricPoint {
    /// 创建数据点
    pub fn new(timestamp: DateTime<Utc>, value: f64) -> Self {
        Self { timestamp, value }
    }
}

/// 实时指标配置（spec §5.26 规则 3）
#[derive(Debug, Clone)]
pub struct RealtimeConfig {
    /// 指标名称
    pub metric_name: String,
    /// 刷新间隔（正数秒，spec §6.26 规则 3）
    pub refresh_interval: Duration,
    /// 最大数据点数
    pub max_points: usize,
}

impl RealtimeConfig {
    /// 创建配置
    pub fn new(metric_name: impl Into<String>) -> Self {
        Self {
            metric_name: metric_name.into(),
            refresh_interval: Duration::from_secs(5),
            max_points: 100,
        }
    }

    /// 设置刷新间隔
    pub fn with_refresh_interval(mut self, interval: Duration) -> Self {
        self.refresh_interval = interval;
        self
    }

    /// 设置最大数据点数
    pub fn with_max_points(mut self, max: usize) -> Self {
        self.max_points = max;
        self
    }

    /// 校验配置
    pub fn validate(&self) -> Result<(), crate::error::MonitorPanelError> {
        if self.metric_name.is_empty() {
            return Err(crate::error::MonitorPanelError::Internal(
                "指标名称不能为空".into(),
            ));
        }
        if self.refresh_interval == Duration::ZERO {
            return Err(crate::error::MonitorPanelError::Internal(
                "刷新间隔不能为零".into(),
            ));
        }
        Ok(())
    }
}

/// 实时指标缓冲区
pub struct RealtimeBuffer {
    config: RealtimeConfig,
    points: parking_lot::Mutex<Vec<MetricPoint>>,
}

impl RealtimeBuffer {
    /// 创建缓冲区
    pub fn new(config: RealtimeConfig) -> Self {
        Self {
            config,
            points: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// 添加数据点
    pub fn push(&self, point: MetricPoint) {
        let mut points = self.points.lock();
        points.push(point);
        if points.len() > self.config.max_points {
            let excess = points.len() - self.config.max_points;
            points.drain(0..excess);
        }
    }

    /// 获取所有数据点
    pub fn get_points(&self) -> Vec<MetricPoint> {
        self.points.lock().clone()
    }

    /// 数据点数量
    pub fn len(&self) -> usize {
        self.points.lock().len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 清空
    pub fn clear(&self) {
        self.points.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_realtime_config_validate() {
        let config = RealtimeConfig::new("cpu_usage");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_realtime_config_empty_name() {
        let config = RealtimeConfig::new("");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_realtime_buffer_push() {
        let buffer = RealtimeBuffer::new(RealtimeConfig::new("cpu"));
        buffer.push(MetricPoint::new(Utc::now(), 50.0));
        buffer.push(MetricPoint::new(Utc::now(), 60.0));
        assert_eq!(buffer.len(), 2);
    }

    #[test]
    fn test_realtime_buffer_max_points() {
        let config = RealtimeConfig::new("cpu").with_max_points(3);
        let buffer = RealtimeBuffer::new(config);
        for i in 0..5 {
            buffer.push(MetricPoint::new(Utc::now(), i as f64));
        }
        assert_eq!(buffer.len(), 3);
        let points = buffer.get_points();
        assert_eq!(points[0].value, 2.0);
    }

    #[test]
    fn test_realtime_buffer_clear() {
        let buffer = RealtimeBuffer::new(RealtimeConfig::new("cpu"));
        buffer.push(MetricPoint::new(Utc::now(), 50.0));
        buffer.clear();
        assert!(buffer.is_empty());
    }
}
