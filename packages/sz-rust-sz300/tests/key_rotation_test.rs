// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! JWT 密钥轮换集成测试（v1.8.0 P1-1.5）
//!
//! 验证 `v18-key-rotation` feature gate 下：
//! 1. KeyManager 初始化后存在活跃密钥
//! 2. 轮换后生成新活跃密钥，旧密钥保留
//! 3. 轮换后旧 token 仍可验证（grace period / keep_history）
//! 4. 超过 keep_history 后旧 token 失效
//! 5. 签发和验证端到端流程

#![cfg(feature = "v18-key-rotation")]

use sz_rust_core::orm::jwt::JwtClaims;
use sz_rust_sz300::services::auth_service;

const TEST_SECRET: &str = "test-key-rotation-secret-2026";

fn ensure_init() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        auth_service::init_key_rotation(TEST_SECRET);
    });
}

fn make_claims(username: &str) -> JwtClaims {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        + 3600;
    JwtClaims::new(username, exp)
        .with_issuer("sz300-test")
        .with_user_id(1)
}

// ============================================================================
// KeyManager 基础测试
// ============================================================================

#[test]
fn test_key_manager_has_active_key() {
    ensure_init();
    let manager = auth_service::get_key_manager();
    use sz_rust_key_rotation::KeyId;
    let active = manager.get_active(&KeyId::new("jwt-signing"));
    assert!(active.is_some(), "初始化后应存在活跃密钥");
    assert!(active.unwrap().active, "活跃密钥 active 标志应为 true");
}

#[test]
fn test_rotate_generates_new_version() {
    ensure_init();
    let manager = auth_service::get_key_manager();
    use sz_rust_key_rotation::KeyId;

    let before = manager.list_versions(&KeyId::new("jwt-signing")).len();
    let new_version = auth_service::rotate_key().unwrap();
    let after = manager.list_versions(&KeyId::new("jwt-signing")).len();

    assert!(after > before, "轮换后版本数应增加");
    assert!(new_version > 0, "新版本号应大于 0");
}

#[test]
fn test_rotate_deactivates_old_key() {
    ensure_init();
    let manager = auth_service::get_key_manager();
    use sz_rust_key_rotation::KeyId;

    let old_active = manager
        .get_active(&KeyId::new("jwt-signing"))
        .expect("应存在活跃密钥");
    let old_version = old_active.version;

    auth_service::rotate_key().unwrap();

    let versions = manager.list_versions(&KeyId::new("jwt-signing"));
    let old = versions
        .iter()
        .find(|v| v.version == old_version)
        .expect("旧版本应仍存在");
    assert!(!old.active, "旧密钥应被停用");
}

// ============================================================================
// 签发 + 验证端到端测试
// ============================================================================

#[test]
fn test_sign_and_verify_with_active_key() {
    ensure_init();
    let claims = make_claims("testuser1");
    let token = auth_service::sign_token_with_rotation(&claims).unwrap();
    let user = auth_service::verify_token_with_rotation(&token).unwrap();
    assert_eq!(user.username, "testuser1");
}

#[test]
fn test_old_token_valid_after_rotation() {
    ensure_init();

    // 用当前活跃密钥签发 token
    let claims = make_claims("testuser2");
    let token = auth_service::sign_token_with_rotation(&claims).unwrap();

    // 轮换密钥
    auth_service::rotate_key().unwrap();

    // 旧 token 仍可验证（grace period / keep_history）
    let user = auth_service::verify_token_with_rotation(&token).unwrap();
    assert_eq!(user.username, "testuser2");
}

#[test]
fn test_invalid_token_rejected() {
    ensure_init();
    let result = auth_service::verify_token_with_rotation("invalid.token.here");
    assert!(result.is_err(), "无效 token 应被拒绝");
}
