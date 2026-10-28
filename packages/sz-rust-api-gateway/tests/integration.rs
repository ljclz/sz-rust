// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! API 网关集成测试（T030）
//!
//! 验证转发/鉴权/限流/熔断/LB 完整链路

use std::collections::HashMap;

use sz_rust_api_gateway::{
    AuthConfig, AuthMiddleware, BackendInstance, CircuitBreakerConfig, CircuitBreakerMiddleware,
    CircuitState, ForwardRequest, GatewayConfig, GatewayHandler, GatewayRoute, MatchRequest,
    RateLimitConfig, RateLimitMiddleware, RouterEngine,
};

fn make_handler() -> GatewayHandler {
    let routes = vec![
        GatewayRoute::simple("public", "/health", "health-svc"),
        GatewayRoute::simple("users", "/api/users/*", "user-svc")
            .with_inject_header("X-Gateway", "sz-rust"),
        GatewayRoute::simple("orders", "/api/orders/*", "order-svc"),
    ];

    GatewayHandler::new(
        RouterEngine::new(routes),
        AuthMiddleware::new(AuthConfig::default()),
        RateLimitMiddleware::new(RateLimitConfig {
            enabled: true,
            requests_per_second: 100,
            burst: 10,
        }),
        CircuitBreakerMiddleware::new(CircuitBreakerConfig::default()),
    )
}

fn make_backends() -> HashMap<String, Vec<BackendInstance>> {
    HashMap::from([
        (
            "health-svc".to_string(),
            vec![BackendInstance::new("10.0.0.1", 8080)],
        ),
        (
            "user-svc".to_string(),
            vec![
                BackendInstance::new("10.0.0.2", 8080),
                BackendInstance::new("10.0.0.3", 8080),
            ],
        ),
        (
            "order-svc".to_string(),
            vec![BackendInstance::new("10.0.0.4", 8080)],
        ),
    ])
}

/// 场景 1：无 Token 访问受保护路由 → 401
#[tokio::test]
async fn test_no_token_returns_401() {
    let handler = make_handler();
    let backends = make_backends();

    let req = ForwardRequest::get("/api/users/123");
    let result = handler.handle(&req, &backends).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(
        err,
        sz_rust_api_gateway::GatewayError::Unauthorized(_)
    ));
}

/// 场景 2：有 Token 访问受保护路由 → 转发
#[tokio::test]
async fn test_valid_token_proceeds_to_forward() {
    let handler = make_handler();
    let backends = make_backends();

    let req = ForwardRequest::get("/api/users/123").with_header("Authorization", "Bearer abc12345");
    let result = handler.handle(&req, &backends).await;

    // 后端不可达（10.0.0.2 不是真实服务），但应该通过鉴权/限流/熔断
    assert!(result.is_err());
    let err = result.unwrap_err();
    // 应该是 Forward 错误而非 Unauthorized/RateLimited/CircuitBroken
    assert!(!matches!(
        err,
        sz_rust_api_gateway::GatewayError::Unauthorized(_)
    ));
    assert!(!matches!(
        err,
        sz_rust_api_gateway::GatewayError::RateLimited(_)
    ));
    assert!(!matches!(
        err,
        sz_rust_api_gateway::GatewayError::CircuitBroken(_)
    ));
}

/// 场景 3：公开路径不需要鉴权
#[tokio::test]
async fn test_public_path_no_auth_needed() {
    let handler = make_handler();
    let backends = make_backends();

    let req = ForwardRequest::get("/health");
    let result = handler.handle(&req, &backends).await;

    // 应该通过鉴权（公开路径），但后端不可达
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(!matches!(
        err,
        sz_rust_api_gateway::GatewayError::Unauthorized(_)
    ));
}

/// 场景 4：路由不匹配 → 404
#[tokio::test]
async fn test_no_route_returns_404() {
    let handler = make_handler();
    let backends = make_backends();

    let req = ForwardRequest::get("/unknown/path");
    let result = handler.handle(&req, &backends).await;

    assert!(matches!(
        result,
        Err(sz_rust_api_gateway::GatewayError::NoRoute(_))
    ));
}

/// 场景 5：后端服务不存在 → 503
#[tokio::test]
async fn test_backend_service_not_found() {
    let routes = vec![GatewayRoute::simple("r1", "/api/*", "missing-svc")];
    let handler = GatewayHandler::new(
        RouterEngine::new(routes),
        AuthMiddleware::new(AuthConfig {
            enabled: false,
            ..Default::default()
        }),
        RateLimitMiddleware::new(RateLimitConfig::default()),
        CircuitBreakerMiddleware::new(CircuitBreakerConfig::default()),
    );

    let req = ForwardRequest::get("/api/data");
    let backends = HashMap::new();
    let result = handler.handle(&req, &backends).await;

    assert!(matches!(
        result,
        Err(sz_rust_api_gateway::GatewayError::BackendUnavailable(_))
    ));
}

