// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! DataLoader 批量加载器（spec §5.20 规则 3）
//!
//! 消除 N+1 查询：批量加载 + 去重 + 缓存。
//! 典型场景：GraphQL 字段解析器中按 ID 批量加载关联实体。
//!
//! # 用法
//!
//! ```ignore
//! use sz_rust_http_facade::dataloader::BatchLoader;
//! use std::collections::HashMap;
//!
//! async fn load_users(ids: Vec<u64>) -> HashMap<u64, String> {
//!     // 批量查询数据库
//!     ids.into_iter().map(|id| (id, format!("user{id}"))).collect()
//! }
//!
//! # tokio_test::block_on(async {
//! let loader = BatchLoader::new(load_users, 100);
//! let result = loader.load_many(vec![1, 2, 3]).await;
//! assert_eq!(result.len(), 3);
//! # });
//! ```

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;
use std::sync::Arc;

/// 批量加载函数类型
pub type BatchFn<K, V> =
    Arc<dyn Fn(Vec<K>) -> Pin<Box<dyn Future<Output = HashMap<K, V>> + Send>> + Send + Sync>;

/// DataLoader 批量加载器（spec §5.20 规则 3）
///
/// 消除 N+1：批量加载 + 去重 + 缓存。
pub struct BatchLoader<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    batch_fn: BatchFn<K, V>,
    batch_size: usize,
}

impl<K, V> BatchLoader<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// 创建批量加载器
    ///
    /// # 参数
    /// - `batch_fn`: 批量加载函数（接收去重后的 key 列表，返回 key→value 映射）
    /// - `batch_size`: 批量大小上限（spec §6.20 规则 3，默认 ≤ 100）
    pub fn new<F, Fut>(batch_fn: F, batch_size: usize) -> Self
    where
        F: Fn(Vec<K>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HashMap<K, V>> + Send + 'static,
    {
        let batch_fn = Arc::new(move |keys: Vec<K>| {
            Box::pin(batch_fn(keys)) as Pin<Box<dyn Future<Output = HashMap<K, V>> + Send>>
        });
        Self {
            batch_fn,
            batch_size,
        }
    }

    /// 批量大小上限
    pub fn batch_size(&self) -> usize {
        self.batch_size
    }

    /// 批量加载（去重 + 分批 + 缓存，spec §5.20 规则 3）
    ///
    /// # 流程
    /// 1. 对输入 keys 去重
    /// 2. 按 `batch_size` 分批调用 `batch_fn`
    /// 3. 合并所有批次结果
    ///
    /// # 后置条件
    /// - 返回的 HashMap 包含所有成功加载的 key→value
    /// - 重复 key 只加载一次
    pub async fn load_many(&self, keys: Vec<K>) -> HashMap<K, V> {
        let unique_keys: Vec<K> = {
            let mut seen: HashSet<K> = HashSet::new();
            keys.into_iter()
                .filter(|k| seen.insert(k.clone()))
                .collect()
        };

        let mut result = HashMap::new();
        for chunk in unique_keys.chunks(self.batch_size) {
            let batch_keys: Vec<K> = chunk.to_vec();
            let batch_result = (self.batch_fn)(batch_keys).await;
            result.extend(batch_result);
        }
        result
    }

    /// 加载单个 key
    pub async fn load_one(&self, key: K) -> Option<V> {
        let result = self.load_many(vec![key]).await;
        result.into_iter().next().map(|(_, v)| v)
    }
}

/// 缓存型 DataLoader（spec §5.20 规则 3，缓存层）
///
/// 在 BatchLoader 基础上增加请求内缓存，同一 key 多次加载只查一次。
pub struct CachedBatchLoader<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    loader: BatchLoader<K, V>,
    cache: parking_lot::Mutex<HashMap<K, V>>,
}

