// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Redis 存储后端集成测试
//!
//! 需要真实 Redis（通过 SSH 隧道连接服务器 Redis）。
//! 默认连接 `redis://127.0.0.1:16379`（SSH 隧道端口）。

#![cfg(feature = "redis-store")]

use std::sync::Arc;
use sz_rust_auth_facade::redis_store::{create_redis_stores, RedisConfig};
use sz_rust_auth_facade::refresh::{
    RefreshTokenConfig, RefreshTokenError, RefreshTokenIssuer, RefreshTokenRevoker,
    RefreshTokenStore, RefreshTokenVerifier, SsoJwtCodec, TokenBlacklist,
};

const REDIS_URL: &str = "redis://127.0.0.1:16379";

async fn make_redis_stores() -> (Arc<dyn RefreshTokenStore>, Arc<dyn TokenBlacklist>) {
    let config = RedisConfig::from_url(REDIS_URL);
    create_redis_stores(config).await.unwrap()
}

async fn cleanup_redis() {
    let client = redis::Client::open(REDIS_URL).unwrap();
    let mut conn = client.get_connection_manager().await.unwrap();
    redis::cmd("FLUSHDB").exec_async(&mut conn).await.unwrap();
}

#[tokio::test]
async fn redis_store_get_version_default_zero() {
    cleanup_redis().await;
    let (store, _) = make_redis_stores().await;

    // 新用户版本号应为 0
    assert_eq!(store.get_version(1).await.unwrap(), 0);
    assert_eq!(store.get_version(999).await.unwrap(), 0);
}

#[tokio::test]
async fn redis_store_increment_version_atomic() {
    cleanup_redis().await;
    let (store, _) = make_redis_stores().await;

    assert_eq!(store.increment_version(1).await.unwrap(), 1);
    assert_eq!(store.increment_version(1).await.unwrap(), 2);
    assert_eq!(store.increment_version(1).await.unwrap(), 3);
    assert_eq!(store.get_version(1).await.unwrap(), 3);
}

#[tokio::test]
async fn redis_store_different_users_independent() {
    cleanup_redis().await;
    let (store, _) = make_redis_stores().await;

    store.increment_version(1).await.unwrap();
    store.increment_version(2).await.unwrap();
    store.increment_version(2).await.unwrap();

    assert_eq!(store.get_version(1).await.unwrap(), 1);
    assert_eq!(store.get_version(2).await.unwrap(), 2);
}

#[tokio::test]
async fn redis_blacklist_revoke_and_check() {
    cleanup_redis().await;
    let (_, blacklist) = make_redis_stores().await;

    // 初始不在黑名单
    assert!(!blacklist.is_revoked("token-1").await.unwrap());

    // 撤销后应在黑名单
    blacklist.revoke("token-1", 60).await.unwrap();
    assert!(blacklist.is_revoked("token-1").await.unwrap());

    // 其他 token 不受影响
    assert!(!blacklist.is_revoked("token-2").await.unwrap());
}

#[tokio::test]
async fn redis_blacklist_ttl_expiry() {
    cleanup_redis().await;
    let (_, blacklist) = make_redis_stores().await;

    // 撤销 1 秒 TTL
    blacklist.revoke("short-lived", 1).await.unwrap();
    assert!(blacklist.is_revoked("short-lived").await.unwrap());

    // 等待 2 秒后应过期
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert!(!blacklist.is_revoked("short-lived").await.unwrap());
}

#[tokio::test]
async fn redis_blacklist_revoke_zero_ttl_noop() {
    cleanup_redis().await;
    let (_, blacklist) = make_redis_stores().await;

    // TTL=0 应为 no-op
    blacklist.revoke("zero-ttl", 0).await.unwrap();
    assert!(!blacklist.is_revoked("zero-ttl").await.unwrap());
}

#[tokio::test]
async fn redis_full_sso_flow_with_redis_backend() {
    cleanup_redis().await;
    let (store, blacklist) = make_redis_stores().await;

    let codec = SsoJwtCodec::new("redis-test-secret");
    let config = RefreshTokenConfig::default();
    let issuer = RefreshTokenIssuer::new(
        codec.clone(),
        blacklist.clone(),
        store.clone(),
        config.clone(),
    );
    let verifier = RefreshTokenVerifier::new(
        codec.clone(),
        blacklist.clone(),
        store.clone(),
        config.issuer.clone(),
    );
    let revoker = RefreshTokenRevoker::new(codec, blacklist, store);

    // 签发 Token
    let pair = issuer.issue(1, "redis_user").await.unwrap();
    verifier.verify_access(&pair.access_token).await.unwrap();
    verifier.verify_refresh(&pair.refresh_token).await.unwrap();

    // 轮换 Token
    let new_pair = issuer.rotate(&pair.refresh_token).await.unwrap();
    verifier
        .verify_access(&new_pair.access_token)
        .await
        .unwrap();

    // 旧 refresh 应已黑名单
    let result = verifier.verify_refresh(&pair.refresh_token).await;
    assert!(matches!(result, Err(RefreshTokenError::Revoked)));

    // 撤销用户所有 Token
    revoker.revoke_all(1).await.unwrap();
    let result = verifier.verify_access(&new_pair.access_token).await;
    assert!(matches!(
        result,
        Err(RefreshTokenError::VersionMismatch { .. })
    ));
}

