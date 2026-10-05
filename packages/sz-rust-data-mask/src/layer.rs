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

/// 响应体脱敏上限（超过则跳过脱敏，防止内存放大 / DoS）
const MAX_MASK_BODY: usize = 1024 * 1024;
/// JSON 递归脱敏最大深度（防止深层嵌套导致栈溢出）
const MAX_MASK_DEPTH: usize = 32;

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

    // 根据 Content-Length 提前短路，避免对超大响应做无谓的整包读取（内存放大 / DoS）
    if let Some(len) = parts
        .headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
    {
        if len > MAX_MASK_BODY {
            tracing::warn!(
                len,
                "响应体超过脱敏上限（{} 字节），跳过脱敏",
                MAX_MASK_BODY
            );
            return Response::from_parts(parts, body);
        }
    }

    // 读取 body bytes；失败时返回 502，绝不返回与 Content-Length 不一致的空 body
    let bytes = match axum::body::to_bytes(body, MAX_MASK_BODY).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "读取待脱敏响应体失败");
            let mut resp = Response::new(Body::empty());
            *resp.status_mut() = http::StatusCode::BAD_GATEWAY;
            return resp;
        }
    };

    // 解析 JSON；解析失败则原样返回
    let mut json: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return Response::from_parts(parts, Body::from(bytes)),
    };

    // 递归脱敏
    mask_json_value(&mut json, engine, scene, 0);

    // 重新序列化；序列化失败时原样返回（不掩盖原始响应）
    let new_body = match serde_json::to_vec(&json) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "脱敏后 JSON 序列化失败，返回原始响应");
            return Response::from_parts(parts, Body::from(bytes));
        }
    };

    // 更新 Content-Length 并清除 ETag（脱敏后 body 长度已变化）
    let mut parts = parts;
    parts.headers.remove(http::header::CONTENT_LENGTH);
    if let Ok(v) = HeaderValue::from_str(&new_body.len().to_string()) {
        parts.headers.insert(http::header::CONTENT_LENGTH, v);
    }
    parts.headers.remove(http::header::ETAG);
    Response::from_parts(parts, Body::from(new_body))
}

/// 递归遍历 JSON value，对匹配字段名应用脱敏
///
/// `depth` 从 0 开始；超过 `MAX_MASK_DEPTH` 时停止下钻，防止深层嵌套 JSON 触发栈溢出。
/// `engine.mask` 返回 `Err` 表示规则执行失败（如自定义函数缺失/出错），此时按 fail-safe
/// 原则将字段替换为 `***`，避免敏感数据以明文返回。
fn mask_json_value(value: &mut Value, engine: &MaskEngine, scene: MaskScene, depth: usize) {
    if depth > MAX_MASK_DEPTH {
        tracing::warn!(
            depth,
            "脱敏递归超过深度上限（{}），跳过更深层字段",
            MAX_MASK_DEPTH
        );
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if let Value::String(s) = val {
                    match engine.mask(scene, key, s) {
                        Ok(masked) => *s = masked,
                        Err(e) => {
                            tracing::warn!(field = %key, error = %e, "脱敏规则执行失败，按 fail-safe 替换为 ***");
                            *s = "***".to_string();
                        }
                    }
                }
                mask_json_value(val, engine, scene, depth + 1);
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                mask_json_value(item, engine, scene, depth + 1);
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
    use crate::error::MaskError;
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

    #[tokio::test]
    async fn test_layer_updates_content_length_after_masking() {
        let engine = Arc::new(make_engine());
        let body = r#"{"phone":"13812345678"}"#;
        let resp = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .header(http::header::CONTENT_LENGTH, body.len().to_string())
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = apply_mask_to_response(resp, &engine, MaskScene::Response).await;
        let (parts, body) = resp.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes();
        let cl = parts
            .headers
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.parse::<usize>().unwrap())
            .unwrap();
        // Content-Length 必须与脱敏后的实际 body 长度一致
        assert_eq!(cl, bytes.len());
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["phone"], "138****5678");
    }

    #[tokio::test]
    async fn test_layer_skips_body_over_mask_limit() {
        let engine = Arc::new(make_engine());
        // 构造 > 1MB 的 JSON（含敏感字段）；Content-Length 超限时应跳过脱敏并原样返回
        let large = format!(
            r#"{{"phone":"13812345678","padding":"{}"}}"#,
            "x".repeat(1024 * 1024)
        );
        let resp = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .header(http::header::CONTENT_LENGTH, large.len().to_string())
            .body(Body::from(large.clone()))
            .unwrap();
        let resp = apply_mask_to_response(resp, &engine, MaskScene::Response).await;
        let (parts, body) = resp.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes();
        let cl = parts
            .headers
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .unwrap();
        assert_eq!(cl, large.len().to_string());
        assert_eq!(String::from_utf8_lossy(&bytes), large);
    }

    #[test]
    fn test_mask_depth_limit_prevents_overflow() {
        let engine = make_engine();
        // 构造深度 40 的嵌套对象，底部放手机号；不应 panic（栈溢出防护）
        let mut deep: Value = serde_json::from_str(r#"{"phone":"13812345678"}"#).unwrap();
        for _ in 0..40 {
            deep = serde_json::json!({"nested": deep});
        }
        mask_json_value(&mut deep, &engine, MaskScene::Response, 0);
        // 深度超过上限的字段不被脱敏
        let mut cursor = &deep;
        for _ in 0..40 {
            cursor = &cursor["nested"];
        }
        assert_eq!(cursor["phone"], "13812345678");

        // 浅层字段正常脱敏
        let mut shallow: Value = serde_json::from_str(r#"{"phone":"13812345678"}"#).unwrap();
        mask_json_value(&mut shallow, &engine, MaskScene::Response, 0);
        assert_eq!(shallow["phone"], "138****5678");
    }

    #[test]
    fn test_mask_rule_failure_redacts_to_asterisks() {
        let mut engine = make_engine();
        engine.register(MaskScene::Response, MaskRule::custom("secret", "boom_fn"));
        engine.register_custom(
            "boom_fn",
            Arc::new(|_: &str| Err(MaskError::Internal("boom".into()))),
        );
        let mut v: Value = serde_json::json!({
            "secret": "plaintext",
            "phone": "13812345678",
        });
        mask_json_value(&mut v, &engine, MaskScene::Response, 0);
        // 规则执行失败 → fail-safe 替换为 ***，而非明文返回
        assert_eq!(v["secret"], "***");
        assert_eq!(v["phone"], "138****5678");
    }
}
