// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 内存泄漏检测器（P4-4）
//!
//! 长时运行周期采样 + 增长趋势分析 + Drop 计数验证。

use std::collections::HashMap;
use std::time::Duration;

use crate::leak_report::{GrowthTrend, LeakReport, LeakedResource, ResourceKind, ResourceSnapshot};

/// 泄漏检测配置
#[derive(Debug, Clone)]
pub struct LeakDetectConfig {
    /// 检测总时长
    pub duration: Duration,
    /// 采样间隔
    pub sample_interval: Duration,
    /// 增长阈值（0~1，采样间增长率超过此值视为增长）
    pub growth_threshold: f32,
    /// 资源数上限
    pub resource_limit: usize,
}

impl Default for LeakDetectConfig {
    fn default() -> Self {
        Self {
            duration: Duration::from_secs(5),
            sample_interval: Duration::from_millis(100),
            growth_threshold: 0.1,
            resource_limit: 1000,
        }
    }
}

impl LeakDetectConfig {
    /// 创建配置
    pub fn new(
        duration: Duration,
        sample_interval: Duration,
        growth_threshold: f32,
        resource_limit: usize,
    ) -> Self {
        Self {
            duration,
            sample_interval,
            growth_threshold: growth_threshold.clamp(0.0, 1.0),
            resource_limit,
        }
    }
}

/// 内存泄漏检测器
///
/// 周期采样资源使用情况，分析增长趋势，
/// 资源单调增长不释放判定为泄漏。
pub struct LeakDetector {
    config: LeakDetectConfig,
    samples: Vec<ResourceSnapshot>,
}

impl LeakDetector {
    /// 创建泄漏检测器
    pub fn new(config: LeakDetectConfig) -> Self {
        Self {
            config,
            samples: Vec::new(),
        }
    }

    /// 获取已采集的样本
    pub fn samples(&self) -> &[ResourceSnapshot] {
        &self.samples
    }

    /// 执行泄漏检测
    ///
    /// 在 `config.duration` 时间内，每隔 `config.sample_interval` 调用 `sampler`
    /// 采集资源快照，结束后分析增长趋势。
    ///
    /// `sampler` 接收当前采样索引（从 0 开始），返回各资源类型的当前持有数。
    pub async fn detect<F, Fut>(&mut self, sampler: F) -> LeakReport
    where
        F: Fn(u64) -> Fut + Send + Sync,
        Fut: std::future::Future<Output = HashMap<ResourceKind, usize>> + Send,
    {
        self.samples.clear();
        let start = std::time::Instant::now();
        let mut sample_idx: u64 = 0;

        while start.elapsed() < self.config.duration {
            let counts = sampler(sample_idx).await;
            let snapshot = ResourceSnapshot::new(start.elapsed().as_millis() as u64);
            let mut snapshot = snapshot;
            for (kind, count) in counts {
                snapshot.counts.insert(kind, count);
            }
            self.samples.push(snapshot);
            sample_idx += 1;
            tokio::time::sleep(self.config.sample_interval).await;
        }

        self.analyze()
    }

    /// 分析采样数据，生成泄漏报告
    fn analyze(&self) -> LeakReport {
        if self.samples.len() < 2 {
            return LeakReport::no_leak(self.samples.len());
        }

        let mut leaked_resources: Vec<LeakedResource> = Vec::new();
        let mut root_causes: Vec<String> = Vec::new();
        let mut has_monotonic = false;

        for kind in [
            ResourceKind::ConnectionPool,
            ResourceKind::AsyncTask,
            ResourceKind::CacheEntry,
        ] {
            let counts: Vec<usize> = self
                .samples
                .iter()
                .filter_map(|s| s.counts.get(&kind).copied())
                .collect();

            if counts.len() < 2 {
                continue;
            }

            let trend = Self::analyze_trend(&counts, self.config.growth_threshold);
            if trend == GrowthTrend::MonotonicGrowth {
                has_monotonic = true;
                let leaked_count = counts
                    .last()
                    .unwrap()
                    .saturating_sub(*counts.first().unwrap());
                if leaked_count > 0 {
                    leaked_resources.push(LeakedResource {
                        kind,
                        count: leaked_count,
                    });
                }
                root_causes.push(Self::infer_root_cause(kind));
            }
        }

        if has_monotonic && !leaked_resources.is_empty() {
            LeakReport::leak(
                GrowthTrend::MonotonicGrowth,
                root_causes.join("; "),
                leaked_resources,
                self.samples.len(),
            )
        } else {
            LeakReport::no_leak(self.samples.len())
        }
    }

    /// 分析增长趋势
    fn analyze_trend(counts: &[usize], growth_threshold: f32) -> GrowthTrend {
        if counts.len() < 2 {
            return GrowthTrend::Stable;
        }

        let first = *counts.first().unwrap() as f32;
        let last = *counts.last().unwrap() as f32;

        if first == 0.0 && last > 0.0 {
            return GrowthTrend::MonotonicGrowth;
        }

        if first > 0.0 {
            let growth_rate = (last - first) / first;
            if growth_rate > growth_threshold {
                let mut increasing = true;
                for i in 1..counts.len() {
                    if counts[i] < counts[i - 1] {
                        increasing = false;
                        break;
                    }
                }
                if increasing {
                    return GrowthTrend::MonotonicGrowth;
                }
                return GrowthTrend::Fluctuating;
            }
        }

        GrowthTrend::Stable
    }

    /// 推断疑似根因
    fn infer_root_cause(kind: ResourceKind) -> String {
        match kind {
            ResourceKind::ConnectionPool => "connection not returned to pool".to_string(),
            ResourceKind::AsyncTask => "async task not completed".to_string(),
            ResourceKind::CacheEntry => "cache entry not evicted".to_string(),
        }
    }

