// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P4-2 API 签名验证 axum 端到端集成测试
//!
//! 验证 SignatureVerifier 在 axum Router 中的请求签名校验流程。

#![cfg(feature = "api-signature")]
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use parking_lot::Mutex;
use sz_rust_middleware_facade::api_signature::{
    compute_signature, ApiKey, SignatureConfig, SignatureVerifier, VerifyResult,
};
use tower::ServiceExt;

#[derive(Clone)]
struct AppState {
    verifier: Arc<Mutex<SignatureVerifier>>,
}

async fn signed_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let key_id = match headers.get("x-key-id").and_then(|v| v.to_str().ok()) {
        Some(k) => k.to_string(),
        None => return (StatusCode::UNAUTHORIZED, "missing key-id").into_response(),
    };
    let timestamp: u64 = match headers
        .get("x-timestamp")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
    {
        Some(t) => t,
        None => return (StatusCode::UNAUTHORIZED, "missing timestamp").into_response(),
    };
    let nonce = match headers.get("x-nonce").and_then(|v| v.to_str().ok()) {
        Some(n) => n.to_string(),
        None => return (StatusCode::UNAUTHORIZED, "missing nonce").into_response(),
    };
    let signature = match headers.get("x-signature").and_then(|v| v.to_str().ok()) {
        Some(s) => s.to_string(),
        None => return (StatusCode::UNAUTHORIZED, "missing signature").into_response(),
    };

    let mut verifier = state.verifier.lock();
    let result = verifier.verify(
        "GET",
        "/api/data",
        &key_id,
        timestamp,
        &nonce,
        &signature,
        b"",
    );

    match result {
        VerifyResult::Ok => (StatusCode::OK, "ok").into_response(),
        VerifyResult::Exempt => (StatusCode::OK, "exempt").into_response(),
        _ => (StatusCode::UNAUTHORIZED, format!("{:?}", result)).into_response(),
    }
}

fn make_verifier() -> SignatureVerifier {
    SignatureVerifier::new(SignatureConfig {
        time_window: Duration::from_secs(300),
        exempt_paths: vec!["/health".to_string()],
        keys: vec![ApiKey::new("test_key", "test_secret")],
    })
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn signed_request(
    uri: &str,
    key_id: &str,
    secret: &str,
    timestamp: u64,
    nonce: &str,
) -> Request<Body> {
    let sig = compute_signature("GET", uri, timestamp, nonce, b"", secret);
    Request::builder()
        .uri(uri)
        .header("x-key-id", key_id)
        .header("x-timestamp", timestamp.to_string())
        .header("x-nonce", nonce)
        .header("x-signature", sig)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn e2e_signature_valid_passes() {
    let app = Router::new()
        .route("/api/data", get(signed_handler))
        .with_state(AppState {
            verifier: Arc::new(Mutex::new(make_verifier())),
        });

    let ts = now_ts();
    let resp = app
        .oneshot(signed_request(
            "/api/data",
            "test_key",
            "test_secret",
            ts,
            "nonce1",
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn e2e_signature_invalid_rejected() {
    let app = Router::new()
        .route("/api/data", get(signed_handler))
        .with_state(AppState {
            verifier: Arc::new(Mutex::new(make_verifier())),
        });

    let ts = now_ts();
    let req = Request::builder()
        .uri("/api/data")
        .header("x-key-id", "test_key")
        .header("x-timestamp", ts.to_string())
        .header("x-nonce", "nonce2")
        .header("x-signature", "invalid_signature")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn e2e_signature_missing_headers_rejected() {
    let app = Router::new()
        .route("/api/data", get(signed_handler))
        .with_state(AppState {
            verifier: Arc::new(Mutex::new(make_verifier())),
        });

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/data")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn e2e_signature_expired_timestamp_rejected() {
    let app = Router::new()
        .route("/api/data", get(signed_handler))
        .with_state(AppState {
            verifier: Arc::new(Mutex::new(make_verifier())),
        });

    let old_ts = now_ts() - 600;
    let resp = app
        .oneshot(signed_request(
            "/api/data",
            "test_key",
            "test_secret",
            old_ts,
            "nonce3",
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn e2e_signature_nonce_replay_rejected() {
    let app = Router::new()
        .route("/api/data", get(signed_handler))
        .with_state(AppState {
            verifier: Arc::new(Mutex::new(make_verifier())),
        });

    let ts = now_ts();
    let req1 = signed_request("/api/data", "test_key", "test_secret", ts, "replay_nonce");
    let req2 = signed_request("/api/data", "test_key", "test_secret", ts, "replay_nonce");

    let resp1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK, "第一次应通过");

    let resp2 = app.oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::UNAUTHORIZED, "重放应拒绝");
}

#[tokio::test]
async fn e2e_signature_unknown_key_rejected() {
    let app = Router::new()
        .route("/api/data", get(signed_handler))
        .with_state(AppState {
            verifier: Arc::new(Mutex::new(make_verifier())),
        });

    let ts = now_ts();
    let resp = app
        .oneshot(signed_request(
            "/api/data",
            "unknown_key",
            "secret",
            ts,
            "nonce4",
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
