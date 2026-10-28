// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P2-2 Inertia.js 适配器 axum 端到端集成测试
//!
//! 验证 InertiaAdapter 在 axum Router 中的 SPA/SSR 模式响应流程。

#![cfg(feature = "inertia")]
#![forbid(unsafe_code)]

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sz_rust_mvc_facade::inertia::{InertiaAdapter, InertiaConfig, InertiaMode};
use sz_rust_mvc_facade::ssr::{ComponentRenderer, RenderError, SsrConfig};
use tokio::sync::Mutex;
use tower::ServiceExt;

struct MockRenderer;

#[async_trait]
impl ComponentRenderer for MockRenderer {
    async fn render_component(&self, _: &str, _: &Value) -> Result<String, RenderError> {
        Ok("<div>Inertia SSR</div>".to_string())
    }
}

#[derive(Clone)]
struct AppState {
    adapter: Arc<Mutex<InertiaAdapter>>,
}

async fn inertia_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let client_version = headers
        .get("x-inertia-version")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let mut adapter = state.adapter.lock().await;
    let resp = adapter
        .render(
            "UsersPage",
            json!({"users": ["alice", "bob"]}),
            client_version.as_deref(),
        )
        .await;

    if resp.force_refresh {
        return (StatusCode::CONFLICT, "version mismatch").into_response();
    }

    if let Some(ref html) = resp.html {
        return (
            StatusCode::OK,
            [("content-type", "text/html")],
            html.clone(),
        )
            .into_response();
    }

    let json_body = json!({
        "component": resp.component,
        "props": resp.props,
        "version": resp.version,
    });
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        json_body.to_string(),
    )
        .into_response()
}

async fn body_string(resp: Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn e2e_inertia_spa_returns_json() {
    let adapter = InertiaAdapter::new_spa(InertiaConfig::default());
    let app = Router::new()
        .route("/users", get(inertia_handler))
        .with_state(AppState {
            adapter: Arc::new(Mutex::new(adapter)),
        });

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    let json: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["component"], "UsersPage", "应返回组件名");
    assert_eq!(json["props"]["users"][0], "alice", "应返回 props");
    assert_eq!(json["version"], "1.0.0", "应返回版本");
}

#[tokio::test]
async fn e2e_inertia_version_mismatch_returns_conflict() {
    let adapter = InertiaAdapter::new_spa(InertiaConfig::default());
    let app = Router::new()
        .route("/users", get(inertia_handler))
        .with_state(AppState {
            adapter: Arc::new(Mutex::new(adapter)),
        });

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/users")
                .header("x-inertia-version", "0.9.0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT, "版本不匹配应返回 409");
}

#[tokio::test]
async fn e2e_inertia_ssr_returns_html() {
    let config = InertiaConfig {
        mode: InertiaMode::Ssr,
        ..Default::default()
    };
    let adapter = InertiaAdapter::new_ssr(config, MockRenderer, SsrConfig::default());
    let app = Router::new()
        .route("/users", get(inertia_handler))
        .with_state(AppState {
            adapter: Arc::new(Mutex::new(adapter)),
        });

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("&lt;div&gt;"),
        "SSR 模式应返回转义 HTML: {}",
        body
    );
}

#[tokio::test]
async fn e2e_inertia_sensitive_props_sanitized() {
    let adapter = InertiaAdapter::new_spa(InertiaConfig::default());
    let app = Router::new()
        .route("/users", get(inertia_handler))
        .with_state(AppState {
            adapter: Arc::new(Mutex::new(adapter)),
        });

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    let json: Value = serde_json::from_str(&body).unwrap();
    assert!(
        json["props"].get("password").is_none(),
        "敏感字段 password 应被脱敏: {}",
        body
    );
    assert!(
        json["props"].get("api_key").is_none(),
        "敏感字段 api_key 应被脱敏: {}",
        body
    );
}
