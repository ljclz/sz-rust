// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 服务注册发现核心 trait 与数据结构

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::error::RegistryError;

/// 实例状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceStatus {
    /// 健康（可接收流量）
    Healthy,
    /// 不健康（不接收流量）
    Unhealthy,
    /// 维护中（不接收流量）
    Maintenance,
}

/// 服务实例
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServiceInstance {
    /// 服务名（逻辑分组）
    pub service_name: String,
    /// 实例唯一 ID
    pub instance_id: String,
    /// 主机地址
    pub host: String,
    /// 端口
    pub port: u16,
    /// 权重（加权 LB 用，默认 1）
    ///
    /// 反序列化时钳制到 >=1：`weight=0` 会使加权负载均衡的总权重为 0，
    /// 进而在 `gen_range(0..0)` 上 panic（v1.4 修复）。
    #[serde(default = "default_weight", deserialize_with = "clamp_weight")]
    pub weight: u32,
    /// 元数据（标签/版本/区域等）
    pub metadata: HashMap<String, String>,
    /// 健康检查 URL
    pub health_check_url: Option<String>,
    /// 实例状态
    pub status: InstanceStatus,
}

fn default_weight() -> u32 {
    1
}

/// 反序列化钳制：weight 不得为 0（0 会导致加权 LB 总权重为 0 → gen_range panic）
fn clamp_weight<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let w = <u32 as serde::Deserialize>::deserialize(deserializer)?;
    Ok(w.max(1))
}

impl ServiceInstance {
    /// 构造新实例
    pub fn new(service_name: impl Into<String>, host: impl Into<String>, port: u16) -> Self {
        let service_name = service_name.into();
        let host = host.into();
        let instance_id = format!("{service_name}-{host}-{port}");
        Self {
            service_name,
            instance_id,
            host,
            port,
            weight: 1,
            metadata: HashMap::new(),
            health_check_url: None,
            status: InstanceStatus::Healthy,
        }
    }

    /// 生成健康检查 URL
    pub fn health_check_endpoint(&self) -> String {
        format!("http://{}:{}/health", self.host, self.port)
    }

    /// 是否可接收流量
    pub fn is_serving(&self) -> bool {
        matches!(self.status, InstanceStatus::Healthy)
    }

    /// 设置权重
    pub fn with_weight(mut self, weight: u32) -> Self {
        self.weight = weight.max(1);
        self
    }

    /// 设置元数据
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// 设置健康检查 URL
    pub fn with_health_check_url(mut self, url: impl Into<String>) -> Self {
        self.health_check_url = Some(url.into());
        self
    }
}

/// 负载均衡策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalanceStrategy {
    /// 轮询
    #[default]
    RoundRobin,
    /// 随机
    Random,
    /// 加权
    Weighted,
    /// 最少连接
    LeastConnections,
    /// 一致性哈希
    ConsistentHash,
}

impl LoadBalanceStrategy {
    /// 所有策略列表（测试用）
    pub fn all() -> &'static [Self] {
        &[
            Self::RoundRobin,
            Self::Random,
            Self::Weighted,
            Self::LeastConnections,
            Self::ConsistentHash,
        ]
    }
}

/// 服务注册中心 trait
///
/// 三后端实现：Consul / Nacos / Kubernetes Service
#[async_trait]
pub trait ServiceRegistry: Send + Sync + 'static {
    /// 注册实例
    async fn register(&self, instance: &ServiceInstance) -> Result<(), RegistryError>;

    /// 注销实例
    async fn deregister(&self, instance_id: &str) -> Result<(), RegistryError>;

    /// 发送心跳
    async fn heartbeat(&self, instance_id: &str) -> Result<(), RegistryError>;

    /// 发现服务实例（仅返回 Healthy 实例）
    async fn discover(&self, service_name: &str) -> Result<Vec<ServiceInstance>, RegistryError>;

    /// 健康检查注册中心自身
    async fn health_check(&self) -> Result<(), RegistryError>;
}

/// 连接计数器（最少连接 LB 用）
#[derive(Debug, Default)]
pub struct ConnectionCounter {
    inner: Arc<parking_lot::RwLock<HashMap<String, AtomicU64>>>,
}

impl ConnectionCounter {
    /// 构造空计数器
    pub fn new() -> Self {
        Self::default()
    }

