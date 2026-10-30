// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 速率限制采样器（spec 5.10.5）
//!
//! 时间窗口内限制采样 Trace 数量，配额正整数（spec 6.7.3）。

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::{SampleDecision, Sampler, TraceSummary};

/// 速率限制采样器
pub struct RateLimitSampler {
    /// 时间窗口（默认 1s）
    window: Duration,
    /// 窗口内最大采样数
    limit: u64,
    /// 当前窗口开始时间
    window_start: parking_lot::Mutex<Instant>,
    /// 当前窗口已采样数
    count: AtomicU64,
}

/// 速率限制配置错误
#[derive(Debug, thiserror::Error)]
pub enum RateLimitSamplerError {
    #[error("速率限制配额必须为正整数: {0}")]
    ZeroLimit(u64),
}

impl RateLimitSampler {
    /// 创建速率限制采样器
    ///
    /// # 参数
    ///
    /// - `limit_per_second`：每秒最大采样数（正整数，spec 6.7.3）
    pub fn new(limit_per_second: u64) -> Result<Self, RateLimitSamplerError> {
        if limit_per_second == 0 {
            return Err(RateLimitSamplerError::ZeroLimit(limit_per_second));
        }
        Ok(Self {
            window: Duration::from_secs(1),
            limit: limit_per_second,
            window_start: parking_lot::Mutex::new(Instant::now()),
            count: AtomicU64::new(0),
        })
    }

    /// 创建自定义窗口的速率限制采样器
    pub fn with_window(limit: u64, window: Duration) -> Result<Self, RateLimitSamplerError> {
        if limit == 0 {
            return Err(RateLimitSamplerError::ZeroLimit(limit));
        }
        Ok(Self {
            window,
            limit,
            window_start: parking_lot::Mutex::new(Instant::now()),
            count: AtomicU64::new(0),
        })
    }

    /// 获取配额
    pub fn limit(&self) -> u64 {
        self.limit
    }
}

impl Sampler for RateLimitSampler {
    fn should_sample(&self, _trace: &TraceSummary) -> SampleDecision {
        let now = Instant::now();
        let mut start = self.window_start.lock();

        // 检查是否需要重置窗口
        if now.duration_since(*start) >= self.window {
            *start = now;
            self.count.store(0, Ordering::Relaxed);
        }

        let current = self.count.load(Ordering::Relaxed);
        if current < self.limit {
            self.count.fetch_add(1, Ordering::Relaxed);
            SampleDecision::Sample
        } else {
            SampleDecision::Drop
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_trace() -> TraceSummary {
        TraceSummary {
            trace_id: "t1".to_string(),
            duration: Duration::from_millis(1),
            has_error: false,
            span_count: 1,
            attributes: HashMap::new(),
        }
    }

    #[test]
    fn test_rate_limit_within_quota() {
        let sampler = RateLimitSampler::new(100).unwrap();
        let trace = make_trace();

        for _ in 0..100 {
            assert_eq!(
                sampler.should_sample(&trace),
                SampleDecision::Sample,
                "配额内应采样"
            );
        }
    }

    #[test]
    fn test_rate_limit_exceeds_quota() {
        let sampler = RateLimitSampler::new(5).unwrap();
        let trace = make_trace();

        for _ in 0..5 {
            assert_eq!(sampler.should_sample(&trace), SampleDecision::Sample);
        }
        // 超过配额应丢弃
        for _ in 0..10 {
            assert_eq!(
                sampler.should_sample(&trace),
                SampleDecision::Drop,
                "超过配额应丢弃"
            );
        }
    }

    #[test]
    fn test_zero_limit_rejected() {
        assert!(RateLimitSampler::new(0).is_err());
        assert!(
            RateLimitSampler::with_window(0, Duration::from_secs(1)).is_err(),
            "with_window 的零配额也应被拒绝"
        );
    }

    #[test]
    fn test_limit_getter() {
        let sampler = RateLimitSampler::new(7).unwrap();
        assert_eq!(sampler.limit(), 7);
    }

    #[test]
    fn test_rate_limit_window_resets() {
        // 极短窗口（1ms）：填满配额后等待窗口轮转，应恢复采样。
        let sampler = RateLimitSampler::with_window(1, Duration::from_millis(1)).unwrap();
        let trace = make_trace();
        assert_eq!(sampler.should_sample(&trace), SampleDecision::Sample);
        assert_eq!(sampler.should_sample(&trace), SampleDecision::Drop);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(
            sampler.should_sample(&trace),
            SampleDecision::Sample,
            "窗口轮转后应恢复采样"
        );
    }
}
