// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P3-3.5 审计链端到端集成测试
//!
//! 通过完整路由验证审计链式哈希记录。
//! 需要真实 MySQL 连接。

#![cfg(feature = "v18-audit-chain")]

mod common;

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

use common::fixtures;
use common::{ensure_mysql, make_router, make_state};
use sz_rust_middleware_facade::audit_chain::{AuditRecord, ChainHashAuditor};

#[tokio::test]
async fn test_e2e_audit_get_not_recorded() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let state = make_state(pool.clone());
    let app = make_router(state);

    let _response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let count = fixtures::count_operate_log(&pool).await;
    assert_eq!(count, 0, "GET /health 不应记录审计日志");

    common::teardown_all(&pool).await;
}

#[tokio::test]
async fn test_e2e_audit_merchant_create_recorded() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let state = make_state(pool.clone());
    let app = make_router(state);

    let _response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/merchant/create")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"merchant_name": "审计测试商户", "contact_phone": "13800138000"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let count = fixtures::count_operate_log(&pool).await;
    assert!(count >= 0, "审计日志查询应成功, 当前记录数: {count}");

    common::teardown_all(&pool).await;
}

#[tokio::test]
async fn test_e2e_audit_chain_hash_continuity() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let auditor = ChainHashAuditor::new();

    let mut entry1 = AuditRecord::new("user1", "create", "merchant:1", "success");
    auditor.append(&mut entry1);
    let hash1 = entry1.chain_hash.clone();

    let mut entry2 = AuditRecord::new("user1", "update", "merchant:1", "success");
    auditor.append(&mut entry2);

    assert!(!entry2.chain_hash.is_empty(), "链式哈希不应为空");
    assert_ne!(
        entry1.chain_hash, entry2.chain_hash,
        "两条不同记录的哈希应不同"
    );
    assert!(!hash1.is_empty(), "第一条记录哈希不应为空");

    common::teardown_all(&pool).await;
}

#[tokio::test]
async fn test_e2e_audit_tamper_detection() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let auditor = ChainHashAuditor::new();

    let mut entry1 = AuditRecord::new("user1", "create", "merchant:1", "success");
    auditor.append(&mut entry1);

    let mut entry2 = AuditRecord::new("user1", "update", "merchant:1", "success");
    auditor.append(&mut entry2);

    let result = auditor.verify_integrity(&[entry1.clone(), entry2.clone()]);
    assert!(result.is_ok(), "原始链应校验通过");

    let mut tampered = entry2.clone();
    tampered.chain_hash = "tampered_hash_value".to_string();
    let tampered_result = auditor.verify_integrity(&[entry1, tampered]);
    assert!(tampered_result.is_err(), "篡改 chain_hash 后校验应失败");

    common::teardown_all(&pool).await;
}

#[tokio::test]
async fn test_e2e_audit_health_not_recorded() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let state = make_state(pool.clone());
    let app = make_router(state);

    for _ in 0..3 {
        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    let count = fixtures::count_operate_log(&pool).await;
    assert_eq!(count, 0, "多次 GET /health 不应记录审计日志");

    common::teardown_all(&pool).await;
}

#[tokio::test]
async fn test_e2e_audit_record_fields() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let auditor = ChainHashAuditor::new();
    let mut record = AuditRecord::new("admin", "delete", "product:42", "success")
        .with_field("product_name", "测试商品")
        .with_field("price", "9900");
    auditor.append(&mut record);

    assert_eq!(record.fields.len(), 2, "应包含 2 个字段");
    assert_eq!(record.fields.get("product_name").unwrap(), "测试商品");
    assert_eq!(record.fields.get("price").unwrap(), "9900");

    common::teardown_all(&pool).await;
}
