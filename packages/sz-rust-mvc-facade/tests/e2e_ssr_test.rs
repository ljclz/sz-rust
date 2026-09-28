// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P2-1 SSR 中间件 axum 端到端集成测试

#![forbid(unsafe_code)]

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use sz_rust_mvc_facade::ssr::{ComponentRenderer, RenderError, SsrConfig, SsrMiddleware};
use tower::ServiceExt;

struct MockRenderer;

#[async_trait]
impl ComponentRenderer for MockRenderer {
    async fn render_component(&self, route: &str, data: &Value) -> Result<String, RenderError> {
        Ok(format!("<div route=\"{}\">{}</div>", route, data))
    }
}

struct FailingRenderer;

#[async_trait]
impl ComponentRenderer for FailingRenderer {
    async fn render_component(&self, _: &str, _: &Value) -> Result<String, RenderError> {
        Err(RenderError::Render("always fails".to_string()))
    }
}

async fn ssr_handler(
    Path(route): Path<String>,
    State(mw): State<Arc<SsrMiddleware<MockRenderer>>>,
) -> String {
    let result = mw.render(&format!("/{}", route), &Value::Null).await;
    result.html
}

async fn ssr_failing_handler(
    Path(route): Path<String>,
    State(mw): State<Arc<SsrMiddleware<FailingRenderer>>>,
) -> String {
    let result = mw.render(&format!("/{}", route), &Value::Null).await;
    result.html
}

async fn body_string(resp: Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn e2e_ssr_normal_render_returns_escaped_html() {
    let mw = Arc::new(SsrMiddleware::new(MockRenderer, SsrConfig::default()));
    let app = Router::new()
        .route("/ssr/{route}", get(ssr_handler))
        .with_state(mw);

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/ssr/home")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("&lt;div"),
        "响应应包含 XSS 转义后的 HTML: {}",
        body
    );
    assert!(body.contains("/home"), "响应应包含路由: {}", body);
}

#[tokio::test]
async fn e2e_ssr_render_failure_returns_fallback_html() {
    let mw = Arc::new(SsrMiddleware::new(FailingRenderer, SsrConfig::default()));
    let app = Router::new()
        .route("/ssr/{route}", get(ssr_failing_handler))
        .with_state(mw);

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/ssr/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("__SSR_FALLBACK__"),
        "渲染失败应返回降级 HTML: {}",
        body
    );
    assert!(
        body.contains("/dashboard"),
        "降级 HTML 应包含路由: {}",
        body
    );
}

#[tokio::test]
async fn e2e_ssr_xss_injection_prevented() {
    let mw = Arc::new(SsrMiddleware::new(MockRenderer, SsrConfig::default()));
    let app = Router::new()
        .route("/ssr/{route}", get(ssr_handler))
        .with_state(mw);

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/ssr/profile")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        !body.contains("<script>"),
        "XSS: <script> 不应出现在响应中: {}",
        body
    );
    assert!(body.contains("&lt;div"), "应包含转义后的 HTML: {}", body);
}
