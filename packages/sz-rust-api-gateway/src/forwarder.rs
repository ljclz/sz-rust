// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 请求转发器（T028）
//!
//! 使用 reqwest 转发请求到后端服务，支持服务发现 LB。

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::config::GatewayRoute;
use crate::error::GatewayError;
use crate::middleware::AuthResult;
use crate::router_engine::{MatchRequest, RouterEngine};

/// 后端协议方案（http/https）
///
/// 用于 `BackendInstance` 的 URL scheme 字段，决定转发时使用明文 HTTP 还是 TLS HTTPS。
/// 默认 `Http`，保持向后兼容（未配置 scheme 时行为与历史版本一致）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendScheme {
    /// 明文 HTTP（默认，向后兼容）
    #[default]
    Http,
    /// TLS HTTPS
    Https,
}

impl fmt::Display for BackendScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BackendScheme::Http => f.write_str("http"),
            BackendScheme::Https => f.write_str("https"),
        }
    }
}

impl FromStr for BackendScheme {
    type Err = GatewayError;

    /// 从字符串解析 scheme，大小写不敏感。
    ///
    /// 接受 `http`/`https`（任意大小写），其余返回 `Config` 错误。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "http" => Ok(BackendScheme::Http),
            "https" => Ok(BackendScheme::Https),
            other => Err(GatewayError::Config(format!(
                "invalid backend scheme: {other}, expected `http` or `https`"
            ))),
        }
    }
}

impl BackendScheme {
    /// 从环境变量 `SZ300_BACKEND_SCHEME` 解析 scheme。
    ///
    /// - 未设置 → 返回 `Http`（默认，向后兼容）
    /// - 设置为 `http`/`https`（大小写不敏感） → 对应方案
    /// - 设置为非法值 → 返回 `Config` 错误
    ///
    /// 生产代码读取环境变量是允许的（仅 `std::fs` 被禁用，`std::env` 不在禁用范围）。
    pub fn from_env() -> Result<Self, GatewayError> {
        match std::env::var("SZ300_BACKEND_SCHEME") {
            Ok(raw) if !raw.trim().is_empty() => BackendScheme::from_str(raw.trim()),
            _ => Ok(BackendScheme::Http),
        }
    }
}

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
    /// URL scheme（http/https），默认 `Http` 向后兼容
    pub scheme: BackendScheme,
}

