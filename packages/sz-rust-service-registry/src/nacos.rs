// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Nacos 服务注册中心适配（T021）
//!
//! Nacos Open API:
//! - 注册: POST /nacos/v1/ns/instance
//! - 注销: DELETE /nacos/v1/ns/instance
//! - 心跳: PUT /nacos/v1/ns/instance/beat
//! - 发现: GET /nacos/v1/ns/instance/list
//! - 健康: GET /nacos/v1/ns/operator/metrics

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::RegistryError;
use crate::registry::{InstanceStatus, ServiceInstance, ServiceRegistry};

/// Nacos 注册中心
pub struct NacosRegistry {
    base_url: String,
    client: reqwest::Client,
    namespace: String,
    username: Option<String>,
    password: Option<String>,
    access_token: parking_lot::RwLock<Option<String>>,
}

impl NacosRegistry {
    /// 创建 Nacos 注册中心客户端
    pub fn new(base_url: &str, namespace: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
            namespace: namespace.to_string(),
            username: None,
            password: None,
            access_token: parking_lot::RwLock::new(None),
        }
    }

    /// 设置认证信息
    pub fn with_auth(mut self, username: &str, password: &str) -> Self {
        self.username = Some(username.to_string());
        self.password = Some(password.to_string());
        self
    }

    fn build_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn add_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref token) = *self.access_token.read() {
            req.header("Authorization", format!("Bearer {token}"))
        } else {
            req
        }
    }
}

#[async_trait]
impl ServiceRegistry for NacosRegistry {
    async fn register(&self, instance: &ServiceInstance) -> Result<(), RegistryError> {
        let url = self.build_url("/nacos/v1/ns/instance");
        let mut form = vec![
            ("serviceName".to_string(), instance.service_name.clone()),
            ("ip".to_string(), instance.host.clone()),
            ("port".to_string(), instance.port.to_string()),
            ("weight".to_string(), instance.weight.to_string()),
            ("instanceId".to_string(), instance.instance_id.clone()),
            ("namespaceId".to_string(), self.namespace.clone()),
            ("enabled".to_string(), "true".to_string()),
            ("healthy".to_string(), "true".to_string()),
        ];
        for (k, v) in &instance.metadata {
            form.push((format!("metadata.{k}"), v.clone()));
        }

        let req = self.add_auth(self.client.post(&url).form(&form));
        let resp = req.send().await.map_err(|e| {
            RegistryError::RegisterFailed(instance.instance_id.clone(), e.to_string())
        })?;

        if !resp.status().is_success() {
            return Err(RegistryError::RegisterFailed(
                instance.instance_id.clone(),
                format!("status: {}", resp.status()),
            ));
        }
        Ok(())
    }

    async fn deregister(&self, instance_id: &str) -> Result<(), RegistryError> {
        let url = self.build_url("/nacos/v1/ns/instance");
        let req = self.add_auth(self.client.delete(&url).query(&[
            ("instanceId", instance_id),
            ("namespaceId", self.namespace.as_str()),
        ]));
        let resp = req
            .send()
            .await
            .map_err(|e| RegistryError::DeregisterFailed(instance_id.to_string(), e.to_string()))?;

        if !resp.status().is_success() {
            return Err(RegistryError::DeregisterFailed(
                instance_id.to_string(),
                format!("status: {}", resp.status()),
            ));
        }
        Ok(())
    }

    async fn heartbeat(&self, instance_id: &str) -> Result<(), RegistryError> {
        let url = self.build_url("/nacos/v1/ns/instance/beat");
        let beat = serde_json::json!({
            "ip": "",
            "port": 0,
            "instanceId": instance_id,
            "cluster": "DEFAULT",
        });
        let req = self.add_auth(self.client.put(&url).query(&[
            ("beat", beat.to_string().as_str()),
            ("namespaceId", self.namespace.as_str()),
        ]));
        let resp = req
            .send()
            .await
            .map_err(|e| RegistryError::HeartbeatFailed(instance_id.to_string(), e.to_string()))?;

        if !resp.status().is_success() {
            return Err(RegistryError::HeartbeatFailed(
                instance_id.to_string(),
                format!("status: {}", resp.status()),
            ));
        }
        Ok(())
    }

    async fn discover(&self, service_name: &str) -> Result<Vec<ServiceInstance>, RegistryError> {
        let url = self.build_url("/nacos/v1/ns/instance/list");
        let req = self.add_auth(self.client.get(&url).query(&[
            ("serviceName", service_name),
            ("namespaceId", self.namespace.as_str()),
            ("healthyOnly", "true"),
        ]));
        let resp = req
            .send()
            .await
            .map_err(|e| RegistryError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(RegistryError::Http(format!(
                "Nacos discover status: {}",
                resp.status()
            )));
        }

        let list: NacosInstanceList = resp
            .json()
            .await
            .map_err(|e| RegistryError::Serialization(e.to_string()))?;

        let instances = list
            .hosts
            .unwrap_or_default()
            .into_iter()
            .map(|h| h.into_instance(service_name))
            .collect();
        Ok(instances)
    }

