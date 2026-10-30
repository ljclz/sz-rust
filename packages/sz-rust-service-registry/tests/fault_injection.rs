// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 服务注册发现故障注入测试（T023）
//!
//! 验证降级逻辑：
//! 1. 注册中心断连后已有调用不中断（LocalCache 降级）
//! 2. 实例列表为空返回 NoAvailableInstance（503）
//! 3. 心跳超时自动恢复

use std::time::Duration;

use sz_rust_service_registry::{
    InstanceStatus, LoadBalanceStrategy, LoadBalancer, LocalCache, RegistryError, ServiceInstance,
    ServiceRegistry,
};

/// Mock 注册中心（模拟断连）
struct MockRegistry {
    available: parking_lot::RwLock<bool>,
    instances: parking_lot::RwLock<Vec<ServiceInstance>>,
}

impl MockRegistry {
    fn new() -> Self {
        Self {
            available: parking_lot::RwLock::new(true),
            instances: parking_lot::RwLock::new(vec![]),
        }
    }

    fn set_available(&self, ok: bool) {
        *self.available.write() = ok;
    }
}

#[async_trait::async_trait]
impl ServiceRegistry for MockRegistry {
    async fn register(&self, instance: &ServiceInstance) -> Result<(), RegistryError> {
        if !*self.available.read() {
            return Err(RegistryError::Unreachable("mock down".into()));
        }
        self.instances.write().push(instance.clone());
        Ok(())
    }

    async fn deregister(&self, instance_id: &str) -> Result<(), RegistryError> {
        self.instances
            .write()
            .retain(|i| i.instance_id != instance_id);
        Ok(())
    }

    async fn heartbeat(&self, _instance_id: &str) -> Result<(), RegistryError> {
        if !*self.available.read() {
            return Err(RegistryError::HeartbeatFailed(
                _instance_id.to_string(),
                "mock down".into(),
            ));
        }
        Ok(())
    }

    async fn discover(&self, service_name: &str) -> Result<Vec<ServiceInstance>, RegistryError> {
        if !*self.available.read() {
            return Err(RegistryError::Unreachable("mock down".into()));
        }
        let instances: Vec<_> = self
            .instances
            .read()
            .iter()
            .filter(|i| i.service_name == service_name && i.is_serving())
            .cloned()
            .collect();
        Ok(instances)
    }

    async fn health_check(&self) -> Result<(), RegistryError> {
        if !*self.available.read() {
            return Err(RegistryError::Unreachable("mock down".into()));
        }
        Ok(())
    }
}

/// 场景 1：注册中心断连后已有调用不中断（LocalCache 降级）
#[tokio::test]
async fn test_registry_down_fallback_to_local_cache() {
    let registry = MockRegistry::new();
    let cache = LocalCache::new(Duration::from_secs(60));

    let inst1 = ServiceInstance::new("user-svc", "10.0.0.1", 8080);
    let inst2 = ServiceInstance::new("user-svc", "10.0.0.2", 8080);
    registry.register(&inst1).await.unwrap();
    registry.register(&inst2).await.unwrap();

    let discovered = registry.discover("user-svc").await.unwrap();
    assert_eq!(discovered.len(), 2);
    cache.update("user-svc", discovered);

    registry.set_available(false);

    let result = registry.discover("user-svc").await;
    assert!(matches!(result, Err(RegistryError::Unreachable(_))));

    let cached = cache.get_stale("user-svc").unwrap();
    assert_eq!(cached.len(), 2, "local cache should still serve instances");
    assert!(cached.iter().any(|i| i.host == "10.0.0.1"));
    assert!(cached.iter().any(|i| i.host == "10.0.0.2"));
}

