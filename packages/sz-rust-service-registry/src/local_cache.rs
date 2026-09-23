// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 本地缓存降级（T022）
//!
//! 注册中心断连时降级为本地缓存的服务实例列表。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use crate::error::RegistryError;
use crate::registry::ServiceInstance;

/// 缓存条目
#[derive(Debug, Clone)]
struct CacheEntry {
    instances: Vec<ServiceInstance>,
    updated_at: Instant,
}

/// 本地服务列表缓存（注册中心断连降级用）
#[derive(Debug, Default)]
pub struct LocalCache {
    inner: Arc<RwLock<HashMap<String, CacheEntry>>>,
    ttl: Duration,
}

impl LocalCache {
    /// 构造指定 TTL 的本地缓存
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// 更新服务实例列表
    pub fn update(&self, service_name: &str, instances: Vec<ServiceInstance>) {
        let mut map = self.inner.write();
        map.insert(
            service_name.to_string(),
            CacheEntry {
                instances,
                updated_at: Instant::now(),
            },
        );
    }

    /// 查询缓存（过期返回错误）
    pub fn get(&self, service_name: &str) -> Result<Vec<ServiceInstance>, RegistryError> {
        let map = self.inner.read();
        let entry = map
            .get(service_name)
            .ok_or_else(|| RegistryError::InstanceNotFound(service_name.to_string()))?;

        if entry.updated_at.elapsed() > self.ttl {
            return Err(RegistryError::Unreachable(format!(
                "local cache expired for {service_name}"
            )));
        }

        Ok(entry.instances.clone())
    }

    /// 查询缓存（忽略 TTL，仅用于断连应急）
    pub fn get_stale(&self, service_name: &str) -> Option<Vec<ServiceInstance>> {
        let map = self.inner.read();
        map.get(service_name).map(|e| e.instances.clone())
    }

    /// 清空缓存
    pub fn clear(&self) {
        self.inner.write().clear();
    }

    /// 从所有服务中移除指定实例
    pub fn remove_instance(&self, instance_id: &str) {
        let mut map = self.inner.write();
        for entry in map.values_mut() {
            entry.instances.retain(|i| i.instance_id != instance_id);
        }
    }

    /// 缓存大小
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_instances() -> Vec<ServiceInstance> {
        vec![
            ServiceInstance::new("svc", "10.0.0.1", 8080),
            ServiceInstance::new("svc", "10.0.0.2", 8080),
        ]
    }

    #[test]
    fn test_cache_update_and_get() {
        let cache = LocalCache::new(Duration::from_secs(60));
        cache.update("user-svc", make_instances());
        let result = cache.get("user-svc").unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_cache_get_missing() {
        let cache = LocalCache::new(Duration::from_secs(60));
        let result = cache.get("missing");
        assert!(matches!(result, Err(RegistryError::InstanceNotFound(_))));
    }

    #[test]
    fn test_cache_get_expired() {
        let cache = LocalCache::new(Duration::from_millis(1));
        cache.update("svc", make_instances());
        std::thread::sleep(Duration::from_millis(10));
        let result = cache.get("svc");
        assert!(matches!(result, Err(RegistryError::Unreachable(_))));
    }

    #[test]
    fn test_cache_get_stale_ignores_ttl() {
        let cache = LocalCache::new(Duration::from_millis(1));
        cache.update("svc", make_instances());
        std::thread::sleep(Duration::from_millis(10));
        let result = cache.get_stale("svc");
        assert!(result.is_some());
        assert_eq!(result.unwrap().len(), 2);
    }

    #[test]
    fn test_cache_get_stale_missing() {
        let cache = LocalCache::new(Duration::from_secs(60));
        assert!(cache.get_stale("missing").is_none());
    }

    #[test]
    fn test_cache_clear() {
        let cache = LocalCache::new(Duration::from_secs(60));
        cache.update("a", make_instances());
        cache.update("b", make_instances());
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_default_empty() {
        let cache = LocalCache::default();
        assert!(cache.is_empty());
    }
}
