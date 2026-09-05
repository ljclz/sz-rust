// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Hello World 端点集成测试
//!
//! 使用 `tower::ServiceExt::oneshot` 直接向 router 发送请求，验证：
//! - `GET /` 返回 HTTP 200 + `{ "code": 1, "msg": "hello", "data": {} }`
//! - `GET /health` 返回 HTTP 200 + `{ "code": 1, "msg": "ok", "data": { "status": "healthy", ... } }`
//! - `GET /nonexistent` 返回 HTTP 404

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use sz_rust_examples::build_router;
use tower::ServiceExt;

/// 解析响应 body 为 String
async fn body_to_string(body: Body) -> String {
    let bytes = body.collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// GET / 应返回 HTTP 200 + 标准 JSON 响应
#[tokio::test]
async fn test_hello_endpoint() {
    let router = build_router();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(json["code"], 1);
    assert_eq!(json["msg"], "hello");
    assert_eq!(json["data"], serde_json::json!({}));
}

/// GET /health 应返回 HTTP 200 + 健康状态 JSON
#[tokio::test]
async fn test_health_endpoint() {
    let router = build_router();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_to_string(response.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(json["code"], 1);
    assert_eq!(json["msg"], "ok");
    assert_eq!(json["data"]["status"], "healthy");
    assert!(json["data"]["version"].is_string());
}

/// GET /nonexistent 应返回 HTTP 404
#[tokio::test]
async fn test_not_found_endpoint() {
    let router = build_router();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/nonexistent")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Content-Type 应为 application/json
#[tokio::test]
async fn test_content_type_is_json() {
    let router = build_router();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();

    let content_type = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap())
        .unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "Content-Type 应为 application/json，实际: {}",
        content_type
    );
}

/// 响应 body 应为精确的 JSON 字符串
#[tokio::test]
async fn test_response_body_exact_match() {
    let router = build_router();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    let body = body_to_string(response.into_body()).await;

    // 验证 body 包含必要字段（顺序可能不同，所以用 contains）
    assert!(body.contains("\"code\":1"));
    assert!(body.contains("\"msg\":\"hello\""));
    assert!(body.contains("\"data\":{}"));
}

/// POST / 应返回 HTTP 405 Method Not Allowed
#[tokio::test]
async fn test_post_method_not_allowed() {
    let router = build_router();
    let request = Request::builder()
        .method(Method::POST)
        .uri("/")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}
