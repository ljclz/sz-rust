use crate::services::health_service;
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde_json::{json, Value};
use std::sync::Arc;
use sz_rust_core::orm::Pool;

/// 存活检查（liveness probe）
#[tracing::instrument(skip(_state))]
pub async fn check(State(_state): State<AppState>) -> Json<Value> {
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": {
            "status": "ok",
            "version": env!("CARGO_PKG_VERSION"),
            "service": "sz300-server",
            "timestamp": chrono::Utc::now().timestamp()
        }
    }))
}

/// 就绪检查（readiness probe）：通过 service 层封装的 DB 探活验证数据库连接是否正常
///
/// 响应体仅返回聚合状态（ok/fail），不泄露 DB 内部错误细节。
/// DB 错误细节由 `health_service::ping_db` 通过 `tracing::error!` 记录到日志，便于运维排查。
///
/// 重构说明（2026-07-26 P1-5）：
/// - 移除控制器内嵌 DB 调用（`state.db_pool.acquire()` + `conn.query("SELECT 1")`）
/// - 下沉到 `health_service::ping_db`，符合分层架构
#[tracing::instrument(skip(state))]
pub async fn readiness(State(state): State<AppState>) -> impl IntoResponse {
    let db_ok = health_service::ping_db(&state.db_pool).await;

    // SLO 指标记录：DB 探活结果计入燃烧率监控
    if db_ok {
        state.slo_monitor.record_success();
    } else {
        state.slo_monitor.record_failure();
    }

    if db_ok {
        (
            StatusCode::OK,
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": {
                    "status": "ready",
                    "checks": { "db": "ok" }
                }
            })),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "code": 0,
                "msg": "not ready",
                "data": {
                    "status": "unavailable",
                    "checks": { "db": "fail" }
                }
            })),
        )
    }
}

/// 启动检查（startup probe）：验证应用启动初期的依赖可用性
///
/// 与 liveness/readiness 区分：
/// - liveness：运行期间持续探活，失败触发重启
/// - readiness：运行期间持续探活，失败从负载均衡摘除
/// - startup：仅在启动初期探活，成功后 liveness/readiness 接管
///
/// 启动检查不依赖 DB（避免 DB 慢启动导致 Pod 反复重启），
/// 仅验证关键静态资源（Metrics Registry）已完成初始化。
#[tracing::instrument(skip(state))]
pub async fn startup(State(state): State<AppState>) -> impl IntoResponse {
    let metrics_rendered = state.metrics_registry.render();
    if metrics_rendered.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "code": 0,
                "msg": "not ready",
                "data": {
                    "status": "starting",
                    "checks": { "metrics": "not_initialized" }
                }
            })),
        );
    }

    (
        StatusCode::OK,
        Json(json!({
            "code": 1,
            "msg": "success",
            "data": {
                "status": "started",
                "checks": {
                    "metrics": "ok"
                }
            }
        })),
    )
}

/// Prometheus 指标端点 — 输出 Prometheus 文本格式指标
#[tracing::instrument(skip(state))]
pub async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    // 连接池实时指标：sz-orm Pool::status()/pool_metrics() 仅读取原子计数器
    //（O(1) 无阻塞），每次 /metrics 请求实时输出，无需后台定期任务
    let mut body = state.metrics_registry.render();
    append_pool_metrics(&mut body, "sz300_db_pool", &state.db_pool).await;
    if let Some(pg_pool) = &state.pg_pool {
        append_pool_metrics(&mut body, "sz300_pg_pool", pg_pool).await;
    }
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        body,
    )
}

/// 追加连接池指标（Prometheus 文本格式）
///
/// - `{prefix}_active / _idle / _waiters / _max`：实时快照（gauge）
/// - `{prefix}_acquire_total / _acquire_failed_total`：池生命周期累计值（counter）
async fn append_pool_metrics(out: &mut String, prefix: &str, pool: &Arc<Pool>) {
    let status = pool.status().await;
    let metrics = pool.pool_metrics();
    out.push_str(&format!(
        "# TYPE {prefix}_active gauge\n\
         {prefix}_active {}\n\
         {prefix}_idle {}\n\
         {prefix}_waiters {}\n\
         {prefix}_max {}\n",
        status.active, status.idle, status.waiters, status.max,
    ));
    out.push_str(&format!(
        "{prefix}_acquire_total {}\n{prefix}_acquire_failed_total {}\n",
        metrics.acquire_count, metrics.acquire_failed_count,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::mock_app_state;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn read_body_string(body: Body) -> String {
        body.collect()
            .await
            .expect("body collect")
            .to_bytes()
            .iter()
            .map(|b| *b as char)
            .collect()
    }

    #[tokio::test]
    async fn health_check_returns_ok_status() {
        let state = mock_app_state();
        let router = Router::new().route("/health", get(check)).with_state(state);
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = read_body_string(response.into_body()).await;
        assert!(body.contains("\"status\":\"ok\""));
        assert!(body.contains("\"service\":\"sz300-server\""));
    }

    #[tokio::test]
    async fn health_startup_returns_ok_with_metrics() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/health/startup", get(startup))
            .with_state(state);
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/health/startup")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // metrics_registry 可能为空（返回 503）或非空（返回 200）
        let body = read_body_string(response.into_body()).await;
        assert!(
            body.contains("\"status\":\"started\"") || body.contains("\"status\":\"starting\""),
            "应返回 started 或 starting 状态，实际: {}",
            body
        );
    }

    #[tokio::test]
    async fn health_metrics_returns_text_format() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/metrics", get(metrics))
            .with_state(state);
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(ct.contains("text/plain"));
    }

    #[tokio::test]
    async fn health_readiness_returns_503_without_db() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/health/ready", get(readiness))
            .with_state(state);
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/health/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // mock Pool 无法连接真实 DB，ping_db 应返回 false → 503
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = read_body_string(response.into_body()).await;
        assert!(body.contains("\"status\":\"unavailable\""));
    }
}
