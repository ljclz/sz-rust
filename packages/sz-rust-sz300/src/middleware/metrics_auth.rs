//! metrics 端点访问控制中间件（T7 MetricsAuthConfig 接线）

use crate::config::MetricsAuthConfig;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// 客户端 IP 扩展类型。
///
/// 生产环境由 `into_make_service_with_connect_info` 注入 `axum::extract::connect_info::ConnectInfo`，
/// 本中间件通过 `axum::serve` 的 `into_make_service_with_connect_info` 变体将 IP 提取后
/// 以本类型写入 extensions；测试通过 `request.extensions_mut().insert(ClientIp(...))` 注入。
///
/// 使用自定义类型而非 `ConnectInfo<SocketAddr>` 的原因：
/// 依赖图中存在 axum 0.7（tonic 传递依赖）和 axum 0.8 两个版本，
/// `Extensions::get` 基于 TypeId 匹配，跨版本 TypeId 不同会导致匹配失败。
#[derive(Clone, Debug)]
pub struct ClientIp(pub String);

/// metrics 端点访问控制中间件
///
/// 校验顺序：`enabled=false` 直接放行 → Bearer token 匹配 → IP 白名单匹配 → 拒绝（403）。
/// 客户端 IP 通过请求 extensions 中的 [`ClientIp`] 获取；未注入时 fail-closed（拒绝）。
///
/// 与业务 JWT 解耦：/metrics 在公开路径白名单中跳过业务鉴权，由本中间件独立控制，
/// 避免 Prometheus 抓取方必须持有业务 Token。
pub async fn metrics_auth_middleware(
    State(config): State<MetricsAuthConfig>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    if !config.enabled {
        return next.run(request).await;
    }

    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let client_ip = request.extensions().get::<ClientIp>().map(|c| c.0.as_str());

    if config.is_allowed(bearer, client_ip) {
        next.run(request).await
    } else {
        (StatusCode::FORBIDDEN, "Forbidden").into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MetricsAuthConfig;
    use axum::body::Body;
    use axum::http::{Request, StatusCode as AxumStatusCode};
    use axum::middleware;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    fn build_router(config: MetricsAuthConfig) -> Router {
        Router::new()
            .route("/test", get(|| async { AxumStatusCode::OK }))
            .layer(middleware::from_fn_with_state(
                config,
                metrics_auth_middleware,
            ))
    }

    #[test]
    fn client_ip_clone_and_debug() {
        let ip = ClientIp("192.168.1.1".to_string());
        let cloned = ip.clone();
        assert_eq!(ip.0, cloned.0);
        let debug_str = format!("{:?}", ip);
        assert!(debug_str.contains("192.168.1.1"));
    }

    #[tokio::test]
    async fn metrics_auth_middleware_disabled_passes_through() {
        let config = MetricsAuthConfig {
            enabled: false,
            ..MetricsAuthConfig::default()
        };
        let router = build_router(config);
        let response = router
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), AxumStatusCode::OK);
    }

    #[tokio::test]
    async fn metrics_auth_middleware_enabled_no_credentials_forbidden() {
        let config = MetricsAuthConfig {
            enabled: true,
            ..MetricsAuthConfig::default()
        };
        let router = build_router(config);
        let response = router
            .oneshot(Request::builder().uri("/test").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), AxumStatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn metrics_auth_middleware_enabled_with_valid_token_passes() {
        let config = MetricsAuthConfig {
            enabled: true,
            bearer_token: Some("test-token".to_string()),
            ..Default::default()
        };
        let router = build_router(config);
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), AxumStatusCode::OK);
    }

    #[tokio::test]
    async fn metrics_auth_middleware_enabled_with_ip_whitelist_passes() {
        let config = MetricsAuthConfig {
            enabled: true,
            allowed_ips: vec!["10.0.0.1".to_string()],
            ..Default::default()
        };
        let router = build_router(config);
        let request = Request::builder()
            .uri("/test")
            .extension(ClientIp("10.0.0.1".to_string()))
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), AxumStatusCode::OK);
    }

    #[tokio::test]
    async fn metrics_auth_middleware_enabled_wrong_token_forbidden() {
        let config = MetricsAuthConfig {
            enabled: true,
            bearer_token: Some("correct-token".to_string()),
            ..Default::default()
        };
        let router = build_router(config);
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("authorization", "Bearer wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), AxumStatusCode::FORBIDDEN);
    }
}
