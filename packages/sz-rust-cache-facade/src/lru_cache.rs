use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::{CacheDriver, CacheError};

#[derive(Debug, Clone)]
pub struct LruCacheConfig {
    pub max_entries: usize,
    pub max_memory_bytes: usize,
    pub default_ttl: Option<Duration>,
}

impl Default for LruCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_memory_bytes: 64 * 1024 * 1024,
            default_ttl: None,
        }
    }
}

struct Entry {
    value: Vec<u8>,
    expires_at: Option<Instant>,
}

impl Entry {
    fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|t| Instant::now() >= t)
    }

    fn byte_size(&self) -> usize {
        self.value.len() + 32
    }
}

pub struct LRUMemoryCache {
    inner: Mutex<LruInner>,
    config: LruCacheConfig,
}

struct LruInner {
    entries: HashMap<String, Entry>,
    access_order: std::collections::VecDeque<String>,
    total_bytes: usize,
}

impl LruInner {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            access_order: std::collections::VecDeque::new(),
            total_bytes: 0,
        }
    }

    fn touch(&mut self, key: &str) {
        self.access_order.retain(|k| k != key);
        self.access_order.push_back(key.to_string());
    }

    fn evict_if_needed(&mut self, max_entries: usize, max_memory_bytes: usize) {
        while self.entries.len() > max_entries || self.total_bytes > max_memory_bytes {
            let key = match self.access_order.pop_front() {
                Some(k) => k,
                None => break,
            };
            if let Some(entry) = self.entries.remove(&key) {
                self.total_bytes -= entry.byte_size();
            }
        }
    }
}

impl LRUMemoryCache {
    pub fn new(config: LruCacheConfig) -> Self {
        Self {
            inner: Mutex::new(LruInner::new()),
            config,
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(LruCacheConfig::default())
    }
}

impl Default for LRUMemoryCache {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl CacheDriver for LRUMemoryCache {
    fn get_raw(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        let mut inner = self.inner.lock();
        let expired = inner.entries.get(key).is_some_and(|e| e.is_expired());
        if expired {
            if let Some(entry) = inner.entries.remove(key) {
                inner.total_bytes -= entry.byte_size();
            }
            inner.access_order.retain(|k| k != key);
            return Ok(None);
        }
        match inner.entries.get(key) {
            Some(entry) => {
                let value = entry.value.clone();
                inner.touch(key);
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    fn set_raw(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>) -> Result<(), CacheError> {
        let mut inner = self.inner.lock();
        let expires_at = ttl.or(self.config.default_ttl).map(|d| Instant::now() + d);
        let new_entry = Entry {
            value: value.clone(),
            expires_at,
        };
        let new_size = new_entry.byte_size();

        if let Some(old) = inner.entries.remove(key) {
            inner.total_bytes -= old.byte_size();
        }
        inner.access_order.retain(|k| k != key);

        inner.total_bytes += new_size;
        inner.entries.insert(key.to_string(), new_entry);
        inner.touch(key);
        inner.evict_if_needed(self.config.max_entries, self.config.max_memory_bytes);
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), CacheError> {
        let mut inner = self.inner.lock();
        if let Some(entry) = inner.entries.remove(key) {
            inner.total_bytes -= entry.byte_size();
        }
        inner.access_order.retain(|k| k != key);
        Ok(())
    }

    fn has(&self, key: &str) -> Result<bool, CacheError> {
        let inner = self.inner.lock();
        match inner.entries.get(key) {
            Some(entry) => Ok(!entry.is_expired()),
            None => Ok(false),
        }
    }

    fn clear(&self) -> Result<(), CacheError> {
        let mut inner = self.inner.lock();
        inner.entries.clear();
        inner.access_order.clear();
        inner.total_bytes = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_lru_eviction() {
        let config = LruCacheConfig {
            max_entries: 3,
            max_memory_bytes: 1024 * 1024,
            default_ttl: None,
        };
        let cache = LRUMemoryCache::new(config);

        cache.set_raw("a", b"1".to_vec(), None).unwrap();
        cache.set_raw("b", b"2".to_vec(), None).unwrap();
        cache.set_raw("c", b"3".to_vec(), None).unwrap();
        assert!(cache.has("a").unwrap());

        cache.set_raw("d", b"4".to_vec(), None).unwrap();
        assert!(!cache.has("a").unwrap(), "a should be evicted (LRU)");
        assert!(cache.has("b").unwrap());
        assert!(cache.has("c").unwrap());
        assert!(cache.has("d").unwrap());
    }

    #[test]
    fn test_lru_memory_limit() {
        let config = LruCacheConfig {
            max_entries: 100,
            max_memory_bytes: 200,
            default_ttl: None,
        };
        let cache = LRUMemoryCache::new(config);

        let big_value = vec![0u8; 100];
        cache.set_raw("a", big_value.clone(), None).unwrap();
        cache.set_raw("b", big_value, None).unwrap();
        assert!(
            !cache.has("a").unwrap() || !cache.has("b").unwrap(),
            "memory limit should evict one entry"
        );
    }

    #[test]
    fn test_lru_cache_driver_trait() {
        let cache = LRUMemoryCache::with_defaults();

        cache.set_raw("key", b"value".to_vec(), None).unwrap();
        assert!(cache.has("key").unwrap());
        assert_eq!(cache.get_raw("key").unwrap(), Some(b"value".to_vec()));

        cache.delete("key").unwrap();
        assert!(!cache.has("key").unwrap());
        assert_eq!(cache.get_raw("key").unwrap(), None);
    }

    #[test]
    fn test_lru_ttl_expiration() {
        let cache = LRUMemoryCache::with_defaults();

        cache
            .set_raw("temp", b"data".to_vec(), Some(Duration::from_millis(50)))
            .unwrap();
        assert!(cache.has("temp").unwrap());

        thread::sleep(Duration::from_millis(60));
        assert!(!cache.has("temp").unwrap());
        assert_eq!(cache.get_raw("temp").unwrap(), None);
    }

    #[test]
    fn test_lru_inc() {
        let cache = LRUMemoryCache::with_defaults();
        assert_eq!(cache.inc("counter", 1).unwrap(), 1);
        assert_eq!(cache.inc("counter", 5).unwrap(), 6);
    }
}
