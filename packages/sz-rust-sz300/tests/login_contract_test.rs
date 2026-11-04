// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 登录端点 HTTP 契约测试（F-2 修复回归，2026-11-05，doc-debt DB-2026-11-05-02）
//!
//! 验证 `POST /api/v1/auth/login` 的状态码契约：
//! 1. 凭据经 query string 提交 → 400（禁止 URL 泄密面）
//! 2. 参数缺失 → 400
//! 3. 错误凭据（body 提交）→ 401 + 失败信封 `code:0`
//!
//! 无成功路径用例：仓库无已知口令哈希夹具，成功路径由
//! `authenticate_async` 的服务层测试覆盖。
//!
//! 需要 MySQL（make_state 构造 AppState 依赖连接池）；不可达时软跳过。

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

async fn ensure_mysql() -> Option<sz_rust_core::orm::Pool> {
    match common::ensure_mysql().await {
        Some(pool) => {
            // login → authenticate_async 依赖全局 auth 静态（JWT 编码器 + DB 池），
            // 与 auth_ctx_cache_test 同模式：进程内一次性初始化
            static INIT: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
            INIT.get_or_init(|| async {
                sz_rust_sz300::services::auth_service::init_auth(
                    "test-secret-key",
                    "sz300-test",
                    86400,
                    pool.clone().into(),
                );
            })
            .await;
            Some(pool)
        }
        None => None,
    }
}

async fn post_login(app: axum::Router, uri: &str, body: &str) -> (StatusCode, String) {
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn test_login_rejects_credentials_in_query_string() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => {
            eprintln!("⚠️ MySQL 不可达，跳过 query 凭据拒绝测试");
            return;
        }
    };
    let app = common::make_router(common::make_state(pool));
    let (status, body) = post_login(app, "/api/v1/auth/login?username=u&password=p", "{}").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际: {status} {body}");
    assert!(body.contains("不得经 URL"), "实际: {body}");
}

#[tokio::test]
async fn test_login_missing_params_returns_400() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => {
            eprintln!("⚠️ MySQL 不可达，跳过参数缺失测试");
            return;
        }
    };
    let app = common::make_router(common::make_state(pool));
    let (status, body) = post_login(app, "/api/v1/auth/login", "{}").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际: {status} {body}");
    assert!(body.contains("不能为空"), "实际: {body}");
}

#[tokio::test]
async fn test_login_wrong_credentials_returns_401_envelope() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => {
            eprintln!("⚠️ MySQL 不可达，跳过错误凭据测试");
            return;
        }
    };
    let app = common::make_router(common::make_state(pool));
    let (status, body) = post_login(
        app,
        "/api/v1/auth/login",
        r#"{"username":"nobody-here","password":"wrong-pass"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "实际: {status} {body}");
    assert!(
        body.contains(r#""code":0"#),
        "失败信封 code:0，实际: {body}"
    );
    assert!(body.contains("用户名或密码错误"), "实际: {body}");
    assert!(
        !body.contains("token"),
        "失败响应不得包含令牌，实际: {body}"
    );
}
