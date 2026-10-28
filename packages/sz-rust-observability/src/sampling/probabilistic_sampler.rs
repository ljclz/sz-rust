// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 概率采样器（spec 5.10.4）
//!
//! 按配置概率随机采样 Trace，概率 0~1（spec 6.7.2）。

#![forbid(unsafe_code)]

use super::{SampleDecision, Sampler, TraceSummary};

/// 概率采样器
pub struct ProbabilisticSampler {
    /// 采样概率（0.0~1.0，spec 6.7.2）
    probability: f64,
}

/// 概率采样配置错误
#[derive(Debug, thiserror::Error)]
pub enum ProbabilisticSamplerError {
    #[error("概率超出范围 [0, 1]: {0}")]
    OutOfRange(f64),
}

impl ProbabilisticSampler {
    /// 创建概率采样器
    ///
    /// # 错误
    ///
    /// 概率不在 [0, 1] 范围时返回错误（spec 6.7.2）
    pub fn new(probability: f64) -> Result<Self, ProbabilisticSamplerError> {
        if !(0.0..=1.0).contains(&probability) {
            return Err(ProbabilisticSamplerError::OutOfRange(probability));
        }
        Ok(Self { probability })
    }

    /// 获取采样概率
    pub fn probability(&self) -> f64 {
        self.probability
    }
}

impl Sampler for ProbabilisticSampler {
    fn should_sample(&self, trace: &TraceSummary) -> SampleDecision {
        if self.probability == 0.0 {
            return SampleDecision::Drop;
        }
        if self.probability == 1.0 {
            return SampleDecision::Sample;
        }

        // 使用 trace_id 哈希做伪随机决策
        let hash = simple_hash(&trace.trace_id);
        // 使用模运算归一化到 [0, 1)
        let normalized = (hash % 1000000) as f64 / 1000000.0;
        if normalized < self.probability {
            SampleDecision::Sample
        } else {
            SampleDecision::Drop
        }
    }
}

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

    fn make_trace(id: &str) -> TraceSummary {
        TraceSummary {
            trace_id: id.to_string(),
            duration: std::time::Duration::from_millis(1),
            has_error: false,
            span_count: 1,
            attributes: HashMap::new(),
        }
    }

    #[test]
    fn test_probability_zero_always_drop() {
        let sampler = ProbabilisticSampler::new(0.0).unwrap();
        let trace = make_trace("t1");
        assert_eq!(sampler.should_sample(&trace), SampleDecision::Drop);
    }

    #[test]
    fn test_probability_getter() {
        let sampler = ProbabilisticSampler::new(0.25).unwrap();
        assert!((sampler.probability() - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn test_simple_hash_exact() {
        // FNV-1a 64 位精确值；杀死 simple_hash 返回 0/1 及 `^=`→`|=`/`&=` 变异体。
        assert_eq!(simple_hash("abc"), 16654208175385433931);
        assert_eq!(simple_hash(""), 14695981039346656037);
    }

    #[test]
    fn test_probability_one_always_sample() {
        let sampler = ProbabilisticSampler::new(1.0).unwrap();
        let trace = make_trace("t1");
        assert_eq!(sampler.should_sample(&trace), SampleDecision::Sample);
    }

    #[test]
    fn test_invalid_probability_rejected() {
        assert!(ProbabilisticSampler::new(-0.1).is_err());
        assert!(ProbabilisticSampler::new(1.1).is_err());
    }

    #[test]
    fn test_probability_0_1_approximately_10_percent() {
        let sampler = ProbabilisticSampler::new(0.1).unwrap();
        let mut sampled = 0;
        for i in 0..1000 {
            let trace = make_trace(&format!("trace_{}", i));
            if sampler.should_sample(&trace) == SampleDecision::Sample {
                sampled += 1;
            }
        }
        // 允许 ±50% 误差（哈希分布不保证精确）

        assert!(
            (50..=150).contains(&sampled),
            "概率 0.1 应约 10% 采样, 实际: {}/1000",
            sampled
        );
    }
}