    async fn health_check(&self) -> Result<(), RegistryError> {
        let url = self.build_url("/nacos/v1/ns/operator/metrics");
        let req = self.add_auth(self.client.get(&url));
        let resp = req
            .send()
            .await
            .map_err(|e| RegistryError::Unreachable(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(RegistryError::Unreachable(format!(
                "Nacos metrics status: {}",
                resp.status()
            )));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct NacosInstanceList {
    hosts: Option<Vec<NacosHost>>,
}

#[derive(Deserialize)]
struct NacosHost {
    ip: String,
    port: u16,
    #[serde(default)]
    valid: bool,
    #[serde(default)]
    healthy: bool,
    #[serde(default)]
    weight: f64,
    #[serde(rename = "instanceId")]
    instance_id: String,
    #[serde(default)]
    metadata: std::collections::HashMap<String, String>,
}

impl NacosHost {
    fn into_instance(self, service_name: &str) -> ServiceInstance {
        let status = if self.healthy && self.valid {
            InstanceStatus::Healthy
        } else {
            InstanceStatus::Unhealthy
        };
        ServiceInstance {
            service_name: service_name.to_string(),
            instance_id: self.instance_id,
            host: self.ip,
            port: self.port,
            weight: self.weight.max(1.0) as u32,
            metadata: self.metadata,
            health_check_url: None,
            status,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nacos_registry_new() {
        let r = NacosRegistry::new("http://127.0.0.1:8848", "public");
        assert_eq!(r.base_url, "http://127.0.0.1:8848");
        assert_eq!(r.namespace, "public");
        assert!(r.username.is_none());
    }

    #[test]
    fn test_nacos_registry_new_trailing_slash() {
        let r = NacosRegistry::new("http://127.0.0.1:8848/", "default");
        assert_eq!(r.base_url, "http://127.0.0.1:8848");
    }

    #[test]
    fn test_nacos_registry_with_auth() {
        let r = NacosRegistry::new("http://127.0.0.1:8848", "public").with_auth("admin", "nacos");
        assert_eq!(r.username.as_deref(), Some("admin"));
        assert_eq!(r.password.as_deref(), Some("nacos"));
    }

    #[test]
    fn test_nacos_build_url() {
        let r = NacosRegistry::new("http://127.0.0.1:8848", "public");
        assert_eq!(
            r.build_url("/nacos/v1/ns/instance"),
            "http://127.0.0.1:8848/nacos/v1/ns/instance"
        );
    }

    #[test]
    fn test_nacos_host_into_instance_healthy() {
        let host = NacosHost {
            ip: "10.0.0.1".into(),
            port: 8080,
            valid: true,
            healthy: true,
            weight: 3.0,
            instance_id: "inst-1".into(),
            metadata: {
                let mut m = std::collections::HashMap::new();
                m.insert("version".into(), "v2".into());
                m
            },
        };
        let inst = host.into_instance("user-svc");
        assert_eq!(inst.service_name, "user-svc");
        assert_eq!(inst.instance_id, "inst-1");
        assert_eq!(inst.host, "10.0.0.1");
        assert_eq!(inst.port, 8080);
        assert_eq!(inst.weight, 3);
        assert_eq!(inst.status, InstanceStatus::Healthy);
        assert_eq!(inst.metadata.get("version").unwrap(), "v2");
    }

    #[test]
    fn test_nacos_host_into_instance_unhealthy() {
        let host = NacosHost {
            ip: "10.0.0.2".into(),
            port: 9090,
            valid: false,
            healthy: true,
            weight: 0.5,
            instance_id: "inst-2".into(),
            metadata: std::collections::HashMap::new(),
        };
        let inst = host.into_instance("order-svc");
        assert_eq!(inst.status, InstanceStatus::Unhealthy);
        assert_eq!(inst.weight, 1);
    }

    #[test]
    fn test_nacos_host_into_instance_weight_clamped() {
        let host = NacosHost {
            ip: "10.0.0.3".into(),
            port: 7000,
            valid: true,
            healthy: true,
            weight: 0.0,
            instance_id: "inst-3".into(),
            metadata: std::collections::HashMap::new(),
        };
        let inst = host.into_instance("pay-svc");
        assert_eq!(inst.weight, 1);
    }

    #[test]
    fn test_nacos_instance_list_empty() {
        let list = NacosInstanceList { hosts: None };
        assert!(list.hosts.is_none());
    }

    // ------------------------------------------------------------------------
    // HTTP 错误路径测试（mockito 本地 mock 服务器）
    // ------------------------------------------------------------------------

    fn test_instance() -> ServiceInstance {
        ServiceInstance::new("user-svc", "10.0.0.1", 8080)
    }

    #[tokio::test]
    async fn test_nacos_register_success_and_error() {
        let mut server = mockito::Server::new_async().await;

        let m_ok = server
            .mock("POST", "/nacos/v1/ns/instance")
            .with_status(200)
            .create_async()
            .await;
        let r = NacosRegistry::new(&server.url(), "public");
        let inst = test_instance();
        assert!(r.register(&inst).await.is_ok(), "200 响应应注册成功");
        m_ok.assert_async().await;

        let m_err = server
            .mock("POST", "/nacos/v1/ns/instance")
            .with_status(500)
            .create_async()
            .await;
        assert!(
            matches!(
                r.register(&inst).await,
                Err(RegistryError::RegisterFailed(_, _))
            ),
            "500 响应应返回 RegisterFailed"
        );
        m_err.assert_async().await;
    }

    #[tokio::test]
    async fn test_nacos_deregister_success_and_error() {
        let mut server = mockito::Server::new_async().await;

        let m_ok = server
            .mock("DELETE", "/nacos/v1/ns/instance")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .create_async()
            .await;
        let r = NacosRegistry::new(&server.url(), "public");
        assert!(r.deregister("inst-1").await.is_ok());
        m_ok.assert_async().await;

        let m_err = server
            .mock("DELETE", "/nacos/v1/ns/instance")
            .match_query(mockito::Matcher::Any)
            .with_status(500)
            .create_async()
            .await;
        assert!(matches!(
            r.deregister("inst-1").await,
            Err(RegistryError::DeregisterFailed(_, _))
        ));
        m_err.assert_async().await;
    }

    #[tokio::test]
    async fn test_nacos_heartbeat_success_and_error() {
        let mut server = mockito::Server::new_async().await;

        let m_ok = server
            .mock("PUT", "/nacos/v1/ns/instance/beat")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .create_async()
            .await;
        let r = NacosRegistry::new(&server.url(), "public");
        assert!(r.heartbeat("inst-1").await.is_ok());
        m_ok.assert_async().await;

        let m_err = server
            .mock("PUT", "/nacos/v1/ns/instance/beat")
            .match_query(mockito::Matcher::Any)
            .with_status(500)
            .create_async()
            .await;
        assert!(matches!(
            r.heartbeat("inst-1").await,
            Err(RegistryError::HeartbeatFailed(_, _))
        ));
        m_err.assert_async().await;
    }

    #[tokio::test]
    async fn test_nacos_discover_success_and_error() {
        let mut server = mockito::Server::new_async().await;

        let body = r#"{
            "hosts": [
                {
                    "ip": "10.0.0.1",
                    "port": 8080,
                    "valid": true,
                    "healthy": true,
                    "weight": 3.0,
                    "instanceId": "inst-1",
                    "metadata": {"version": "v2"}
                }
            ]
        }"#;
        let m_ok = server
            .mock("GET", "/nacos/v1/ns/instance/list")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("Content-Type", "application/json")
            .with_body(body)
            .create_async()
            .await;
        let r = NacosRegistry::new(&server.url(), "public");
        let instances = r.discover("user-svc").await.unwrap();
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].instance_id, "inst-1");
        assert_eq!(instances[0].service_name, "user-svc");
        assert_eq!(instances[0].weight, 3);
        m_ok.assert_async().await;

        let m_err = server
            .mock("GET", "/nacos/v1/ns/instance/list")
            .match_query(mockito::Matcher::Any)
            .with_status(500)
            .create_async()
            .await;
        assert!(matches!(
            r.discover("user-svc").await,
            Err(RegistryError::Http(_))
        ));
        m_err.assert_async().await;
    }

    #[tokio::test]
    async fn test_nacos_health_check_success_and_error() {
        let mut server = mockito::Server::new_async().await;

        let m_ok = server
            .mock("GET", "/nacos/v1/ns/operator/metrics")
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
        let r = NacosRegistry::new(&server.url(), "public");
        assert!(r.health_check().await.is_ok());
        m_ok.assert_async().await;

        let m_err = server
            .mock("GET", "/nacos/v1/ns/operator/metrics")
            .with_status(500)
            .create_async()
            .await;
        assert!(matches!(
            r.health_check().await,
            Err(RegistryError::Unreachable(_))
        ));
        m_err.assert_async().await;
    }
}
