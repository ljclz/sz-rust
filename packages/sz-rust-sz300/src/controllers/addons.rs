use axum::response::Json;
use serde_json::{json, Value};

/// Addon 状态查询 — 列出所有已链接的 addon crate 及其路由/类型
pub async fn status() -> Json<Value> {
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": {
            "addons": [
                { "name": "ecommerce", "version": env!("CARGO_PKG_VERSION"), "routes": "/api/ecommerce/*", "status": "active" },
                { "name": "erp", "version": env!("CARGO_PKG_VERSION"), "routes": "/api/erp/*", "status": "active" },
                { "name": "forum", "version": env!("CARGO_PKG_VERSION"), "routes": "/api/forum/*", "status": "active" },
                { "name": "im", "version": env!("CARGO_PKG_VERSION"), "routes": "/api/im/*", "status": "active" },
                { "name": "operate", "version": env!("CARGO_PKG_VERSION"), "routes": "/api/operate/*", "status": "active" },
                { "name": "workflow", "version": env!("CARGO_PKG_VERSION"), "routes": "/api/workflow/*", "status": "active" },
                { "name": "tracing", "version": env!("CARGO_PKG_VERSION"), "routes": "/api/tracing/*", "status": "active" },
                { "name": "pdf", "version": env!("CARGO_PKG_VERSION"), "routes": "/api/pdf/*", "status": "active" }
            ]
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn addons_status_lists_all_seven_crates() {
        let router = Router::new().route("/api/addons/status", get(status));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/addons/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let addons = json["data"]["addons"].as_array().unwrap();
        assert_eq!(addons.len(), 8);
        let names: Vec<&str> = addons.iter().map(|a| a["name"].as_str().unwrap()).collect();
        for expected in &[
            "ecommerce",
            "erp",
            "forum",
            "im",
            "operate",
            "workflow",
            "tracing",
            "pdf",
        ] {
            assert!(names.contains(expected), "缺少 addon: {}", expected);
        }
    }
}
