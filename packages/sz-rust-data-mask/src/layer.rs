// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! axum Layer 适配 — 拦截 JSON 响应并对配置字段脱敏
//!
//! v1.8.0 新增：提供 `DataMaskLayer`，可直接挂载到 `Router::layer()`。
//!
//! 工作原理：
//! 1. 拦截响应 body
//! 2. 若 Content-Type 为 application/json，解析 JSON
//! 3. 递归遍历 JSON tree，对匹配字段名应用脱敏规则
//! 4. 重新序列化 JSON 并替换 body

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::response::Response;
use http::{HeaderValue, Request};
use serde_json::Value;
use tower::{Layer, Service};

use crate::mask::{BuiltinMaskRule, MaskScene};
use crate::MaskEngine;

/// 数据脱敏 tower Layer
#[derive(Clone)]
pub struct DataMaskLayer {
    engine: Arc<MaskEngine>,
    scene: MaskScene,
}

impl DataMaskLayer {
    /// 创建数据脱敏 layer
    pub fn new(engine: Arc<MaskEngine>, scene: MaskScene) -> Self {
        Self { engine, scene }
    }
}

impl<S> Layer<S> for DataMaskLayer
where
    S: Clone,
{
    type Service = DataMaskService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DataMaskService {
            inner,
            engine: self.engine.clone(),
            scene: self.scene,
        }
    }
}

/// 数据脱敏 tower Service
#[derive(Clone)]
pub struct DataMaskService<S> {
    inner: S,
    engine: Arc<MaskEngine>,
    scene: MaskScene,
}

impl<S> Service<Request<Body>> for DataMaskService<S>
where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send,
{
    type Response = Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn std::future::Future<Output = Result<Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let engine = self.engine.clone();
        let scene = self.scene;
        // tower 要求 call(&mut self) 但不消耗 self，需要 clone inner
        // 参考 tower::util::MapRequest 的实现模式
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move {
            let response = inner.call(req).await?;
            Ok(apply_mask_to_response(response, &engine, scene).await)
        })
    }
}

/// 对响应应用脱敏（async，需要读取 body）
async fn apply_mask_to_response(
    response: Response,
    engine: &MaskEngine,
    scene: MaskScene,
) -> Response {
    let (parts, body) = response.into_parts();

    // 检查 Content-Type 是否为 JSON
    let is_json = parts
        .headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v: &HeaderValue| v.to_str().ok())
        .map(|s| s.contains("application/json"))
        .unwrap_or(false);

    if !is_json {
        return Response::from_parts(parts, body);
    }

    // 读取 body bytes
    let bytes = match axum::body::to_bytes(body, 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return Response::from_parts(parts, Body::empty()),
    };

    // 解析 JSON
    let mut json: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return Response::from_parts(parts, Body::from(bytes)),
    };

    // 递归脱敏
    mask_json_value(&mut json, engine, scene);

    // 重新序列化
    let new_body = serde_json::to_vec(&json).unwrap_or_else(|_| bytes.to_vec());
    Response::from_parts(parts, Body::from(new_body))
}

/// 递归遍历 JSON value，对匹配字段名应用脱敏
fn mask_json_value(value: &mut Value, engine: &MaskEngine, scene: MaskScene) {
    match value {
        Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if let Value::String(s) = val {
                    if let Ok(masked) = engine.mask(scene, key, s) {
                        *s = masked;
                    }
                }
                mask_json_value(val, engine, scene);
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                mask_json_value(item, engine, scene);
            }
        }
        _ => {}
    }
}

