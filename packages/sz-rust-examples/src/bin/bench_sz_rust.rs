//! P2-6 横向对比 benchmark：sz-rust 框架服务器
//!
//! 使用 sz-rust 的路由 + 中间件 + JSON 响应，对比框架开销。
//!
//! 运行：`cargo run --release --bin bench_sz_rust -- 127.0.0.1:8080`

use axum::{routing::get, Json, Router};
use serde_json::{json, Value};
use sz_rust_core::error::ErrorCode;
use tower_http::trace::TraceLayer;

async fn json_endpoint() -> Json<Value> {
    Json(json!({
        "code": ErrorCode::Success as i32,
        "msg": "ok",
        "data": {
            "id": 1,
            "name": "benchmark",
            "value": 42
        }
    }))
}

async fn plaintext_endpoint() -> &'static str {
    "Hello, World!"
}

async fn user_endpoint() -> Json<Value> {
    Json(json!({
        "code": ErrorCode::Success as i32,
        "msg": "ok",
        "data": {
            "id": 1,
            "name": "user_1",
            "email": "user1@example.com",
            "status": 1
        }
    }))
}

async fn list_endpoint() -> Json<Value> {
    let items: Vec<Value> = (1..=20)
        .map(|i| {
            json!({
                "id": i,
                "name": format!("item_{}", i),
                "value": i * 10
            })
        })
        .collect();

    Json(json!({
        "code": ErrorCode::Success as i32,
        "msg": "ok",
        "data": {
            "list": items,
            "total": 20,
            "page": 1,
            "page_size": 20
        }
    }))
}

fn build_router() -> Router {
    Router::new()
        .route("/json", get(json_endpoint))
        .route("/plaintext", get(plaintext_endpoint))
        .route("/user", get(user_endpoint))
        .route("/list", get(list_endpoint))
        .layer(TraceLayer::new_for_http())
}

#[tokio::main]
async fn main() {
    let addr: std::net::SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".into())
        .parse()
        .expect("invalid address");

    let router = build_router();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("绑定监听地址失败");
    println!("sz-rust benchmark server on {}", addr);
    axum::serve(listener, router)
        .await
        .expect("HTTP 服务启动失败");
}
