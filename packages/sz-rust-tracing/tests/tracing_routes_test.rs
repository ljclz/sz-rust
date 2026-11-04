// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use sz_rust_core::router::RouterBuilder;
use sz_rust_tracing::{register_routes, TracingState};
use tower::ServiceExt;

#[tokio::test]
async fn test_register_routes_health_endpoint() {
    let builder = RouterBuilder::new();
    let state = TracingState::default();
    let router = register_routes(builder, state).build();

    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/api/tracing/health")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 1);
    assert_eq!(json["data"]["plugin"], "tracing");
    assert_eq!(json["data"]["status"], "active");
}

#[tokio::test]
async fn test_register_routes_list_spans_endpoint() {
    let builder = RouterBuilder::new();
    let state = TracingState::default();
    let router = register_routes(builder, state).build();

    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/api/tracing/spans")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 1);
    assert_eq!(json["data"]["total"], 0);
}

#[tokio::test]
async fn test_register_routes_create_span_endpoint() {
    let builder = RouterBuilder::new();
    let state = TracingState::default();
    let router = register_routes(builder, state).build();

    let req_body = serde_json::json!({
        "operation_name": "test_op",
        "service_name": "test_svc",
        "tags": {"env": "test"}
    });

    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/tracing/spans")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(req_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 1);
    assert_eq!(json["data"]["operation_name"], "test_op");
    assert_eq!(json["data"]["service_name"], "test_svc");
    assert!(!json["data"]["trace_id"].as_str().unwrap().is_empty());
    assert!(!json["data"]["span_id"].as_str().unwrap().is_empty());
}
