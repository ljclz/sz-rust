// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 插件生态端到端测试（spec §5.10）
//!
//! 验证 6 个场景：
//! - 规则 1: 插件发布到市场 → 市场可见 + 签名验证通过
//! - 规则 2: 安装已签名插件 → 加载成功 + list 可见
//! - 规则 3: 卸载插件 → list 不可见 + 资源已释放
//! - 规则 4: 安装未签名插件（签名长度<64）→ 拒绝安装
//! - 异常 1: 插件市场不可用 → 503
//! - 异常 2: 插件不存在于市场 → 404

#![cfg(feature = "v19-plugin-flow")]

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
    Router,
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use sz_rust_sz300::controllers::plugin;
use sz_rust_sz300::services::plugin_manager::{
    InMemoryMarketplace, PluginError, PluginInfo, PluginMarketplace,
};
use sz_rust_sz300::state::AppState;

mod common;
use common::ensure_mysql;

/// 构建只挂载插件路由的测试 Router（绕过 CSRF/JWT 中间件，聚焦插件逻辑验证）
fn make_plugin_router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/plugin/install", post(plugin::install))
        .route("/api/v1/plugin/uninstall", post(plugin::uninstall))
        .route("/api/v1/plugin/list", get(plugin::list))
        .with_state(state)
}

fn make_signed_plugin(id: &str) -> PluginInfo {
    PluginInfo {
        id: id.to_string(),
        name: format!("plugin-{}", id),
        version: "1.0.0".to_string(),
        description: "test plugin".to_string(),
        signature: "a".repeat(64),
        package: "wasm_bytes_base64".to_string(),
    }
}

fn make_unsigned_plugin(id: &str) -> PluginInfo {
    PluginInfo {
        id: id.to_string(),
        name: format!("plugin-{}", id),
        version: "1.0.0".to_string(),
        description: "unsigned plugin".to_string(),
        signature: "short".to_string(),
        package: "wasm_bytes_base64".to_string(),
    }
}

async fn send_request(
    router: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let request = if let Some(json_body) = body {
        builder = builder.header("content-type", "application/json");
        let bytes = serde_json::to_vec(&json_body).unwrap();
        builder.body(Body::from(bytes)).unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

#[tokio::test]
async fn test_rule1_plugin_published_to_marketplace() {
    let marketplace = Arc::new(InMemoryMarketplace::new());
    marketplace.publish(make_signed_plugin("p1")).await;

    assert!(marketplace.contains("p1").await);
    assert!(!marketplace.contains("nonexistent").await);
}

#[tokio::test]
async fn test_rule2_install_signed_plugin_success() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };

    let marketplace = Arc::new(InMemoryMarketplace::new());
    marketplace.publish(make_signed_plugin("p2")).await;

    let state = common::make_state_with_marketplace(pool, marketplace);
    let router = make_plugin_router(state);

    let (status, json) = send_request(
        &router,
        "POST",
        "/api/v1/plugin/install",
        Some(serde_json::json!({ "plugin_id": "p2" })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["code"], 1);
    assert_eq!(json["data"]["id"], "p2");
    assert_eq!(json["data"]["name"], "plugin-p2");

    let (status, json) = send_request(&router, "GET", "/api/v1/plugin/list", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["code"], 1);
    assert_eq!(json["data"].as_array().unwrap().len(), 1);
    assert_eq!(json["data"][0]["id"], "p2");
}

#[tokio::test]
async fn test_rule3_uninstall_plugin_releases_resources() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };

    let marketplace = Arc::new(InMemoryMarketplace::new());
    marketplace.publish(make_signed_plugin("p3")).await;

    let state = common::make_state_with_marketplace(pool, marketplace);
    let router = make_plugin_router(state);

    let (status, _) = send_request(
        &router,
        "POST",
        "/api/v1/plugin/install",
        Some(serde_json::json!({ "plugin_id": "p3" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, json) = send_request(
        &router,
        "POST",
        "/api/v1/plugin/uninstall",
        Some(serde_json::json!({ "plugin_id": "p3" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["code"], 1);

    let (status, json) = send_request(&router, "GET", "/api/v1/plugin/list", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"].as_array().unwrap().len(), 0);

    let (status, json) = send_request(
        &router,
        "POST",
        "/api/v1/plugin/uninstall",
        Some(serde_json::json!({ "plugin_id": "p3" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["code"], 0);
}

#[tokio::test]
async fn test_rule4_install_unsigned_plugin_rejected() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };

    let marketplace = Arc::new(InMemoryMarketplace::new());
    marketplace.publish(make_unsigned_plugin("p4")).await;

    let state = common::make_state_with_marketplace(pool, marketplace);
    let router = make_plugin_router(state);

    let (status, json) = send_request(
        &router,
        "POST",
        "/api/v1/plugin/install",
        Some(serde_json::json!({ "plugin_id": "p4" })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["code"], 0);
    assert!(json["msg"].as_str().unwrap().contains("签名"));

    let (status, json) = send_request(&router, "GET", "/api/v1/plugin/list", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"].as_array().unwrap().len(), 0);
}

struct FailingMarketplace;

#[async_trait::async_trait]
impl PluginMarketplace for FailingMarketplace {
    async fn fetch(&self, plugin_id: &str) -> Result<PluginInfo, PluginError> {
        Err(PluginError::MarketUnavailable(format!(
            "connection refused: {}",
            plugin_id
        )))
    }
}

#[tokio::test]
async fn test_exception1_market_unavailable_returns_503() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };

    let state = common::make_state_with_marketplace(pool, Arc::new(FailingMarketplace));
    let router = make_plugin_router(state);

    let (status, json) = send_request(
        &router,
        "POST",
        "/api/v1/plugin/install",
        Some(serde_json::json!({ "plugin_id": "any" })),
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json["code"], 0);
    assert!(json["msg"].as_str().unwrap().contains("市场不可用"));
}

#[tokio::test]
async fn test_exception2_plugin_not_found_returns_404() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };

    let marketplace = Arc::new(InMemoryMarketplace::new());
    let state = common::make_state_with_marketplace(pool, marketplace);
    let router = make_plugin_router(state);

    let (status, json) = send_request(
        &router,
        "POST",
        "/api/v1/plugin/install",
        Some(serde_json::json!({ "plugin_id": "nonexistent" })),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["code"], 0);
    assert!(json["msg"].as_str().unwrap().contains("未找到"));
}
