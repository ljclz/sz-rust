// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 透明缓存层 + 表级失效（spec §5.7 规则 1/3/4）
//!
//! 对应用透明拦截查询，自动计算缓存键（SQL + 参数哈希），
//! 写操作后按表失效相关缓存。

use std::collections::{HashMap, HashSet};

use parking_lot::RwLock;
use sha2::{Digest, Sha256};

use crate::query_cache::{QueryCache, QueryCacheError};

/// 透明缓存层（对应用透明拦截，spec §5.7 规则 4）
pub struct TransparentCacheLayer {
    inner: QueryCache,
    invalidator: TableInvalidator,
}

impl TransparentCacheLayer {
    /// 创建透明缓存层
    pub fn new(inner: QueryCache) -> Self {
        Self {
            inner,
            invalidator: TableInvalidator::new(),
        }
    }

    /// 计算缓存键（SQL 文本 + 绑定参数哈希，spec §5.7 规则 1）
    ///
    /// 相同 SQL + 参数 → 相同键；不同参数 → 不同键
    pub fn compute_cache_key(sql: &str, params: &[Vec<u8>]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(sql.as_bytes());
        for p in params {
            hasher.update(p);
        }
        hex::encode(hasher.finalize())
    }

    /// 透明查询：命中返回缓存，未命中回源并填充（spec §5.7 规则 2/4）
    pub async fn query<F, Fut>(
        &self,
        sql: &str,
        params: &[Vec<u8>],
        tables: &[&str],
        fallback: F,
    ) -> Result<Vec<u8>, QueryCacheError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<u8>, QueryCacheError>>,
    {
        let key = Self::compute_cache_key(sql, params);
        let result = self.inner.get_or_query(&key, fallback).await?;
        // 注册缓存键所属表（spec §5.7 规则 3）
        for table in tables {
            self.invalidator.register(table, &key);
        }
        Ok(result)
    }

    /// 写操作后失效该表所有缓存（spec §5.7 规则 3）
    pub fn invalidate_table(&self, table: &str) -> usize {
        let keys = self.invalidator.get_keys(table);
        let mut count = 0;
        for key in &keys {
            count += self.inner.invalidate(key);
        }
        self.invalidator.invalidate_table(table);
        count
    }

    /// 获取内部缓存引用
    pub fn inner(&self) -> &QueryCache {
        &self.inner
    }

    /// 获取表失效器引用
    pub fn invalidator(&self) -> &TableInvalidator {
        &self.invalidator
    }
}

/// 表级失效器（写表后失效该表相关缓存，spec §5.7 规则 3）
pub struct TableInvalidator {
    /// 表名 → 缓存键集合
    table_keys: RwLock<HashMap<String, HashSet<String>>>,
}

impl TableInvalidator {
    /// 创建表级失效器
    pub fn new() -> Self {
        Self {
            table_keys: RwLock::new(HashMap::new()),
        }
    }

    /// 注册缓存键所属表
    pub fn register(&self, table: &str, cache_key: &str) {
        self.table_keys
            .write()
            .entry(table.to_string())
            .or_default()
            .insert(cache_key.to_string());
    }

    /// 获取表关联的所有缓存键
    pub fn get_keys(&self, table: &str) -> HashSet<String> {
        self.table_keys
            .read()
            .get(table)
            .cloned()
            .unwrap_or_default()
    }

    /// 写操作后失效该表所有缓存
    pub fn invalidate_table(&self, table: &str) {
        self.table_keys.write().remove(table);
    }

    /// 已注册的表数
    pub fn table_count(&self) -> usize {
        self.table_keys.read().len()
    }

    /// 指定表的缓存键数
    pub fn key_count(&self, table: &str) -> usize {
        self.table_keys
            .read()
            .get(table)
            .map(|s| s.len())
            .unwrap_or(0)
    }

    /// 清空所有注册
    pub fn clear(&self) {
        self.table_keys.write().clear();
    }
}

impl Default for TableInvalidator {
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

