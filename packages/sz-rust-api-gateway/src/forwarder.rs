// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 请求转发器（T028）
//!
//! 使用 reqwest 转发请求到后端服务，支持服务发现 LB。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::GatewayRoute;
use crate::error::GatewayError;
use crate::middleware::AuthResult;
use crate::router_engine::{MatchRequest, RouterEngine};

/// 转发请求
#[derive(Debug, Clone)]
pub struct ForwardRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
}

impl ForwardRequest {
    /// 构造 GET 请求
    pub fn get(path: &str) -> Self {
        Self {
            method: "GET".to_string(),
            path: path.to_string(),
            headers: HashMap::new(),
            body: None,
        }
    }

    /// 构造 POST 请求
    pub fn post(path: &str, body: Vec<u8>) -> Self {
        Self {
            method: "POST".to_string(),
            path: path.to_string(),
            headers: HashMap::new(),
            body: Some(body),
        }
    }

    /// 添加 Header
    pub fn with_header(mut self, key: &str, value: &str) -> Self {
        self.headers.insert(key.to_string(), value.to_string());
        self
    }
}

/// 转发响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl ForwardResponse {
    /// 构造成功响应
    pub fn ok(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            headers: HashMap::new(),
            body,
        }
    }

    /// 构造错误响应
    pub fn error(status: u16, message: &str) -> Self {
        Self {
            status,
            headers: HashMap::new(),
            body: message.as_bytes().to_vec(),
        }
    }

    /// 是否成功
    pub fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }
}

/// 后端实例
#[derive(Debug, Clone)]
pub struct BackendInstance {
    pub host: String,
    pub port: u16,
}

impl BackendInstance {
    /// 构造后端实例
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            host: host.to_string(),
            port,
        }
    }

    /// 生成 URL
    pub fn url(&self, path: &str) -> String {
        format!("http://{}:{}{}", self.host, self.port, path)
    }
}

/// 请求转发器
pub struct RequestForwarder {
    client: reqwest::Client,
}

impl Default for RequestForwarder {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestForwarder {
    /// 创建转发器
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// 转发请求到后端
    pub async fn forward(
        &self,
        route: &GatewayRoute,
        backend: &BackendInstance,
        request: &ForwardRequest,
        auth: &AuthResult,
    ) -> Result<ForwardResponse, GatewayError> {
        let path = RouterEngine::rewrite_path(route, &request.path);
        let url = backend.url(&path);

        let mut req = match request.method.to_uppercase().as_str() {
            "GET" => self.client.get(&url),
            "POST" => self.client.post(&url),
            "PUT" => self.client.put(&url),
            "DELETE" => self.client.delete(&url),
            "PATCH" => self.client.patch(&url),
            _ => {
                return Err(GatewayError::Forward(format!(
                    "unsupported method: {}",
                    request.method
                )))
            }
        };

        for (key, value) in &request.headers {
            req = req.header(key, value);
        }

        for (key, value) in &route.inject_headers {
            req = req.header(key, value);
        }

        for (key, value) in &auth.user_context {
            req = req.header(key, value);
        }

        if let Some(body) = &request.body {
            req = req.body(body.clone());
        }

        let resp = req
            .send()
            .await
            .map_err(|e| GatewayError::Forward(e.to_string()))?;

        let status = resp.status().as_u16();
        let mut headers = HashMap::new();
        for (key, value) in resp.headers() {
            if let Ok(v) = value.to_str() {
                headers.insert(key.as_str().to_string(), v.to_string());
            }
        }
        let body = resp
            .bytes()
            .await
            .map_err(|e| GatewayError::Forward(e.to_string()))?
            .to_vec();

        Ok(ForwardResponse {
            status,
            headers,
            body,
        })
    }

    /// 选择后端实例（简化版，生产环境应使用 ServiceRegistry + LoadBalancer）
    pub fn select_backend(instances: &[BackendInstance]) -> Result<&BackendInstance, GatewayError> {
        if instances.is_empty() {
            return Err(GatewayError::BackendUnavailable("no instances".into()));
        }
        Ok(&instances[0])
    }
}

/// 网关处理器（组合路由 + 鉴权 + 限流 + 熔断 + 转发）
pub struct GatewayHandler {
    router: RouterEngine,
    forwarder: RequestForwarder,
    auth: crate::middleware::AuthMiddleware,
    rate_limit: crate::middleware::RateLimitMiddleware,
    circuit_breaker: crate::middleware::CircuitBreakerMiddleware,
}

impl GatewayHandler {
    /// 创建网关处理器
    pub fn new(
        router: RouterEngine,
        auth: crate::middleware::AuthMiddleware,
        rate_limit: crate::middleware::RateLimitMiddleware,
        circuit_breaker: crate::middleware::CircuitBreakerMiddleware,
    ) -> Self {
        Self {
            router,
            forwarder: RequestForwarder::new(),
            auth,
            rate_limit,
            circuit_breaker,
        }
    }

