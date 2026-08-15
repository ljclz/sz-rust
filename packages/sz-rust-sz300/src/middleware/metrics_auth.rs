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
