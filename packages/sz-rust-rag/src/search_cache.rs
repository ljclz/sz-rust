//! 检索缓存（v1.5.0 P2-3）
//!
//! 查询缓存（TTL 可配置，0 不缓存，spec 5.7.8）。
//! 相同查询命中缓存返回，避免重复检索。
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// 缓存条目
struct CacheEntry<T> {
    value: Arc<T>,
    expires_at: Instant,
}

/// 检索缓存
///
/// TTL 可配置，TTL=0 表示不缓存。
pub struct SearchCache<T> {
    inner: Mutex<HashMap<String, CacheEntry<T>>>,
    ttl: Duration,
}

impl<T> SearchCache<T> {
    /// 创建缓存，指定 TTL
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// 查询缓存
    pub fn get(&self, key: &str) -> Option<Arc<T>> {
        if self.ttl.is_zero() {
            return None;
        }
        let mut inner = self.inner.lock();
        let entry = inner.get(key)?;
        if Instant::now() >= entry.expires_at {
            inner.remove(key);
            return None;
        }
        Some(Arc::clone(&entry.value))
    }

    /// 写入缓存
    pub fn set(&self, key: &str, value: T) {
        if self.ttl.is_zero() {
            return;
        }
        let entry = CacheEntry {
            value: Arc::new(value),
            expires_at: Instant::now() + self.ttl,
        };
        self.inner.lock().insert(key.to_string(), entry);
    }

    /// 清空缓存
    pub fn clear(&self) {
        self.inner.lock().clear();
    }

    /// 当前缓存条目数
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_set_get() {
        let cache = SearchCache::new(Duration::from_secs(60));
        cache.set("query1", vec!["result1", "result2"]);
        let result = cache.get("query1").expect("应命中缓存");
        assert_eq!(result.as_slice(), &["result1", "result2"]);
    }

    #[test]
    fn test_cache_miss() {
        let cache: SearchCache<Vec<&str>> = SearchCache::new(Duration::from_secs(60));
        assert!(cache.get("nonexistent").is_none());
    }

    #[test]
    fn test_cache_ttl_zero_no_cache() {
        let cache = SearchCache::new(Duration::ZERO);
        cache.set("query1", vec!["result1"]);
        assert!(cache.get("query1").is_none(), "TTL=0 不应缓存");
    }

    #[test]
    fn test_cache_expiry() {
        let cache = SearchCache::new(Duration::from_millis(10));
        cache.set("query1", vec!["result1"]);
        std::thread::sleep(Duration::from_millis(20));
        assert!(cache.get("query1").is_none(), "过期后应未命中");
    }

    #[test]
    fn test_cache_clear() {
        let cache = SearchCache::new(Duration::from_secs(60));
        cache.set("q1", 1);
        cache.set("q2", 2);
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
    }
}
