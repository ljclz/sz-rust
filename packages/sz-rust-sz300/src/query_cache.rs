// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 查询缓存（spec §5.8.1 规则 3）
//!
//! 高频只读查询（商品列表）支持缓存，缓存命中率可观测。
//! 使用 LRU 内存缓存，TTL 过期自动失效。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sz_rust_core::cache::lru_cache::{LRUMemoryCache, LruCacheConfig};
use sz_rust_core::cache::CacheDriver;

/// 查询缓存统计（命中率可观测，spec §5.8.1 规则 3）
#[derive(Debug, Default)]
pub struct CacheStats {
    hits: AtomicU64,
    misses: AtomicU64,
}

impl CacheStats {
    fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    /// 命中次数
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// 未命中次数
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    /// 命中率（0.0 - 1.0）
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits() + self.misses();
        if total == 0 {
            0.0
        } else {
            self.hits() as f64 / total as f64
        }
    }
}

/// v1.9.0 查询缓存（spec §5.8.1 规则 3）
///
/// 高频只读查询结果缓存，LRU + TTL 淘汰。
pub struct QueryCache {
    inner: LRUMemoryCache,
    stats: CacheStats,
}

impl QueryCache {
    /// 创建查询缓存
    pub fn new(max_entries: usize, default_ttl: Duration) -> Self {
        let config = LruCacheConfig {
            max_entries,
            max_memory_bytes: 128 * 1024 * 1024,
            default_ttl: Some(default_ttl),
        };
        Self {
            inner: LRUMemoryCache::new(config),
            stats: CacheStats::default(),
        }
    }

    /// 使用默认配置创建（10000 条，TTL 60s）
    pub fn with_defaults() -> Self {
        Self::new(10_000, Duration::from_secs(60))
    }

    /// 查询缓存
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        match self.inner.get_raw(key) {
            Ok(Some(value)) => {
                self.stats.record_hit();
                Some(value)
            }
            Ok(None) => {
                self.stats.record_miss();
                None
            }
            Err(_) => {
                self.stats.record_miss();
                None
            }
        }
    }

    /// 写入缓存
    pub fn set(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>) {
        let _ = self.inner.set_raw(key, value, ttl);
    }

    /// 失效缓存
    pub fn invalidate(&self, key: &str) {
        let _ = self.inner.delete(key);
    }

    /// 清空所有缓存
    pub fn clear(&self) {
        let _ = self.inner.clear();
    }

    /// 缓存统计（命中率可观测）
    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }
}

impl Default for QueryCache {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// 共享查询缓存句柄
pub type SharedQueryCache = Arc<QueryCache>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_get_set() {
        let cache = QueryCache::with_defaults();
        assert!(cache.get("product:list:1").is_none());
        cache.set("product:list:1", b"page1_data".to_vec(), None);
        let val = cache.get("product:list:1").expect("应命中");
        assert_eq!(val, b"page1_data");
    }

    #[test]
    fn test_cache_hit_rate() {
        let cache = QueryCache::with_defaults();
        cache.set("k1", b"v1".to_vec(), None);
        cache.get("k1");
        cache.get("k1");
        cache.get("missing");
        let stats = cache.stats();
        assert_eq!(stats.hits(), 2);
        assert_eq!(stats.misses(), 1);
        assert!((stats.hit_rate() - (2.0 / 3.0)).abs() < 0.001);
    }

    #[test]
    fn test_cache_invalidate() {
        let cache = QueryCache::with_defaults();
        cache.set("k1", b"v1".to_vec(), None);
        assert!(cache.get("k1").is_some());
        cache.invalidate("k1");
        assert!(cache.get("k1").is_none());
    }

    #[test]
    fn test_cache_ttl_expiry() {
        let cache = QueryCache::new(100, Duration::from_millis(10));
        cache.set("k1", b"v1".to_vec(), Some(Duration::from_millis(10)));
        assert!(cache.get("k1").is_some());
        std::thread::sleep(Duration::from_millis(20));
        assert!(cache.get("k1").is_none());
    }

    #[test]
    fn test_cache_clear() {
        let cache = QueryCache::with_defaults();
        cache.set("k1", b"v1".to_vec(), None);
        cache.set("k2", b"v2".to_vec(), None);
        cache.clear();
        assert!(cache.get("k1").is_none());
        assert!(cache.get("k2").is_none());
    }

    #[test]
    fn test_cache_stats_zero_hit_rate() {
        let cache = QueryCache::with_defaults();
        assert_eq!(cache.stats().hit_rate(), 0.0);
    }
}
