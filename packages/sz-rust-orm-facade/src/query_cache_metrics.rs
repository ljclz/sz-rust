// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 查询缓存 Prometheus 指标暴露（spec §5.7 规则 5）
//!
//! 暴露命中率/miss 率/驱逐数指标。

use crate::query_cache::QueryCache;

/// 查询缓存指标快照
#[derive(Debug, Clone)]
pub struct QueryCacheMetricsSnapshot {
    /// 缓存条目数
    pub entries: usize,
    /// 命中数
    pub hits: u64,
    /// 未命中数
    pub misses: u64,
    /// 驱逐数
    pub evictions: u64,
    /// 命中率
    pub hit_rate: f64,
    /// miss 率
    pub miss_rate: f64,
}

impl QueryCacheMetricsSnapshot {
    /// 从 QueryCache 生成指标快照
    pub fn from_cache(cache: &QueryCache) -> Self {
        let hits = cache.hits();
        let misses = cache.misses();
        let total = hits + misses;
        let hit_rate = if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        };
        let miss_rate = if total == 0 {
            0.0
        } else {
            misses as f64 / total as f64
        };
        Self {
            entries: cache.len(),
            hits,
            misses,
            evictions: cache.evictions(),
            hit_rate,
            miss_rate,
        }
    }
}

/// 查询缓存指标导出器（Prometheus text exposition 格式）
pub struct QueryCacheMetricsExporter {
    prefix: String,
}

impl QueryCacheMetricsExporter {
    /// 创建导出器，默认前缀 `sz_rust_query_cache`
    pub fn new() -> Self {
        Self {
            prefix: "sz_rust_query_cache".to_string(),
        }
    }

    /// 创建带自定义前缀的导出器
    pub fn with_prefix(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }

    /// 导出 Prometheus text exposition 格式指标（spec §5.7 规则 5）
    pub fn export(&self, snapshot: &QueryCacheMetricsSnapshot) -> String {
        let p = &self.prefix;
        let mut out = String::with_capacity(256);

        out.push_str(&format!("# HELP {p}_entries 缓存条目数\n"));
        out.push_str(&format!("# TYPE {p}_entries gauge\n"));
        out.push_str(&format!("{p}_entries {}\n", snapshot.entries));

        out.push_str(&format!("# HELP {p}_hits_total 缓存命中数\n"));
        out.push_str(&format!("# TYPE {p}_hits_total counter\n"));
        out.push_str(&format!("{p}_hits_total {}\n", snapshot.hits));

        out.push_str(&format!("# HELP {p}_misses_total 缓存未命中数\n"));
        out.push_str(&format!("# TYPE {p}_misses_total counter\n"));
        out.push_str(&format!("{p}_misses_total {}\n", snapshot.misses));

        out.push_str(&format!("# HELP {p}_evictions_total 缓存驱逐数\n"));
        out.push_str(&format!("# TYPE {p}_evictions_total counter\n"));
        out.push_str(&format!("{p}_evictions_total {}\n", snapshot.evictions));

        out.push_str(&format!("# HELP {p}_hit_rate 缓存命中率\n"));
        out.push_str(&format!("# TYPE {p}_hit_rate gauge\n"));
        out.push_str(&format!("{p}_hit_rate {}\n", snapshot.hit_rate));

        out.push_str(&format!("# HELP {p}_miss_rate 缓存miss率\n"));
        out.push_str(&format!("# TYPE {p}_miss_rate gauge\n"));
        out.push_str(&format!("{p}_miss_rate {}\n", snapshot.miss_rate));

        out
    }

    /// 从 QueryCache 直接导出
    pub fn export_cache(&self, cache: &QueryCache) -> String {
        let snapshot = QueryCacheMetricsSnapshot::from_cache(cache);
        self.export(&snapshot)
    }
}

