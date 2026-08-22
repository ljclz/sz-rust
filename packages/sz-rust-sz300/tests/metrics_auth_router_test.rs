//! metrics 鉴权接线集成测试（T7 MetricsAuthConfig → router）
//!
//! 验证 /metrics 路由上的 metrics_auth_middleware 完整链路：
//! Bearer token / IP 白名单（CIDR）/ enabled=false 显式关闭。
//! 不依赖 DB：用微型 Router 复刻生产接线方式（route + route_layer）。
//!
//! 客户端 IP 通过自定义 `ClientIp` 扩展类型注入请求 extensions，
//! 对应生产环境中由 `into_make_service_with_connect_info` + 转换层写入。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{middleware, Router};
use sz_rust_sz300::config::MetricsAuthConfig;
use sz_rust_sz300::middleware::metrics_auth::{metrics_auth_middleware, ClientIp};
use tower::ServiceExt;

/// 复刻生产接线：/metrics 路由 + route_layer(metrics_auth_middleware)
fn test_router(cfg: MetricsAuthConfig) -> Router {
    Router::new()
        .route("/metrics", get(|| async { "metrics-ok" }))
        .route_layer(middleware::from_fn_with_state(cfg, metrics_auth_middleware))
}

/// 构建带 ClientIp extension 的请求（模拟生产 into_make_service_with_connect_info 注入）
fn req_with_ip(path: &str, ip: &str) -> Request<Body> {
    let mut req = Request::builder().uri(path).body(Body::empty()).unwrap();
    req.extensions_mut().insert(ClientIp(ip.to_string()));
    req
}

fn req_with_ip_and_bearer(path: &str, ip: &str, token: &str) -> Request<Body> {
    let mut req = Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ClientIp(ip.to_string()));
    req
}

async fn get_status(app: Router, req: Request<Body>) -> StatusCode {
    app.oneshot(req).await.unwrap().status()
}

/// 默认配置（enabled=true，无 token 无白名单）→ 拒绝匿名访问（fail-closed）
#[tokio::test]
async fn metrics_requires_auth_by_default() {
    let status = get_status(
        test_router(MetricsAuthConfig::default()),
        req_with_ip("/metrics", "127.0.0.1"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// 配置 Bearer token 后，正确 token 放行
#[tokio::test]
async fn metrics_allows_correct_bearer_token() {
    let cfg = MetricsAuthConfig {
        bearer_token: Some("secret".to_string()),
        ..Default::default()
    };
    let status = get_status(
        test_router(cfg),
        req_with_ip_and_bearer("/metrics", "127.0.0.1", "secret"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// 错误 token 拒绝
#[tokio::test]
async fn metrics_rejects_wrong_bearer_token() {
    let cfg = MetricsAuthConfig {
        bearer_token: Some("secret".to_string()),
        ..Default::default()
    };
    let status = get_status(
        test_router(cfg),
        req_with_ip_and_bearer("/metrics", "127.0.0.1", "wrong"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// 未带 token 拒绝
#[tokio::test]
async fn metrics_rejects_missing_bearer_token() {
    let cfg = MetricsAuthConfig {
        bearer_token: Some("secret".to_string()),
        ..Default::default()
    };
    let status = get_status(test_router(cfg), req_with_ip("/metrics", "127.0.0.1")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// enabled=false 显式关闭 → 匿名放行
#[tokio::test]
async fn metrics_disabled_allows_anonymous() {
    let cfg = MetricsAuthConfig {
        enabled: false,
        ..Default::default()
    };
    let status = get_status(test_router(cfg), req_with_ip("/metrics", "127.0.0.1")).await;
    assert_eq!(status, StatusCode::OK);
}

/// IP 白名单 CIDR 匹配放行
#[tokio::test]
async fn metrics_ip_whitelist_cidr_allows() {
    let cfg = MetricsAuthConfig {
        allowed_ips: vec!["10.0.0.0/8".to_string()],
        ..Default::default()
    };
    let status = get_status(test_router(cfg), req_with_ip("/metrics", "10.1.2.3")).await;
    assert_eq!(status, StatusCode::OK);
}

/// IP 白名单拒绝非白名单 IP（fail-closed）
#[tokio::test]
async fn metrics_ip_whitelist_rejects_other_ip() {
    let cfg = MetricsAuthConfig {
        allowed_ips: vec!["10.0.0.0/8".to_string()],
        ..Default::default()
    };
    let status = get_status(test_router(cfg), req_with_ip("/metrics", "192.168.1.1")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