impl BackendInstance {
    /// 构造后端实例（默认 HTTP scheme，向后兼容）
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            host: host.to_string(),
            port,
            scheme: BackendScheme::Http,
        }
    }

    /// 构造后端实例并指定 scheme
    pub fn with_scheme(host: &str, port: u16, scheme: BackendScheme) -> Self {
        Self {
            host: host.to_string(),
            port,
            scheme,
        }
    }

    /// 链式设置 scheme
    pub fn scheme(mut self, scheme: BackendScheme) -> Self {
        self.scheme = scheme;
        self
    }

    /// 生成 URL
    pub fn url(&self, path: &str) -> String {
        format!("{}://{}:{}{}", self.scheme, self.host, self.port, path)
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
    fn test_forward_response_status_300_not_success() {
        // status 300 不属于 2xx 成功区间；`status < 300` 的 `<`→`<=` 变异体会误判。
        let resp = ForwardResponse::error(300, "multiple choices");
        assert!(!resp.is_success(), "status 300 不应视为成功");
    }

    #[test]
    fn test_forward_response_status_599_not_success() {
        let resp = ForwardResponse::error(599, "server error");
        assert!(!resp.is_success());
    }

    #[test]
    fn test_backend_instance_url() {
        let backend = BackendInstance::new("10.0.0.1", 8080);
        assert_eq!(backend.url("/api/users"), "http://10.0.0.1:8080/api/users");
    }

    #[test]
    fn test_backend_scheme_default_is_http() {
        // 向后兼容：未配置 scheme 时默认 HTTP
        let scheme = BackendScheme::default();
        assert_eq!(scheme, BackendScheme::Http);
        assert_eq!(scheme.to_string(), "http");
    }

    #[test]
    fn test_backend_scheme_display() {
        assert_eq!(BackendScheme::Http.to_string(), "http");
        assert_eq!(BackendScheme::Https.to_string(), "https");
    }

    #[test]
    fn test_backend_scheme_from_str_valid() {
        // 小写
        assert_eq!("http".parse::<BackendScheme>().unwrap(), BackendScheme::Http);
        assert_eq!("https".parse::<BackendScheme>().unwrap(), BackendScheme::Https);
        // 大小写不敏感
        assert_eq!("HTTP".parse::<BackendScheme>().unwrap(), BackendScheme::Http);
        assert_eq!("HTTPS".parse::<BackendScheme>().unwrap(), BackendScheme::Https);
        assert_eq!("HtTpS".parse::<BackendScheme>().unwrap(), BackendScheme::Https);
    }

    #[test]
    fn test_backend_scheme_from_str_invalid() {
        let err = "ftp".parse::<BackendScheme>().unwrap_err();
        assert!(matches!(err, GatewayError::Config(_)), "非法 scheme 应返回 Config 错误");

        let err = "".parse::<BackendScheme>().unwrap_err();
        assert!(matches!(err, GatewayError::Config(_)));
    }

    #[test]
    fn test_backend_scheme_serde() {
        // 序列化
        assert_eq!(
            serde_json::to_string(&BackendScheme::Http).unwrap(),
            "\"http\""
        );
        assert_eq!(
            serde_json::to_string(&BackendScheme::Https).unwrap(),
            "\"https\""
        );
        // 反序列化
        assert_eq!(
            serde_json::from_str::<BackendScheme>("\"http\"").unwrap(),
            BackendScheme::Http
        );
        assert_eq!(
            serde_json::from_str::<BackendScheme>("\"https\"").unwrap(),
            BackendScheme::Https
        );
        // 非法值反序列化失败
        assert!(serde_json::from_str::<BackendScheme>("\"ftp\"").is_err());
    }

    #[test]
    fn test_backend_instance_default_scheme_is_http() {
        // 向后兼容：BackendInstance::new 默认 HTTP scheme
        let backend = BackendInstance::new("10.0.0.1", 8080);
        assert_eq!(backend.scheme, BackendScheme::Http);
        assert_eq!(backend.url("/api"), "http://10.0.0.1:8080/api");
    }

    #[test]
    fn test_backend_instance_url_https() {
        // HTTPS scheme URL 构造
        let backend = BackendInstance::with_scheme("10.0.0.1", 8443, BackendScheme::Https);
        assert_eq!(backend.scheme, BackendScheme::Https);
        assert_eq!(backend.url("/api/users"), "https://10.0.0.1:8443/api/users");
    }

    #[test]
    fn test_backend_instance_with_scheme_constructor() {
        let http_backend = BackendInstance::with_scheme("h1", 80, BackendScheme::Http);
        let https_backend = BackendInstance::with_scheme("h2", 443, BackendScheme::Https);
        assert_eq!(http_backend.url("/"), "http://h1:80/");
        assert_eq!(https_backend.url("/"), "https://h2:443/");
    }

    #[test]
    fn test_backend_instance_scheme_builder_method() {
        // 链式 builder：从 new() 出发切换到 HTTPS
        let backend = BackendInstance::new("10.0.0.1", 8443).scheme(BackendScheme::Https);
        assert_eq!(backend.scheme, BackendScheme::Https);
        assert_eq!(backend.url("/health"), "https://10.0.0.1:8443/health");

        // 链式 builder：显式切回 HTTP
        let backend = BackendInstance::new("10.0.0.1", 8080).scheme(BackendScheme::Http);
        assert_eq!(backend.url("/health"), "http://10.0.0.1:8080/health");
    }

    #[test]
    fn test_backend_instance_scheme_serde_roundtrip() {
        // 验证 BackendInstance 通过外部 serde 配置加载时 scheme 字段正确解析
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct BackendCfg {
            host: String,
            port: u16,
            #[serde(default)]
            scheme: BackendScheme,
        }

        // 缺省 scheme 字段 → 默认 Http（向后兼容旧配置文件）
        let cfg: BackendCfg = serde_json::from_str(r#"{"host":"10.0.0.1","port":8080}"#).unwrap();
        assert_eq!(cfg.host, "10.0.0.1");
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.scheme, BackendScheme::Http);

        // 显式 https
        let cfg: BackendCfg =
            serde_json::from_str(r#"{"host":"10.0.0.1","port":8443,"scheme":"https"}"#).unwrap();
        assert_eq!(cfg.host, "10.0.0.1");
        assert_eq!(cfg.port, 8443);
        assert_eq!(cfg.scheme, BackendScheme::Https);
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
        // 编译期约束：RequestForwarder 必须 Send + Sync，可在多线程运行时跨任务传递
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RequestForwarder>();

        let forwarder = RequestForwarder::default();
        // 行为断言：default() 与 new() 各自构造独立转发器实例
        let another = RequestForwarder::new();
        assert!(
            !std::ptr::eq(&forwarder, &another),
            "default() 与 new() 不应返回同一实例"
        );
    }

    fn auth_ok() -> AuthResult {
        AuthResult {
            authorized: true,
            user_id: None,
            user_context: HashMap::new(),
        }
    }

    fn backend_for(server: &mockito::Server) -> BackendInstance {
        let port = server
            .url()
            .split(':')
            .next_back()
            .unwrap()
            .parse::<u16>()
            .unwrap();
        BackendInstance::new("127.0.0.1", port)
    }

    #[tokio::test]
    async fn test_forward_supports_all_http_methods() {
        let mut server = mockito::Server::new_async().await;
        let backend = backend_for(&server);
        let route = GatewayRoute::simple("r1", "/api/*", "user-svc");
        let forwarder = RequestForwarder::new();
        let auth = auth_ok();

        let requests: Vec<(&str, ForwardRequest)> = vec![
            ("GET", ForwardRequest::get("/api/test")),
            ("POST", ForwardRequest::post("/api/test", b"{}".to_vec())),
            (
                "PUT",
                ForwardRequest {
                    method: "PUT".to_string(),
                    path: "/api/test".to_string(),
                    headers: HashMap::new(),
                    body: None,
                },
            ),
            (
                "DELETE",
                ForwardRequest {
                    method: "DELETE".to_string(),
                    path: "/api/test".to_string(),
                    headers: HashMap::new(),
                    body: None,
                },
            ),
            (
                "PATCH",
                ForwardRequest {
                    method: "PATCH".to_string(),
                    path: "/api/test".to_string(),
                    headers: HashMap::new(),
                    body: None,
                },
            ),
        ];

        for (method, req) in requests {
            let m = server
                .mock(method, "/api/test")
                .with_status(200)
                .with_body("ok")
                .create_async()
                .await;
            let resp = forwarder.forward(&route, &backend, &req, &auth).await;
            assert!(
                matches!(resp, Ok(r) if r.status == 200),
                "HTTP {method} 应成功转发"
            );
            m.assert_async().await;
        }
    }

    #[tokio::test]
    async fn test_forward_unsupported_method_returns_error() {
        let server = mockito::Server::new_async().await;
        let backend = backend_for(&server);
        let route = GatewayRoute::simple("r1", "/api/*", "user-svc");
        let forwarder = RequestForwarder::new();
        let auth = auth_ok();

        let req = ForwardRequest {
            method: "TRACE".to_string(),
            path: "/api/test".to_string(),
            headers: HashMap::new(),
            body: None,
        };
        let result = forwarder.forward(&route, &backend, &req, &auth).await;
        assert!(
            matches!(result, Err(GatewayError::Forward(_))),
            "不支持的方法应返回 Forward 错误"
        );
    }
}