/// 便捷函数：创建默认脱敏引擎（注册常用字段规则）
pub fn default_mask_engine() -> MaskEngine {
    use crate::mask::MaskRule;

    let mut engine = MaskEngine::new();
    engine.register(
        MaskScene::Response,
        MaskRule::builtin("phone", BuiltinMaskRule::Phone),
    );
    engine.register(
        MaskScene::Response,
        MaskRule::builtin("contact_phone", BuiltinMaskRule::Phone),
    );
    engine.register(
        MaskScene::Response,
        MaskRule::builtin("bank_account", BuiltinMaskRule::BankCard),
    );
    engine.register(
        MaskScene::Response,
        MaskRule::builtin("password_hash", BuiltinMaskRule::Full),
    );
    engine.register(
        MaskScene::Response,
        MaskRule::builtin("id_card", BuiltinMaskRule::IdCard),
    );
    engine.register(
        MaskScene::Response,
        MaskRule::builtin("email", BuiltinMaskRule::Email),
    );
    engine
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask::{BuiltinMaskRule, MaskRule};
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::Router;
    use http_body_util::BodyExt as HttpBodyExt;
    use tower::ServiceExt;

    fn make_engine() -> MaskEngine {
        let mut engine = MaskEngine::new();
        engine.register(
            MaskScene::Response,
            MaskRule::builtin("phone", BuiltinMaskRule::Phone),
        );
        engine.register(
            MaskScene::Response,
            MaskRule::builtin("bank_account", BuiltinMaskRule::BankCard),
        );
        engine.register(
            MaskScene::Response,
            MaskRule::builtin("password_hash", BuiltinMaskRule::Full),
        );
        engine.register(
            MaskScene::Response,
            MaskRule::builtin("id_card", BuiltinMaskRule::IdCard),
        );
        engine
    }

    async fn json_handler() -> impl axum::response::IntoResponse {
        (
            StatusCode::OK,
            [("content-type", "application/json")],
            r#"{"name":"张三","phone":"13812345678","bank_account":"6222021234567890"}"#,
        )
    }

    fn make_request() -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/test")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn test_layer_masks_phone_field() {
        let engine = Arc::new(make_engine());
        let layer = DataMaskLayer::new(engine, MaskScene::Response);
        let app: Router = Router::new()
            .route("/test", post(json_handler))
            .layer(layer);

        let res = app.oneshot(make_request()).await.unwrap();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["phone"], "138****5678");
        assert_eq!(json["name"], "张三");
    }

    #[tokio::test]
    async fn test_layer_masks_bank_account_field() {
        let engine = Arc::new(make_engine());
        let layer = DataMaskLayer::new(engine, MaskScene::Response);
        let app: Router = Router::new()
            .route("/test", post(json_handler))
            .layer(layer);

        let res = app.oneshot(make_request()).await.unwrap();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["bank_account"], "6222********7890");
    }

    #[tokio::test]
    async fn test_layer_masks_nested_object() {
        async fn handler() -> impl axum::response::IntoResponse {
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                r#"{"merchant":{"name":"李四","phone":"13987654321"},"id":1}"#,
            )
        }
        let engine = Arc::new(make_engine());
        let layer = DataMaskLayer::new(engine, MaskScene::Response);
        let app: Router = Router::new().route("/test", post(handler)).layer(layer);

        let res = app.oneshot(make_request()).await.unwrap();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["merchant"]["phone"], "139****4321");
        assert_eq!(json["merchant"]["name"], "李四");
    }

    #[tokio::test]
    async fn test_layer_masks_array_of_objects() {
        async fn handler() -> impl axum::response::IntoResponse {
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                r#"[{"phone":"13812345678"},{"phone":"13987654321"}]"#,
            )
        }
        let engine = Arc::new(make_engine());
        let layer = DataMaskLayer::new(engine, MaskScene::Response);
        let app: Router = Router::new().route("/test", post(handler)).layer(layer);

        let res = app.oneshot(make_request()).await.unwrap();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json[0]["phone"], "138****5678");
        assert_eq!(json[1]["phone"], "139****4321");
    }

    #[tokio::test]
    async fn test_layer_masks_password_hash_full() {
        async fn handler() -> impl axum::response::IntoResponse {
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                r#"{"user":"admin","password_hash":"$2b$12$abcdef"}"#,
            )
        }
        let engine = Arc::new(make_engine());
        let layer = DataMaskLayer::new(engine, MaskScene::Response);
        let app: Router = Router::new().route("/test", post(handler)).layer(layer);

        let res = app.oneshot(make_request()).await.unwrap();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["password_hash"], "*************");
        assert_eq!(json["user"], "admin");
    }

    #[tokio::test]
    async fn test_layer_preserves_non_json_response() {
        async fn handler() -> impl axum::response::IntoResponse {
            (StatusCode::OK, [("content-type", "text/plain")], "hello")
        }
        let engine = Arc::new(make_engine());
        let layer = DataMaskLayer::new(engine, MaskScene::Response);
        let app: Router = Router::new().route("/test", post(handler)).layer(layer);

        let res = app.oneshot(make_request()).await.unwrap();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.as_ref(), b"hello");
    }

    #[tokio::test]
    async fn test_layer_preserves_unmasked_fields() {
        async fn handler() -> impl axum::response::IntoResponse {
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                r#"{"id":1,"name":"测试","status":"active"}"#,
            )
        }
        let engine = Arc::new(make_engine());
        let layer = DataMaskLayer::new(engine, MaskScene::Response);
        let app: Router = Router::new().route("/test", post(handler)).layer(layer);

        let res = app.oneshot(make_request()).await.unwrap();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], 1);
        assert_eq!(json["name"], "测试");
        assert_eq!(json["status"], "active");
    }

    #[test]
    fn test_default_mask_engine_has_common_rules() {
        let engine = default_mask_engine();
        let result = engine
            .mask(MaskScene::Response, "phone", "13812345678")
            .unwrap();
        assert_eq!(result, "138****5678");

        let result = engine
            .mask(MaskScene::Response, "password_hash", "secret")
            .unwrap();
        assert_eq!(result, "******");

        let result = engine
            .mask(MaskScene::Response, "bank_account", "6222021234567890")
            .unwrap();
        assert_eq!(result, "6222********7890");
    }

    #[test]
    fn test_full_mask_rule() {
        assert_eq!(BuiltinMaskRule::Full.apply("hello"), "*****");
        assert_eq!(BuiltinMaskRule::Full.apply("ab"), "****");
        assert_eq!(BuiltinMaskRule::Full.apply(""), "****");
    }
}
