// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 分布式追踪采样策略（spec 5.10）
//!
//! 提供尾部采样/概率采样/速率限制采样多种策略。

#![forbid(unsafe_code)]

pub mod probabilistic_sampler;
pub mod rate_limit_sampler;
pub mod sampler_chain;
pub mod tail_sampler;

use std::time::Duration;

/// 采样决策
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleDecision {
    /// 采样保留
    Sample,
    /// 丢弃
    Drop,
    /// 待定（尾部采样缓冲中）
    Pending,
}

/// Trace 摘要信息
#[derive(Debug, Clone)]
pub struct TraceSummary {
    /// Trace ID
    pub trace_id: String,
    /// 总耗时
    pub duration: Duration,
    /// 是否包含错误
    pub has_error: bool,
    /// Span 数量
    pub span_count: usize,
    /// 自定义属性
    pub attributes: std::collections::HashMap<String, String>,
}

/// 采样器 trait
pub trait Sampler: Send + Sync {
    /// 判断是否采样
    fn should_sample(&self, trace: &TraceSummary) -> SampleDecision;
}

/// 采样统计快照（spec 5.10.9）
#[derive(Debug, Clone)]
pub struct SamplerStatsSnapshot {
    /// 总 Trace 数
    pub total_traces: u64,
    /// 已采样 Trace 数
    pub sampled_traces: u64,
    /// 已丢弃 Trace 数
    pub dropped_traces: u64,
}

/// 采样统计
#[derive(Debug, Default)]
pub struct SamplerStats {
    total: std::sync::atomic::AtomicU64,
    sampled: std::sync::atomic::AtomicU64,
    dropped: std::sync::atomic::AtomicU64,
}

impl SamplerStats {
    /// 创建统计器
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录采样决策
    pub fn record(&self, decision: SampleDecision) {
        use std::sync::atomic::Ordering;
        self.total.fetch_add(1, Ordering::Relaxed);
        match decision {
            SampleDecision::Sample => {
                self.sampled.fetch_add(1, Ordering::Relaxed);
            }
            SampleDecision::Drop => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            SampleDecision::Pending => {}
        }
    }

    /// 获取快照
    pub fn snapshot(&self) -> SamplerStatsSnapshot {
        use std::sync::atomic::Ordering;
        SamplerStatsSnapshot {
            total_traces: self.total.load(Ordering::Relaxed),
            sampled_traces: self.sampled.load(Ordering::Relaxed),
            dropped_traces: self.dropped.load(Ordering::Relaxed),
        }
    }
}
