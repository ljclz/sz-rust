// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 数据脱敏中间件集成测试（v1.8.0 P1-1.2）
//!
//! 验证 `v18-data-mask` feature gate 下：
//! 1. DataMaskLayer 正确脱敏 JSON 响应中的敏感字段
//! 2. 嵌套对象和数组中的字段被递归脱敏
//! 3. 非 JSON 响应不被处理
//! 4. 未注册字段保持原值
//! 5. 默认脱敏引擎包含所有常用规则

#![cfg(feature = "v18-data-mask")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sz_rust_data_mask::{BuiltinMaskRule, DataMaskLayer, MaskEngine, MaskRule, MaskScene};
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

fn make_request() -> axum::http::Request<Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri("/test")
        .body(Body::empty())
        .unwrap()
}

fn json_response(value: Value) -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        serde_json::to_string(&value).unwrap(),
    )
}

async fn parse_json_response(res: axum::http::Response<Body>) -> Value {
    let body = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

#[test]
fn test_mask_engine_from_config_has_all_rules() {
    let engine = sz_rust_sz300::config::mask_engine();
    assert_eq!(
        engine
            .mask(MaskScene::Response, "phone", "13812345678")
            .unwrap(),
        "138****5678"
    );
    assert_eq!(
        engine
            .mask(MaskScene::Response, "bank_account", "6222021234567890")
            .unwrap(),
        "6222********7890"
    );
    assert_eq!(
        engine
            .mask(MaskScene::Response, "password_hash", "secret")
            .unwrap(),
        "******"
    );
    assert_eq!(
        engine
            .mask(MaskScene::Response, "id_card", "110101199001011234")
            .unwrap(),
        "110***********1234"
    );
}

#[tokio::test]
async fn test_data_mask_layer_masks_phone() {
    let engine = Arc::new(make_engine());
    let layer = DataMaskLayer::new(engine, MaskScene::Response);
    let app: Router = Router::new()
        .route(
            "/test",
            post(|| async { json_response(json!({"name": "张三", "phone": "13812345678"})) }),
        )
        .layer(layer);

    let res = app.oneshot(make_request()).await.unwrap();
    let json = parse_json_response(res).await;
    assert_eq!(json["phone"], "138****5678");
    assert_eq!(json["name"], "张三");
}

#[tokio::test]
async fn test_data_mask_layer_masks_bank_account() {
    let engine = Arc::new(make_engine());
    let layer = DataMaskLayer::new(engine, MaskScene::Response);
    let app: Router = Router::new()
        .route(
            "/test",
            post(|| async { json_response(json!({"bank_account": "6222021234567890"})) }),
        )
        .layer(layer);

    let res = app.oneshot(make_request()).await.unwrap();
    let json = parse_json_response(res).await;
    assert_eq!(json["bank_account"], "6222********7890");
}

#[tokio::test]
async fn test_data_mask_layer_masks_password_hash() {
    let engine = Arc::new(make_engine());
    let layer = DataMaskLayer::new(engine, MaskScene::Response);
    let app: Router = Router::new()
        .route(
            "/test",
            post(|| async { json_response(json!({"password_hash": "secret_hash_value"})) }),
        )
        .layer(layer);

    let res = app.oneshot(make_request()).await.unwrap();
    let json = parse_json_response(res).await;
    assert_eq!(json["password_hash"], "*****************");
}

#[tokio::test]
async fn test_data_mask_layer_masks_id_card() {
    let engine = Arc::new(make_engine());
    let layer = DataMaskLayer::new(engine, MaskScene::Response);
    let app: Router = Router::new()
        .route(
            "/test",
            post(|| async { json_response(json!({"id_card": "110101199001011234"})) }),
        )
        .layer(layer);

    let res = app.oneshot(make_request()).await.unwrap();
    let json = parse_json_response(res).await;
    assert_eq!(json["id_card"], "110***********1234");
}

#[tokio::test]
async fn test_data_mask_layer_masks_nested_object() {
    let engine = Arc::new(make_engine());
    let layer = DataMaskLayer::new(engine, MaskScene::Response);
    let app: Router = Router::new()
        .route(
            "/test",
            post(|| async {
                json_response(
                    json!({"merchant": {"name": "李四", "phone": "13987654321"}, "id": 1}),
                )
            }),
        )
        .layer(layer);

    let res = app.oneshot(make_request()).await.unwrap();
    let json = parse_json_response(res).await;
    assert_eq!(json["merchant"]["phone"], "139****4321");
    assert_eq!(json["merchant"]["name"], "李四");
}

#[tokio::test]
async fn test_data_mask_layer_masks_array() {
    let engine = Arc::new(make_engine());
    let layer = DataMaskLayer::new(engine, MaskScene::Response);
    let app: Router = Router::new()
        .route(
            "/test",
            post(|| async {
                json_response(json!([{"phone": "13812345678"}, {"phone": "13987654321"}]))
            }),
        )
        .layer(layer);

    let res = app.oneshot(make_request()).await.unwrap();
    let json = parse_json_response(res).await;
    assert_eq!(json[0]["phone"], "138****5678");
    assert_eq!(json[1]["phone"], "139****4321");
}

#[tokio::test]
async fn test_data_mask_layer_preserves_non_json() {
    let engine = Arc::new(make_engine());
    let layer = DataMaskLayer::new(engine, MaskScene::Response);
    let app: Router = Router::new()
        .route(
            "/test",
            post(|| async {
                (
                    StatusCode::OK,
                    [("content-type", "text/plain")],
                    "hello world",
                )
            }),
        )
        .layer(layer);

    let res = app.oneshot(make_request()).await.unwrap();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"hello world");
}

