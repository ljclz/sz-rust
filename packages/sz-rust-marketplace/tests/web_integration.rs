// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Web 集成测试 — 需要 PostgreSQL
//! 运行: cargo test -p sz-rust-marketplace --test web_integration -- --ignored

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use sz_rust_addons_loader::manifest::AddonManifest;
use sz_rust_marketplace::manifest::MarketplaceManifest;
use sz_rust_marketplace::web::{build_router, AppState, JwtConfig};
use tower::ServiceExt;

async fn read_body(body: Body) -> String {
    let bytes = body.collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

async fn setup_state() -> AppState {
    let url = std::env::var("MARKETPLACE_PG_URL")
        .unwrap_or_else(|_| "postgres://lewuli:JkbC2jsaWAYDe2Gz@127.0.0.1:5433/marketplace_test".to_string());
    let pool = PgPoolOptions::new().max_connections(5).connect(&url).await.unwrap();
    cleanup(&pool).await;
    migrate(&pool).await;
    let plugins = sz_rust_marketplace::repository::PluginRepository::new(pool.clone());
    let versions = sz_rust_marketplace::repository::VersionRepository::new(pool.clone());
    let reviews = sz_rust_marketplace::repository::ReviewRepository::new(pool.clone());
    let developers = sz_rust_marketplace::repository::DeveloperRepository::new(pool.clone());
    let store = Arc::new(sz_rust_marketplace::storage::LocalObjectStore::new(
        std::path::PathBuf::from("/tmp/sz-rust-web-test-store"),
    ));
    let service = Arc::new(sz_rust_marketplace::service::MarketplaceService::new(
        plugins, versions, reviews, developers, store,
    ));
    let jwt = Arc::new(JwtConfig::new("test-secret"));
    AppState { pool, service, jwt }
}

async fn cleanup(pool: &sqlx::PgPool) {
    let _ = sqlx::query("DROP TABLE IF EXISTS install_records CASCADE").execute(pool).await;
    let _ = sqlx::query("DROP TABLE IF EXISTS review_records CASCADE").execute(pool).await;
    let _ = sqlx::query("DROP TABLE IF EXISTS plugin_versions CASCADE").execute(pool).await;
    let _ = sqlx::query("DROP TABLE IF EXISTS plugins CASCADE").execute(pool).await;
    let _ = sqlx::query("DROP TABLE IF EXISTS developers CASCADE").execute(pool).await;
}

async fn migrate(pool: &sqlx::PgPool) {
    let migrations = [
        include_str!("../migrations/001_create_developers.sql"),
        include_str!("../migrations/002_create_plugins.sql"),
        include_str!("../migrations/003_create_plugin_versions.sql"),
        include_str!("../migrations/004_create_review_records.sql"),
        include_str!("../migrations/005_create_install_records.sql"),
    ];
    for sql in &migrations {
        for stmt in sql.split(';').filter(|s| !s.trim().is_empty()) {
            let _ = sqlx::query(stmt).execute(pool).await;
        }
    }
}

async fn create_developer(pool: &sqlx::PgPool, username: &str, is_reviewer: bool) -> i64 {
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO developers (username, email, public_key, is_reviewer) VALUES ($1, $2, 'pk_test', $3) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@test.com"))
    .bind(is_reviewer)
    .fetch_one(pool).await.unwrap();
    row.0
}

fn make_manifest_json(name: &str, version: &str) -> String {
    let m = MarketplaceManifest {
        base: AddonManifest {
            name: name.to_string(),
            title: format!("Test {name}"),
            identifier: format!("com.test.{name}"),
            icon: String::new(),
            author: "tester".to_string(),
            version: version.to_string(),
            admin: String::new(),
            status: 1,
            addon_path: std::path::PathBuf::new(),
        },
        description: Some("test plugin".to_string()),
        tags: vec!["test".to_string()],
        capabilities: vec![],
        dependencies: vec![],
        license: "Apache-2.0".to_string(),
        homepage: None,
        price: 0.0,
        signature: String::new(),
        status: sz_rust_marketplace::manifest::ReviewStatus::Pending,
    };
    serde_json::to_string(&m).unwrap()
}

fn make_multipart(manifest_json: &str, archive: &[u8]) -> (String, Body) {
    let boundary = "----testboundary123";
    let manifest_hdr = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"manifest\"\r\n\r\n"
    );
    let archive_hdr = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"archive\"\r\n\r\n"
    );
    let end = format!("--{boundary}--\r\n");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(manifest_hdr.as_bytes());
    bytes.extend_from_slice(manifest_json.as_bytes());
    bytes.extend_from_slice(b"\r\n");
    bytes.extend_from_slice(archive_hdr.as_bytes());
    bytes.extend_from_slice(archive);
    bytes.extend_from_slice(b"\r\n");
    bytes.extend_from_slice(end.as_bytes());
    let ct = format!("multipart/form-data; boundary={boundary}");
    (ct, Body::from(bytes))
}

