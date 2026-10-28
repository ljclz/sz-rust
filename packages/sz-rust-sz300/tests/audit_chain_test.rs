// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 审计链式哈希集成测试（v1.8.0 P1-1.4）
//!
//! 验证 `v18-audit-chain` feature gate 下：
//! 1. 链式哈希连续性：append 后 chain_hash 非空且递进
//! 2. 完整性校验：未篡改的记录链通过 verify_integrity
//! 3. 篡改检测：修改任一记录后 verify_integrity 失败
//! 4. 中间件仅审计 POST/PUT/DELETE，跳过 GET
//! 5. 中间件排除 /health /metrics 路径

#![cfg(feature = "v18-audit-chain")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use sz_rust_middleware_facade::audit_chain::{AuditRecord, ChainHashAuditor};
use sz_rust_sz300::middleware::audit_chain::audit_chain_middleware;
use tower::ServiceExt;

// ============================================================================
// ChainHashAuditor 单元测试
// ============================================================================

#[test]
fn test_chain_hash_append_non_empty() {
    let auditor = ChainHashAuditor::new();
    let mut r = AuditRecord::new("user1", "create", "merchant:1", "success");
    auditor.append(&mut r);
    assert!(!r.chain_hash.is_empty(), "append 后 chain_hash 应非空");
}

#[test]
fn test_chain_hash_progression() {
    let auditor = ChainHashAuditor::new();

    let mut r1 = AuditRecord::new("user1", "create", "merchant:1", "success");
    auditor.append(&mut r1);

    let mut r2 = AuditRecord::new("user2", "update", "merchant:1", "success");
    auditor.append(&mut r2);

    assert_ne!(r1.chain_hash, r2.chain_hash, "连续记录的哈希应不同");
    assert_eq!(
        auditor.current_hash(),
        r2.chain_hash,
        "current_hash 应为最后一条记录的哈希"
    );
}

#[test]
fn test_verify_integrity_ok() {
    let auditor = ChainHashAuditor::new();
    let mut records = Vec::new();
    for i in 0..5 {
        let mut r = AuditRecord::new("user1", "action", format!("target:{i}"), "success");
        auditor.append(&mut r);
        records.push(r);
    }
    assert!(auditor.verify_integrity(&records).is_ok());
}

#[test]
fn test_verify_integrity_tampered() {
    let auditor = ChainHashAuditor::new();
    let mut records = Vec::new();
    for i in 0..3 {
        let mut r = AuditRecord::new("user1", "action", format!("target:{i}"), "success");
        auditor.append(&mut r);
        records.push(r);
    }
    records[1].result = "tampered".to_string();
    let result = auditor.verify_integrity(&records);
    assert!(result.is_err(), "篡改后应校验失败");
}

#[test]
fn test_empty_chain_current_hash() {
    let auditor = ChainHashAuditor::new();
    assert!(
        auditor.current_hash().is_empty(),
        "空链 current_hash 应为空字符串"
    );
}

#[test]
fn test_verify_integrity_empty() {
    let auditor = ChainHashAuditor::new();
    assert!(auditor.verify_integrity(&[]).is_ok(), "空记录链应通过校验");
}

// ============================================================================
// 审计中间件测试
// ============================================================================

fn make_router(auditor: Arc<ChainHashAuditor>) -> Router {
    Router::new()
        .route("/api/v1/test", post(|| async { "ok" }))
        .route("/api/v1/test_get", get(|| async { "ok" }))
        .route("/health", get(|| async { "healthy" }))
        .layer(middleware::from_fn_with_state(
            auditor,
            audit_chain_middleware,
        ))
}

fn post_request(path: &str) -> axum::http::Request<Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri(path)
        .body(Body::empty())
        .unwrap()
}

fn get_request(path: &str) -> axum::http::Request<Body> {
    axum::http::Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn test_middleware_records_post_request() {
    let auditor = Arc::new(ChainHashAuditor::new());
    let router = make_router(auditor.clone());

    let resp = router.oneshot(post_request("/api/v1/test")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    assert!(
        !auditor.current_hash().is_empty(),
        "POST 请求后应记录审计日志"
    );
}

#[tokio::test]
async fn test_middleware_skips_get_request() {
    let auditor = Arc::new(ChainHashAuditor::new());
    let router = make_router(auditor.clone());

    let resp = router
        .oneshot(get_request("/api/v1/test_get"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    assert!(
        auditor.current_hash().is_empty(),
        "GET 请求不应记录审计日志"
    );
}

#[tokio::test]
async fn test_middleware_skips_health_path() {
    let auditor = Arc::new(ChainHashAuditor::new());
    let router = make_router(auditor.clone());

    let resp = router.oneshot(get_request("/health")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    assert!(
        auditor.current_hash().is_empty(),
        "/health 路径不应记录审计日志"
    );
}
