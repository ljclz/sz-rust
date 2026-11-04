// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! Prometheus metrics 自动埋点（spec 5.8）
//!
//! 提供标准化 metrics 埋点 + 命名空间 + 直方图桶配置。

#![forbid(unsafe_code)]

use std::sync::Arc;

/// 直方图桶配置（spec 6.5.3）
#[derive(Debug, Clone)]
pub struct HistogramBucketConfig {
    /// 桶边界（单调递增正数）
    pub buckets: Vec<f64>,
    /// 命名空间前缀
    pub namespace: String,
}

impl Default for HistogramBucketConfig {
    fn default() -> Self {
        Self {
            buckets: vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ],
            namespace: "sz_rust".to_string(),
        }
    }
}

impl HistogramBucketConfig {
    /// 验证桶配置（spec 6.5.3）
    ///
    /// - 桶边界单调递增
    /// - 桶数量 ≤ 20
    pub fn validate(&self) -> Result<(), String> {
        if self.buckets.len() > 20 {
            return Err(format!("桶数量 {} > 20", self.buckets.len()));
        }
        for i in 1..self.buckets.len() {
            if self.buckets[i] <= self.buckets[i - 1] {
                return Err(format!(
                    "桶边界非单调递增: [{}] = {} <= [{}] = {}",
                    i,
                    self.buckets[i],
                    i - 1,
                    self.buckets[i - 1]
                ));
            }
        }
        Ok(())
    }
}

/// Metrics 自动埋点
pub struct MetricsInstrumentation {
    /// 指标命名空间
    namespace: String,
    /// 直方图桶配置
    histogram_buckets: HistogramBucketConfig,
}

impl MetricsInstrumentation {
    /// 创建埋点实例
    pub fn new(namespace: String, histogram_buckets: HistogramBucketConfig) -> Self {
        Self {
            namespace,
            histogram_buckets,
        }
    }

    /// 获取命名空间
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// 获取直方图桶配置
    pub fn histogram_buckets(&self) -> &HistogramBucketConfig {
        &self.histogram_buckets
    }

    /// 生成带命名空间的指标名（spec 5.8.2 + 6.5.1）
    ///
    /// 规则：
    /// - snake_case
    /// - 单位后缀（_seconds/_bytes/_total）
    /// - 命名空间前缀
    pub fn metric_name(&self, name: &str, unit: MetricUnit) -> String {
        let unit_suffix = match unit {
            MetricUnit::Seconds => "_seconds",
            MetricUnit::Bytes => "_bytes",
            MetricUnit::Total => "_total",
            MetricUnit::None => "",
        };
        format!("{}_{}{}", self.namespace, name, unit_suffix)
    }

    /// 验证标签值（spec 5.8.10 + 6.5.4）
    ///
    /// 标签值不得为空，不得包含敏感信息。
    pub fn validate_label(&self, key: &str, value: &str) -> Result<(), String> {
        if value.is_empty() {
            return Err(format!("标签值不得为空: key={}", key));
        }
        const SENSITIVE_PATTERNS: &[&str] = &["password", "secret", "token", "api_key"];
        let lower = key.to_lowercase();
        for pattern in SENSITIVE_PATTERNS {
            if lower.contains(pattern) {
                return Err(format!("标签键包含敏感信息: key={}", key));
            }
        }
        Ok(())
    }
}

/// 指标单位（spec 6.5.1）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricUnit {
    /// 秒
    Seconds,
    /// 字节
    Bytes,
    /// 计数
    Total,
    /// 无单位
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_name_with_namespace_and_unit() {
        let mi = MetricsInstrumentation::new("sz300".to_string(), HistogramBucketConfig::default());
        assert_eq!(
            mi.metric_name("http_request", MetricUnit::Total),
            "sz300_http_request_total"
        );
        assert_eq!(
            mi.metric_name("response_time", MetricUnit::Seconds),
            "sz300_response_time_seconds"
        );
        assert_eq!(
            mi.metric_name("body_size", MetricUnit::Bytes),
            "sz300_body_size_bytes"
        );
        assert_eq!(mi.metric_name("counter", MetricUnit::None), "sz300_counter");
    }

    #[test]
    fn test_metrics_instrumentation_getters() {
        let custom_buckets = HistogramBucketConfig {
            buckets: vec![0.1, 0.5, 1.0],
            namespace: "custom_ns".to_string(),
        };
        let mi = MetricsInstrumentation::new("prod".to_string(), custom_buckets.clone());
        assert_eq!(mi.namespace(), "prod");
        // 使用自定义配置断言（默认配置与 Default 相同，无法杀死 histogram_buckets→Default 变异体）
        assert_eq!(mi.histogram_buckets().namespace, "custom_ns");
        assert_eq!(mi.histogram_buckets().buckets, vec![0.1, 0.5, 1.0]);
    }

    #[test]
    fn test_histogram_bucket_validation() {
        let config = HistogramBucketConfig::default();
        assert!(config.validate().is_ok(), "默认配置应有效");
    }

    #[test]
    fn test_histogram_bucket_too_many() {
        let config = HistogramBucketConfig {
            buckets: vec![1.0; 25],
            namespace: "test".to_string(),
        };
        assert!(config.validate().is_err(), "超过 20 个桶应无效");
    }

    #[test]
    fn test_histogram_bucket_exactly_twenty_valid() {
        // 恰好 20 个严格递增桶 → 有效。`buckets.len() > 20` 的 `>=` 变异体会误报。
        let buckets: Vec<f64> = (0..20).map(|i| i as f64 + 1.0).collect();
        let config = HistogramBucketConfig {
            buckets,
            namespace: "test".to_string(),
        };
        assert!(config.validate().is_ok(), "恰好 20 个递增桶应有效");
    }

    #[test]
    fn test_histogram_bucket_twenty_one_invalid() {
        // 21 个严格递增桶 → 仍应无效（数量超限）。`>`→`==`/`<` 变异体会漏报。
        let buckets: Vec<f64> = (0..21).map(|i| i as f64 + 1.0).collect();
        let config = HistogramBucketConfig {
            buckets,
            namespace: "test".to_string(),
        };
        assert!(config.validate().is_err(), "21 个桶应因数量超限无效");
    }

    #[test]
    fn test_histogram_bucket_not_monotonic() {
        let config = HistogramBucketConfig {
            buckets: vec![1.0, 0.5, 2.0],
            namespace: "test".to_string(),
        };
        assert!(config.validate().is_err(), "非单调递增应无效");
    }

    #[test]
    fn test_label_validation_empty_value() {
        let mi = MetricsInstrumentation::new("sz300".to_string(), HistogramBucketConfig::default());
        assert!(mi.validate_label("method", "").is_err(), "空标签值应无效");
    }

    #[test]
    fn test_label_validation_sensitive_key() {
        let mi = MetricsInstrumentation::new("sz300".to_string(), HistogramBucketConfig::default());
        assert!(
            mi.validate_label("password", "abc").is_err(),
            "敏感键应无效"
        );
        assert!(mi.validate_label("api_key", "xyz").is_err(), "敏感键应无效");
        assert!(mi.validate_label("method", "GET").is_ok(), "正常标签应有效");
    }
}
