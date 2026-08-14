use axum::{extract::{Path, State}, response::Json, routing::get, Router};
use redis::aio::MultiplexedConnection;
use serde_json::json;
use std::net::SocketAddr;

async fn db(State(conn): State<MultiplexedConnection>) -> String {
    use redis::AsyncCommands;
    let mut conn = conn;
    let val: String = conn.get("counter").await.unwrap_or_default();
    val
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8401);

    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let client = redis::Client::open(redis_url.as_str()).unwrap();
    let conn = client.get_multiplexed_async_connection().await.unwrap();

    let app = Router::new()
        .route("/simple", get(|| async { "Hello, World!" }))
        .route(
            "/json",
            get(|| async { Json(json!({"message": "Hello, World!"})) }),
        )
        .route(
            "/user/{id}",
            get(|Path(id): Path<u64>| async move { Json(json!({"user_id": id})) }),
        )
        .route("/db", get(db))
        .with_state(conn);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
