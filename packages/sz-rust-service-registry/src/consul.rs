// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Consul 服务注册中心适配（T021）
//!
//! Consul Agent HTTP API:
//! - 注册: PUT /v1/agent/service/register
//! - 注销: PUT /v1/agent/service/deregister/{id}
//! - 心跳: PUT /v1/agent/check/pass/{check_id}
//! - 发现: GET /v1/health/service/{name}?passing=true
//! - 健康: GET /v1/status/leader

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::RegistryError;
use crate::registry::{InstanceStatus, ServiceInstance, ServiceRegistry};

/// Consul 注册中心
pub struct ConsulRegistry {
    base_url: String,
    client: reqwest::Client,
    token: Option<String>,
}

impl ConsulRegistry {
    /// 创建 Consul 注册中心客户端
    pub fn new(base_url: &str, token: Option<String>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
            token,
        }
    }

    fn build_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn add_token(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref token) = self.token {
            req.header("X-Consul-Token", token)
        } else {
            req
        }
    }
}

#[async_trait]
impl ServiceRegistry for ConsulRegistry {
    async fn register(&self, instance: &ServiceInstance) -> Result<(), RegistryError> {
        let url = self.build_url("/v1/agent/service/register");
        let body = serde_json::json!({
            "ID": instance.instance_id,
            "Name": instance.service_name,
            "Address": instance.host,
            "Port": instance.port,
            "Weight": instance.weight,
            "Tags": instance.metadata.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>(),
            "Check": {
                "HTTP": instance.health_check_url
                    .as_deref()
                    .unwrap_or(&instance.health_check_endpoint()),
                "Interval": "10s",
                "DeregisterCriticalServiceAfter": "30s",
            }
        });

        let req = self.add_token(self.client.put(&url).json(&body));
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
        let url = self.build_url(&format!("/v1/agent/service/deregister/{instance_id}"));
        let req = self.add_token(self.client.put(&url));
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
        let check_id = format!("service:{instance_id}");
        let url = self.build_url(&format!("/v1/agent/check/pass/{check_id}"));
        let req = self.add_token(self.client.put(&url));
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
        let url = self.build_url(&format!("/v1/health/service/{service_name}?passing=true"));
        let req = self.add_token(self.client.get(&url));
        let resp = req
            .send()
            .await
            .map_err(|e| RegistryError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(RegistryError::Http(format!(
                "Consul discover status: {}",
                resp.status()
            )));
        }

        let entries: Vec<ConsulHealthEntry> = resp
            .json()
            .await
            .map_err(|e| RegistryError::Serialization(e.to_string()))?;

        let instances = entries.into_iter().map(|e| e.into_instance()).collect();
        Ok(instances)
    }

