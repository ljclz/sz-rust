// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! API 网关配置（T027）

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// 后端协议
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum BackendProtocol {
    /// HTTP（优先）
    #[default]
    Http,
    /// gRPC（实验性）
    Grpc,
}

/// 网关路由规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayRoute {
    /// 路由 ID
    pub route_id: String,
    /// 路径匹配（支持 `*` 通配符，如 `/api/users/*`）
    pub match_path: String,
    /// Host 匹配（可选）
    pub match_host: Option<String>,
    /// Header 匹配（全部匹配才命中）
    pub match_headers: HashMap<String, String>,
    /// 后端服务名（服务发现用）
    pub backend_service: String,
    /// 后端协议
    pub protocol: BackendProtocol,
    /// 路径重写（可选，如 `/api/users/*` → `/users/*`）
    pub path_rewrite: Option<String>,
    /// 附加注入 Header
    pub inject_headers: HashMap<String, String>,
}

impl GatewayRoute {
    /// 构造简单 HTTP 路由
    pub fn simple(route_id: &str, path: &str, service: &str) -> Self {
        Self {
            route_id: route_id.to_string(),
            match_path: path.to_string(),
            match_host: None,
            match_headers: HashMap::new(),
            backend_service: service.to_string(),
            protocol: BackendProtocol::Http,
            path_rewrite: None,
            inject_headers: HashMap::new(),
        }
    }

    /// 设置 Host 匹配
    pub fn with_host(mut self, host: &str) -> Self {
        self.match_host = Some(host.to_string());
        self
    }

    /// 设置路径重写
    pub fn with_rewrite(mut self, rewrite: &str) -> Self {
        self.path_rewrite = Some(rewrite.to_string());
        self
    }

    /// 添加 Header 匹配
    pub fn with_header_match(mut self, key: &str, value: &str) -> Self {
        self.match_headers
            .insert(key.to_string(), value.to_string());
        self
    }

    /// 添加注入 Header
    pub fn with_inject_header(mut self, key: &str, value: &str) -> Self {
        self.inject_headers
            .insert(key.to_string(), value.to_string());
        self
    }

    /// 设置后端协议
    pub fn with_protocol(mut self, protocol: BackendProtocol) -> Self {
        self.protocol = protocol;
        self
    }
}

/// 鉴权配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// 是否启用鉴权
    pub enabled: bool,
    /// Token Header 名
    pub token_header: String,
    /// 公开路径（不需要鉴权）
    pub public_paths: Vec<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            token_header: "Authorization".to_string(),
            public_paths: vec!["/health".to_string(), "/metrics".to_string()],
        }
    }
}

/// 限流配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// 是否启用限流
    pub enabled: bool,
    /// 每秒请求数
    pub requests_per_second: u32,
    /// 突发容量
    pub burst: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests_per_second: 100,
            burst: 200,
        }
    }
}

/// 熔断配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// 是否启用熔断
    pub enabled: bool,
    /// 失败阈值
    pub failure_threshold: u32,
    /// 熔断恢复时间（秒）
    pub reset_timeout_secs: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            failure_threshold: 5,
            reset_timeout_secs: 30,
        }
    }
}

/// 网关配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GatewayConfig {
    /// 路由规则
    pub routes: Vec<GatewayRoute>,
    /// 鉴权配置
    pub auth: AuthConfig,
    /// 限流配置
    pub rate_limit: RateLimitConfig,
    /// 熔断配置
    pub circuit_breaker: CircuitBreakerConfig,
}

impl GatewayConfig {
    /// 构造空配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加路由
    pub fn with_route(mut self, route: GatewayRoute) -> Self {
        self.routes.push(route);
        self
    }

    /// 从 JSON 加载
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_protocol_default() {
        assert_eq!(BackendProtocol::default(), BackendProtocol::Http);
    }

    #[test]
    fn test_backend_protocol_serde() {
        assert_eq!(
            serde_json::to_string(&BackendProtocol::Http).unwrap(),
            "\"http\""
        );
        assert_eq!(
            serde_json::to_string(&BackendProtocol::Grpc).unwrap(),
            "\"grpc\""
        );
    }

    #[test]
    fn test_gateway_route_simple() {
        let route = GatewayRoute::simple("r1", "/api/users", "user-svc");
        assert_eq!(route.route_id, "r1");
        assert_eq!(route.match_path, "/api/users");
        assert_eq!(route.backend_service, "user-svc");
        assert_eq!(route.protocol, BackendProtocol::Http);
        assert!(route.match_host.is_none());
        assert!(route.path_rewrite.is_none());
    }

    #[test]
    fn test_gateway_route_builder() {
        let route = GatewayRoute::simple("r1", "/api/*", "backend")
            .with_host("api.example.com")
            .with_rewrite("/v2/*")
            .with_header_match("X-Version", "2")
            .with_inject_header("X-Gateway", "sz-rust")
            .with_protocol(BackendProtocol::Grpc);

        assert_eq!(route.match_host.as_deref(), Some("api.example.com"));
        assert_eq!(route.path_rewrite.as_deref(), Some("/v2/*"));
        assert_eq!(route.match_headers.get("X-Version").unwrap(), "2");
        assert_eq!(route.inject_headers.get("X-Gateway").unwrap(), "sz-rust");
        assert_eq!(route.protocol, BackendProtocol::Grpc);
    }

    #[test]
    fn test_auth_config_default() {
        let auth = AuthConfig::default();
        assert!(auth.enabled);
        assert_eq!(auth.token_header, "Authorization");
        assert!(auth.public_paths.contains(&"/health".to_string()));
    }

    #[test]
    fn test_rate_limit_config_default() {
        let rl = RateLimitConfig::default();
        assert!(rl.enabled);
        assert_eq!(rl.requests_per_second, 100);
        assert_eq!(rl.burst, 200);
    }

    #[test]
    fn test_circuit_breaker_config_default() {
        let cb = CircuitBreakerConfig::default();
        assert!(cb.enabled);
        assert_eq!(cb.failure_threshold, 5);
        assert_eq!(cb.reset_timeout_secs, 30);
    }

    #[test]
    fn test_gateway_config_builder() {
        let config = GatewayConfig::new()
            .with_route(GatewayRoute::simple("r1", "/api", "svc1"))
            .with_route(GatewayRoute::simple("r2", "/web", "svc2"));
        assert_eq!(config.routes.len(), 2);
    }

    #[test]
    fn test_gateway_config_from_json() {
        let json = r#"{"routes":[],"auth":{"enabled":false,"token_header":"X-Token","public_paths":[]},"rate_limit":{"enabled":false,"requests_per_second":0,"burst":0},"circuit_breaker":{"enabled":false,"failure_threshold":0,"reset_timeout_secs":0}}"#;
        let config = GatewayConfig::from_json(json).unwrap();
        assert!(!config.auth.enabled);
        assert_eq!(config.auth.token_header, "X-Token");
    }
}