/// 场景 6：限流触发 → 429
#[tokio::test]
async fn test_rate_limit_triggers() {
    let rl = RateLimitMiddleware::new(RateLimitConfig {
        enabled: true,
        requests_per_second: 1,
        burst: 2,
    });

    assert!(rl.check("client").is_ok());
    assert!(rl.check("client").is_ok());
    assert!(matches!(
        rl.check("client"),
        Err(sz_rust_api_gateway::GatewayError::RateLimited(_))
    ));
}

/// 场景 7：熔断触发 → 503
#[tokio::test]
async fn test_circuit_breaker_triggers() {
    let cb = CircuitBreakerMiddleware::new(CircuitBreakerConfig {
        enabled: true,
        failure_threshold: 3,
        reset_timeout_secs: 60,
    });

    cb.record_failure();
    cb.record_failure();
    cb.record_failure();

    assert_eq!(cb.state(), CircuitState::Open);
    assert!(matches!(
        cb.can_request(),
        Err(sz_rust_api_gateway::GatewayError::CircuitBroken(_))
    ));
}

/// 场景 8：路由匹配特异性
#[tokio::test]
async fn test_route_specificity() {
    let routes = vec![
        GatewayRoute::simple("wildcard", "/api/*", "svc1"),
        GatewayRoute::simple("exact", "/api/users", "svc2"),
    ];
    let engine = RouterEngine::new(routes);

    let req = MatchRequest::new("/api/users");
    let route = engine.match_route(&req).unwrap();
    assert_eq!(route.backend_service, "svc2", "exact match should win");

    let req = MatchRequest::new("/api/orders");
    let route = engine.match_route(&req).unwrap();
    assert_eq!(route.backend_service, "svc1", "wildcard should match");
}

/// 场景 9：路径重写
#[tokio::test]
async fn test_path_rewrite() {
    let route = GatewayRoute::simple("r1", "/api/*", "svc").with_rewrite("/v2/*");
    let rewritten = RouterEngine::rewrite_path(&route, "/api/users/123");
    assert_eq!(rewritten, "/v2/users/123");
}

/// 场景 10：配置从 JSON 加载
#[tokio::test]
async fn test_config_from_json() {
    let json = r#"{
        "routes": [
            {
                "route_id": "r1",
                "match_path": "/api/*",
                "match_host": null,
                "match_headers": {},
                "backend_service": "backend",
                "protocol": "http",
                "path_rewrite": null,
                "inject_headers": {}
            }
        ],
        "auth": {"enabled": true, "token_header": "Authorization", "public_paths": []},
        "rate_limit": {"enabled": false, "requests_per_second": 0, "burst": 0},
        "circuit_breaker": {"enabled": false, "failure_threshold": 0, "reset_timeout_secs": 0}
    }"#;
    let config = GatewayConfig::from_json(json).unwrap();
    assert_eq!(config.routes.len(), 1);
    assert_eq!(config.routes[0].backend_service, "backend");
}

/// 场景 11：多后端 LB 选择
#[tokio::test]
async fn test_lb_selects_from_multiple_backends() {
    let instances = vec![
        BackendInstance::new("10.0.0.1", 8080),
        BackendInstance::new("10.0.0.2", 8080),
        BackendInstance::new("10.0.0.3", 8080),
    ];
    let selected = sz_rust_api_gateway::RequestForwarder::select_backend(&instances).unwrap();
    assert_eq!(
        selected.host, "10.0.0.1",
        "should select first instance (round robin starts at 0)"
    );
}

/// 场景 12：完整链路 - 鉴权通过 + 限流通过 + 熔断 Closed
#[tokio::test]
async fn test_full_chain_auth_rate_limit_circuit() {
    let routes = vec![GatewayRoute::simple("r1", "/api/*", "svc")];
    let handler = GatewayHandler::new(
        RouterEngine::new(routes),
        AuthMiddleware::new(AuthConfig::default()),
        RateLimitMiddleware::new(RateLimitConfig {
            enabled: true,
            requests_per_second: 100,
            burst: 10,
        }),
        CircuitBreakerMiddleware::new(CircuitBreakerConfig::default()),
    );

    let backends = HashMap::from([(
        "svc".to_string(),
        vec![BackendInstance::new("10.0.0.1", 8080)],
    )]);

    let req = ForwardRequest::get("/api/test").with_header("Authorization", "Bearer token123");
    let result = handler.handle(&req, &backends).await;

    // 鉴权通过、限流通过、熔断 Closed，但后端不可达
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, sz_rust_api_gateway::GatewayError::Forward(_)));
}
