use actix_web::{web, App, HttpResponse, HttpServer};
use redis::aio::MultiplexedConnection;
use serde_json::json;

async fn simple() -> &'static str {
    "Hello, World!"
}
async fn json_resp() -> HttpResponse {
    HttpResponse::Ok().json(json!({"message": "Hello, World!"}))
}
async fn user(path: web::Path<u64>) -> HttpResponse {
    HttpResponse::Ok().json(json!({"user_id": path.into_inner()}))
}
async fn db(conn: web::Data<MultiplexedConnection>) -> String {
    use redis::AsyncCommands;
    let mut conn = conn.get_ref().clone();
    let val: String = conn.get("counter").await.unwrap_or_default();
    val
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8402);

    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let client = redis::Client::open(redis_url.as_str()).expect("Redis 连接失败");
    let conn = client.get_multiplexed_async_connection().await.expect("Redis 连接池建立失败");

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(conn.clone()))
            .route("/simple", web::get().to(simple))
            .route("/json", web::get().to(json_resp))
            .route("/user/{id}", web::get().to(user))
            .route("/db", web::get().to(db))
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
