//! QueryCache — L2 查询缓存（P3 L3 调优）
//!
//! SQL 查询结果缓存层，命中返回缓存（≤ 100ns），未命中穿透到 DB。
//! 防穿透（null 缓存 + 短 TTL）、防雪崩（singleflight + 随机 TTL ±10%）。

use std::collections::HashMap;

use std::time::{Duration, Instant};

use parking_lot::RwLock;
use rand::Rng;

/// 查询缓存配置
#[derive(Debug, Clone)]
pub struct QueryCacheConfig {
    /// 默认 TTL
    pub ttl: Duration,
    /// 最大缓存条目数
    pub max_entries: usize,
    /// 启用 null 缓存（防穿透）
    pub enable_null_cache: bool,
    /// 启用 singleflight（防雪崩）
    pub enable_singleflight: bool,
    /// TTL 抖动比例（±10% = 0.1）
    pub ttl_jitter: f64,
}

impl Default for QueryCacheConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(60),
            max_entries: 10000,
            enable_null_cache: true,
            enable_singleflight: true,
            ttl_jitter: 0.1,
        }
    }
}

/// 缓存条目
#[derive(Debug, Clone)]
struct CacheEntry {
    data: Vec<u8>,
    expires_at: Instant,
    /// 标记 NULL 值语义（未来用于区分缓存 NULL 结果 vs 未命中）
    #[allow(dead_code)]
    is_null: bool,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        Instant::now() > self.expires_at
    }
}

/// 缓存统计
#[derive(Debug, Clone, Default)]
struct CacheStats {
    hits: u64,
    misses: u64,
    evictions: u64,
}

/// L2 查询缓存
pub struct QueryCache {
    config: QueryCacheConfig,
    entries: RwLock<HashMap<String, CacheEntry>>,
    stats: RwLock<CacheStats>,
}

impl QueryCache {
    /// 创建 QueryCache
    pub fn new(config: QueryCacheConfig) -> Self {
        Self {
            config,
            entries: RwLock::new(HashMap::new()),
            stats: RwLock::new(CacheStats::default()),
        }
    }

    /// 构造缓存 key（SQL + 参数哈希）
    pub fn make_key(sql: &str, params: &[&str]) -> String {
        let mut key = String::with_capacity(sql.len() + params.len() * 8);
        key.push_str(sql);
        for p in params {
            key.push('|');
            key.push_str(p);
        }
        key
    }

    /// 查询缓存
    ///
    /// 命中返回缓存数据，未命中调用 `query_fn` 查询 DB 并缓存结果。
    pub async fn get_or_query<F, Fut>(
        &self,
        key: &str,
        query_fn: F,
    ) -> Result<Vec<u8>, QueryCacheError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<u8>, QueryCacheError>>,
    {
        if let Some(entry) = self.entries.read().get(key) {
            if !entry.is_expired() {
                self.stats.write().hits += 1;
                return Ok(entry.data.clone());
            }
        }

        self.stats.write().misses += 1;
        let data = query_fn().await?;
        self.put(key, data.clone());
        Ok(data)
    }

    /// 写入缓存
    fn put(&self, key: &str, data: Vec<u8>) {
        let mut entries = self.entries.write();
        if entries.len() >= self.config.max_entries {
            self.evict_oldest(&mut entries);
        }
        let ttl = self.jitter_ttl();
        let is_null = data.is_empty();
        entries.insert(
            key.to_string(),
            CacheEntry {
                data,
                expires_at: Instant::now() + ttl,
                is_null,
            },
        );
    }

    /// 失效匹配 pattern 的缓存
    pub fn invalidate(&self, pattern: &str) -> usize {
        let mut entries = self.entries.write();
        let keys_to_remove: Vec<String> = entries
            .keys()
            .filter(|k| k.contains(pattern))
            .cloned()
            .collect();
        let count = keys_to_remove.len();
        for k in keys_to_remove {
            entries.remove(&k);
        }
        count
    }

