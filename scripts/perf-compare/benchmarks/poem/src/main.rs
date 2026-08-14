use poem::{
    get, handler, listener::TcpListener,
    web::{Path, Json, Data},
    EndpointExt, Route, Server,
};
use redis::aio::MultiplexedConnection;
use serde_json::{json, Value};

#[handler]
fn simple() -> &'static str { "Hello, World!" }

#[handler]
fn json_resp() -> Json<Value> { Json(json!({"message": "Hello, World!"})) }

#[handler]
fn user(Path(id): Path<u64>) -> Json<Value> { Json(json!({"user_id": id})) }

#[handler]
async fn db(Data(conn): Data<&MultiplexedConnection>) -> String {
    use redis::AsyncCommands;
    let mut conn = conn.clone();
    let val: String = conn.get("counter").await.unwrap_or_default();
    val
}

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    let port: u16 = std::env::var("PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(8405);

    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let client = redis::Client::open(redis_url.as_str()).unwrap();
    let conn = client.get_multiplexed_async_connection().await.unwrap();

    let app = Route::new()
        .at("/simple", get(simple))
        .at("/json", get(json_resp))
        .at("/user/:id", get(user))
        .at("/db", get(db))
        .data(conn);

    Server::new(TcpListener::bind(format!("0.0.0.0:{}", port)))
        .run(app)
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
}
