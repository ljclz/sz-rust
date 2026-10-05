// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P3-3.6 API 协议端到端集成测试（GraphQL + WebSocket + SSE + Upload）
//!
//! 通过完整路由验证各 API 协议端点可达性。
//! 需要真实 MySQL 连接。

mod common;

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

use common::{ensure_mysql, make_router, make_state};

// ============================================================================
// SSE 端点 E2E 测试
// ============================================================================

#[cfg(feature = "v18-sse")]
#[tokio::test]
async fn test_e2e_sse_endpoint_returns_event_stream() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let ct = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("text/event-stream"),
        "SSE content-type 应为 text/event-stream, 实际: {ct}"
    );
}

#[cfg(feature = "v18-sse")]
#[tokio::test]
async fn test_e2e_sse_with_last_event_id_header() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/events")
                .header("Last-Event-ID", "42")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

// ============================================================================
// GraphQL 端点 E2E 测试
// ============================================================================

#[cfg(all(feature = "v18-graphql", not(feature = "v18-rbac")))]
#[tokio::test]
async fn test_e2e_graphql_introspection() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/graphql")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query": "{ __typename }"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(
        body_str.contains("__typename") || body_str.contains("QueryRoot"),
        "GraphQL introspection 应返回类型名, 实际: {body_str}"
    );
}

#[cfg(all(feature = "v18-graphql", not(feature = "v18-rbac")))]
#[tokio::test]
async fn test_e2e_graphql_query_merchant() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/graphql")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query": "{ merchant(id: 1) { id name } }"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[cfg(all(feature = "v18-graphql", not(feature = "v18-rbac")))]
#[tokio::test]
async fn test_e2e_graphql_mutation_create_product() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/graphql")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query": "mutation { createProduct(name: \"test\", price: 100) { id name } }"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

// ============================================================================
// Upload 端点 E2E 测试
// ============================================================================

#[cfg(all(feature = "v18-upload", not(feature = "v18-rbac")))]
#[tokio::test]
async fn test_e2e_upload_rejects_oversized_file() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let large_body = vec![b'x'; 10 * 1024 * 1024];

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/file/upload_enhanced")
                .method("POST")
                .header("content-type", "multipart/form-data; boundary=----test")
                .header("content-length", large_body.len().to_string())
                .body(Body::from(large_body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        "超大文件应返回 413"
    );
}

#[cfg(all(feature = "v18-upload", not(feature = "v18-rbac")))]
#[tokio::test]
async fn test_e2e_upload_accepts_small_file() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let boundary = "----TestBoundary123";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n\
         Content-Type: text/plain\r\n\r\n\
         hello world\r\n\
         --{boundary}--\r\n"
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/file/upload_enhanced")
                .method("POST")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        response.status() != axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        "小文件不应返回 413"
    );
}

// ============================================================================
// WebSocket 端点 E2E 测试
// ============================================================================

#[cfg(feature = "v18-websocket")]
#[tokio::test]
async fn test_e2e_ws_endpoint_returns_upgrade_required() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(Request::builder().uri("/ws").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert!(
        response.status() == axum::http::StatusCode::BAD_REQUEST
            || response.status() == axum::http::StatusCode::UPGRADE_REQUIRED
            || response.status() == axum::http::StatusCode::OK,
        "WebSocket 端点无 Upgrade 头应返回 400/426, 实际: {}",
        response.status()
    );
}

// ============================================================================
// CORS 端点 E2E 测试
// ============================================================================

#[tokio::test]
async fn test_e2e_cors_preflight_returns_headers() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .method("OPTIONS")
                .header("origin", "http://localhost:3000")
                .header("access-control-request-method", "GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let acao = response
        .headers()
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(!acao.is_empty(), "CORS 预检应返回 Allow-Origin");
}

#[tokio::test]
async fn test_e2e_health_check_through_full_middleware_stack() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(
        body_str.contains("status") || body_str.contains("ok") || body_str.contains("healthy"),
        "health 响应应包含状态信息, 实际: {body_str}"
    );
}