async fn login_as(app: &axum::Router, developer_id: i64, username: &str, is_reviewer: bool) -> String {
    let body = serde_json::json!({"developer_id": developer_id, "username": username, "is_reviewer": is_reviewer});
    let resp = app.clone().oneshot(
        Request::builder().method("POST").uri("/api/v1/auth/login")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&read_body(resp.into_body()).await).unwrap();
    json["token"].as_str().unwrap().to_string()
}

async fn publish_plugin(app: &axum::Router, token: &str, name: &str, version: &str) -> (StatusCode, serde_json::Value) {
    let manifest_json = make_manifest_json(name, version);
    let archive = b"fake-tar-gz-content";
    let (ct, body) = make_multipart(&manifest_json, archive);
    let resp = app.clone().oneshot(
        Request::builder().method("POST").uri("/api/v1/plugins/publish")
            .header("Content-Type", &ct)
            .header("Authorization", format!("Bearer {token}"))
            .body(body).unwrap()
    ).await.unwrap();
    let status = resp.status();
    let json: serde_json::Value = serde_json::from_str(&read_body(resp.into_body()).await).unwrap_or(serde_json::Value::Null);
    (status, json)
}

// ── 基础端点 ──

#[tokio::test]
#[ignore]
async fn test_web_search_success() {
    let state = setup_state().await;
    let app = build_router(state.clone());
    let resp = app
        .oneshot(Request::builder().uri("/api/v1/plugins/search?q=test").body(Body::empty()).unwrap())
        .await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_body(resp.into_body()).await;
    assert!(body.contains("plugins"));
}

#[tokio::test]
#[ignore]
async fn test_web_get_plugin_not_found() {
    let state = setup_state().await;
    let app = build_router(state.clone());
    let resp = app
        .oneshot(Request::builder().uri("/api/v1/plugins/nonexistent").body(Body::empty()).unwrap())
        .await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore]