/// 场景 2：实例列表为空返回 NoAvailableInstance（503）
#[tokio::test]
async fn test_empty_instance_list_returns_503() {
    let registry = MockRegistry::new();
    let discovered = registry.discover("empty-svc").await.unwrap();
    assert!(discovered.is_empty());

    let lb = LoadBalancer::new(LoadBalanceStrategy::RoundRobin);
    let result = lb.select(&discovered, None);
    assert!(
        matches!(result, Err(RegistryError::NoAvailableInstance(_))),
        "empty list should return NoAvailableInstance (503)"
    );
}

/// 场景 2b：所有实例不健康时返回 503
#[tokio::test]
async fn test_all_unhealthy_returns_503() {
    let lb = LoadBalancer::new(LoadBalanceStrategy::RoundRobin);
    let instances = vec![ServiceInstance {
        service_name: "svc".into(),
        instance_id: "bad".into(),
        host: "h".into(),
        port: 80,
        weight: 1,
        metadata: std::collections::HashMap::new(),
        health_check_url: None,
        status: InstanceStatus::Unhealthy,
    }];
    let result = lb.select(&instances, None);
    assert!(matches!(result, Err(RegistryError::NoAvailableInstance(_))));
}

/// 场景 3：心跳超时自动恢复
#[tokio::test]
async fn test_heartbeat_timeout_recovery() {
    let registry = MockRegistry::new();
    let inst = ServiceInstance::new("svc", "10.0.0.1", 8080);
    registry.register(&inst).await.unwrap();

    registry.set_available(false);
    let heartbeat_result = registry.heartbeat(&inst.instance_id).await;
    assert!(matches!(
        heartbeat_result,
        Err(RegistryError::HeartbeatFailed(_, _))
    ));

    tokio::time::sleep(Duration::from_millis(10)).await;

    registry.set_available(true);
    let heartbeat_result = registry.heartbeat(&inst.instance_id).await;
    assert!(
        heartbeat_result.is_ok(),
        "heartbeat should recover after registry comes back"
    );

    let discovered = registry.discover("svc").await.unwrap();
    assert_eq!(discovered.len(), 1);
}

/// 场景 4：注册中心断连期间 LB 仍可用（基于缓存实例）
#[tokio::test]
async fn test_lb_works_with_cached_instances() {
    let registry = MockRegistry::new();
    let cache = LocalCache::new(Duration::from_secs(60));

    let inst1 = ServiceInstance::new("svc", "10.0.0.1", 8080).with_weight(1);
    let inst2 = ServiceInstance::new("svc", "10.0.0.2", 8080).with_weight(3);
    registry.register(&inst1).await.unwrap();
    registry.register(&inst2).await.unwrap();

    let discovered = registry.discover("svc").await.unwrap();
    cache.update("svc", discovered);

    registry.set_available(false);

    let cached = cache.get_stale("svc").unwrap();
    let lb = LoadBalancer::new(LoadBalanceStrategy::RoundRobin);
    let selected = lb.select(&cached, None).unwrap();
    assert!(cached.iter().any(|i| i.instance_id == selected.instance_id));
}

/// 场景 5：注册中心恢复后缓存自动刷新
#[tokio::test]
async fn test_cache_refresh_after_recovery() {
    let registry = MockRegistry::new();
    let cache = LocalCache::new(Duration::from_secs(60));

    let inst1 = ServiceInstance::new("svc", "10.0.0.1", 8080);
    registry.register(&inst1).await.unwrap();
    let discovered = registry.discover("svc").await.unwrap();
    cache.update("svc", discovered);
    assert_eq!(cache.get_stale("svc").unwrap().len(), 1);

    let inst2 = ServiceInstance::new("svc", "10.0.0.2", 8080);
    registry.register(&inst2).await.unwrap();

    registry.set_available(false);
    assert_eq!(
        cache.get_stale("svc").unwrap().len(),
        1,
        "cache still has 1 instance during outage"
    );

    registry.set_available(true);
    let fresh = registry.discover("svc").await.unwrap();
    cache.update("svc", fresh);
    assert_eq!(
        cache.get_stale("svc").unwrap().len(),
        2,
        "cache refreshed to 2 after recovery"
    );
}
