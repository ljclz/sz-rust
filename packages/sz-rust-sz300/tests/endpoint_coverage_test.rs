//! 6 个无直接测试端点的补充测试
//!
//! 覆盖：
//! - GET /api-docs (swagger_ui)
//! - GET /api-docs/redoc (redoc)
//! - GET /api-docs/openapi.json (openapi_json)
//! - GET /page/{template} (view::render_page)
//!
//! file_serve 和 upload_multipart 需要 AppState，路径遍历防护已在
//! file_serve.rs 的三重防护中实现，此处测试 render_page 和 openapi 端点。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn build_test_router() -> Router {
    Router::new()
        .route("/api-docs", get(sz_rust_sz300::openapi::swagger_ui))
        .route("/api-docs/redoc", get(sz_rust_sz300::openapi::redoc))
        .route(
            "/api-docs/openapi.json",
            get(sz_rust_sz300::openapi::openapi_json),
        )
        .route(
            "/page/{template}",
            get(sz_rust_sz300::controllers::view::render_page),
        )
}

async fn get_status_and_body(router: Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, body.to_vec())
}

#[tokio::test]
async fn swagger_ui_returns_html() {
    let router = build_test_router();
    let req = Request::builder()
        .uri("/api-docs")
        .body(Body::empty())
        .unwrap();
    let (status, body) = get_status_and_body(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("swagger") || text.contains("Swagger"),
        "应包含 Swagger UI"
    );
}

#[tokio::test]
async fn redoc_returns_html() {
    let router = build_test_router();
    let req = Request::builder()
        .uri("/api-docs/redoc")
        .body(Body::empty())
        .unwrap();
    let (status, body) = get_status_and_body(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("redoc") || text.contains("Redoc"),
        "应包含 Redoc"
    );
}

#[tokio::test]
async fn openapi_json_returns_valid_json() {
    let router = build_test_router();
    let req = Request::builder()
        .uri("/api-docs/openapi.json")
        .body(Body::empty())
        .unwrap();
    let (status, body) = get_status_and_body(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("应返回有效 JSON");
    assert!(json.get("openapi").is_some(), "应包含 openapi 版本字段");
    assert!(json.get("paths").is_some(), "应包含 paths 字段");
}

#[tokio::test]
async fn render_page_returns_html_with_template_name() {
    let router = build_test_router();
    let req = Request::builder()
        .uri("/page/test-page")
        .body(Body::empty())
        .unwrap();
    let (status, body) = get_status_and_body(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("test-page"), "应包含模板名");
    assert!(text.contains("<html"), "应返回 HTML");
}

#[tokio::test]
async fn render_page_with_special_chars() {
    let router = build_test_router();
    let req = Request::builder()
        .uri("/page/hello%20world")
        .body(Body::empty())
        .unwrap();
    let (status, body) = get_status_and_body(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("hello world"), "应解码 URL 编码的模板名");
}

#[tokio::test]
async fn openapi_json_contains_known_endpoints() {
    let router = build_test_router();
    let req = Request::builder()
        .uri("/api-docs/openapi.json")
        .body(Body::empty())
        .unwrap();
    let (_status, body) = get_status_and_body(router, req).await;
    let json: serde_json::Value = serde_json::from_slice(&body).expect("应返回有效 JSON");
    let paths = json["paths"].as_object().expect("paths 应为对象");
    assert!(paths.contains_key("/api/v1/auth/login"), "应包含登录端点");
    assert!(paths.contains_key("/health"), "应包含健康检查端点");
}