#[tokio::test]
async fn redis_concurrent_increment_no_lost_update() {
    cleanup_redis().await;
    let (store, _) = make_redis_stores().await;

    // 并发 10 次 increment，结果应为 10（无丢失更新）
    let mut handles = Vec::new();
    for _ in 0..10 {
        let store = store.clone();
        handles.push(tokio::spawn(async move {
            store.increment_version(1).await.unwrap()
        }));
    }

    let results: Vec<u64> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();
    let final_version = store.get_version(1).await.unwrap();

    assert_eq!(final_version, 10);
    // 每次返回的版本号应唯一（1-10）
    let mut sorted = results.clone();
    sorted.sort();
    assert_eq!(sorted, (1u64..=10).collect::<Vec<_>>());
}

// ── RedisDeviceSessionStore 集成测试 ──

use sz_rust_auth_facade::redis_store::create_redis_stores_with_devices;
use sz_rust_auth_facade::refresh::{DeviceInfo, DeviceSessionStore};

async fn make_redis_device_store() -> Arc<dyn DeviceSessionStore> {
    let config = RedisConfig::from_url(REDIS_URL);
    let (_, _, device_store) = create_redis_stores_with_devices(config).await.unwrap();
    device_store
}

#[tokio::test]
async fn redis_device_session_register_and_get() {
    cleanup_redis().await;
    let store = make_redis_device_store().await;

    let info = DeviceInfo::with_device_id("web-1");
    store
        .register_session(1, "web-1", &info, "refresh-jti-1", "access-jti-1")
        .await
        .unwrap();

    let session = store.get_session(1, "web-1").await.unwrap();
    assert!(session.is_some());
    let s = session.unwrap();
    assert_eq!(s.device_id, "web-1");
    assert_eq!(s.jti, "refresh-jti-1");
    assert_eq!(s.access_jti, "access-jti-1");

    let sessions = store.get_sessions(1).await.unwrap();
    assert_eq!(sessions.len(), 1);
}

#[tokio::test]
async fn redis_device_session_revoke() {
    cleanup_redis().await;
    let store = make_redis_device_store().await;

    let info = DeviceInfo::with_device_id("dev-1");
    store
        .register_session(1, "dev-1", &info, "jti-1", "access-jti-1")
        .await
        .unwrap();

    let result = store.revoke_session(1, "dev-1").await.unwrap();
    assert!(result.is_some());
    let (jti, access_jti) = result.unwrap();
    assert_eq!(jti, "jti-1");
    assert_eq!(access_jti, "access-jti-1");

    let session = store.get_session(1, "dev-1").await.unwrap();
    assert!(session.is_none());
}

#[tokio::test]
async fn redis_device_session_update_last_active() {
    cleanup_redis().await;
    let store = make_redis_device_store().await;

    let info = DeviceInfo::with_device_id("dev-1");
    store
        .register_session(1, "dev-1", &info, "jti-1", "access-jti-1")
        .await
        .unwrap();

    let before = store.get_session(1, "dev-1").await.unwrap().unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    store.update_last_active(1, "dev-1").await.unwrap();
    let after = store.get_session(1, "dev-1").await.unwrap().unwrap();

    assert!(after.last_active > before.last_active);
}

#[tokio::test]
async fn redis_device_session_update_jti() {
    cleanup_redis().await;
    let store = make_redis_device_store().await;

    let info = DeviceInfo::with_device_id("dev-1");
    store
        .register_session(1, "dev-1", &info, "old-jti", "access-jti")
        .await
        .unwrap();

    store
        .update_session_jti(1, "dev-1", "new-jti")
        .await
        .unwrap();

    let session = store.get_session(1, "dev-1").await.unwrap().unwrap();
    assert_eq!(session.jti, "new-jti");
}

#[tokio::test]
async fn redis_device_session_cleanup_expired() {
    cleanup_redis().await;
    let store = make_redis_device_store().await;

    let info1 = DeviceInfo::with_device_id("dev-1");
    store
        .register_session(1, "dev-1", &info1, "jti-1", "access-jti-1")
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let info2 = DeviceInfo::with_device_id("dev-2");
    store
        .register_session(1, "dev-2", &info2, "jti-2", "access-jti-2")
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let expired = store.cleanup_expired(1, 1).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].0, "jti-1");

    let sessions = store.get_sessions(1).await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].device_id, "dev-2");
}

#[tokio::test]
async fn redis_device_session_clear_user_sessions() {
    cleanup_redis().await;
    let store = make_redis_device_store().await;

    for i in 1..=3 {
        let info = DeviceInfo::with_device_id(&format!("dev-{i}"));
        store
            .register_session(
                1,
                &format!("dev-{i}"),
                &info,
                &format!("jti-{i}"),
                &format!("access-jti-{i}"),
            )
            .await
            .unwrap();
    }

    let jti_list = store.clear_user_sessions(1).await.unwrap();
    assert_eq!(jti_list.len(), 3);

    let sessions = store.get_sessions(1).await.unwrap();
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn redis_create_stores_with_devices_shared_connection() {
    cleanup_redis().await;
    let config = RedisConfig::from_url(REDIS_URL);
    let (store, blacklist, device_store) = create_redis_stores_with_devices(config).await.unwrap();

    store.increment_version(1).await.unwrap();
    blacklist.revoke("test-jti", 60).await.unwrap();

    let info = DeviceInfo::with_device_id("dev-1");
    device_store
        .register_session(1, "dev-1", &info, "jti-1", "access-jti-1")
        .await
        .unwrap();

    assert_eq!(store.get_version(1).await.unwrap(), 1);
    assert!(blacklist.is_revoked("test-jti").await.unwrap());
    assert!(device_store
        .get_session(1, "dev-1")
        .await
        .unwrap()
        .is_some());
}