    /// 处理请求
    pub async fn handle(
        &self,
        request: &ForwardRequest,
        backends: &HashMap<String, Vec<BackendInstance>>,
    ) -> Result<ForwardResponse, GatewayError> {
        let match_req = MatchRequest::new(&request.path);
        let route = self.router.match_route(&match_req)?;

        let auth_result = self.auth.check(&request.path, &request.headers)?;

        let rl_key = auth_result.user_id.as_deref().unwrap_or("anonymous");
        self.rate_limit.check(rl_key)?;

        self.circuit_breaker.can_request()?;

        let instances = backends
            .get(&route.backend_service)
            .ok_or_else(|| GatewayError::BackendUnavailable(route.backend_service.clone()))?;

        let backend = RequestForwarder::select_backend(instances)?;

        match self
            .forwarder
            .forward(route, backend, request, &auth_result)
            .await
        {
            Ok(resp) => {
                self.circuit_breaker.record_success();
                Ok(resp)
            }
            Err(e) => {
                self.circuit_breaker.record_failure();
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthConfig, CircuitBreakerConfig, GatewayRoute, RateLimitConfig};
    use crate::router_engine::RouterEngine;

    #[test]
    fn test_forward_request_get() {
        let req = ForwardRequest::get("/api/users").with_header("X-Test", "true");
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/api/users");
        assert_eq!(req.headers.get("X-Test").unwrap(), "true");
        assert!(req.body.is_none());
    }

    #[test]
    fn test_forward_request_post() {
        let req = ForwardRequest::post("/api/users", b"{}".to_vec());
        assert_eq!(req.method, "POST");
        assert_eq!(req.body.as_ref().unwrap(), b"{}");
    }

    #[test]
    fn test_forward_response_ok() {
        let resp = ForwardResponse::ok(b"success".to_vec());
        assert!(resp.is_success());
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn test_forward_response_error() {
        let resp = ForwardResponse::error(404, "not found");
        assert!(!resp.is_success());
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn test_backend_instance_url() {
        let backend = BackendInstance::new("10.0.0.1", 8080);
        assert_eq!(backend.url("/api/users"), "http://10.0.0.1:8080/api/users");
    }

    #[test]
    fn test_select_backend_empty() {
        let result = RequestForwarder::select_backend(&[]);
        assert!(matches!(result, Err(GatewayError::BackendUnavailable(_))));
    }

    #[test]
    fn test_select_backend_returns_first() {
        let instances = vec![
            BackendInstance::new("10.0.0.1", 8080),
            BackendInstance::new("10.0.0.2", 8080),
        ];
        let selected = RequestForwarder::select_backend(&instances).unwrap();
        assert_eq!(selected.host, "10.0.0.1");
    }

    #[tokio::test]
    async fn test_gateway_handler_no_route() {
        let handler = GatewayHandler::new(
            RouterEngine::new(vec![]),
            crate::middleware::AuthMiddleware::new(AuthConfig::default()),
            crate::middleware::RateLimitMiddleware::new(RateLimitConfig::default()),
            crate::middleware::CircuitBreakerMiddleware::new(CircuitBreakerConfig::default()),
        );

        let req = ForwardRequest::get("/unknown");
        let backends = HashMap::new();
        let result = handler.handle(&req, &backends).await;
        assert!(matches!(result, Err(GatewayError::NoRoute(_))));
    }

    #[tokio::test]
    async fn test_gateway_handler_no_backend() {
        let route = GatewayRoute::simple("r1", "/api/*", "user-svc");
        let handler = GatewayHandler::new(
            RouterEngine::new(vec![route]),
            crate::middleware::AuthMiddleware::new(AuthConfig {
                enabled: false,
                ..Default::default()
            }),
            crate::middleware::RateLimitMiddleware::new(RateLimitConfig {
                enabled: false,
                ..Default::default()
            }),
            crate::middleware::CircuitBreakerMiddleware::new(CircuitBreakerConfig {
                enabled: false,
                ..Default::default()
            }),
        );

        let req = ForwardRequest::get("/api/users");
        let backends = HashMap::new();
        let result = handler.handle(&req, &backends).await;
        assert!(matches!(result, Err(GatewayError::BackendUnavailable(_))));
    }

    #[test]
    fn test_forwarder_default() {
        let _forwarder = RequestForwarder::default();
    }
}