impl Default for QueryCacheMetricsExporter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_cache::{QueryCache, QueryCacheConfig};

    fn make_cache() -> QueryCache {
        QueryCache::new(QueryCacheConfig {
            ttl: std::time::Duration::from_secs(60),
            max_entries: 100,
            enable_null_cache: true,
            enable_singleflight: false,
            ttl_jitter: 0.0,
        })
    }

    #[tokio::test]
    async fn test_metrics_snapshot_empty() {
        let cache = make_cache();
        let snap = QueryCacheMetricsSnapshot::from_cache(&cache);
        assert_eq!(snap.entries, 0);
        assert_eq!(snap.hits, 0);
        assert_eq!(snap.misses, 0);
        assert_eq!(snap.hit_rate, 0.0);
        assert_eq!(snap.miss_rate, 0.0);
    }

    #[tokio::test]
    async fn test_metrics_snapshot_with_activity() {
        let cache = make_cache();
        cache
            .get_or_query("key1", || async { Ok(vec![1, 2, 3]) })
            .await
            .unwrap();
        cache
            .get_or_query("key1", || async { Ok(vec![9, 9, 9]) })
            .await
            .unwrap();
        cache
            .get_or_query("key2", || async { Ok(vec![4, 5, 6]) })
            .await
            .unwrap();

        let snap = QueryCacheMetricsSnapshot::from_cache(&cache);
        assert_eq!(snap.entries, 2);
        assert_eq!(snap.hits, 1);
        assert_eq!(snap.misses, 2);
        assert!((snap.hit_rate - (1.0 / 3.0)).abs() < 0.001);
        assert!((snap.miss_rate - (2.0 / 3.0)).abs() < 0.001);
    }

    #[test]
    fn test_export_contains_all_metrics() {
        let exporter = QueryCacheMetricsExporter::new();
        let snapshot = QueryCacheMetricsSnapshot {
            entries: 10,
            hits: 50,
            misses: 30,
            evictions: 5,
            hit_rate: 0.625,
            miss_rate: 0.375,
        };
        let output = exporter.export(&snapshot);
        assert!(output.contains("sz_rust_query_cache_entries 10"));
        assert!(output.contains("sz_rust_query_cache_hits_total 50"));
        assert!(output.contains("sz_rust_query_cache_misses_total 30"));
        assert!(output.contains("sz_rust_query_cache_evictions_total 5"));
        assert!(output.contains("sz_rust_query_cache_hit_rate 0.625"));
        assert!(output.contains("sz_rust_query_cache_miss_rate 0.375"));
    }

    #[test]
    fn test_export_has_prometheus_format() {
        let exporter = QueryCacheMetricsExporter::new();
        let snapshot = QueryCacheMetricsSnapshot {
            entries: 0,
            hits: 0,
            misses: 0,
            evictions: 0,
            hit_rate: 0.0,
            miss_rate: 0.0,
        };
        let output = exporter.export(&snapshot);
        assert!(output.contains("# HELP"));
        assert!(output.contains("# TYPE"));
        assert!(output.contains("gauge"));
        assert!(output.contains("counter"));
    }

    #[test]
    fn test_export_custom_prefix() {
        let exporter = QueryCacheMetricsExporter::with_prefix("my_cache");
        let snapshot = QueryCacheMetricsSnapshot {
            entries: 1,
            hits: 0,
            misses: 0,
            evictions: 0,
            hit_rate: 0.0,
            miss_rate: 0.0,
        };
        let output = exporter.export(&snapshot);
        assert!(output.contains("my_cache_entries 1"));
    }

    #[tokio::test]
    async fn test_export_cache_directly() {
        let cache = make_cache();
        cache
            .get_or_query("k1", || async { Ok(vec![1]) })
            .await
            .unwrap();
        let exporter = QueryCacheMetricsExporter::new();
        let output = exporter.export_cache(&cache);
        assert!(output.contains("sz_rust_query_cache_entries 1"));
        assert!(output.contains("sz_rust_query_cache_misses_total 1"));
    }
}
