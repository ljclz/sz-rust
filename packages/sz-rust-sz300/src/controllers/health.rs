use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde_json::{json, Value};

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
pub async fn readiness(State(state): State<AppState>) -> impl IntoResponse {
    let db_ok = match state.db_pool.acquire().await {
        Ok(mut conn) => conn.query("SELECT 1").await.is_ok(),
        Err(_) => false,
    };

    if db_ok {
        (
            StatusCode::OK,
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": { "status": "ready" }
            })),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "code": 0,
                "msg": "not ready",
                "data": { "status": "unavailable" }
            })),
        )
    }
}