    async fn health_check(&self) -> Result<(), RegistryError> {
        let url = self.build_url("/v1/status/leader");
        let req = self.add_token(self.client.get(&url));
        let resp = req
            .send()
            .await
            .map_err(|e| RegistryError::Unreachable(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(RegistryError::Unreachable(format!(
                "Consul leader status: {}",
                resp.status()
            )));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct ConsulHealthEntry {
    #[serde(rename = "Service")]
    service: ConsulService,
    #[serde(rename = "Checks")]
    checks: Vec<ConsulCheck>,
}

#[derive(Deserialize)]
struct ConsulService {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Service")]
    name: String,
    #[serde(rename = "Address")]
    address: String,
    #[serde(rename = "Port")]
    port: u16,
    #[serde(rename = "Weight", default)]
    weight: u32,
    #[serde(rename = "Tags", default)]
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct ConsulCheck {
    #[serde(rename = "Status")]
    status: String,
}

impl ConsulHealthEntry {
    fn into_instance(self) -> ServiceInstance {
        let status = if self.checks.iter().any(|c| c.status == "passing") {
            InstanceStatus::Healthy
        } else {
            InstanceStatus::Unhealthy
        };

        let mut metadata = std::collections::HashMap::new();
        for tag in self.service.tags {
            if let Some((k, v)) = tag.split_once('=') {
                metadata.insert(k.to_string(), v.to_string());
            }
        }

        ServiceInstance {
            service_name: self.service.name,
            instance_id: self.service.id,
            host: self.service.address,
            port: self.service.port,
            weight: self.service.weight.max(1),
            metadata,
            health_check_url: None,
            status,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consul_registry_new() {
        let r = ConsulRegistry::new("http://127.0.0.1:8500", None);
        assert_eq!(r.base_url, "http://127.0.0.1:8500");
        assert!(r.token.is_none());
    }

    #[test]
    fn test_consul_registry_new_trailing_slash() {
        let r = ConsulRegistry::new("http://127.0.0.1:8500/", None);
        assert_eq!(r.base_url, "http://127.0.0.1:8500");
    }

    #[test]
    fn test_consul_registry_with_token() {
        let r = ConsulRegistry::new("http://127.0.0.1:8500", Some("secret".into()));
        assert_eq!(r.token.as_deref(), Some("secret"));
    }

    #[test]
    fn test_consul_build_url() {
        let r = ConsulRegistry::new("http://127.0.0.1:8500", None);
        assert_eq!(
            r.build_url("/v1/status/leader"),
            "http://127.0.0.1:8500/v1/status/leader"
        );
    }

    #[test]
    fn test_consul_health_entry_into_instance_healthy() {
        let entry = ConsulHealthEntry {
            service: ConsulService {
                id: "svc-1".into(),
                name: "user-svc".into(),
                address: "10.0.0.1".into(),
                port: 8080,
                weight: 3,
                tags: vec!["version=v2".into(), "region=cn-east".into()],
            },
            checks: vec![ConsulCheck {
                status: "passing".into(),
            }],
        };
        let inst = entry.into_instance();
        assert_eq!(inst.instance_id, "svc-1");
        assert_eq!(inst.service_name, "user-svc");
        assert_eq!(inst.weight, 3);
        assert_eq!(inst.status, InstanceStatus::Healthy);
        assert_eq!(inst.metadata.get("version").unwrap(), "v2");
        assert_eq!(inst.metadata.get("region").unwrap(), "cn-east");
    }

    #[test]
    fn test_consul_health_entry_into_instance_unhealthy() {
        let entry = ConsulHealthEntry {
            service: ConsulService {
                id: "svc-2".into(),
                name: "order-svc".into(),
                address: "10.0.0.2".into(),
                port: 9090,
                weight: 0,
                tags: vec![],
            },
            checks: vec![ConsulCheck {
                status: "critical".into(),
            }],
        };
        let inst = entry.into_instance();
        assert_eq!(inst.status, InstanceStatus::Unhealthy);
        assert_eq!(inst.weight, 1);
    }

    #[test]
    fn test_consul_health_entry_mixed_checks() {
        let entry = ConsulHealthEntry {
            service: ConsulService {
                id: "svc-3".into(),
                name: "pay-svc".into(),
                address: "10.0.0.3".into(),
                port: 7000,
                weight: 1,
                tags: vec![],
            },
            checks: vec![
                ConsulCheck {
                    status: "passing".into(),
                },
                ConsulCheck {
                    status: "critical".into(),
                },
            ],
        };
        let inst = entry.into_instance();
        assert_eq!(inst.status, InstanceStatus::Healthy);
    }

    // ------------------------------------------------------------------------
    // HTTP 错误路径测试（mockito 本地 mock 服务器）
    // ------------------------------------------------------------------------

    fn test_instance() -> ServiceInstance {
        ServiceInstance::new("user-svc", "10.0.0.1", 8080)
    }

    #[tokio::test]
    async fn test_consul_register_success_and_error() {
        let mut server = mockito::Server::new_async().await;

        let m_ok = server
            .mock("PUT", "/v1/agent/service/register")
            .match_header("X-Consul-Token", "secret")
            .with_status(200)
            .create_async()
            .await;
        let r = ConsulRegistry::new(&server.url(), Some("secret".into()));
        let inst = test_instance();
        assert!(r.register(&inst).await.is_ok(), "200 响应应注册成功");
        m_ok.assert_async().await;

        // 500 → RegisterFailed；同时杀死 `!resp.status().is_success()` 的 `delete !` 变异体
        let m_err = server
            .mock("PUT", "/v1/agent/service/register")
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
    async fn test_consul_deregister_success_and_error() {
        let mut server = mockito::Server::new_async().await;

        let m_ok = server
            .mock("PUT", "/v1/agent/service/deregister/svc-1")
            .with_status(200)
            .create_async()
            .await;
        let r = ConsulRegistry::new(&server.url(), None);
        assert!(r.deregister("svc-1").await.is_ok());
        m_ok.assert_async().await;

        let m_err = server
            .mock("PUT", "/v1/agent/service/deregister/svc-1")
            .with_status(500)
            .create_async()
            .await;
        assert!(matches!(
            r.deregister("svc-1").await,
            Err(RegistryError::DeregisterFailed(_, _))
        ));
        m_err.assert_async().await;
    }

    #[tokio::test]
    async fn test_consul_heartbeat_success_and_error() {
        let mut server = mockito::Server::new_async().await;

        let m_ok = server
            .mock("PUT", "/v1/agent/check/pass/service:svc-1")
            .with_status(200)
            .create_async()
            .await;
        let r = ConsulRegistry::new(&server.url(), None);
        assert!(r.heartbeat("svc-1").await.is_ok());
        m_ok.assert_async().await;

        let m_err = server
            .mock("PUT", "/v1/agent/check/pass/service:svc-1")
            .with_status(500)
            .create_async()
            .await;
        assert!(matches!(
            r.heartbeat("svc-1").await,
            Err(RegistryError::HeartbeatFailed(_, _))
        ));
        m_err.assert_async().await;
    }

    #[tokio::test]
    async fn test_consul_discover_success_and_error() {
        let mut server = mockito::Server::new_async().await;

        let body = r#"[
            {
                "Service": {
                    "ID": "svc-1",
                    "Service": "user-svc",
                    "Address": "10.0.0.1",
                    "Port": 8080,
                    "Weight": 3,
                    "Tags": ["version=v2"]
                },
                "Checks": [{"Status": "passing"}]
            }
        ]"#;
        let m_ok = server
            .mock("GET", "/v1/health/service/user-svc?passing=true")
            .with_status(200)
            .with_header("Content-Type", "application/json")
            .with_body(body)
            .create_async()
            .await;
        let r = ConsulRegistry::new(&server.url(), None);
        let instances = r.discover("user-svc").await.unwrap();
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].instance_id, "svc-1");
        assert_eq!(instances[0].service_name, "user-svc");
        assert_eq!(instances[0].weight, 3);
        m_ok.assert_async().await;

        let m_err = server
            .mock("GET", "/v1/health/service/user-svc?passing=true")
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
    async fn test_consul_health_check_success_and_error() {
        let mut server = mockito::Server::new_async().await;

        let m_ok = server
            .mock("GET", "/v1/status/leader")
            .with_status(200)
            .with_body("\"127.0.0.1:8300\"")
            .create_async()
            .await;
        let r = ConsulRegistry::new(&server.url(), None);
        assert!(r.health_check().await.is_ok());
        m_ok.assert_async().await;

        let m_err = server
            .mock("GET", "/v1/status/leader")
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