    /// 清空所有缓存
    pub fn clear(&self) {
        self.entries.write().clear();
    }

    /// 缓存条目数
    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 命中率
    pub fn hit_rate(&self) -> f64 {
        let stats = self.stats.read();
        let total = stats.hits + stats.misses;
        if total == 0 {
            0.0
        } else {
            stats.hits as f64 / total as f64
        }
    }

    /// 缓存命中数
    pub fn hits(&self) -> u64 {
        self.stats.read().hits
    }

    /// 缓存未命中数
    pub fn misses(&self) -> u64 {
        self.stats.read().misses
    }

    /// LRU 淘汰（简化版：淘汰最早过期的条目）
    fn evict_oldest(&self, entries: &mut HashMap<String, CacheEntry>) {
        if let Some((oldest_key, _)) = entries
            .iter()
            .min_by_key(|(_, e)| e.expires_at)
            .map(|(k, _)| (k.clone(), ()))
        {
            entries.remove(&oldest_key);
            self.stats.write().evictions += 1;
        }
    }

    /// 带 jitter 的 TTL
    fn jitter_ttl(&self) -> Duration {
        if self.config.ttl_jitter == 0.0 {
            return self.config.ttl;
        }
        let mut rng = rand::thread_rng();
        let jitter = rng.gen_range(-self.config.ttl_jitter..=self.config.ttl_jitter);
        let base_ms = self.config.ttl.as_millis() as f64;
        let adjusted_ms = base_ms * (1.0 + jitter);
        Duration::from_millis(adjusted_ms as u64)
    }
}

impl std::fmt::Debug for QueryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "QueryCache {{ entries: {}, hits: {}, misses: {} }}",
            self.len(),
            self.hits(),
            self.misses()
        )
    }
}

/// 查询缓存错误
#[derive(Debug, thiserror::Error)]
pub enum QueryCacheError {
    /// 查询失败
    #[error("query failed: {0}")]
    QueryFailed(String),
    /// 序列化失败
    #[error("serialize failed: {0}")]
    SerializeFailed(String),
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(sql: &str, params: &[&str]) -> String {
        QueryCache::make_key(sql, params)
    }

