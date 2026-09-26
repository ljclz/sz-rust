// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use std::sync::Arc;
use std::time::Duration;

use tracing::warn;

use crate::{apply_ttl_jitter, CacheDriver, CacheMetrics, LRUMemoryCache, RedisCacheDriver};

/// 多级缓存（L1 本地 LRU + L2 Redis）
///
/// 写策略：write-through + invalidate
/// - `set`：先写 Redis（含 TTL jitter），再写本地 L1
/// - `get`：先查 L1，命中返回；未命中查 Redis，命中回填 L1
/// - `invalidate`：先删 Redis，再删 L1
///
/// Redis 不可达时降级为仅本地缓存，记录告警日志，不返回错误。
pub struct MultiLevelCache {
    local: Arc<LRUMemoryCache>,
    redis: Arc<RedisCacheDriver>,
    metrics: Arc<CacheMetrics>,
    jitter_ratio: f64,
}

impl MultiLevelCache {
    pub fn new(
        local: Arc<LRUMemoryCache>,
        redis: Arc<RedisCacheDriver>,
        metrics: Arc<CacheMetrics>,
    ) -> Self {
        Self {
            local,
            redis,
            metrics,
            jitter_ratio: 0.2,
        }
    }

    pub fn with_jitter_ratio(mut self, ratio: f64) -> Self {
        self.jitter_ratio = ratio;
        self
    }

    pub fn metrics(&self) -> &CacheMetrics {
        &self.metrics
    }

    /// 读取缓存：L1 → L2，L2 命中回填 L1
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        if let Ok(Some(value)) = self.local.get_raw(key) {
            self.metrics.record_hit();
            return Some(value);
        }

        match self.redis.get_raw(key) {
            Ok(Some(value)) => {
                let _ = self.local.set_raw(key, value.clone(), None);
                self.metrics.record_hit();
                Some(value)
            }
            Ok(None) => {
                self.metrics.record_miss();
                None
            }
            Err(e) => {
                warn!(key = key, error = %e, "Redis unreachable, degraded to local-only");
                self.metrics.record_miss();
                None
            }
        }
    }

    /// 写入缓存：先写 Redis（含 TTL jitter），再写 L1（write-through）
    pub fn set(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>) {
        let jittered_ttl = apply_ttl_jitter(ttl, self.jitter_ratio);

        match self.redis.set_raw(key, value.clone(), jittered_ttl) {
            Ok(()) => {}
            Err(e) => {
                warn!(key = key, error = %e, "Redis unreachable on set, degraded to local-only");
            }
        }

        let _ = self.local.set_raw(key, value, ttl);
    }

    /// 失效缓存：先删 Redis，再删 L1
    pub fn invalidate(&self, key: &str) {
        match self.redis.delete(key) {
            Ok(()) => {}
            Err(e) => {
                warn!(key = key, error = %e, "Redis unreachable on invalidate");
            }
        }
        let _ = self.local.delete(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LruCacheConfig, RedisConfig};
    use std::time::Instant;

    fn make_cache() -> MultiLevelCache {
        let local = Arc::new(LRUMemoryCache::new(LruCacheConfig {
            max_entries: 100,
            max_memory_bytes: 1024 * 1024,
            default_ttl: None,
        }));
        let redis = Arc::new(RedisCacheDriver::new(RedisConfig::default()));
        let metrics = Arc::new(CacheMetrics::new());
        MultiLevelCache::new(local, redis, metrics)
    }

    #[test]
    fn test_multi_level_get_hit_local() {
        let cache = make_cache();
        cache.set("key", b"value".to_vec(), None);

        assert_eq!(cache.get("key"), Some(b"value".to_vec()));
        assert_eq!(cache.metrics().hits(), 1);
    }

    #[test]
    fn test_multi_level_get_hit_redis_backfill() {
        let cache = make_cache();

        cache
            .redis
            .set_raw("backfill_key", b"from_redis".to_vec(), None)
            .unwrap();
        assert!(!cache.local.has("backfill_key").unwrap());

        let result = cache.get("backfill_key");
        assert_eq!(result, Some(b"from_redis".to_vec()));
        assert!(cache.local.has("backfill_key").unwrap());
    }

    #[test]
    fn test_multi_level_set_write_through_invalidate() {
        let cache = make_cache();

        cache.set("wt_key", b"data".to_vec(), None);

        assert_eq!(
            cache.redis.get_raw("wt_key").unwrap(),
            Some(b"data".to_vec())
        );
        assert_eq!(
            cache.local.get_raw("wt_key").unwrap(),
            Some(b"data".to_vec())
        );

        cache.invalidate("wt_key");
        assert_eq!(cache.redis.get_raw("wt_key").unwrap(), None);
        assert_eq!(cache.local.get_raw("wt_key").unwrap(), None);
    }

    #[test]
    fn test_multi_level_redis_unreachable_degrade() {
        let local = Arc::new(LRUMemoryCache::with_defaults());
        let redis = Arc::new(RedisCacheDriver::new(RedisConfig::default()));
        let metrics = Arc::new(CacheMetrics::new());
        let cache = MultiLevelCache::new(local, redis, metrics);

        cache
            .local
            .set_raw("degrade_key", b"local_only".to_vec(), None)
            .unwrap();

        let start = Instant::now();
        let result = cache.get("degrade_key");
        assert!(start.elapsed().as_millis() < 100);

        assert_eq!(result, Some(b"local_only".to_vec()));
    }

    #[test]
    fn test_multi_level_miss_records_metric() {
        let cache = make_cache();
        assert_eq!(cache.get("nonexistent"), None);
        assert_eq!(cache.metrics().misses(), 1);
    }
}