#[tokio::test]
async fn test_data_mask_layer_preserves_unmasked_fields() {
    let engine = Arc::new(make_engine());
    let layer = DataMaskLayer::new(engine, MaskScene::Response);
    let app: Router = Router::new()
        .route(
            "/test",
            post(|| async {
                json_response(json!({"id": 1, "name": "测试", "status": "active"}))
            }),
        )
        .layer(layer);

    let res = app.oneshot(make_request()).await.unwrap();
    let json = parse_json_response(res).await;
    assert_eq!(json["id"], 1);
    assert_eq!(json["name"], "测试");
    assert_eq!(json["status"], "active");
}

#[tokio::test]
async fn test_data_mask_layer_masks_multiple_fields() {
    let engine = Arc::new(make_engine());
    let layer = DataMaskLayer::new(engine, MaskScene::Response);
    let app: Router = Router::new()
        .route(
            "/test",
            post(|| async {
                json_response(json!({
                    "phone": "13812345678",
                    "bank_account": "6222021234567890",
                    "password_hash": "abc123",
                    "id_card": "110101199001011234",
                    "name": "王五"
                }))
            }),
        )
        .layer(layer);

    let res = app.oneshot(make_request()).await.unwrap();
    let json = parse_json_response(res).await;
    assert_eq!(json["phone"], "138****5678");
    assert_eq!(json["bank_account"], "6222********7890");
    assert_eq!(json["password_hash"], "******");
    assert_eq!(json["id_card"], "110***********1234");
    assert_eq!(json["name"], "王五");
}

#[tokio::test]
async fn test_data_mask_layer_empty_engine_preserves_all() {
    let engine = Arc::new(MaskEngine::new());
    let layer = DataMaskLayer::new(engine, MaskScene::Response);
    let app: Router = Router::new()
        .route(
            "/test",
            post(|| async { json_response(json!({"phone": "13812345678", "name": "测试"})) }),
        )
        .layer(layer);

    let res = app.oneshot(make_request()).await.unwrap();
    let json = parse_json_response(res).await;
    assert_eq!(json["phone"], "13812345678");
    assert_eq!(json["name"], "测试");
}
