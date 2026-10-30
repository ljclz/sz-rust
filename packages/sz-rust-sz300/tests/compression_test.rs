// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 响应 gzip 压缩集成测试（ADR-050）
//!
//! 无需数据库：用一条与生产同构的最小服务栈验证压缩语义 ——
//! `handler → 路由级 NoCompression 标记 → 脱敏层 → 压缩层`，
//! 其中脱敏层夹在中间，用来证明 `NoCompression` 扩展能穿过它传播到压缩谓词。
//!
//! 运行：`cargo test -p sz-rust-sz300 --test compression_test`

#![forbid(unsafe_code)]

use axum::body::Body;
use axum::http::{header::ACCEPT_ENCODING, Request, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use sz_rust_sz300::middleware::compression::{compression_layer, mark_no_compress};
use tower::ServiceExt;

/// 一段可压缩的业务 JSON（重复字段值，gzip 收益明显）
fn big_json() -> String {
    let items: Vec<String> = (0..40)
        .map(|i| format!(r#"{{"sku":"SKU-COMPRESSION-TEST-{i:04}","name":"商品名称占位"}} "#))
        .collect();
    format!(
        r#"{{"code":0,"msg":"成功","data":{{"total":40,"list":[{}]}}}}"#,
        items.join(",")
    )
}

async fn json_response() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from(big_json()))
        .unwrap()
}

/// 与生产同构的服务栈：脱敏层在内、压缩层在外，令牌路由带 NoCompression 标记
fn app() -> Router {
    Router::new()
        .route("/big", get(json_response))
        .route(
            "/auth/login",
            get(json_response).layer(axum::middleware::from_fn(mark_no_compress)),
        )
        .route(
            "/small",
            get(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"code":0}"#))
                    .unwrap()
            }),
        )
        .route(
            "/events",
            get(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .body(Body::from(format!("data: {}\n\n", big_json())))
                    .unwrap()
            }),
        )
        .layer(sz_rust_data_mask::DataMaskLayer::new(
            sz_rust_sz300::config::mask_engine(),
            sz_rust_data_mask::MaskScene::Response,
        ))
        .layer(compression_layer())
}

/// 发起带 `Accept-Encoding: gzip` 的请求，返回 (响应头, 响应体字节)
async fn call(uri: &str) -> (axum::http::HeaderMap, Vec<u8>) {
    let resp = app()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(ACCEPT_ENCODING, "gzip")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes().to_vec();
    (parts.headers, bytes)
}

/// 业务 JSON 响应应被 gzip，且确实是 gzip 帧（魔数 1f 8b）
#[tokio::test]
async fn test_business_json_is_gzipped() {
    let (headers, body) = call("/big").await;
    assert_eq!(
        headers
            .get("content-encoding")
            .and_then(|v| v.to_str().ok()),
        Some("gzip"),
        "业务 JSON 响应应带 content-encoding: gzip"
    );
    assert!(
        body.starts_with(&[0x1f, 0x8b]),
        "响应体应是 gzip 数据（前 2 字节为魔数 1f 8b），实际前缀: {body:?}"
    );
    assert!(
        (body.len() as f64) < big_json().len() as f64,
        "压缩后应小于原文：{} vs {}",
        body.len(),
        big_json().len()
    );
}

/// 令牌签发路由带 NoCompression 标记，必须原样返回明文（BREACH 防线）
#[tokio::test]
async fn test_token_issuing_route_skips_compression() {
    let (headers, body) = call("/auth/login").await;
    assert_eq!(
        headers.get("content-encoding"),
        None,
        "带 NoCompression 标记的响应不应被压缩（标记需穿过脱敏层传播到压缩谓词）"
    );
    // 脱敏层会重新序列化 JSON（空白归一），故按明文内容断言而非逐字节相等
    let text = String::from_utf8(body).expect("未压缩响应体应是 UTF-8 JSON");
    assert!(
        text.contains("SKU-COMPRESSION-TEST-0039"),
        "应返回可读明文，实际: {text}"
    );
}

/// 小于 32 B 的响应不压缩（tower-http 默认谓词，省 CPU）
#[tokio::test]
async fn test_tiny_response_skips_compression() {
    let (headers, _) = call("/small").await;
    assert_eq!(
        headers.get("content-encoding"),
        None,
        "小于 32 字节的响应不值得压缩"
    );
}

/// SSE 响应不压缩（压缩会缓冲掉分块，破坏 `text/event-stream` 的即时性）
#[tokio::test]
async fn test_sse_skips_compression() {
    let (headers, _) = call("/events").await;
    assert_eq!(
        headers.get("content-encoding"),
        None,
        "text/event-stream 不应被压缩"
    );
}

/// 未声明 `Accept-Encoding: gzip` 时不压缩（压缩只在客户端支持时发生）
#[tokio::test]
async fn test_client_without_accept_encoding_gets_identity() {
    let resp = app()
        .oneshot(Request::builder().uri("/big").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("content-encoding"), None);
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    assert!(
        !body.starts_with(&[0x1f, 0x8b]),
        "无 Accept-Encoding 时不应返回 gzip 数据"
    );
    let text = String::from_utf8(body).expect("响应体应是 UTF-8 JSON");
    assert!(text.contains("SKU-COMPRESSION-TEST-0039"), "实际: {text}");
}
