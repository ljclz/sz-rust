// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 采样器组合链（spec 5.10.6）
//!
//! 按优先级组合多个采样策略。

#![forbid(unsafe_code)]

use std::sync::Arc;

use super::{SampleDecision, Sampler, SamplerStats, SamplerStatsSnapshot, TraceSummary};

/// 采样器链
///
/// 按优先级组合策略（尾部优先 + 概率兜底，spec 5.10.6）。
pub struct SamplerChain {
    samplers: Vec<Box<dyn Sampler>>,
    stats: Arc<SamplerStats>,
}

impl SamplerChain {
    /// 创建采样器链
    pub fn new(samplers: Vec<Box<dyn Sampler>>) -> Self {
        Self {
            samplers,
            stats: Arc::new(SamplerStats::new()),
        }
    }

    /// 决策：按优先级遍历采样器，第一个非 Pending 决策生效
    ///
    /// 规则（spec 5.10.6）：
    /// - 某采样器返回 Sample → 采样
    /// - 某采样器返回 Drop → 丢弃
    /// - 所有采样器返回 Pending → 丢弃（默认不采样）
    pub fn decide(&self, trace: &TraceSummary) -> SampleDecision {
        let mut final_decision = SampleDecision::Drop;

        for sampler in &self.samplers {
            let decision = sampler.should_sample(trace);
            if decision != SampleDecision::Pending {
                final_decision = decision;
                break;
            }
        }

        self.stats.record(final_decision);
        final_decision
    }

    /// 获取统计快照（spec 5.10.9）
    pub fn stats_snapshot(&self) -> SamplerStatsSnapshot {
        self.stats.snapshot()
    }
}

impl Sampler for SamplerChain {
    fn should_sample(&self, trace: &TraceSummary) -> SampleDecision {
        self.decide(trace)
    }
}

#[cfg(test)]
mod tests {
    use super::super::probabilistic_sampler::ProbabilisticSampler;
    use super::super::tail_sampler::{TailSampler, TailSamplerConfig};
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;

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
    fn test_chain_error_trace_sampled_by_tail() {
        let tail = TailSampler::new(TailSamplerConfig::default());
        let prob = ProbabilisticSampler::new(0.0).unwrap(); // 0% 采样
        let chain = SamplerChain::new(vec![Box::new(tail), Box::new(prob)]);

        let trace = make_trace(true, Duration::from_millis(1));
        let decision = chain.decide(&trace);
        assert_eq!(
            decision,
            SampleDecision::Sample,
            "错误链路应被尾部采样器保留"
        );
    }

    #[test]
    fn test_chain_normal_trace_fallback_to_probability() {
        let tail = TailSampler::new(TailSamplerConfig::default());
        let prob = ProbabilisticSampler::new(1.0).unwrap(); // 100% 采样
        let chain = SamplerChain::new(vec![Box::new(tail), Box::new(prob)]);

        let trace = make_trace(false, Duration::from_millis(1));
        let decision = chain.decide(&trace);
        // 尾部返回 Pending → 概率采样器返回 Sample
        assert_eq!(decision, SampleDecision::Sample);
    }

    #[test]
    fn test_chain_stats_tracked() {
        let prob = ProbabilisticSampler::new(1.0).unwrap();
        let chain = SamplerChain::new(vec![Box::new(prob)]);

        let trace = make_trace(false, Duration::from_millis(1));
        chain.decide(&trace);
        chain.decide(&trace);
        chain.decide(&trace);

        let stats = chain.stats_snapshot();
        assert_eq!(stats.total_traces, 3);
        assert_eq!(stats.sampled_traces, 3);
        assert_eq!(stats.dropped_traces, 0);
    }

    #[test]
    fn test_chain_drop_stats() {
        let prob = ProbabilisticSampler::new(0.0).unwrap();
        let chain = SamplerChain::new(vec![Box::new(prob)]);

        let trace = make_trace(false, Duration::from_millis(1));
        chain.decide(&trace);

        let stats = chain.stats_snapshot();
        assert_eq!(stats.total_traces, 1);
        assert_eq!(stats.dropped_traces, 1);
    }
}
