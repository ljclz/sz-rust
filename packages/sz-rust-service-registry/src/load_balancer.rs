// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 五种负载均衡策略（T022）
//!
//! - RoundRobin：原子计数器取模
//! - Random：均匀随机
//! - Weighted：权重随机
//! - LeastConnections：最少活跃连接
//! - ConsistentHash：FNV-1a 一致性哈希环

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rand::Rng;

use crate::error::RegistryError;
use crate::registry::{ConnectionCounter, LoadBalanceStrategy, ServiceInstance};

/// 负载均衡选择器
pub struct LoadBalancer {
    strategy: LoadBalanceStrategy,
    rr_counter: AtomicU64,
    connections: ConnectionCounter,
    hash_ring: parking_lot::RwLock<HashMap<String, Vec<(u64, String)>>>,
}

impl LoadBalancer {
    /// 创建指定策略的负载均衡器
    pub fn new(strategy: LoadBalanceStrategy) -> Self {
        Self {
            strategy,
            rr_counter: AtomicU64::new(0),
            connections: ConnectionCounter::new(),
            hash_ring: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// 选择一个实例
    pub fn select<'a>(
        &self,
        instances: &'a [ServiceInstance],
        key: Option<&str>,
    ) -> Result<&'a ServiceInstance, RegistryError> {
        let serving: Vec<&ServiceInstance> = instances.iter().filter(|i| i.is_serving()).collect();
        if serving.is_empty() {
            return Err(RegistryError::NoAvailableInstance(
                "no healthy instance".into(),
            ));
        }
        if serving.len() == 1 {
            return Ok(serving[0]);
        }

        match self.strategy {
            LoadBalanceStrategy::RoundRobin => self.select_round_robin(&serving),
            LoadBalanceStrategy::Random => self.select_random(&serving),
            LoadBalanceStrategy::Weighted => self.select_weighted(&serving),
            LoadBalanceStrategy::LeastConnections => self.select_least_connections(&serving),
            LoadBalanceStrategy::ConsistentHash => self.select_consistent_hash(&serving, key),
        }
    }

    /// 记数递增（选择后调用）
    pub fn on_acquire(&self, instance_id: &str) {
        if matches!(self.strategy, LoadBalanceStrategy::LeastConnections) {
            self.connections.incr(instance_id);
        }
    }

    /// 计数递减（释放后调用）
    pub fn on_release(&self, instance_id: &str) {
        if matches!(self.strategy, LoadBalanceStrategy::LeastConnections) {
            self.connections.decr(instance_id);
        }
    }

    fn select_round_robin<'a>(
        &self,
        serving: &[&'a ServiceInstance],
    ) -> Result<&'a ServiceInstance, RegistryError> {
        let idx = self.rr_counter.fetch_add(1, Ordering::Relaxed);
        Ok(serving[(idx as usize) % serving.len()])
    }

    fn select_random<'a>(
        &self,
        serving: &[&'a ServiceInstance],
    ) -> Result<&'a ServiceInstance, RegistryError> {
        let mut rng = rand::thread_rng();
        let idx = rng.gen_range(0..serving.len());
        Ok(serving[idx])
    }

    fn select_weighted<'a>(
        &self,
        serving: &[&'a ServiceInstance],
    ) -> Result<&'a ServiceInstance, RegistryError> {
        let total: u64 = serving.iter().map(|i| i.weight as u64).sum();
        // 兜底：全部实例 weight=0 时总权重为 0，gen_range(0..0) 会 panic
        // （weight 虽在反序列化与 with_weight 处钳制 >=1，但 pub 字段仍可能被直构改写）。
        if total == 0 {
            return self.select_random(serving);
        }
        let mut rng = rand::thread_rng();
        let mut point = rng.gen_range(0..total);
        for inst in serving {
            if point < inst.weight as u64 {
                return Ok(inst);
            }
            point -= inst.weight as u64;
        }
        Ok(serving[serving.len() - 1])
    }

    fn select_least_connections<'a>(
        &self,
        serving: &[&'a ServiceInstance],
    ) -> Result<&'a ServiceInstance, RegistryError> {
        serving
            .iter()
            .min_by_key(|i| self.connections.get(&i.instance_id))
            .copied()
            .ok_or_else(|| RegistryError::LoadBalance("min selection failed".into()))
    }

    fn select_consistent_hash<'a>(
        &self,
        serving: &[&'a ServiceInstance],
        key: Option<&str>,
    ) -> Result<&'a ServiceInstance, RegistryError> {
        let key =
            key.ok_or_else(|| RegistryError::LoadBalance("consistent hash requires key".into()))?;

        let ring_key = serving
            .iter()
            .map(|i| i.instance_id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let ring = self.hash_ring.read();
        if let Some(r) = ring.get(&ring_key) {
            return Ok(self.ring_select(serving, r, key));
        }
        drop(ring);

        let mut new_ring: Vec<(u64, String)> = Vec::with_capacity(serving.len() * 150);
        for inst in serving {
            for vnode in 0..150u32 {
                let hash = fnv1a(&format!("{}:{vnode}", inst.instance_id));
                new_ring.push((hash, inst.instance_id.clone()));
            }
        }
        new_ring.sort_by_key(|(h, _)| *h);

        let mut ring = self.hash_ring.write();
        ring.insert(ring_key.clone(), new_ring);
        let r = ring
            .get(&ring_key)
            .expect("ring_key 刚插入 hash_ring，get 必命中");
        Ok(self.ring_select(serving, r, key))
    }

    fn ring_select<'a>(
        &self,
        serving: &[&'a ServiceInstance],
        ring: &[(u64, String)],
        key: &str,
    ) -> &'a ServiceInstance {
        let hash = fnv1a(key);
        let pos = ring.partition_point(|(h, _)| *h < hash);
        let pos = if pos >= ring.len() { 0 } else { pos };
        let target_id = &ring[pos].1;
        serving
            .iter()
            .find(|i| i.instance_id == *target_id)
            .copied()
            .unwrap_or(serving[0])
    }
}

