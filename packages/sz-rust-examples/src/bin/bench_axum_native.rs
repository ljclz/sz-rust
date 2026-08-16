//! P2-6 横向对比 benchmark：纯 axum 对照服务器
//!
//! 实现与 bench_sz_rust 相同的端点，用于对比 sz-rust 框架开销。
//!
//! 运行：`cargo run --release --bin bench_axum_native -- 127.0.0.1:8081`

use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

async fn json_endpoint() -> Json<Value> {
    Json(json!({
        "code": 1,
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
        "code": 1,
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
        "code": 1,
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
}

#[tokio::main]
async fn main() {
    let addr: std::net::SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8081".into())
        .parse()
        .expect("invalid address");

    let router = build_router();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("绑定监听地址失败: {addr}: {e}"));
    println!("axum native benchmark server on {}", addr);
    axum::serve(listener, router)
        .await
        .unwrap_or_else(|e| panic!("HTTP 服务启动失败: {e}"));
}