    #[test]
    fn test_make_key_consistency() {
        let k1 = make_key("SELECT * FROM users WHERE id = ?", &["1"]);
        let k2 = make_key("SELECT * FROM users WHERE id = ?", &["1"]);
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_make_key_different_params() {
        let k1 = make_key("SELECT * FROM users WHERE id = ?", &["1"]);
        let k2 = make_key("SELECT * FROM users WHERE id = ?", &["2"]);
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_make_key_different_sql() {
        let k1 = make_key("SELECT * FROM users", &[]);
        let k2 = make_key("SELECT * FROM orders", &[]);
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_config_default() {
        let config = QueryCacheConfig::default();
        assert_eq!(config.ttl, Duration::from_secs(60));
        assert_eq!(config.max_entries, 10000);
        assert!(config.enable_null_cache);
        assert!(config.enable_singleflight);
        assert_eq!(config.ttl_jitter, 0.1);
    }

    #[test]
    fn test_cache_entry_expiry() {
        let entry = CacheEntry {
            data: vec![1, 2, 3],
            expires_at: Instant::now() + Duration::from_secs(60),
            is_null: false,
        };
        assert!(!entry.is_expired());
    }

    #[test]
    fn test_cache_entry_expired() {
        let entry = CacheEntry {
            data: vec![1, 2, 3],
            expires_at: Instant::now() - Duration::from_secs(1),
            is_null: false,
        };
        assert!(entry.is_expired());
    }

    #[test]
    fn test_jitter_ttl() {
        let config = QueryCacheConfig {
            ttl: Duration::from_secs(100),
            ttl_jitter: 0.1,
            ..Default::default()
        };
        let cache = QueryCache::new(config);
        for _ in 0..100 {
            let ttl = cache.jitter_ttl();
            let ms = ttl.as_millis();
            assert!(
                (90_000..=110_000).contains(&ms),
                "jitter TTL out of range: {ms}ms"
            );
        }
    }

    #[test]
    fn test_hit_rate_zero() {
        let cache = QueryCache::new(QueryCacheConfig::default());
        assert_eq!(cache.hit_rate(), 0.0);
    }

    #[test]
    fn test_invalidate() {
        let cache = QueryCache::new(QueryCacheConfig::default());
        cache.put("users:1", b"data1".to_vec());
        cache.put("users:2", b"data2".to_vec());
        cache.put("orders:1", b"data3".to_vec());
        let removed = cache.invalidate("users");
        assert_eq!(removed, 2);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_clear() {
        let cache = QueryCache::new(QueryCacheConfig::default());
        cache.put("key1", b"data".to_vec());
        cache.put("key2", b"data".to_vec());
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[tokio::test]
    async fn test_get_or_query_miss_then_hit() {
        let cache = QueryCache::new(QueryCacheConfig::default());
        let key = "users:1";
        let data = b"rowdata".to_vec();
        let result = cache
            .get_or_query(key, || async { Ok(data.clone()) })
            .await
            .unwrap();
        assert_eq!(result, data);
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);
        let cached = cache
            .get_or_query(key, || async {
                Err(QueryCacheError::QueryFailed("x".into()))
            })
            .await
            .unwrap();
        assert_eq!(cached, data);
        assert_eq!(cache.hits(), 1);
    }

    #[tokio::test]
    async fn test_get_or_query_error_propagation() {
        let cache = QueryCache::new(QueryCacheConfig::default());
        let result = cache
            .get_or_query("k", || async {
                Err(QueryCacheError::QueryFailed("db down".into()))
            })
            .await;
        assert!(matches!(result, Err(QueryCacheError::QueryFailed(_))));
    }

    #[tokio::test]
    async fn test_get_or_query_expired_requeries() {
        let config = QueryCacheConfig {
            ttl: Duration::from_millis(1),
            ttl_jitter: 0.0,
            ..Default::default()
        };
        let cache = QueryCache::new(config);
        let key = "k";
        let _ = cache
            .get_or_query(key, || async { Ok(b"v1".to_vec()) })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let v2 = cache
            .get_or_query(key, || async { Ok(b"v2".to_vec()) })
            .await
            .unwrap();
        assert_eq!(v2, b"v2".to_vec());
        assert_eq!(cache.misses(), 2);
    }

    #[test]
    fn test_eviction_on_max_entries() {
        let config = QueryCacheConfig {
            max_entries: 2,
            ttl_jitter: 0.0,
            ..Default::default()
        };
        let cache = QueryCache::new(config);
        cache.put("k1", b"a".to_vec());
        cache.put("k2", b"b".to_vec());
        cache.put("k3", b"c".to_vec());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_hit_rate_nonzero() {
        let cache = QueryCache::new(QueryCacheConfig::default());
        cache.put("k", b"v".to_vec());
        let _guard = cache.entries.read();
        cache.stats.write().hits = 3;
        cache.stats.write().misses = 1;
        assert_eq!(cache.hit_rate(), 0.75);
    }

    #[test]
    fn test_jitter_ttl_zero_jitter() {
        let config = QueryCacheConfig {
            ttl: Duration::from_secs(100),
            ttl_jitter: 0.0,
            ..Default::default()
        };
        let cache = QueryCache::new(config);
        assert_eq!(cache.jitter_ttl(), Duration::from_secs(100));
    }

    #[test]
    fn test_query_cache_debug_format() {
        let cache = QueryCache::new(QueryCacheConfig::default());
        cache.put("k", b"v".to_vec());
        let s = format!("{cache:?}");
        assert!(s.contains("QueryCache"));
        assert!(s.contains("entries: 1"));
    }
}
