// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 JWT Audience 安全增强集成测试
//!
//! 覆盖 P0 债务 DB-2026-09-21-01：audience 校验 + grace period 兼容
//!
//! ## 测试矩阵
//!
//! | 场景 | aud | iat | deny | 预期 |
//! |------|-----|-----|------|------|
//! | aud 匹配 | "sz300-api" | now | - | Ok |
//! | aud 不匹配 | "other-svc" | now | - | AudienceMismatch |
//! | aud 多值匹配 | ["a","sz300-api"] | now | - | Ok |
//! | 无 aud + grace 内 | 无 | now | true | Ok |
//! | 无 aud + grace 过期 + deny | 无 | 远古 | true | NoAudienceDenied |
//! | 无 aud + grace 过期 + !deny | 无 | 远古 | false | Ok |
//! | enabled=false | 任意 | - | - | Ok |
//! | 无效 token | - | - | - | InvalidToken |

#![cfg(feature = "v19-jwt-audience")]

use sz_rust_core::orm::jwt::{JwtClaims, JwtEncoder};
use sz_rust_sz300::services::auth_service::{
    self, encode_token_with_audience, verify_token_with_audience, AudienceConfig, AudienceError,
};

const TEST_SECRET: &str = "test-secret-for-v19-jwt-audience-32bytes-min!!";

fn setup_auth() {
    // init_auth_test_only 内部用 OnceLock，重复调用无副作用
    auth_service::init_auth_test_only(TEST_SECRET);
}

/// 生成不带 aud 的基础 JWT（模拟旧令牌），可指定 iat
///
/// `exp` 固定为未来时间（now + 3600），确保 token 未过期；
/// `iat` 由参数控制，用于测试 grace period 边界。
fn make_token_no_aud(iat: i64) -> String {
    setup_auth();
    let encoder = JwtEncoder::new(TEST_SECRET);
    // 手动构造 claims 以控制 iat（JwtClaims::new 会将 iat 设为当前时间）
    let mut claims = JwtClaims::new("test-user", now() + 3600);
    claims.iat = iat;
    encoder.encode(&claims).expect("encode")
}

/// 生成带 aud 的 JWT（aud 为 String 格式）
fn make_token_with_aud(aud: &str) -> String {
    setup_auth();
    let encoder = JwtEncoder::new(TEST_SECRET);
    let claims = JwtClaims::new("test-user", now() + 3600).with_user_id(1);
    encode_token_with_audience(&encoder, &claims, aud).expect("encode with aud")
}

/// 生成带 aud 的 JWT（aud 为 Vec<String> 格式，通过手动注入 JSON 数组）
fn make_token_with_aud_multi(auds: &[&str]) -> String {
    setup_auth();
    let encoder = JwtEncoder::new(TEST_SECRET);
    let claims = JwtClaims::new("test-user", now() + 3600).with_user_id(1);
    let base_token = encoder.encode(&claims).expect("encode");

    // 手动注入 aud 数组
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let parts: Vec<&str> = base_token.split('.').collect();
    let payload_bytes = URL_SAFE_NO_PAD.decode(parts[1]).expect("decode");
    let mut payload: serde_json::Value = serde_json::from_slice(&payload_bytes).expect("parse");
    if let Some(obj) = payload.as_object_mut() {
        let arr: Vec<serde_json::Value> = auds
            .iter()
            .map(|s| serde_json::Value::String(s.to_string()))
            .collect();
        obj.insert("aud".to_string(), serde_json::Value::Array(arr));
    }
    let new_payload_json = serde_json::to_string(&payload).expect("serialize");
    let new_payload_b64 = URL_SAFE_NO_PAD.encode(new_payload_json.as_bytes());

    // 重新签名
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let signing_input = format!("{}.{}", parts[0], new_payload_b64);
    let mut mac = HmacSha256::new_from_slice(TEST_SECRET.as_bytes()).expect("mac");
    mac.update(signing_input.as_bytes());
    let sig = mac.finalize().into_bytes();
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
    format!("{}.{}.{}", parts[0], new_payload_b64, sig_b64)
}

fn now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn sz300_config() -> AudienceConfig {
    AudienceConfig {
        service_id: "sz300-api".to_string(),
        deny_no_audience: true,
        grace_period_secs: 30 * 86400,
        enabled: true,
    }
}

#[test]
fn test_aud_match_string_format() {
    setup_auth();
    let token = make_token_with_aud("sz300-api");
    let config = sz300_config();
    let result = verify_token_with_audience(&token, &config);
    assert!(result.is_ok(), "aud 匹配应放行");
    let user = result.unwrap();
    assert_eq!(user.username, "test-user");
}

