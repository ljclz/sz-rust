// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 流水线指标（spec §5.8 规则 5）
//!
//! 各阶段耗时/并行度/背压触发次数指标。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use crate::stage::StageId;

/// 阶段执行统计
#[derive(Debug, Clone)]
pub struct StageStats {
    /// 阶段标识
    pub stage_id: StageId,
    /// 执行次数
    pub execution_count: u64,
    /// 总耗时
    pub total_duration: Duration,
    /// 最大耗时
    pub max_duration: Duration,
    /// 失败次数
    pub failure_count: u64,
}

impl StageStats {
    /// 创建阶段统计
    pub fn new(stage_id: StageId) -> Self {
        Self {
            stage_id,
            execution_count: 0,
            total_duration: Duration::ZERO,
            max_duration: Duration::ZERO,
            failure_count: 0,
        }
    }

    /// 平均耗时
    pub fn avg_duration(&self) -> Duration {
        if self.execution_count == 0 {
            Duration::ZERO
        } else {
            self.total_duration / self.execution_count as u32
        }
    }

    /// 成功率
    pub fn success_rate(&self) -> f64 {
        if self.execution_count == 0 {
            1.0
        } else {
            (self.execution_count - self.failure_count) as f64 / self.execution_count as f64
        }
    }
}

/// 流水线指标收集器
pub struct PipelineMetrics {
    /// 各阶段统计
    stats: Arc<RwLock<HashMap<StageId, StageStats>>>,
    /// 背压触发总次数
    backpressure_triggers: Arc<RwLock<u64>>,
    /// 流水线执行总次数
    pipeline_executions: Arc<RwLock<u64>>,
}

impl PipelineMetrics {
    /// 创建指标收集器
    pub fn new() -> Self {
        Self {
            stats: Arc::new(RwLock::new(HashMap::new())),
            backpressure_triggers: Arc::new(RwLock::new(0)),
            pipeline_executions: Arc::new(RwLock::new(0)),
        }
    }

    /// 记录阶段执行
    pub fn record_stage_execution(&self, stage_id: &StageId, duration: Duration, success: bool) {
        let mut stats = self.stats.write();
        let entry = stats
            .entry(stage_id.clone())
            .or_insert_with(|| StageStats::new(stage_id.clone()));
        entry.execution_count += 1;
        entry.total_duration += duration;
        if duration > entry.max_duration {
            entry.max_duration = duration;
        }
        if !success {
            entry.failure_count += 1;
        }
    }

    /// 记录背压触发
    pub fn record_backpressure(&self) {
        *self.backpressure_triggers.write() += 1;
    }

    /// 记录流水线执行
    pub fn record_pipeline_execution(&self) {
        *self.pipeline_executions.write() += 1;
    }

    /// 获取阶段统计
    pub fn stage_stats(&self, stage_id: &StageId) -> Option<StageStats> {
        self.stats.read().get(stage_id).cloned()
    }

    /// 获取所有阶段统计
    pub fn all_stage_stats(&self) -> Vec<StageStats> {
        self.stats.read().values().cloned().collect()
    }

    /// 背压触发总次数
    pub fn backpressure_trigger_count(&self) -> u64 {
        *self.backpressure_triggers.read()
    }

    /// 流水线执行总次数
    pub fn pipeline_execution_count(&self) -> u64 {
        *self.pipeline_executions.read()
    }
}

impl Default for PipelineMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// 阶段执行计时器（RAII guard）
pub struct StageTimer {
    stage_id: StageId,
    start: Instant,
    metrics: Arc<PipelineMetrics>,
}

impl StageTimer {
    /// 开始计时
    pub fn start(stage_id: StageId, metrics: Arc<PipelineMetrics>) -> Self {
        Self {
            stage_id,
            start: Instant::now(),
            metrics,
        }
    }

    /// 结束计时并记录
    pub fn finish(self, success: bool) {
        let duration = self.start.elapsed();
        self.metrics
            .record_stage_execution(&self.stage_id, duration, success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_stats_avg_duration() {
        let mut stats = StageStats::new(StageId::new("test"));
        stats.execution_count = 3;
        stats.total_duration = Duration::from_millis(300);
        assert_eq!(stats.avg_duration(), Duration::from_millis(100));
    }

    #[test]
    fn test_stage_stats_success_rate() {
        let mut stats = StageStats::new(StageId::new("test"));
        stats.execution_count = 10;
        stats.failure_count = 2;
        assert!((stats.success_rate() - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_pipeline_metrics_record() {
        let metrics = PipelineMetrics::new();
        let stage_id = StageId::new("parse");

        metrics.record_stage_execution(&stage_id, Duration::from_millis(50), true);
        metrics.record_stage_execution(&stage_id, Duration::from_millis(100), true);
        metrics.record_stage_execution(&stage_id, Duration::from_millis(200), false);

        let stats = metrics.stage_stats(&stage_id).unwrap();
        assert_eq!(stats.execution_count, 3);
        assert_eq!(stats.failure_count, 1);
        assert_eq!(stats.max_duration, Duration::from_millis(200));
        assert!((stats.success_rate() - (2.0 / 3.0)).abs() < 0.001);
    }

    #[test]
    fn test_pipeline_metrics_backpressure() {
        let metrics = PipelineMetrics::new();
        metrics.record_backpressure();
        metrics.record_backpressure();
        assert_eq!(metrics.backpressure_trigger_count(), 2);
    }

    #[test]
    fn test_pipeline_metrics_executions() {
        let metrics = PipelineMetrics::new();
        metrics.record_pipeline_execution();
        metrics.record_pipeline_execution();
        metrics.record_pipeline_execution();
        assert_eq!(metrics.pipeline_execution_count(), 3);
    }

    #[test]
    fn test_stage_timer() {
        let metrics = Arc::new(PipelineMetrics::new());
        let timer = StageTimer::start(StageId::new("test"), metrics.clone());
        std::thread::sleep(Duration::from_millis(10));
        timer.finish(true);

        let stats = metrics.stage_stats(&StageId::new("test")).unwrap();
        assert_eq!(stats.execution_count, 1);
        assert!(stats.total_duration >= Duration::from_millis(10));
    }
}