    #[test]
    fn test_compute_cache_key_same_input() {
        let sql = "SELECT * FROM users WHERE id = ?";
        let params = vec![vec![1u8]];
        let key1 = TransparentCacheLayer::compute_cache_key(sql, &params);
        let key2 = TransparentCacheLayer::compute_cache_key(sql, &params);
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_compute_cache_key_different_params() {
        let sql = "SELECT * FROM users WHERE id = ?";
        let key1 = TransparentCacheLayer::compute_cache_key(sql, &[vec![1u8]]);
        let key2 = TransparentCacheLayer::compute_cache_key(sql, &[vec![2u8]]);
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_compute_cache_key_different_sql() {
        let params = vec![vec![1u8]];
        let key1 = TransparentCacheLayer::compute_cache_key("SELECT * FROM users", &params);
        let key2 = TransparentCacheLayer::compute_cache_key("SELECT * FROM orders", &params);
        assert_ne!(key1, key2);
    }

    #[tokio::test]
    async fn test_transparent_query_cache_hit() {
        let layer = TransparentCacheLayer::new(make_cache());
        let sql = "SELECT * FROM users WHERE id = ?";
        let params = vec![vec![1u8]];
        let tables = ["users"];

        let result1 = layer
            .query(sql, &params, &tables, || async { Ok(vec![1, 2, 3]) })
            .await
            .unwrap();
        assert_eq!(result1, vec![1, 2, 3]);

        let result2 = layer
            .query(sql, &params, &tables, || async { Ok(vec![9, 9, 9]) })
            .await
            .unwrap();
        assert_eq!(result2, vec![1, 2, 3]);
        assert_eq!(layer.inner().hits(), 1);
    }

    #[tokio::test]
    async fn test_transparent_query_cache_miss() {
        let layer = TransparentCacheLayer::new(make_cache());
        let result = layer
            .query(
                "SELECT * FROM users WHERE id = ?",
                &[vec![1u8]],
                &["users"],
                || async { Ok(vec![4, 5, 6]) },
            )
            .await
            .unwrap();
        assert_eq!(result, vec![4, 5, 6]);
        assert_eq!(layer.inner().misses(), 1);
    }

    #[tokio::test]
    async fn test_invalidate_table() {
        let layer = TransparentCacheLayer::new(make_cache());
        let sql = "SELECT * FROM users WHERE id = ?";
        let params = vec![vec![1u8]];

        layer
            .query(sql, &params, &["users"], || async { Ok(vec![1, 2, 3]) })
            .await
            .unwrap();

        assert_eq!(layer.inner().len(), 1);
        let evicted = layer.invalidate_table("users");
        assert_eq!(evicted, 1);
        assert_eq!(layer.inner().len(), 0);
    }

    #[tokio::test]
    async fn test_invalidate_table_no_keys() {
        let layer = TransparentCacheLayer::new(make_cache());
        let evicted = layer.invalidate_table("nonexistent");
        assert_eq!(evicted, 0);
    }

    #[tokio::test]
    async fn test_multi_table_invalidation() {
        let layer = TransparentCacheLayer::new(make_cache());

        layer
            .query(
                "SELECT * FROM users WHERE id = ?",
                &[vec![1u8]],
                &["users"],
                || async { Ok(vec![1]) },
            )
            .await
            .unwrap();

        layer
            .query(
                "SELECT * FROM orders WHERE user_id = ?",
                &[vec![1u8]],
                &["orders"],
                || async { Ok(vec![2]) },
            )
            .await
            .unwrap();

        assert_eq!(layer.inner().len(), 2);
        layer.invalidate_table("users");
        assert_eq!(layer.inner().len(), 1);
    }

    #[test]
    fn test_table_invalidator_register_and_get() {
        let inv = TableInvalidator::new();
        inv.register("users", "key1");
        inv.register("users", "key2");
        inv.register("orders", "key3");
        assert_eq!(inv.key_count("users"), 2);
        assert_eq!(inv.key_count("orders"), 1);
        assert_eq!(inv.table_count(), 2);
    }

    #[test]
    fn test_table_invalidator_invalidate() {
        let inv = TableInvalidator::new();
        inv.register("users", "key1");
        inv.register("users", "key2");
        inv.invalidate_table("users");
        assert_eq!(inv.key_count("users"), 0);
        assert_eq!(inv.table_count(), 0);
    }

    #[test]
    fn test_table_invalidator_clear() {
        let inv = TableInvalidator::new();
        inv.register("a", "k1");
        inv.register("b", "k2");
        inv.clear();
        assert_eq!(inv.table_count(), 0);
    }
}