#[test]
fn test_aud_match_vec_format() {
    setup_auth();
    let token = make_token_with_aud_multi(&["other-svc", "sz300-api"]);
    let config = sz300_config();
    let result = verify_token_with_audience(&token, &config);
    assert!(result.is_ok(), "aud 多值包含 service_id 应放行");
}

#[test]
fn test_aud_mismatch() {
    setup_auth();
    let token = make_token_with_aud("other-service");
    let config = sz300_config();
    let result = verify_token_with_audience(&token, &config);
    assert_eq!(
        result.err(),
        Some(AudienceError::AudienceMismatch),
        "aud 不匹配应返回 AudienceMismatch"
    );
}

#[test]
fn test_aud_mismatch_vec_without_service_id() {
    setup_auth();
    let token = make_token_with_aud_multi(&["svc-a", "svc-b"]);
    let config = sz300_config();
    let result = verify_token_with_audience(&token, &config);
    assert_eq!(
        result.err(),
        Some(AudienceError::AudienceMismatch),
        "aud 多值不含 service_id 应返回 AudienceMismatch"
    );
}

#[test]
fn test_no_aud_within_grace_period() {
    setup_auth();
    // iat = now（当前时间），在 grace period 内
    let token = make_token_no_aud(now());
    let config = sz300_config();
    let result = verify_token_with_audience(&token, &config);
    assert!(
        result.is_ok(),
        "无 aud 但在 grace period 内应放行（向后兼容）"
    );
}

#[test]
fn test_no_aud_grace_expired_deny() {
    setup_auth();
    // iat = 1（远古），超出 grace period
    let token = make_token_no_aud(1);
    let mut config = sz300_config();
    config.grace_period_secs = 3600; // 1 小时 grace period
    config.deny_no_audience = true;
    let result = verify_token_with_audience(&token, &config);
    assert_eq!(
        result.err(),
        Some(AudienceError::NoAudienceDenied),
        "无 aud + grace 过期 + deny 应返回 NoAudienceDenied"
    );
}

#[test]
fn test_no_aud_grace_expired_allow() {
    setup_auth();
    // iat = 1（远古），超出 grace period，但 deny_no_audience = false
    let token = make_token_no_aud(1);
    let mut config = sz300_config();
    config.grace_period_secs = 3600;
    config.deny_no_audience = false;
    let result = verify_token_with_audience(&token, &config);
    assert!(
        result.is_ok(),
        "无 aud + grace 过期 + !deny 应放行（宽松模式）"
    );
}

#[test]
fn test_aud_disabled_skips_check() {
    setup_auth();
    // aud 不匹配，但 enabled = false → 应放行
    let token = make_token_with_aud("wrong-audience");
    let mut config = sz300_config();
    config.enabled = false;
    let result = verify_token_with_audience(&token, &config);
    assert!(result.is_ok(), "enabled=false 应跳过 audience 校验");
}

#[test]
fn test_invalid_token_returns_invalid_token_error() {
    setup_auth();
    let config = sz300_config();
    let result = verify_token_with_audience("not.a.valid.jwt", &config);
    assert_eq!(
        result.err(),
        Some(AudienceError::InvalidToken),
        "无效 token 应返回 InvalidToken"
    );
}

#[test]
fn test_empty_token_returns_invalid_token_error() {
    setup_auth();
    let config = sz300_config();
    let result = verify_token_with_audience("", &config);
    assert_eq!(
        result.err(),
        Some(AudienceError::InvalidToken),
        "空 token 应返回 InvalidToken"
    );
}

#[test]
fn test_tampered_token_signature_invalid() {
    setup_auth();
    let token = make_token_with_aud("sz300-api");
    // 篡改签名（最后 5 字符替换）
    let mut tampered = token.clone();
    let len = tampered.len();
    tampered.replace_range(len - 5.., "XXXXX");
    let config = sz300_config();
    let result = verify_token_with_audience(&tampered, &config);
    assert_eq!(
        result.err(),
        Some(AudienceError::InvalidToken),
        "签名篡改应返回 InvalidToken"
    );
}

#[test]
fn test_default_config_for_sz300() {
    let config = AudienceConfig::default_for_sz300();
    assert_eq!(config.service_id, "sz300-api");
    assert!(config.deny_no_audience);
    assert_eq!(config.grace_period_secs, 30 * 86400);
    assert!(config.enabled);
}

#[test]
fn test_audience_error_equality() {
    assert_eq!(AudienceError::InvalidToken, AudienceError::InvalidToken);
    assert_ne!(AudienceError::InvalidToken, AudienceError::AudienceMismatch);
    assert_ne!(
        AudienceError::AudienceMismatch,
        AudienceError::NoAudienceDenied
    );
}
