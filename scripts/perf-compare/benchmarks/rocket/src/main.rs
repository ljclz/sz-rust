use rocket::{get, launch, routes, serde::json::Json};
use serde_json::json;

#[get("/simple")]
fn simple() -> &'static str { "Hello, World!" }

#[get("/json")]
fn json_resp() -> Json<serde_json::Value> { Json(json!({"message": "Hello, World!"})) }

#[get("/user/<id>")]
fn user(id: u64) -> Json<serde_json::Value> { Json(json!({"user_id": id})) }

#[launch]
fn rocket() -> _ {
    let port: u16 = std::env::var("PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(8404);
    rocket::build()
        .configure(rocket::Config { port, ..rocket::Config::default() })
        .mount("/", routes![simple, json_resp, user])
}