/// FNV-1a 64 位哈希
fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 共享负载均衡器句柄
pub type SharedLoadBalancer = Arc<LoadBalancer>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::InstanceStatus;

    fn make_instances() -> Vec<ServiceInstance> {
        vec![
            ServiceInstance::new("svc", "10.0.0.1", 8080).with_weight(1),
            ServiceInstance::new("svc", "10.0.0.2", 8080).with_weight(3),
            ServiceInstance::new("svc", "10.0.0.3", 8080).with_weight(2),
        ]
    }

    fn make_unhealthy() -> Vec<ServiceInstance> {
        vec![ServiceInstance {
            service_name: "svc".into(),
            instance_id: "bad".into(),
            host: "h".into(),
            port: 80,
            weight: 1,
            metadata: HashMap::new(),
            health_check_url: None,
            status: InstanceStatus::Unhealthy,
        }]
    }

    #[test]
    fn test_fnv1a_deterministic() {
        assert_eq!(fnv1a("hello"), fnv1a("hello"));
        assert_ne!(fnv1a("hello"), fnv1a("world"));
    }

    #[test]
    fn test_fnv1a_known_values() {
        assert_eq!(fnv1a(""), 0xcbf29ce484222325);
    }

    #[test]
    fn test_select_empty_returns_error() {
        let lb = LoadBalancer::new(LoadBalanceStrategy::RoundRobin);
        let result = lb.select(&[], None);
        assert!(matches!(result, Err(RegistryError::NoAvailableInstance(_))));
    }

    #[test]
    fn test_select_only_unhealthy_returns_error() {
        let lb = LoadBalancer::new(LoadBalanceStrategy::RoundRobin);
        let instances = make_unhealthy();
        let result = lb.select(&instances, None);
        assert!(matches!(result, Err(RegistryError::NoAvailableInstance(_))));
    }

    #[test]
    fn test_select_single_instance() {
        let lb = LoadBalancer::new(LoadBalanceStrategy::RoundRobin);
        let instances = vec![ServiceInstance::new("svc", "10.0.0.1", 8080)];
        let selected = lb.select(&instances, None).unwrap();
        assert_eq!(selected.host, "10.0.0.1");
    }

    #[test]
    fn test_round_robin_distribution() {
        let lb = LoadBalancer::new(LoadBalanceStrategy::RoundRobin);
        let instances = make_instances();
        let first = lb.select(&instances, None).unwrap().instance_id.clone();
        let second = lb.select(&instances, None).unwrap().instance_id.clone();
        let third = lb.select(&instances, None).unwrap().instance_id.clone();
        let fourth = lb.select(&instances, None).unwrap().instance_id.clone();
        assert_ne!(first, second);
        assert_ne!(second, third);
        assert_eq!(first, fourth);
    }

    #[test]
    fn test_random_select_returns_valid() {
        let lb = LoadBalancer::new(LoadBalanceStrategy::Random);
        let instances = make_instances();
        for _ in 0..20 {
            let selected = lb.select(&instances, None).unwrap();
            assert!(instances
                .iter()
                .any(|i| i.instance_id == selected.instance_id));
        }
    }

    #[test]
    fn test_weighted_select_returns_valid() {
        let lb = LoadBalancer::new(LoadBalanceStrategy::Weighted);
        let instances = make_instances();
        let mut counts = HashMap::new();
        for _ in 0..600 {
            let selected = lb.select(&instances, None).unwrap();
            *counts.entry(selected.instance_id.clone()).or_insert(0) += 1;
        }
        let w2 = counts.get("svc-10.0.0.2-8080").copied().unwrap_or(0);
        let w1 = counts.get("svc-10.0.0.1-8080").copied().unwrap_or(0);
        assert!(
            w2 > w1,
            "weight=3 instance should be selected more than weight=1: w2={w2} w1={w1}"
        );
    }

    #[test]
    fn test_least_connections_picks_least() {
        let lb = LoadBalancer::new(LoadBalanceStrategy::LeastConnections);
        let instances = make_instances();
        lb.on_acquire(&instances[0].instance_id);
        lb.on_acquire(&instances[0].instance_id);
        lb.on_acquire(&instances[1].instance_id);
        let selected = lb.select(&instances, None).unwrap();
        assert_eq!(selected.instance_id, instances[2].instance_id);
    }

    #[test]
    fn test_least_connections_release() {
        let lb = LoadBalancer::new(LoadBalanceStrategy::LeastConnections);
        let instances = make_instances();
        lb.on_acquire(&instances[0].instance_id);
        lb.on_acquire(&instances[0].instance_id);
        lb.on_acquire(&instances[1].instance_id);
        lb.on_release(&instances[0].instance_id);
        let selected = lb.select(&instances, None).unwrap();
        assert_eq!(selected.instance_id, instances[2].instance_id);
    }

    #[test]
    fn test_consistent_hash_requires_key() {
        let lb = LoadBalancer::new(LoadBalanceStrategy::ConsistentHash);
        let instances = make_instances();
        let result = lb.select(&instances, None);
        assert!(matches!(result, Err(RegistryError::LoadBalance(_))));
    }

    #[test]
    fn test_consistent_hash_deterministic() {
        let lb = LoadBalancer::new(LoadBalanceStrategy::ConsistentHash);
        let instances = make_instances();
        let first = lb
            .select(&instances, Some("user-123"))
            .unwrap()
            .instance_id
            .clone();
        let second = lb
            .select(&instances, Some("user-123"))
            .unwrap()
            .instance_id
            .clone();
        assert_eq!(first, second);
    }

    #[test]
    fn test_consistent_hash_different_keys() {
        let lb = LoadBalancer::new(LoadBalanceStrategy::ConsistentHash);
        let instances = make_instances();
        let mut hits = std::collections::HashSet::new();
        for i in 0..30u32 {
            let selected = lb.select(&instances, Some(&format!("key-{i}"))).unwrap();
            hits.insert(selected.instance_id.clone());
        }
        assert!(
            hits.len() > 1,
            "different keys should hit different instances"
        );
    }

    #[test]
    fn test_on_acquire_release_no_op_for_non_lc() {
        let lb = LoadBalancer::new(LoadBalanceStrategy::RoundRobin);
        let instances = make_instances();
        lb.on_acquire(&instances[0].instance_id);
        assert_eq!(lb.connections.get(&instances[0].instance_id), 0);
        lb.on_release(&instances[0].instance_id);
    }

    /// 回归测试（v1.4）：全部实例 weight=0 时加权选择不得 panic。
    ///
    /// 修复前 `gen_range(0..total)` 遇 total=0 直接 panic；
    /// 修复后回退到随机选择。
    #[test]
    fn test_weighted_all_zero_weights_no_panic() {
        let lb = LoadBalancer::new(LoadBalanceStrategy::Weighted);
        let instances = vec![
            ServiceInstance {
                service_name: "svc".into(),
                instance_id: "z1".into(),
                host: "h1".into(),
                port: 80,
                weight: 0, // 直构绕过 with_weight 钳制
                metadata: HashMap::new(),
                health_check_url: None,
                status: InstanceStatus::Healthy,
            },
            ServiceInstance {
                service_name: "svc".into(),
                instance_id: "z2".into(),
                host: "h2".into(),
                port: 80,
                weight: 0,
                metadata: HashMap::new(),
                health_check_url: None,
                status: InstanceStatus::Healthy,
            },
        ];
        for _ in 0..10 {
            let selected = lb.select(&instances, None).unwrap();
            assert!(selected.instance_id == "z1" || selected.instance_id == "z2");
        }
    }

    /// 回归测试（v1.4）：反序列化 weight=0 被钳制为 1
    #[test]
    fn test_instance_weight_clamped_on_deserialize() {
        let json = r#"{"service_name":"svc","instance_id":"i1","host":"h","port":80,"weight":0,"metadata":{},"status":"healthy"}"#;
        let inst: ServiceInstance = serde_json::from_str(json).unwrap();
        assert_eq!(inst.weight, 1, "反序列化 weight=0 必须钳制为 1");
    }
}
