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

/// 就绪检查（readiness probe）：通过执行 SELECT 1 验证数据库连接是否正常
///
/// 响应体包含 DB 检查细节，便于运维诊断：
/// ```json
/// { "code": 1, "msg": "success", "data": { "status": "ready", "checks": { "db": "ok" } } }
/// ```
#[tracing::instrument(skip(state))]
pub async fn readiness(State(state): State<AppState>) -> impl IntoResponse {
    let (db_ok, db_error) = match state.db_pool.acquire().await {
        Ok(mut conn) => match conn.query("SELECT 1").await {
            Ok(_) => (true, None),
            Err(e) => (false, Some(format!("{}", e))),
        },
        Err(e) => (false, Some(format!("pool acquire failed: {}", e))),
    };

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
                    "checks": { "db": "fail" },
                    "error": db_error
                }
            })),
        )
    }
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
