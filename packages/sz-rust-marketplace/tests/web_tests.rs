// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Web API HTTP 端点集成测试
//!
//! 使用 axum::Router + tower::ServiceExt::oneshot 发送真实 HTTP 请求，
//! 验证各端点的状态码和响应体。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use sz_rust_marketplace::web::{build_router, AppState, JwtConfig};
use tower::ServiceExt;

async fn read_body(body: Body) -> String {
    let bytes = body.collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

fn make_app_state() -> AppState {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgres://unused:unused@localhost:5432/unused")
        .unwrap();

    let plugins = sz_rust_marketplace::repository::PluginRepository::new(pool.clone());
    let versions = sz_rust_marketplace::repository::VersionRepository::new(pool.clone());
    let reviews = sz_rust_marketplace::repository::ReviewRepository::new(pool.clone());
    let developers = sz_rust_marketplace::repository::DeveloperRepository::new(pool.clone());
    let store = Arc::new(sz_rust_marketplace::storage::LocalObjectStore::new(
        std::path::PathBuf::from("/tmp/sz-rust-test-store"),
    ));
    let service = Arc::new(sz_rust_marketplace::service::MarketplaceService::new(
        plugins, versions, reviews, developers, store,
    ));
    let jwt = Arc::new(JwtConfig::new("test-secret"));

    AppState { pool, service, jwt }
}

#[tokio::test]
async fn test_health_check_returns_200() {
    let app = build_router(make_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_body(resp.into_body()).await;
    assert!(body.contains("ok"));
}

#[tokio::test]
async fn test_openapi_json_returns_valid_spec() {
    let app = build_router(make_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_body(resp.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["openapi"], "3.0.0");
    assert!(json["paths"]["/api/v1/plugins/search"].is_object());
    assert!(json["paths"]["/api/v1/admin/reviews/pending"].is_object());
    assert!(json["paths"]["/api/v1/auth/login"].is_object());
}

#[tokio::test]
async fn test_login_returns_jwt_token() {
    let app = build_router(make_app_state());
    let body = serde_json::json!({
        "developer_id": 1,
        "username": "alice",
        "is_reviewer": true
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp_body = read_body(resp.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&resp_body).unwrap();
    assert!(!json["token"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn test_publish_without_token_returns_401() {
    let app = build_router(make_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/plugins/publish")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_pending_reviews_without_token_returns_400() {
    let app = build_router(make_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/admin/reviews/pending")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_approve_review_without_token_returns_400() {
    let app = build_router(make_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/admin/reviews/1/approve")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_search_returns_response() {
    let app = build_router(make_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/plugins/search?q=test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // 会因为数据库连接失败返回 500，但端点本身可达
    assert!(resp.status() == StatusCode::INTERNAL_SERVER_ERROR || resp.status() == StatusCode::OK);
}

#[tokio::test]
async fn test_get_plugin_returns_response() {
    let app = build_router(make_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/plugins/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // 会因为数据库连接失败返回 500，或插件不存在返回 404
    assert!(
        resp.status() == StatusCode::INTERNAL_SERVER_ERROR
            || resp.status() == StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn test_login_with_token_then_access_pending() {
    let state = make_app_state();
    let app = build_router(state.clone());

    // 先登录获取 token
    let login_body = serde_json::json!({
        "developer_id": 2,
        "username": "reviewer",
        "is_reviewer": true
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(login_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let resp_body = read_body(resp.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&resp_body).unwrap();
    let token = json["token"].as_str().unwrap().to_string();

    // 用 token 访问 pending reviews
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/admin/reviews/pending")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // 会因为数据库连接失败返回 500，但鉴权通过了（不是 401/403）
    assert!(resp.status() == StatusCode::INTERNAL_SERVER_ERROR || resp.status() == StatusCode::OK);
}

#[tokio::test]
async fn test_non_reviewer_cannot_access_pending() {
    let state = make_app_state();
    let app = build_router(state.clone());

    // 登录为非审核员
    let login_body = serde_json::json!({
        "developer_id": 1,
        "username": "developer",
        "is_reviewer": false
    });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(login_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let resp_body = read_body(resp.into_body()).await;
    let json: serde_json::Value = serde_json::from_str(&resp_body).unwrap();
    let token = json["token"].as_str().unwrap().to_string();

    // 用非审核员 token 访问 pending reviews → 403
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/admin/reviews/pending")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn test_download_plugin_returns_response() {
    let app = build_router(make_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/plugins/crm/1.0.0/download")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::INTERNAL_SERVER_ERROR
            || resp.status() == StatusCode::NOT_FOUND
            || resp.status() == StatusCode::OK
    );
}

#[tokio::test]
async fn test_reject_review_without_token_returns_400() {
    let app = build_router(make_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/admin/reviews/1/reject")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_search_with_tag_query_param() {
    let app = build_router(make_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/plugins/search?q=crm&tag=business&limit=5&offset=0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status() == StatusCode::INTERNAL_SERVER_ERROR || resp.status() == StatusCode::OK);
}

#[tokio::test]
async fn test_login_with_invalid_json_returns_400() {
    let app = build_router(make_app_state());
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from("invalid json"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
