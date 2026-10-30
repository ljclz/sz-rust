// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 授权上下文缓存集成测试（ADR-049）
//!
//! 验证：
//! 1. 首次读取回源查库（misses +1），第二次命中缓存（hits +1、misses 不变）
//! 2. 命中缓存时确实不查库：DB 行删除后仍返回原上下文（陈旧窗口 = TTL 60 s）
//! 3. `role == 2`（操作员）不被判为管理员
//! 4. 用户不存在时返回 None 且**不缓存负结果**（连续两次调用各记一次 miss）
//!
//! 四个场景必须在**同一个 `#[tokio::test]` 运行时**内执行：`auth_service::DB_POOL` 是进程级
//! `OnceLock`，首个用例结束后其创建时的 Tokio 运行时会关闭，池内连接随之失效；拆成两个
//! `#[tokio::test]` 时第二个用例会拿到死池并误判为「用户不存在」。
//!
//! 运行前确保 MySQL 运行于 127.0.0.1:3306，root/$SZ_TEST_DB_PASSWORD，sz_orm_test 数据库存在。
//! 手动运行：`cargo test -p sz-rust-sz300 --test auth_ctx_cache_test -- --ignored`

#![forbid(unsafe_code)]

use std::sync::Arc;

use sz_rust_core::orm::{Pool, Value};
use sz_rust_sz300::services::auth_service::{self, AuthContext};

mod common;

/// 插入商户用户（列对齐生产 DDL：merchant_id NOT NULL、role 1 管理员 / 2 操作员）
async fn insert_user(pool: &Pool, user_id: i64, merchant_id: i64, username: &str, role: i64) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute_with_params(
        "INSERT INTO merchant_user (user_id, merchant_id, username, password_hash, role, status) \
         VALUES (?, ?, ?, 'x-not-a-real-hash', ?, 1)",
        &[
            Value::I64(user_id),
            Value::I64(merchant_id),
            Value::String(username.to_string()),
            Value::I64(role),
        ],
    )
    .await
    .expect("插入商户用户失败");
}

async fn delete_user(pool: &Pool, user_id: i64) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute_with_params(
        "DELETE FROM merchant_user WHERE user_id = ?",
        &[Value::I64(user_id)],
    )
    .await
    .expect("删除商户用户失败");
}

/// 取连接池并写入认证模块的进程级静态（`auth_context` 只认这个池）
///
/// 池不在用例结束时 `close_all`：`DB_POOL` 是 `OnceLock`，两个用例共用同一进程内先写入的那个池，
/// 关闭它会让后跑用例的「用户不存在」与「池已死」不可区分。
async fn init_auth_pool(pool: Pool) -> Arc<Pool> {
    let pool = Arc::new(pool);
    auth_service::init_auth("test-secret-key", "sz300-test", 86400, pool.clone());
    pool
}

#[tokio::test]
#[ignore = "requires real MySQL"]
async fn test_auth_context_cache_behavior() {
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let pool = init_auth_pool(pool).await;
    common::setup_all_tables(&pool).await;

    // ---- 场景 1/2/3：首次回源、跨 user 隔离、role 语义 ----
    insert_user(&pool, 7101, 8801, "auth-ctx-admin", 1).await;
    insert_user(&pool, 7102, 8802, "auth-ctx-operator", 2).await;

    let stats = auth_service::auth_ctx_stats();
    let (m0, h0) = (stats.misses(), stats.hits());

    let admin = auth_service::auth_context(7101)
        .await
        .expect("管理员上下文应存在");
    assert_eq!(
        admin,
        AuthContext {
            merchant_id: Some(8801),
            is_admin: true
        }
    );
    assert_eq!(stats.misses(), m0 + 1, "首次读取应只有一次 miss");
    assert_eq!(stats.hits(), h0, "首次读取不应有 hit");

    // 操作员：role=2 不是管理员（回归旧实现 `role == 1` 语义）
    let operator = auth_service::auth_context(7102)
        .await
        .expect("操作员上下文应存在");
    assert_eq!(
        operator,
        AuthContext {
            merchant_id: Some(8802),
            is_admin: false
        }
    );
    assert_eq!(stats.misses(), m0 + 2, "不同 user_id 各自回源");

    // 第二次：命中缓存，计数应显示 hits+1 / misses 不变
    let again = auth_service::auth_context(7101)
        .await
        .expect("命中缓存后仍应返回上下文");
    assert_eq!(again, admin, "缓存值应与首次查库结果一致");
    assert_eq!(stats.hits(), h0 + 1, "第二次读取应命中缓存");
    assert_eq!(stats.misses(), m0 + 2, "命中不应再记 miss");

    // 删除 DB 行后仍返回缓存值 —— 直接证明命中路径没有查库
    delete_user(&pool, 7101).await;
    let after_delete = auth_service::auth_context(7101)
        .await
        .expect("缓存窗口内应仍返回原上下文");
    assert_eq!(
        after_delete, admin,
        "TTL 窗口内不应回源，DB 行删除不该影响结果"
    );
    assert_eq!(stats.misses(), m0 + 2, "删除后命中缓存，不应新增 miss");

    // ---- 场景 4：负结果不缓存（带可达性对照）----
    insert_user(&pool, 7103, 8803, "auth-ctx-reachability", 1).await;

    let reachable = auth_service::auth_context(7103)
        .await
        .expect("已存在的用户应查到上下文");
    assert_eq!(
        reachable,
        AuthContext {
            merchant_id: Some(8803),
            is_admin: true
        }
    );
    let (m1, h1) = (stats.misses(), stats.hits());

    assert!(auth_service::auth_context(799_999).await.is_none());
    assert!(auth_service::auth_context(799_999).await.is_none());

    assert_eq!(
        stats.misses(),
        m1 + 2,
        "负结果不该被缓存：两次调用应各回源一次"
    );
    assert_eq!(stats.hits(), h1, "两次都应记 miss 而非 hit");

    common::teardown_all(&pool).await;
}
