// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct CacheMetrics {
    hits: AtomicUsize,
    misses: AtomicUsize,
}

impl CacheMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::Relaxed)
    }

    pub fn misses(&self) -> usize {
        self.misses.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> usize {
        self.hits() + self.misses()
    }

    pub fn hit_rate(&self) -> f64 {
        let total = self.total();
        if total == 0 {
            0.0
        } else {
            self.hits() as f64 / total as f64
        }
    }

    pub fn reset(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
    }

    pub fn to_prometheus(&self) -> String {
        let hits = self.hits();
        let misses = self.misses();
        let total = hits + misses;
        let rate = if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        };
        format!(
            "# HELP cache_hit_total Total cache hits\n\
             # TYPE cache_hit_total counter\n\
             cache_hit_total {hits}\n\
             # HELP cache_miss_total Total cache misses\n\
             # TYPE cache_miss_total counter\n\
             cache_miss_total {misses}\n\
             # HELP cache_hit_rate Cache hit rate (0.0-1.0)\n\
             # TYPE cache_hit_rate gauge\n\
             cache_hit_rate {rate}\n"
        )
    }
}

pub type SharedCacheMetrics = Arc<CacheMetrics>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_metrics_hit_rate() {
        let m = CacheMetrics::new();
        assert_eq!(m.hit_rate(), 0.0);

        m.record_hit();
        m.record_hit();
        m.record_miss();

        assert_eq!(m.hits(), 2);
        assert_eq!(m.misses(), 1);
        assert_eq!(m.total(), 3);
        assert!((m.hit_rate() - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn test_cache_metrics_prometheus() {
        let m = CacheMetrics::new();
        m.record_hit();
        m.record_miss();

        let output = m.to_prometheus();
        assert!(output.contains("cache_hit_total 1"));
        assert!(output.contains("cache_miss_total 1"));
        assert!(output.contains("cache_hit_rate 0.5"));
        assert!(output.contains("# TYPE cache_hit_total counter"));
        assert!(output.contains("# TYPE cache_hit_rate gauge"));
    }

    #[test]
    fn test_cache_metrics_reset() {
        let m = CacheMetrics::new();
        m.record_hit();
        m.record_miss();
        m.reset();
        assert_eq!(m.total(), 0);
        assert_eq!(m.hit_rate(), 0.0);
    }
}
