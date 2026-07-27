use crate::services::health_service;
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde_json::{json, Value};

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
    let body = state.metrics_registry.render();
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        body,
    )
}