    /// 增加实例连接数
    pub fn incr(&self, instance_id: &str) -> u64 {
        let map = self.inner.read();
        if let Some(c) = map.get(instance_id) {
            return c.fetch_add(1, Ordering::Relaxed) + 1;
        }
        drop(map);
        let mut map = self.inner.write();
        map.entry(instance_id.to_string())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed)
            + 1
    }

    /// 减少实例连接数
    pub fn decr(&self, instance_id: &str) -> u64 {
        let map = self.inner.read();
        if let Some(c) = map.get(instance_id) {
            let prev = c.fetch_sub(1, Ordering::Relaxed);
            return prev.saturating_sub(1);
        }
        0
    }

    /// 获取实例当前连接数
    pub fn get(&self, instance_id: &str) -> u64 {
        let map = self.inner.read();
        map.get(instance_id)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_instance_new() {
        let inst = ServiceInstance::new("user-svc", "10.0.0.1", 8080);
        assert_eq!(inst.service_name, "user-svc");
        assert_eq!(inst.host, "10.0.0.1");
        assert_eq!(inst.port, 8080);
        assert_eq!(inst.weight, 1);
        assert_eq!(inst.status, InstanceStatus::Healthy);
        assert!(inst.is_serving());
        assert_eq!(inst.instance_id, "user-svc-10.0.0.1-8080");
    }

    #[test]
    fn test_health_check_endpoint() {
        let inst = ServiceInstance::new("order-svc", "10.0.0.2", 9090);
        assert_eq!(inst.health_check_endpoint(), "http://10.0.0.2:9090/health");
    }

    #[test]
    fn test_builder_methods() {
        let inst = ServiceInstance::new("pay-svc", "10.0.0.3", 7000)
            .with_weight(5)
            .with_metadata("version", "v2")
            .with_metadata("region", "cn-east-1")
            .with_health_check_url("http://10.0.0.3:7000/healthz");
        assert_eq!(inst.weight, 5);
        assert_eq!(inst.metadata.get("version").unwrap(), "v2");
        assert_eq!(inst.metadata.get("region").unwrap(), "cn-east-1");
        assert_eq!(
            inst.health_check_url.as_ref().unwrap(),
            "http://10.0.0.3:7000/healthz"
        );
    }

    #[test]
    fn test_weight_zero_clamped_to_one() {
        let inst = ServiceInstance::new("svc", "h", 80).with_weight(0);
        assert_eq!(inst.weight, 1);
    }

    #[test]
    fn test_instance_status_serde() {
        let s = serde_json::to_string(&InstanceStatus::Maintenance).unwrap();
        assert_eq!(s, "\"maintenance\"");
        let v: InstanceStatus = serde_json::from_str("\"unhealthy\"").unwrap();
        assert_eq!(v, InstanceStatus::Unhealthy);
    }

    #[test]
    fn test_is_serving() {
        assert!(ServiceInstance {
            service_name: "s".into(),
            instance_id: "i".into(),
            host: "h".into(),
            port: 80,
            weight: 1,
            metadata: HashMap::new(),
            health_check_url: None,
            status: InstanceStatus::Healthy,
        }
        .is_serving());

        assert!(!ServiceInstance {
            service_name: "s".into(),
            instance_id: "i".into(),
            host: "h".into(),
            port: 80,
            weight: 1,
            metadata: HashMap::new(),
            health_check_url: None,
            status: InstanceStatus::Unhealthy,
        }
        .is_serving());
    }

    #[test]
    fn test_lb_strategy_all() {
        let all = LoadBalanceStrategy::all();
        assert_eq!(all.len(), 5);
        assert!(all.contains(&LoadBalanceStrategy::RoundRobin));
        assert!(all.contains(&LoadBalanceStrategy::ConsistentHash));
    }

    #[test]
    fn test_lb_strategy_default() {
        assert_eq!(
            LoadBalanceStrategy::default(),
            LoadBalanceStrategy::RoundRobin
        );
    }

    #[test]
    fn test_lb_strategy_serde() {
        let s = serde_json::to_string(&LoadBalanceStrategy::LeastConnections).unwrap();
        assert_eq!(s, "\"least_connections\"");
    }

    #[test]
    fn test_connection_counter_basic() {
        let counter = ConnectionCounter::new();
        assert_eq!(counter.get("a"), 0);
        assert_eq!(counter.incr("a"), 1);
        assert_eq!(counter.incr("a"), 2);
        assert_eq!(counter.get("a"), 2);
        assert_eq!(counter.decr("a"), 1);
        assert_eq!(counter.get("a"), 1);
    }

    #[test]
    fn test_connection_counter_decr_below_zero() {
        let counter = ConnectionCounter::new();
        assert_eq!(counter.decr("unknown"), 0);
    }

    #[test]
    fn test_connection_counter_multiple_instances() {
        let counter = ConnectionCounter::new();
        counter.incr("a");
        counter.incr("a");
        counter.incr("b");
        assert_eq!(counter.get("a"), 2);
        assert_eq!(counter.get("b"), 1);
    }
}