async fn test_web_health_and_openapi() {
    let state = setup_state().await;
    let app = build_router(state.clone());
    let resp = app.clone().oneshot(
        Request::builder().uri("/api/v1/health").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app.oneshot(
        Request::builder().uri("/api/v1/openapi.json").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore]
async fn test_web_login_success() {
    let state = setup_state().await;
    let app = build_router(state.clone());
    let resp = app.oneshot(
        Request::builder().method("POST").uri("/api/v1/auth/login")
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::json!({"developer_id": 1, "username": "alice", "is_reviewer": false}).to_string()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&read_body(resp.into_body()).await).unwrap();
    assert!(json["token"].as_str().is_some());
}

// ── publish handler ──

#[tokio::test]
#[ignore]
async fn test_web_publish_invalid_token_401() {
    let state = setup_state().await;
    let app = build_router(state.clone());
    let manifest_json = make_manifest_json("web-test-pub", "1.0.0");
    let (ct, body) = make_multipart(&manifest_json, b"archive");
    let resp = app.oneshot(
        Request::builder().method("POST").uri("/api/v1/plugins/publish")
            .header("Content-Type", &ct)
            .header("Authorization", "Bearer invalid-token")
            .body(body).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore]
async fn test_web_publish_success() {
    let state = setup_state().await;
    create_developer(&state.pool, "pub-dev", false).await;
    let app = build_router(state.clone());
    let token = login_as(&app, 1, "pub-dev", false).await;
    let (status, json) = publish_plugin(&app, &token, "web-pub-plugin", "1.0.0").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json["version_id"].as_i64().is_some());
}

#[tokio::test]
#[ignore]
async fn test_web_publish_invalid_manifest_422() {
    let state = setup_state().await;
    create_developer(&state.pool, "bad-dev", false).await;
    let app = build_router(state.clone());
    let token = login_as(&app, 1, "bad-dev", false).await;
    let (ct, body) = make_multipart("not-valid-json", b"archive");
    let resp = app.oneshot(
        Request::builder().method("POST").uri("/api/v1/plugins/publish")
            .header("Content-Type", &ct)
            .header("Authorization", format!("Bearer {token}"))
            .body(body).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// ── download handler ──

#[tokio::test]
#[ignore]
async fn test_web_download_plugin_not_found() {
    let state = setup_state().await;
    let app = build_router(state.clone());
    let resp = app.oneshot(
        Request::builder().uri("/api/v1/plugins/no-plug/1.0.0/download").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore]
async fn test_web_download_version_not_found() {
    let state = setup_state().await;
    create_developer(&state.pool, "dl-dev", false).await;
    let app = build_router(state.clone());
    let token = login_as(&app, 1, "dl-dev", false).await;
    let (status, _) = publish_plugin(&app, &token, "dl-plugin", "1.0.0").await;
    assert_eq!(status, StatusCode::OK);

    let resp = app.oneshot(
        Request::builder().uri("/api/v1/plugins/dl-plugin/9.9.9/download").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore]
async fn test_web_download_not_approved_403() {
    let state = setup_state().await;
    create_developer(&state.pool, "na-dev", false).await;
    let app = build_router(state.clone());
    let token = login_as(&app, 1, "na-dev", false).await;
    let (status, _) = publish_plugin(&app, &token, "na-plugin", "1.0.0").await;
    assert_eq!(status, StatusCode::OK);

    let resp = app.oneshot(
        Request::builder().uri("/api/v1/plugins/na-plugin/1.0.0/download").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore]
async fn test_web_download_approved_success() {
    let state = setup_state().await;
    let dev_id = create_developer(&state.pool, "ok-dev", false).await;
    create_developer(&state.pool, "ok-rev", true).await;
    let app = build_router(state.clone());
    let dev_token = login_as(&app, dev_id, "ok-dev", false).await;
    let (status, json) = publish_plugin(&app, &dev_token, "ok-plugin", "1.0.0").await;
    assert_eq!(status, StatusCode::OK);
    let version_id = json["version_id"].as_i64().unwrap();

    let rev_token = login_as(&app, 2, "ok-rev", true).await;
    let resp = app.clone().oneshot(
        Request::builder().method("POST").uri(format!("/api/v1/admin/reviews/{version_id}/approve"))
            .header("Authorization", format!("Bearer {rev_token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::json!({"developer_id": dev_id}).to_string()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app.oneshot(
        Request::builder().uri("/api/v1/plugins/ok-plugin/1.0.0/download").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(!bytes.is_empty());
}

// ── review handler ──

#[tokio::test]
#[ignore]
async fn test_web_pending_reviews_with_reviewer() {
    let state = setup_state().await;
    create_developer(&state.pool, "rev1", true).await;
    let app = build_router(state.clone());
    let token = login_as(&app, 1, "rev1", true).await;
    let resp = app.oneshot(
        Request::builder().uri("/api/v1/admin/reviews/pending")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_body(resp.into_body()).await;
    assert!(body.contains("versions"));
}

#[tokio::test]
#[ignore]
async fn test_web_pending_reviews_not_reviewer_403() {
    let state = setup_state().await;
    create_developer(&state.pool, "norm", false).await;
    let app = build_router(state.clone());
    let token = login_as(&app, 1, "norm", false).await;
    let resp = app.oneshot(
        Request::builder().uri("/api/v1/admin/reviews/pending")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[ignore]
async fn test_web_approve_invalid_token_401() {
    let state = setup_state().await;
    let app = build_router(state.clone());
    let resp = app.oneshot(
        Request::builder().method("POST").uri("/api/v1/admin/reviews/1/approve")
            .header("Authorization", "Bearer invalid-token")
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::json!({"developer_id": 1}).to_string()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore]
async fn test_web_approve_review_success() {
    let state = setup_state().await;
    let dev_id = create_developer(&state.pool, "ap-dev", false).await;
    create_developer(&state.pool, "ap-rev", true).await;
    let app = build_router(state.clone());
    let dev_token = login_as(&app, dev_id, "ap-dev", false).await;
    let (_, json) = publish_plugin(&app, &dev_token, "ap-plugin", "1.0.0").await;
    let version_id = json["version_id"].as_i64().unwrap();

    let rev_token = login_as(&app, 2, "ap-rev", true).await;
    let resp = app.oneshot(
        Request::builder().method("POST").uri(format!("/api/v1/admin/reviews/{version_id}/approve"))
            .header("Authorization", format!("Bearer {rev_token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::json!({"developer_id": dev_id, "comment": "lgfm"}).to_string()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore]
async fn test_web_reject_review_success() {
    let state = setup_state().await;
    let dev_id = create_developer(&state.pool, "rj-dev", false).await;
    create_developer(&state.pool, "rj-rev", true).await;
    let app = build_router(state.clone());
    let dev_token = login_as(&app, dev_id, "rj-dev", false).await;
    let (_, json) = publish_plugin(&app, &dev_token, "rj-plugin", "1.0.0").await;
    let version_id = json["version_id"].as_i64().unwrap();

    let rev_token = login_as(&app, 2, "rj-rev", true).await;
    let resp = app.oneshot(
        Request::builder().method("POST").uri(format!("/api/v1/admin/reviews/{version_id}/reject"))
            .header("Authorization", format!("Bearer {rev_token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::json!({"developer_id": dev_id, "comment": "bad"}).to_string()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore]
async fn test_web_reject_invalid_token_401() {
    let state = setup_state().await;
    let app = build_router(state.clone());
    let resp = app.oneshot(
        Request::builder().method("POST").uri("/api/v1/admin/reviews/1/reject")
            .header("Authorization", "Bearer invalid-token")
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::json!({"developer_id": 1}).to_string()))
            .unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
