use axum::extract::State;
use axum::response::Json;
use serde_json::{json, Value};
use crate::state::AppState;

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
