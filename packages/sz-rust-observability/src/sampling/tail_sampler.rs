// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 尾部采样器（spec 5.10.1-5.10.3）
//!
//! Trace 完成后按耗时/错误/属性决策。

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::{SampleDecision, Sampler, TraceSummary};

/// 尾部采样配置
#[derive(Debug, Clone)]
pub struct TailSamplerConfig {
    /// 是否 100% 保留错误链路（默认 true，spec 6.7.5）
    pub error_keep: bool,
    /// 慢链路阈值（超过则保留，spec 5.10.3）
    pub slow_threshold: Duration,
    /// 缓冲上限（spec 5.10.3 异常1）
    pub buffer_limit: usize,
    /// 缓冲溢出时的降级概率采样率
    pub fallback_probability: f64,
}

impl Default for TailSamplerConfig {
    fn default() -> Self {
        Self {
            error_keep: true,
            slow_threshold: Duration::from_millis(500),
            buffer_limit: 1000,
            fallback_probability: 0.1,
        }
    }
}

/// 尾部采样器
pub struct TailSampler {
    config: TailSamplerConfig,
    buffer_count: AtomicUsize,
}

impl TailSampler {
    /// 创建尾部采样器
    pub fn new(config: TailSamplerConfig) -> Self {
        Self {
            config,
            buffer_count: AtomicUsize::new(0),
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &TailSamplerConfig {
        &self.config
    }

    /// 当前缓冲使用量
    pub fn buffer_usage(&self) -> usize {
        self.buffer_count.load(Ordering::Relaxed)
    }
}

impl Sampler for TailSampler {
    fn should_sample(&self, trace: &TraceSummary) -> SampleDecision {
        // 错误链路 100% 保留（spec 5.10.2）
        if self.config.error_keep && trace.has_error {
            return SampleDecision::Sample;
        }

        // 慢链路保留（spec 5.10.3）
        if trace.duration >= self.config.slow_threshold {
            return SampleDecision::Sample;
        }

        // 缓冲溢出检查
        let current = self.buffer_count.load(Ordering::Relaxed);
        if current >= self.config.buffer_limit {
            // 缓冲溢出 → 降级概率采样（spec 5.10.3 异常1）
            let rand_val = simple_hash(&trace.trace_id) as f64 / u64::MAX as f64;
            if rand_val < self.config.fallback_probability {
                return SampleDecision::Sample;
            }
            return SampleDecision::Drop;
        }

        // 正常情况：缓冲中待决策
        self.buffer_count.fetch_add(1, Ordering::Relaxed);
        SampleDecision::Pending
    }
}

/// 简单字符串哈希（用于概率采样的伪随机）
fn simple_hash(s: &str) -> u64 {
    let mut hash: u64 = 14695981039346656037;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_trace(has_error: bool, duration: Duration) -> TraceSummary {
        TraceSummary {
            trace_id: format!("trace_{}", has_error as u8),
            duration,
            has_error,
            span_count: 1,
            attributes: HashMap::new(),
        }
    }

    #[test]
    fn test_error_trace_always_sampled() {
        let sampler = TailSampler::new(TailSamplerConfig::default());
        let trace = make_trace(true, Duration::from_millis(1));
        assert_eq!(
            sampler.should_sample(&trace),
            SampleDecision::Sample,
            "错误链路应 100% 保留"
        );
    }

    #[test]
    fn test_slow_trace_sampled() {
        let config = TailSamplerConfig {
            slow_threshold: Duration::from_millis(100),
            ..Default::default()
        };
        let sampler = TailSampler::new(config);
        let trace = make_trace(false, Duration::from_millis(200));
        assert_eq!(
            sampler.should_sample(&trace),
            SampleDecision::Sample,
            "慢链路应保留"
        );
    }

    #[test]
    fn test_normal_trace_pending() {
        let sampler = TailSampler::new(TailSamplerConfig::default());
        let trace = make_trace(false, Duration::from_millis(10));
        assert_eq!(
            sampler.should_sample(&trace),
            SampleDecision::Pending,
            "普通链路应进入待定状态"
        );
    }

    #[test]
    fn test_config_getter() {
        let config = TailSamplerConfig {
            error_keep: false,
            slow_threshold: Duration::from_millis(123),
            buffer_limit: 42,
            fallback_probability: 0.5,
        };
        let sampler = TailSampler::new(config.clone());
        assert_eq!(sampler.config().error_keep, false);
        assert_eq!(sampler.config().slow_threshold, Duration::from_millis(123));
        assert_eq!(sampler.config().buffer_limit, 42);
        assert!((sampler.config().fallback_probability - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_slow_trace_at_threshold_sampled() {
        // duration 恰好等于 slow_threshold → 应采样；`>=`→`<` 变异体会误判为 Pending。
        let config = TailSamplerConfig {
            slow_threshold: Duration::from_millis(100),
            ..Default::default()
        };
        let sampler = TailSampler::new(config);
        let trace = make_trace(false, Duration::from_millis(100));
        assert_eq!(
            sampler.should_sample(&trace),
            SampleDecision::Sample,
            "耗时等于阈值时应采样"
        );
    }

    #[test]
    fn test_error_keep_disabled_not_auto_sampled() {
        // error_keep=false 时错误链路不应被 100% 保留（`&&`→`||` 变异体会误采样）。
        let config = TailSamplerConfig {
            error_keep: false,
            ..Default::default()
        };
        let sampler = TailSampler::new(config);
        let trace = make_trace(true, Duration::from_millis(1));
        assert_ne!(
            sampler.should_sample(&trace),
            SampleDecision::Sample,
            "error_keep=false 时错误链路不应自动采样"
        );
    }

    #[test]
    fn test_buffer_usage_tracks_pending() {
        let sampler = TailSampler::new(TailSamplerConfig::default());
        assert_eq!(sampler.buffer_usage(), 0);
        let trace = make_trace(false, Duration::from_millis(1));
        sampler.should_sample(&trace);
        sampler.should_sample(&trace);
        assert_eq!(sampler.buffer_usage(), 2, "两次 Pending 后缓冲使用量应为 2");
    }

    #[test]
    fn test_simple_hash_exact() {
        // FNV-1a 64 位精确值；杀死 simple_hash 返回 0/1 及 `^=`→`|=`/`&=` 变异体。
        assert_eq!(simple_hash("abc"), 16654208175385433931);
        assert_eq!(simple_hash(""), 14695981039346656037);
    }

    #[test]
    fn test_buffer_overflow_fallback_sampling() {
        let config = TailSamplerConfig {
            buffer_limit: 2,
            fallback_probability: 1.0, // 100% 采样
            ..Default::default()
        };
        let sampler = TailSampler::new(config);

        // 填满缓冲
        let trace1 = TraceSummary {
            trace_id: "t1".to_string(),
            duration: Duration::from_millis(1),
            has_error: false,
            span_count: 1,
            attributes: HashMap::new(),
        };
        sampler.should_sample(&trace1);
        sampler.should_sample(&trace1);

        // 缓冲溢出 → 降级概率采样（100% 应采样）
        let trace2 = TraceSummary {
            trace_id: "t2".to_string(),
            duration: Duration::from_millis(1),
            has_error: false,
            span_count: 1,
            attributes: HashMap::new(),
        };
        let decision = sampler.should_sample(&trace2);
        assert_eq!(
            decision,
            SampleDecision::Sample,
            "缓冲溢出 + fallback_probability=1.0 应采样"
        );
    }
}
