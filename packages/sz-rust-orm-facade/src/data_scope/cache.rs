// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 部门树缓存 — DeptTreeCache
//!
//! 使用 DashMap 分片锁实现高并发读，TTL 过期自动刷新。
//! 递归展开算法维护 HashSet 检测循环引用。

use crate::data_scope::error::DataScopeError;
use async_trait::async_trait;
use dashmap::DashMap;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 部门树数据源 trait
#[async_trait]
pub trait DeptTreeProvider: Send + Sync {
    /// 获取指定部门的直接子部门 ID 列表
    async fn sub_depts(&self, dept_id: i64) -> Result<Vec<i64>, DataScopeError>;
}

/// 缓存条目
struct CachedEntry {
    depts: Vec<i64>,
    inserted_at: Instant,
}

/// 部门树缓存
pub struct DeptTreeCache {
    provider: Arc<dyn DeptTreeProvider>,
    cache: DashMap<i64, CachedEntry>,
    ttl: Duration,
}

impl DeptTreeCache {
    /// 创建缓存
    pub fn new(provider: Arc<dyn DeptTreeProvider>, ttl: Duration) -> Self {
        Self {
            provider,
            cache: DashMap::new(),
            ttl,
        }
    }

    /// 获取指定部门及其所有子部门 ID 列表（含自身）
    ///
    /// 缓存命中（键存在且未过期）直接返回；未命中调用 provider 递归展开。
    pub async fn get_with_sub(&self, dept_id: i64) -> Result<Vec<i64>, DataScopeError> {
        if let Some(entry) = self.cache.get(&dept_id) {
            if entry.inserted_at.elapsed() < self.ttl {
                return Ok(entry.depts.clone());
            }
        }

        let result = self.expand_with_sub(dept_id).await?;
        self.cache.insert(
            dept_id,
            CachedEntry {
                depts: result.clone(),
                inserted_at: Instant::now(),
            },
        );
        Ok(result)
    }

    /// 递归展开部门树（BFS + 循环引用检测）
    async fn expand_with_sub(&self, dept_id: i64) -> Result<Vec<i64>, DataScopeError> {
        let mut result = vec![dept_id];
        let mut visited = HashSet::new();
        visited.insert(dept_id);
        let mut queue = VecDeque::new();
        queue.push_back(dept_id);

        while let Some(current) = queue.pop_front() {
            let subs = self.provider.sub_depts(current).await?;
            for sub in subs {
                if visited.contains(&sub) {
                    tracing::warn!(
                        target: "data_scope",
                        "circular reference detected: dept {} -> dept {} (skipping)",
                        current, sub
                    );
                    continue;
                }
                visited.insert(sub);
                result.push(sub);
                queue.push_back(sub);
            }
        }

        Ok(result)
    }

    /// 失效单键
    pub fn invalidate(&self, dept_id: i64) {
        self.cache.remove(&dept_id);
    }

    /// 全量失效
    pub fn invalidate_all(&self) {
        self.cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockProvider {
        tree: std::collections::HashMap<i64, Vec<i64>>,
    }

    #[async_trait]
    impl DeptTreeProvider for MockProvider {
        async fn sub_depts(&self, dept_id: i64) -> Result<Vec<i64>, DataScopeError> {
            Ok(self.tree.get(&dept_id).cloned().unwrap_or_default())
        }
    }

    #[tokio::test]
    async fn test_cache_hit() {
        let provider = Arc::new(MockProvider {
            tree: [(5, vec![6, 7]), (6, vec![8]), (7, vec![])]
                .into_iter()
                .collect(),
        });
        let cache = DeptTreeCache::new(provider, Duration::from_secs(300));
        let result1 = cache.get_with_sub(5).await.unwrap();
        assert_eq!(result1, vec![5, 6, 7, 8]);
        let result2 = cache.get_with_sub(5).await.unwrap();
        assert_eq!(result2, result1);
    }

    #[tokio::test]
    async fn test_circular_reference() {
        let provider = Arc::new(MockProvider {
            tree: [(1, vec![2]), (2, vec![1])].into_iter().collect(),
        });
        let cache = DeptTreeCache::new(provider, Duration::from_secs(300));
        let result = cache.get_with_sub(1).await.unwrap();
        assert!(result.contains(&1));
        assert!(result.contains(&2));
    }

    #[tokio::test]
    async fn test_invalidate() {
        let provider = Arc::new(MockProvider {
            tree: [(5, vec![6])].into_iter().collect(),
        });
        let cache = DeptTreeCache::new(provider, Duration::from_secs(300));
        let _ = cache.get_with_sub(5).await.unwrap();
        cache.invalidate(5);
        let result = cache.get_with_sub(5).await.unwrap();
        assert_eq!(result, vec![5, 6]);
    }

    #[tokio::test]
    async fn test_invalidate_all() {
        let provider = Arc::new(MockProvider {
            tree: [(5, vec![6]), (10, vec![11])].into_iter().collect(),
        });
        let cache = DeptTreeCache::new(provider, Duration::from_secs(300));
        let _ = cache.get_with_sub(5).await.unwrap();
        let _ = cache.get_with_sub(10).await.unwrap();
        cache.invalidate_all();
        let _ = cache.get_with_sub(5).await.unwrap();
    }

    #[tokio::test]
    async fn test_cache_ttl_expired_requeries() {
        let provider = Arc::new(MockProvider {
            tree: [(5, vec![6])].into_iter().collect(),
        });
        let cache = DeptTreeCache::new(provider, Duration::from_millis(1));
        let r1 = cache.get_with_sub(5).await.unwrap();
        assert_eq!(r1, vec![5, 6]);
        tokio::time::sleep(Duration::from_millis(20)).await;
        let r2 = cache.get_with_sub(5).await.unwrap();
        assert_eq!(r2, vec![5, 6]);
    }
}