    /// 清理临时文件
    ///
    /// 删除指定目录下的所有临时文件，使用 `tokio::fs` 异步操作。
    pub async fn cleanup(&self, temp_dir: &str) -> Result<(), String> {
        let mut entries = tokio::fs::read_dir(temp_dir)
            .await
            .map_err(|e| format!("read_dir failed: {e}"))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| format!("next_entry failed: {e}"))?
        {
            let path = entry.path();
            if path.is_file() {
                tokio::fs::remove_file(&path)
                    .await
                    .map_err(|e| format!("remove_file failed: {e}"))?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_config_default() {
        let cfg = LeakDetectConfig::default();
        assert_eq!(cfg.duration, Duration::from_secs(5));
        assert_eq!(cfg.sample_interval, Duration::from_millis(100));
    }

    #[test]
    fn test_config_clamp() {
        let cfg =
            LeakDetectConfig::new(Duration::from_secs(1), Duration::from_millis(10), 1.5, 100);
        assert!((cfg.growth_threshold - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_analyze_trend_stable() {
        let counts = vec![5, 5, 5, 5, 5];
        let trend = LeakDetector::analyze_trend(&counts, 0.1);
        assert_eq!(trend, GrowthTrend::Stable);
    }

    #[test]
    fn test_analyze_trend_monotonic() {
        let counts = vec![1, 2, 3, 4, 5];
        let trend = LeakDetector::analyze_trend(&counts, 0.1);
        assert_eq!(trend, GrowthTrend::MonotonicGrowth);
    }

    #[test]
    fn test_analyze_trend_fluctuating() {
        let counts = vec![1, 5, 2, 6, 3];
        let trend = LeakDetector::analyze_trend(&counts, 0.1);
        assert_eq!(trend, GrowthTrend::Fluctuating);
    }

    #[tokio::test]
    async fn test_detect_no_leak() {
        let cfg = LeakDetectConfig::new(
            Duration::from_millis(200),
            Duration::from_millis(50),
            0.1,
            100,
        );
        let mut detector = LeakDetector::new(cfg);

        let report = detector
            .detect(|_idx| async move {
                let mut counts = HashMap::new();
                counts.insert(ResourceKind::ConnectionPool, 5);
                counts.insert(ResourceKind::AsyncTask, 3);
                counts
            })
            .await;

        assert!(!report.leaked);
        assert!(report.sample_count >= 2);
    }

    #[tokio::test]
    async fn test_detect_connection_pool_leak() {
        let cfg = LeakDetectConfig::new(
            Duration::from_millis(300),
            Duration::from_millis(50),
            0.1,
            100,
        );
        let mut detector = LeakDetector::new(cfg);
        let counter = Arc::new(AtomicUsize::new(0));

        let report = detector
            .detect(|idx| {
                let c = counter.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    let mut counts = HashMap::new();
                    counts.insert(ResourceKind::ConnectionPool, 5 + idx as usize);
                    counts
                }
            })
            .await;

        assert!(report.leaked, "should detect connection pool leak");
        assert_eq!(report.trend, GrowthTrend::MonotonicGrowth);
        assert!(report
            .root_cause
            .as_deref()
            .unwrap_or("")
            .contains("connection"));
        assert!(report
            .resources
            .iter()
            .any(|r| r.kind == ResourceKind::ConnectionPool));
    }

    #[tokio::test]
    async fn test_detect_async_task_leak() {
        let cfg = LeakDetectConfig::new(
            Duration::from_millis(300),
            Duration::from_millis(50),
            0.1,
            100,
        );
        let mut detector = LeakDetector::new(cfg);

        let report = detector
            .detect(|idx| async move {
                let mut counts = HashMap::new();
                counts.insert(ResourceKind::AsyncTask, 2 + idx as usize * 2);
                counts
            })
            .await;

        assert!(report.leaked, "should detect async task leak");
        assert!(report.root_cause.as_deref().unwrap_or("").contains("task"));
    }

    #[tokio::test]
    async fn test_detect_cache_leak() {
        let cfg = LeakDetectConfig::new(
            Duration::from_millis(300),
            Duration::from_millis(50),
            0.1,
            100,
        );
        let mut detector = LeakDetector::new(cfg);

        let report = detector
            .detect(|idx| async move {
                let mut counts = HashMap::new();
                counts.insert(ResourceKind::CacheEntry, 10 + idx as usize * 3);
                counts
            })
            .await;

        assert!(report.leaked, "should detect cache leak");
        assert!(report.root_cause.as_deref().unwrap_or("").contains("cache"));
    }

    #[tokio::test]
    async fn test_cleanup_removes_files() {
        let temp_dir = std::env::temp_dir().join("sz_rust_leak_test");
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let file1 = temp_dir.join("tmp1.txt");
        let file2 = temp_dir.join("tmp2.txt");
        tokio::fs::write(&file1, "test").await.unwrap();
        tokio::fs::write(&file2, "test").await.unwrap();

        let detector = LeakDetector::new(LeakDetectConfig::default());
        let result = detector.cleanup(temp_dir.to_str().unwrap()).await;
        assert!(result.is_ok());

        let exists1 = tokio::fs::try_exists(&file1).await.unwrap_or(false);
        let exists2 = tokio::fs::try_exists(&file2).await.unwrap_or(false);
        assert!(!exists1, "file1 should be removed");
        assert!(!exists2, "file2 should be removed");

        let _ = tokio::fs::remove_dir(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_cleanup_nonexistent_dir() {
        let detector = LeakDetector::new(LeakDetectConfig::default());
        let result = detector
            .cleanup("/nonexistent/path/that/does/not/exist")
            .await;
        assert!(result.is_err());
    }
}
