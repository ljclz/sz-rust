// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Kubernetes 服务注册降级适配（T022）
//!
//! 对齐 ADR-038：通过 K8s API 降级为本地服务列表，不实现 Operator。
//! Pod 就绪时注册到本地缓存，Pod 终止时从缓存移除。

use std::time::Duration;

use async_trait::async_trait;

use crate::error::RegistryError;
use crate::local_cache::LocalCache;
use crate::registry::{ServiceInstance, ServiceRegistry};

/// Kubernetes 服务注册（降级为本地缓存）
pub struct KubernetesRegistry {
    cache: LocalCache,
    api_server: String,
    namespace: String,
    client: reqwest::Client,
}

impl KubernetesRegistry {
    /// 创建 K8s 注册中心客户端
    pub fn new(api_server: &str, namespace: &str) -> Self {
        Self {
            cache: LocalCache::new(Duration::from_secs(60)),
            api_server: api_server.trim_end_matches('/').to_string(),
            namespace: namespace.to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// 获取本地缓存引用（降级发现用）
    pub fn cache(&self) -> &LocalCache {
        &self.cache
    }

    fn endpoints_url(&self, service_name: &str) -> String {
        format!(
            "{}/api/v1/namespaces/{}/endpoints/{}",
            self.api_server, self.namespace, service_name
        )
    }
}

#[async_trait]
impl ServiceRegistry for KubernetesRegistry {
    async fn register(&self, instance: &ServiceInstance) -> Result<(), RegistryError> {
        let mut current = self
            .cache
            .get_stale(&instance.service_name)
            .unwrap_or_default();
        current.retain(|i| i.instance_id != instance.instance_id);
        current.push(instance.clone());
        self.cache.update(&instance.service_name, current);
        Ok(())
    }

    async fn deregister(&self, instance_id: &str) -> Result<(), RegistryError> {
        self.cache.remove_instance(instance_id);
        Ok(())
    }

    async fn heartbeat(&self, _instance_id: &str) -> Result<(), RegistryError> {
        Ok(())
    }

    async fn discover(&self, service_name: &str) -> Result<Vec<ServiceInstance>, RegistryError> {
        if let Ok(cached) = self.cache.get(service_name) {
            let healthy: Vec<_> = cached.into_iter().filter(|i| i.is_serving()).collect();
            if !healthy.is_empty() {
                return Ok(healthy);
            }
        }

        let url = self.endpoints_url(service_name);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| RegistryError::Unreachable(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(RegistryError::Unreachable(format!(
                "K8s endpoints status: {}",
                resp.status()
            )));
        }

        let endpoints: K8sEndpoints = resp
            .json()
            .await
            .map_err(|e| RegistryError::Serialization(e.to_string()))?;

        let instances = endpoints.into_instances(service_name);
        self.cache.update(service_name, instances.clone());
        Ok(instances)
    }

    async fn health_check(&self) -> Result<(), RegistryError> {
        let url = format!("{}/healthz", self.api_server);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| RegistryError::Unreachable(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(RegistryError::Unreachable(format!(
                "K8s healthz status: {}",
                resp.status()
            )));
        }
        Ok(())
    }
}

#[derive(serde::Deserialize)]
struct K8sEndpoints {
    #[serde(default)]
    subsets: Vec<K8sSubset>,
}

#[derive(serde::Deserialize)]
struct K8sSubset {
    #[serde(default)]
    addresses: Vec<K8sAddress>,
    #[serde(default)]
    ports: Vec<K8sPort>,
}

#[derive(serde::Deserialize)]
struct K8sAddress {
    ip: String,
    #[serde(default)]
    target_ref: Option<K8sTargetRef>,
}

#[derive(serde::Deserialize)]
struct K8sTargetRef {
    name: String,
}

#[derive(serde::Deserialize)]
struct K8sPort {
    port: u16,
}

impl K8sEndpoints {
    fn into_instances(self, service_name: &str) -> Vec<ServiceInstance> {
        let mut instances = Vec::new();
        for subset in self.subsets {
            for addr in subset.addresses {
                let host = addr.ip;
                let instance_id = addr
                    .target_ref
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| format!("{}-{}", service_name, host));
                for port in &subset.ports {
                    instances.push(ServiceInstance {
                        service_name: service_name.to_string(),
                        instance_id: format!("{instance_id}-{}", port.port),
                        host: host.clone(),
                        port: port.port,
                        weight: 1,
                        metadata: std::collections::HashMap::new(),
                        health_check_url: None,
                        status: crate::registry::InstanceStatus::Healthy,
                    });
                }
            }
        }
        instances
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_k8s_registry_new() {
        let r = KubernetesRegistry::new("https://kubernetes.default.svc", "default");
        assert_eq!(r.api_server, "https://kubernetes.default.svc");
        assert_eq!(r.namespace, "default");
    }

    #[test]
    fn test_k8s_registry_new_trailing_slash() {
        let r = KubernetesRegistry::new("https://10.0.0.1:6443/", "prod");
        assert_eq!(r.api_server, "https://10.0.0.1:6443");
    }

    #[test]
    fn test_k8s_endpoints_url() {
        let r = KubernetesRegistry::new("https://10.0.0.1:6443", "default");
        assert_eq!(
            r.endpoints_url("user-svc"),
            "https://10.0.0.1:6443/api/v1/namespaces/default/endpoints/user-svc"
        );
    }

    #[tokio::test]
    async fn test_k8s_register_and_discover() {
        let r = KubernetesRegistry::new("http://unreachable.invalid", "default");
        let inst = ServiceInstance::new("user-svc", "10.0.0.1", 8080);
        r.register(&inst).await.unwrap();
        let discovered = r.discover("user-svc").await.unwrap();
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].host, "10.0.0.1");
    }

    #[tokio::test]
    async fn test_k8s_deregister() {
        let r = KubernetesRegistry::new("http://unreachable.invalid", "default");
        let inst = ServiceInstance::new("svc", "10.0.0.1", 8080);
        r.register(&inst).await.unwrap();
        assert_eq!(r.discover("svc").await.unwrap().len(), 1);
        r.deregister(&inst.instance_id).await.unwrap();
        let discovered = r.discover("svc").await;
        assert!(discovered.is_err() || discovered.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_k8s_heartbeat_noop() {
        let r = KubernetesRegistry::new("http://unreachable.invalid", "default");
        let result = r.heartbeat("any-id").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_k8s_register_replaces_existing() {
        let r = KubernetesRegistry::new("http://unreachable.invalid", "default");
        let inst1 = ServiceInstance::new("svc", "10.0.0.1", 8080);
        let inst2 = ServiceInstance::new("svc", "10.0.0.2", 8080);
        r.register(&inst1).await.unwrap();
        r.register(&inst2).await.unwrap();
        let discovered = r.discover("svc").await.unwrap();
        assert_eq!(discovered.len(), 2);
    }

    #[test]
    fn test_k8s_endpoints_into_instances() {
        let endpoints = K8sEndpoints {
            subsets: vec![K8sSubset {
                addresses: vec![K8sAddress {
                    ip: "10.0.0.1".into(),
                    target_ref: Some(K8sTargetRef {
                        name: "pod-1".into(),
                    }),
                }],
                ports: vec![K8sPort { port: 8080 }],
            }],
        };
        let instances = endpoints.into_instances("user-svc");
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].instance_id, "pod-1-8080");
        assert_eq!(instances[0].host, "10.0.0.1");
        assert_eq!(instances[0].port, 8080);
    }
}
