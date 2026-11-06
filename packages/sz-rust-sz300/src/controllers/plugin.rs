// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 插件管理控制器（spec §5.10）
//!
//! 端点：
//! - `POST /api/v1/plugin/install` — 安装插件
//! - `POST /api/v1/plugin/uninstall` — 卸载插件
//! - `GET  /api/v1/plugin/list` — 列出已安装插件

use crate::services::plugin_manager::{InstalledPlugin, PluginError};
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde::Deserialize;
use serde_json::{json, Value};

/// 安装请求体
#[derive(Debug, Deserialize)]
pub struct InstallRequest {
    /// 插件 ID
    pub plugin_id: String,
}

/// 卸载请求体
#[derive(Debug, Deserialize)]
pub struct UninstallRequest {
    /// 插件 ID
    pub plugin_id: String,
}

/// 安装插件（spec §5.10.1 规则 2）
#[tracing::instrument(skip(state))]
pub async fn install(
    State(state): State<AppState>,
    Json(req): Json<InstallRequest>,
) -> impl IntoResponse {
    let mgr = state.plugin_manager.clone();
    match mgr.install(&req.plugin_id).await {
        Ok(record) => (
            StatusCode::OK,
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": record,
            })),
        ),
        Err(e) => plugin_error_response(e),
    }
}

/// 卸载插件（spec §5.10.1 规则 3）
#[tracing::instrument(skip(state))]
pub async fn uninstall(
    State(state): State<AppState>,
    Json(req): Json<UninstallRequest>,
) -> impl IntoResponse {
    let mgr = state.plugin_manager.clone();
    match mgr.uninstall(&req.plugin_id).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": null,
            })),
        ),
        Err(e) => plugin_error_response(e),
    }
}

/// 列出已安装插件（spec §5.10.1）
#[tracing::instrument(skip(state))]
pub async fn list(State(state): State<AppState>) -> Json<Value> {
    let mgr = state.plugin_manager.clone();
    let plugins: Vec<InstalledPlugin> = mgr.list().await;
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": plugins,
    }))
}

