use std::sync::Arc;
use std::time::Duration;
use sz_rust_mvc_facade::controller::{KeyRotation, KeyRotationError};
use sz_rust_orm_facade::jwt::{JwtClaims, JwtEncoder};

fn make_claims(user_id: i64) -> JwtClaims {
    JwtClaims::new("test_sub", 4_000_000_000_i64).with_user_id(user_id)
}

fn make_key_rotation(secret: &str) -> KeyRotation {
    KeyRotation::new(
        secret.to_string(),
        Duration::from_secs(86400),
        Duration::from_secs(3600),
        3,
    )
}

#[test]
fn test_key_rotation_from_env_missing_secret() {
    std::env::remove_var("SZ300_JWT_SECRET");
    let kr = KeyRotation::from_env();
    assert!(matches!(kr, Err(KeyRotationError::SecretMissing)));
}

#[test]
fn test_key_rotation_from_env_empty_secret() {
    std::env::set_var("SZ300_JWT_SECRET", "");
    let kr = KeyRotation::from_env();
    assert!(matches!(kr, Err(KeyRotationError::SecretMissing)));
    std::env::remove_var("SZ300_JWT_SECRET");
}

#[test]
fn test_sign_and_verify_with_current_key() {
    let kr = make_key_rotation("current-secret-key");
    let claims = make_claims(42);
    let token = kr.sign_token(&claims).unwrap();
    let verified = kr.verify_token(&token).unwrap();
    assert_eq!(verified.user_id, Some(42));
}

#[test]
fn test_verify_with_wrong_key_fails() {
    let kr = make_key_rotation("key-a");

    let encoder = JwtEncoder::new("key-b");
    let claims = make_claims(99);
    let token = encoder.encode(&claims).unwrap();

    let result = kr.verify_token(&token);
    assert!(matches!(result, Err(KeyRotationError::InvalidToken)));
}

#[test]
fn test_debug_output_redacts_keys() {
    let kr = make_key_rotation("super-secret-not-in-debug");
    let debug_output = format!("{:?}", kr);
    assert!(
        !debug_output.contains("super-secret-not-in-debug"),
        "Debug should not contain key: {debug_output}"
    );
    assert!(debug_output.contains("[REDACTED]"));
}

#[test]
fn test_fingerprint_does_not_contain_key() {
    let key = "my-secret-key-12345";
    let fp = KeyRotation::fingerprint(key);
    assert!(
        !fp.contains(key),
        "fingerprint should not contain key: {fp}"
    );
    assert_eq!(fp.len(), 8, "fingerprint should be 8 hex chars: {fp}");
}

#[test]
fn test_fingerprint_is_deterministic() {
    let key = "same-key";
    let fp1 = KeyRotation::fingerprint(key);
    let fp2 = KeyRotation::fingerprint(key);
    assert_eq!(fp1, fp2);
}

#[test]
fn test_fingerprint_differs_for_different_keys() {
    let fp1 = KeyRotation::fingerprint("key-one");
    let fp2 = KeyRotation::fingerprint("key-two");
    assert_ne!(fp1, fp2);
}

#[test]
fn test_current_fingerprint_not_empty() {
    let kr = make_key_rotation("test-key-for-fp");
    let fp = kr.current_fingerprint();
    assert!(!fp.is_empty());
    assert_eq!(fp.len(), 8);
}

#[tokio::test]
async fn test_rotation_moves_old_key_to_previous() {
    let kr = Arc::new(make_key_rotation("original-key"));

    let claims = make_claims(100);
    let token = kr.sign_token(&claims).unwrap();

    KeyRotation::do_rotation(&kr, Duration::from_secs(3600), 3)
        .await
        .unwrap();

    let verified = kr.verify_token(&token).unwrap();
    assert_eq!(verified.user_id, Some(100));
}

#[tokio::test]
async fn test_rotation_signs_with_new_key() {
    let kr = Arc::new(make_key_rotation("before-rotation"));

    let fp_before = kr.current_fingerprint();

    KeyRotation::do_rotation(&kr, Duration::from_secs(3600), 3)
        .await
        .unwrap();

    let fp_after = kr.current_fingerprint();
    assert_ne!(
        fp_before, fp_after,
        "fingerprint should change after rotation"
    );

    let claims = make_claims(200);
    let token = kr.sign_token(&claims).unwrap();
    let verified = kr.verify_token(&token).unwrap();
    assert_eq!(verified.user_id, Some(200));
}

#[tokio::test]
async fn test_multiple_rotations_keep_max_previous() {
    let kr = Arc::new(make_key_rotation("key-0"));

    // 签发 token with key-0
    let claims = make_claims(1);
    let token_0 = kr.sign_token(&claims).unwrap();

    // 3 次轮换
    for _ in 0..3 {
        KeyRotation::do_rotation(&kr, Duration::from_secs(3600), 3)
            .await
            .unwrap();
    }

    // token_0 仍应验证通过（key-0 在 previous 中，grace period 内）
    let verified = kr.verify_token(&token_0).unwrap();
    assert_eq!(verified.user_id, Some(1));
}
