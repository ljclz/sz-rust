//! SZ-Rust Examples — 示例库
//!
//! 提供 Hello World 端点的 router 构建函数，便于集成测试。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use axum::{routing::get, Json, Router};
use serde_json::{json, Value};
use sz_rust_core::error::ErrorCode;
use sz_rust_core::sql_string;
use tower_http::trace::TraceLayer;

/// Hello World 处理器
///
/// 返回标准 JSON 响应，对齐 PHP `renderJson($code=1, $msg='', $data=[])`：
/// ```json
/// { "code": 1, "msg": "hello", "data": {} }
/// ```
pub async fn hello() -> Json<Value> {
    // 验证编译时 SQL 校验宏可用（Phase 0.8）
    let _sql = sql_string!("SELECT 1 FROM dual");

    // 标准响应：对齐 PHP renderJson(code=1, msg='hello', data=[])
    Json(json!({
        "code": ErrorCode::Success as i32,
        "msg": "hello",
        "data": {},
    }))
}

/// 健康检查处理器
pub async fn health() -> Json<Value> {
    Json(json!({
        "code": ErrorCode::Success as i32,
        "msg": "ok",
        "data": {
            "status": "healthy",
            "version": env!("CARGO_PKG_VERSION"),
        },
    }))
}

/// 构建示例 Router
pub fn build_router() -> Router {
    Router::new()
        .route("/", get(hello))
        .route("/health", get(health))
        .layer(TraceLayer::new_for_http())
}