/// 将 PluginError 映射为 HTTP 响应
fn plugin_error_response(e: PluginError) -> (StatusCode, Json<Value>) {
    let (status, msg) = match &e {
        PluginError::MarketUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "插件市场不可用"),
        PluginError::SignatureInvalid => (StatusCode::BAD_REQUEST, "签名校验失败"),
        PluginError::NotFound(_) => (StatusCode::NOT_FOUND, "插件未找到"),
        PluginError::AlreadyInstalled(_) => (StatusCode::CONFLICT, "插件已安装"),
    };
    (status, Json(json!({ "code": 0, "msg": msg, "data": null })))
}
// ====================================================================
// handler 分支测试（axum oneshot 模式，覆盖 Ok/Err 双路径）
//
// 契约引用：
// - C-MOCK-APP-STATE@1: crate::state::mock_app_state()
// - C-AXUM-ONESHOT@1: Router + tower::ServiceExt::oneshot
// - C-PLUGIN-MANAGER@1: PluginManager/InMemoryMarketplace/PluginError
// ====================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::plugin_manager::{InMemoryMarketplace, PluginInfo, PluginManager};
    use crate::state::mock_app_state;
    use axum::body::Body;
    use axum::http::Request;
    use axum::response::Response;
    use axum::routing::{get, post};
    use axum::Router;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use ed25519_dalek::{Signer, SigningKey};
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;

    /// 收集响应体为 String
    async fn body_string(response: Response) -> String {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        String::from_utf8(bytes.to_vec()).expect("UTF-8")
    }

    /// 构造 POST JSON 请求
    fn json_request(uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    /// 固定测试签名密钥（确定性，seed=42）
    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[42u8; 32])
    }

    /// 测试公钥（Base64 编码的 32 字节 Ed25519 公钥）
    fn test_public_key_b64() -> String {
        B64.encode(test_signing_key().verifying_key().to_bytes())
    }

    /// 构造有效签名插件（用测试私钥对 package 真实签名）
    async fn make_valid_plugin(id: &str) -> PluginInfo {
        let package = format!("wasm-bytes-{id}");
        let package_b64 = B64.encode(package.as_bytes());
        let sig = test_signing_key().sign(package.as_bytes());
        let signature = B64.encode(sig.to_bytes());
        PluginInfo {
            id: id.to_string(),
            name: format!("plugin-{id}"),
            version: "1.0.0".to_string(),
            description: "test plugin".to_string(),
            signature,
            package: package_b64,
        }
    }

    /// 构造带有效插件 marketplace + 有效公钥的 AppState（覆盖 install/uninstall Ok 分支）
    async fn state_with_valid_plugin(id: &str) -> AppState {
        let mut state = mock_app_state();
        let marketplace = Arc::new(InMemoryMarketplace::new());
        marketplace.publish(make_valid_plugin(id).await).await;
        let mgr = PluginManager::new(marketplace, test_public_key_b64());
        state.plugin_manager = Arc::new(mgr);
        state
    }

    // ─────────────────────────────────────────────
    // plugin_error_response 纯逻辑：PluginError → (StatusCode, Json) 映射
    // ─────────────────────────────────────────────

    /// MarketUnavailable → 503 SERVICE_UNAVAILABLE
    #[test]
    fn plugin_error_response_market_unavailable_returns_503() {
        let (status, _) = plugin_error_response(PluginError::MarketUnavailable("timeout".into()));
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    /// SignatureInvalid → 400 BAD_REQUEST
    #[test]
    fn plugin_error_response_signature_invalid_returns_400() {
        let (status, _) = plugin_error_response(PluginError::SignatureInvalid);
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// NotFound → 404 NOT_FOUND
    #[test]
    fn plugin_error_response_not_found_returns_404() {
        let (status, _) = plugin_error_response(PluginError::NotFound("p1".into()));
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// AlreadyInstalled → 409 CONFLICT
    #[test]
    fn plugin_error_response_already_installed_returns_409() {
        let (status, _) = plugin_error_response(PluginError::AlreadyInstalled("p1".into()));
        assert_eq!(status, StatusCode::CONFLICT);
    }

    // ─────────────────────────────────────────────
    // install handler
    // ─────────────────────────────────────────────

    /// install 不存在的 plugin_id → marketplace.fetch 返回 NotFound → 404
    #[tokio::test]
    async fn install_nonexistent_plugin_returns_404() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/plugin/install", post(install))
            .with_state(state);
        let response = router
            .oneshot(json_request(
                "/api/v1/plugin/install",
                r#"{"plugin_id": "nonexistent"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = body_string(response).await;
        assert!(
            body.contains("插件未找到"),
            "应返回插件未找到, 实际: {body}"
        );
    }

    /// install 有效 plugin_id → 验签通过 → 200 + code:1 + data 含插件信息
    #[tokio::test]
    async fn install_valid_plugin_returns_200() {
        let state = state_with_valid_plugin("p1").await;
        let router = Router::new()
            .route("/api/v1/plugin/install", post(install))
            .with_state(state);
        let response = router
            .oneshot(json_request(
                "/api/v1/plugin/install",
                r#"{"plugin_id": "p1"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(
            body.contains("\"code\":1"),
            "成功安装应返回 code:1, 实际: {body}"
        );
        assert!(
            body.contains("plugin-p1"),
            "应返回插件名称 plugin-p1, 实际: {body}"
        );
    }

    /// install 已安装插件 → 409 CONFLICT
    #[tokio::test]
    async fn install_already_installed_returns_409() {
        let state = state_with_valid_plugin("p1").await;
        // 先直接通过 manager 安装，使插件进入已安装状态
        state.plugin_manager.install("p1").await.unwrap();
        let router = Router::new()
            .route("/api/v1/plugin/install", post(install))
            .with_state(state);
        let response = router
            .oneshot(json_request(
                "/api/v1/plugin/install",
                r#"{"plugin_id": "p1"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = body_string(response).await;
        assert!(
            body.contains("插件已安装"),
            "重复安装应返回插件已安装, 实际: {body}"
        );
    }

    // ─────────────────────────────────────────────
    // uninstall handler
    // ─────────────────────────────────────────────

    /// uninstall 不存在的 plugin_id → 404
    #[tokio::test]
    async fn uninstall_nonexistent_plugin_returns_404() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/plugin/uninstall", post(uninstall))
            .with_state(state);
        let response = router
            .oneshot(json_request(
                "/api/v1/plugin/uninstall",
                r#"{"plugin_id": "nonexistent"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = body_string(response).await;
        assert!(
            body.contains("插件未找到"),
            "应返回插件未找到, 实际: {body}"
        );
    }

    /// uninstall 已安装插件 → 200 + code:1
    #[tokio::test]
    async fn uninstall_installed_plugin_returns_200() {
        let state = state_with_valid_plugin("p1").await;
        // 先安装插件
        state.plugin_manager.install("p1").await.unwrap();
        let router = Router::new()
            .route("/api/v1/plugin/uninstall", post(uninstall))
            .with_state(state);
        let response = router
            .oneshot(json_request(
                "/api/v1/plugin/uninstall",
                r#"{"plugin_id": "p1"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(
            body.contains("\"code\":1"),
            "成功卸载应返回 code:1, 实际: {body}"
        );
    }

    // ─────────────────────────────────────────────
    // list handler
    // ─────────────────────────────────────────────

    /// list 初始无已安装插件 → 200 + data 为空数组
    #[tokio::test]
    async fn list_returns_empty_array() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/plugin/list", get(list))
            .with_state(state);
        let response = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/plugin/list")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains("\"code\":1"), "应返回 code:1, 实际: {body}");
        assert!(
            body.contains("\"data\":[]"),
            "初始列表应为空数组, 实际: {body}"
        );
    }
}