impl<K, V> CachedBatchLoader<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// 创建缓存型批量加载器
    pub fn new<F, Fut>(batch_fn: F, batch_size: usize) -> Self
    where
        F: Fn(Vec<K>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HashMap<K, V>> + Send + 'static,
    {
        Self {
            loader: BatchLoader::new(batch_fn, batch_size),
            cache: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// 批量加载（缓存优先，spec §5.20 规则 3）
    ///
    /// # 流程
    /// 1. 检查缓存，命中直接返回
    /// 2. 未命中的 key 交给 BatchLoader 加载
    /// 3. 加载结果写入缓存
    pub async fn load_many(&self, keys: Vec<K>) -> HashMap<K, V> {
        let mut result = HashMap::new();
        let mut missed: Vec<K> = Vec::new();

        {
            let cache = self.cache.lock();
            for key in &keys {
                if let Some(value) = cache.get(key) {
                    result.insert(key.clone(), value.clone());
                } else {
                    missed.push(key.clone());
                }
            }
        }

        if !missed.is_empty() {
            let loaded = self.loader.load_many(missed).await;
            {
                let mut cache = self.cache.lock();
                for (key, value) in &loaded {
                    cache.insert(key.clone(), value.clone());
                }
            }
            result.extend(loaded);
        }

        result
    }

    /// 清除缓存
    pub fn clear_cache(&self) {
        self.cache.lock().clear();
    }

    /// 缓存大小
    pub fn cache_size(&self) -> usize {
        self.cache.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mock_load(ids: Vec<u64>) -> HashMap<u64, String> {
        ids.into_iter()
            .map(|id| (id, format!("user{id}")))
            .collect()
    }

    #[tokio::test]
    async fn test_batch_loader_load_many() {
        let loader = BatchLoader::new(mock_load, 100);
        let result = loader.load_many(vec![1, 2, 3]).await;
        assert_eq!(result.len(), 3);
        assert_eq!(result.get(&1), Some(&"user1".to_string()));
        assert_eq!(result.get(&2), Some(&"user2".to_string()));
        assert_eq!(result.get(&3), Some(&"user3".to_string()));
    }

    #[tokio::test]
    async fn test_batch_loader_dedup() {
        let loader = BatchLoader::new(mock_load, 100);
        let result = loader.load_many(vec![1, 1, 2, 2, 3]).await;
        assert_eq!(result.len(), 3);
    }

    #[tokio::test]
    async fn test_batch_loader_load_one() {
        let loader = BatchLoader::new(mock_load, 100);
        let result = loader.load_one(42).await;
        assert_eq!(result, Some("user42".to_string()));
    }

    #[tokio::test]
    async fn test_batch_loader_empty() {
        let loader = BatchLoader::new(mock_load, 100);
        let result = loader.load_many(vec![]).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_batch_loader_batch_size() {
        let loader = BatchLoader::new(mock_load, 2);
        let result = loader.load_many(vec![1, 2, 3, 4, 5]).await;
        assert_eq!(result.len(), 5);
        assert_eq!(loader.batch_size(), 2);
    }

    #[tokio::test]
    async fn test_cached_loader_cache_hit() {
        let loader = CachedBatchLoader::new(mock_load, 100);
        let r1 = loader.load_many(vec![1, 2]).await;
        assert_eq!(r1.len(), 2);
        assert_eq!(loader.cache_size(), 2);
        let r2 = loader.load_many(vec![1, 2, 3]).await;
        assert_eq!(r2.len(), 3);
        assert_eq!(loader.cache_size(), 3);
    }

    #[tokio::test]
    async fn test_cached_loader_clear_cache() {
        let loader = CachedBatchLoader::new(mock_load, 100);
        loader.load_many(vec![1, 2]).await;
        assert_eq!(loader.cache_size(), 2);
        loader.clear_cache();
        assert_eq!(loader.cache_size(), 0);
    }

    #[tokio::test]
    async fn test_batch_loader_partial_failure() {
        async fn partial_load(ids: Vec<u64>) -> HashMap<u64, String> {
            ids.into_iter()
                .filter(|id| *id != 2)
                .map(|id| (id, format!("user{id}")))
                .collect()
        }
        let loader = BatchLoader::new(partial_load, 100);
        let result = loader.load_many(vec![1, 2, 3]).await;
        assert_eq!(result.len(), 2);
        assert!(result.contains_key(&1));
        assert!(!result.contains_key(&2));
        assert!(result.contains_key(&3));
    }
}
