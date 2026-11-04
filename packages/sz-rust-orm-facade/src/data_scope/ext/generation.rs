// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 策略世代号 — 单调递增的配置版本标识

use std::sync::atomic::{AtomicU64, Ordering};

/// 策略世代号，用于配置重载时的版本追踪与冲突检测
pub struct PolicyGeneration {
    inner: AtomicU64,
}

impl PolicyGeneration {
    pub fn new() -> Self {
        Self {
            inner: AtomicU64::new(0),
        }
    }

    /// 当前世代号（SeqCst 读）
    pub fn current(&self) -> u64 {
        self.inner.load(Ordering::SeqCst)
    }

    /// 世代号自增并返回新值（fetch_add(1, SeqCst) + 1）
    pub fn bump(&self) -> u64 {
        self.inner.fetch_add(1, Ordering::SeqCst) + 1
    }
}

impl Default for PolicyGeneration {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_generation_monotonic() {
        let gen = PolicyGeneration::new();
        assert_eq!(gen.current(), 0);
        assert_eq!(gen.bump(), 1);
        assert_eq!(gen.current(), 1);
        assert_eq!(gen.bump(), 2);
        assert_eq!(gen.bump(), 3);
        assert_eq!(gen.current(), 3);
    }

    #[test]
    fn test_generation_concurrent_bump() {
        use std::thread;
        let gen = Arc::new(PolicyGeneration::new());
        let mut handles = vec![];
        for _ in 0..8 {
            let g = gen.clone();
            handles.push(thread::spawn(move || g.bump()));
        }
        let mut results: Vec<u64> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        results.sort();
        // 8 次并发 bump，结果应为 1..=8 且无重复
        assert_eq!(results, (1..=8).collect::<Vec<_>>());
        assert_eq!(gen.current(), 8);
    }
}
