// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! SSO 集成测试 — 完整登录→校验→刷新→撤销流程
//!
//! 对齐 spec.md AC-1 ~ AC-18 验收标准。
use std::sync::Arc;
use sz_rust_auth_facade::refresh::{
    MemoryRefreshTokenStore, MemoryTokenBlacklist, RefreshTokenConfig, RefreshTokenError,
    RefreshTokenIssuer, RefreshTokenRevoker, RefreshTokenVerifier, SsoJwtCodec,
};
use sz_rust_auth_facade::sso::{LoginResponse, SsoService, UserAuthService, UserInfo};

struct SimpleUserService {
    users: Vec<(String, String, UserInfo)>,
}

#[async_trait::async_trait]
impl UserAuthService for SimpleUserService {
    async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<UserInfo, RefreshTokenError> {
        self.users
            .iter()
            .find(|(u, p, _)| u == username && p == password)
            .map(|(_, _, info)| info.clone())
            .ok_or(RefreshTokenError::InvalidCredentials)
    }

    async fn get_user_info(&self, user_id: i64) -> Result<UserInfo, RefreshTokenError> {
        self.users
            .iter()
            .find(|(_, _, info)| info.user_id == user_id)
            .map(|(_, _, info)| info.clone())
            .ok_or(RefreshTokenError::UserNotFound)
    }
}

fn make_sso_service() -> SsoService {
    let codec = SsoJwtCodec::new("integration-secret");
    let blacklist: Arc<dyn sz_rust_auth_facade::refresh::TokenBlacklist> =
        Arc::new(MemoryTokenBlacklist::new());
    let store: Arc<dyn sz_rust_auth_facade::refresh::RefreshTokenStore> =
        Arc::new(MemoryRefreshTokenStore::new());
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

    let users = vec![(
        "alice".to_string(),
        "alice_pass".to_string(),
        UserInfo {
            user_id: 1,
            username: "alice".to_string(),
            roles: vec!["admin".to_string()],
            permissions: vec!["read".to_string(), "write".to_string()],
        },
    )];

    SsoService::new(
        issuer,
        verifier,
        revoker,
        Arc::new(SimpleUserService { users }),
    )
}

#[tokio::test]
async fn integration_full_login_refresh_revoke_flow() {
    let sso = make_sso_service();

    // AC-1: 登录获取双 Token
    let login: LoginResponse = sso.login("alice", "alice_pass").await.unwrap();
    assert!(!login.tokens.access_token.is_empty());
    assert!(!login.tokens.refresh_token.is_empty());
    assert!(login.tokens.access_expires_at < login.tokens.refresh_expires_at);
    assert_eq!(login.username, "alice");

    // AC-2: accessToken 校验通过
    let claims = sso.validate(&login.tokens.access_token).await.unwrap();
    assert_eq!(claims.user_id, Some(1));
    assert!(claims.is_access());

    // AC-3: refreshToken 不能用作 accessToken
    let result = sso.validate(&login.tokens.refresh_token).await;
    assert!(matches!(
        result,
        Err(RefreshTokenError::WrongTokenType { .. })
    ));

    // AC-4: 刷新 Token
    let new_tokens = sso.refresh(&login.tokens.refresh_token).await.unwrap();
    assert_ne!(new_tokens.access_token, login.tokens.access_token);
    assert_ne!(new_tokens.refresh_token, login.tokens.refresh_token);

    // AC-6: 旧 Token 可正常校验（在触发复用检测之前验证）
    let new_claims = sso.validate(&new_tokens.access_token).await.unwrap();
    assert_eq!(new_claims.user_id, Some(1));

    // AC-5: 旧 refreshToken 刷新后失效（复用检测，会撤销用户所有 Token）
    let result = sso.refresh(&login.tokens.refresh_token).await;
    assert!(matches!(result, Err(RefreshTokenError::ReuseDetected)));

    // AC-8: me 获取用户信息
    let user_info = sso.me(1).await.unwrap();
    assert_eq!(user_info.username, "alice");
    assert_eq!(user_info.roles, vec!["admin".to_string()]);
}

#[tokio::test]
async fn integration_revoke_all_invalidates_everything() {
    let sso = make_sso_service();
    let login1 = sso.login("alice", "alice_pass").await.unwrap();
    let login2 = sso.login("alice", "alice_pass").await.unwrap();

    // 撤销用户所有 Token
    sso.revoke_all(1).await.unwrap();

    // 两组 Token 都应失效（版本不匹配）
    let result = sso.validate(&login1.tokens.access_token).await;
    assert!(matches!(
        result,
        Err(RefreshTokenError::VersionMismatch { .. })
    ));
    let result = sso.validate(&login2.tokens.access_token).await;
    assert!(matches!(
        result,
        Err(RefreshTokenError::VersionMismatch { .. })
    ));

    // 重新登录获取新 Token（新版本号）
    let login3 = sso.login("alice", "alice_pass").await.unwrap();
    sso.validate(&login3.tokens.access_token).await.unwrap();
}

#[tokio::test]
async fn integration_wrong_password_rejected() {
    let sso = make_sso_service();
    let result = sso.login("alice", "wrong_password").await;
    assert!(matches!(result, Err(RefreshTokenError::InvalidCredentials)));
}

#[tokio::test]
async fn integration_nonexistent_user_rejected() {
    let sso = make_sso_service();
    let result = sso.login("bob", "bob_pass").await;
    assert!(matches!(result, Err(RefreshTokenError::InvalidCredentials)));
}

#[tokio::test]
async fn integration_token_survives_multiple_validations() {
    let sso = make_sso_service();
    let login = sso.login("alice", "alice_pass").await.unwrap();

    // 同一 Token 多次校验应都成功（幂等）
    for _ in 0..10 {
        sso.validate(&login.tokens.access_token).await.unwrap();
    }
}

#[tokio::test]
async fn integration_refresh_chain_preserves_user_identity() {
    let sso = make_sso_service();
    let mut current = sso.login("alice", "alice_pass").await.unwrap();

    // 连续刷新 3 次，user_id 应始终一致
    for _ in 0..3 {
        current.tokens = sso.refresh(&current.tokens.refresh_token).await.unwrap();
        let claims = sso.validate(&current.tokens.access_token).await.unwrap();
        assert_eq!(claims.user_id, Some(1));
        assert_eq!(claims.sub, "alice");
    }
}

// ── Token 自动续期集成测试 ──

use sz_rust_auth_facade::sso::RenewalConfig;

fn make_sso_service_with_renewal() -> SsoService {
    let codec = SsoJwtCodec::new("integration-secret");
    let blacklist: Arc<dyn sz_rust_auth_facade::refresh::TokenBlacklist> =
        Arc::new(MemoryTokenBlacklist::new());
    let store: Arc<dyn sz_rust_auth_facade::refresh::RefreshTokenStore> =
        Arc::new(MemoryRefreshTokenStore::new());
    let config = RefreshTokenConfig {
        access_token_ttl: chrono::Duration::seconds(60),
        refresh_token_ttl: chrono::Duration::seconds(3600),
        issuer: "sz-rust-sso".to_string(),
    };
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

    let users = vec![(
        "alice".to_string(),
        "alice_pass".to_string(),
        UserInfo {
            user_id: 1,
            username: "alice".to_string(),
            roles: vec!["admin".to_string()],
            permissions: vec!["read".to_string(), "write".to_string()],
        },
    )];

    let mut sso = SsoService::new(
        issuer,
        verifier,
        revoker,
        Arc::new(SimpleUserService { users }),
    );
    sso.with_renewal_config(RenewalConfig {
        enabled: true,
        renewal_threshold: chrono::Duration::seconds(30),
        renewal_ratio: 0.2,
        access_token_ttl: chrono::Duration::seconds(60),
    });
    sso
}

#[tokio::test]
async fn integration_renewal_triggers_when_ttl_low() {
    let sso = make_sso_service_with_renewal();
    let login = sso.login("alice", "alice_pass").await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_secs(35)).await;

    let (claims, renewed) = sso
        .validate_with_renewal(&login.tokens.access_token)
        .await
        .unwrap();
    assert_eq!(claims.user_id, Some(1));
    assert!(renewed.is_some());

    let renewed = renewed.unwrap();
    assert!(!renewed.access_token.is_empty());
    assert!(renewed.expires_at > chrono::Utc::now().timestamp());

    let new_claims = sso.validate(&renewed.access_token).await.unwrap();
    assert_eq!(new_claims.user_id, Some(1));
    assert_eq!(new_claims.sub, "alice");
}

#[tokio::test]
async fn integration_renewal_not_triggered_when_ttl_high() {
    let sso = make_sso_service_with_renewal();
    let login = sso.login("alice", "alice_pass").await.unwrap();

    let (claims, renewed) = sso
        .validate_with_renewal(&login.tokens.access_token)
        .await
        .unwrap();
    assert_eq!(claims.user_id, Some(1));
    assert!(renewed.is_none());
}

#[tokio::test]
async fn integration_renewal_disabled_never_renews() {
    let sso = make_sso_service();
    let login = sso.login("alice", "alice_pass").await.unwrap();

    let (claims, renewed) = sso
        .validate_with_renewal(&login.tokens.access_token)
        .await
        .unwrap();
    assert_eq!(claims.user_id, Some(1));
    assert!(renewed.is_none());
}

#[tokio::test]
async fn integration_renewal_preserves_version() {
    let sso = make_sso_service_with_renewal();
    let login = sso.login("alice", "alice_pass").await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_secs(35)).await;

    let (claims, renewed) = sso
        .validate_with_renewal(&login.tokens.access_token)
        .await
        .unwrap();
    assert!(renewed.is_some());

    let renewed = renewed.unwrap();
    let new_claims = sso.validate(&renewed.access_token).await.unwrap();
    assert_eq!(new_claims.ver, claims.ver);
}

#[tokio::test]
async fn integration_renewal_old_token_still_valid() {
    let sso = make_sso_service_with_renewal();
    let login = sso.login("alice", "alice_pass").await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_secs(35)).await;

    let (_, renewed) = sso
        .validate_with_renewal(&login.tokens.access_token)
        .await
        .unwrap();
    assert!(renewed.is_some());

    sso.validate(&login.tokens.access_token).await.unwrap();
}

#[tokio::test]
async fn integration_renewal_after_revoke_all_fails() {
    let sso = make_sso_service_with_renewal();
    let login = sso.login("alice", "alice_pass").await.unwrap();

    sso.revoke_all(1).await.unwrap();

    let result = sso.validate_with_renewal(&login.tokens.access_token).await;
    assert!(matches!(
        result,
        Err(RefreshTokenError::VersionMismatch { .. })
    ));
}

#[tokio::test]
async fn integration_renewal_chained_multiple_times() {
    let sso = make_sso_service_with_renewal();
    let login = sso.login("alice", "alice_pass").await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_secs(35)).await;

    let (_, renewed1) = sso
        .validate_with_renewal(&login.tokens.access_token)
        .await
        .unwrap();
    assert!(renewed1.is_some());
    let new_token = renewed1.unwrap().access_token;

    let (claims2, renewed2) = sso.validate_with_renewal(&new_token).await.unwrap();
    assert_eq!(claims2.user_id, Some(1));
    assert!(renewed2.is_none());
}

#[tokio::test]
async fn integration_renewal_does_not_issue_refresh_token() {
    let sso = make_sso_service_with_renewal();
    let login = sso.login("alice", "alice_pass").await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_secs(35)).await;

    let (_, renewed) = sso
        .validate_with_renewal(&login.tokens.access_token)
        .await
        .unwrap();
    assert!(renewed.is_some());

    let renewed = renewed.unwrap();
    let result = sso.refresh(&renewed.access_token).await;
    assert!(result.is_err());
}

// ── 多设备会话管理集成测试 ──

use sz_rust_auth_facade::refresh::{
    DeviceInfo, DeviceSessionConfig, MemoryDegradationStore, MemoryDeviceSessionStore,
};

fn make_sso_service_with_device_store() -> SsoService {
    let mut sso = make_sso_service();
    sso.with_device_store(
        Arc::new(MemoryDeviceSessionStore::new()),
        DeviceSessionConfig::default(),
    );
    sso
}

#[tokio::test]
async fn integration_multi_device_login_and_list() {
    let sso = make_sso_service_with_device_store();

    let resp1 = sso
        .login_with_device(
            "alice",
            "alice_pass",
            &DeviceInfo::with_device_id("web-browser"),
        )
        .await
        .unwrap();
    let resp2 = sso
        .login_with_device(
            "alice",
            "alice_pass",
            &DeviceInfo::with_device_id("mobile-app"),
        )
        .await
        .unwrap();

    let devices = sso.list_devices(1).await.unwrap();
    assert_eq!(devices.len(), 2);

    sso.validate(&resp1.tokens.access_token).await.unwrap();
    sso.validate(&resp2.tokens.access_token).await.unwrap();
}

#[tokio::test]
async fn integration_revoke_device_invalidates_both_tokens() {
    let sso = make_sso_service_with_device_store();

    let resp1 = sso
        .login_with_device("alice", "alice_pass", &DeviceInfo::with_device_id("dev1"))
        .await
        .unwrap();
    let resp2 = sso
        .login_with_device("alice", "alice_pass", &DeviceInfo::with_device_id("dev2"))
        .await
        .unwrap();

    sso.revoke_device(1, "dev1").await.unwrap();

    let result = sso.validate(&resp1.tokens.access_token).await;
    assert!(result.is_err());

    sso.validate(&resp2.tokens.access_token).await.unwrap();

    let devices = sso.list_devices(1).await.unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].device_id, "dev2");

    let result = sso.refresh(&resp1.tokens.refresh_token).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn integration_revoke_all_clears_all_devices() {
    let sso = make_sso_service_with_device_store();

    sso.login_with_device("alice", "alice_pass", &DeviceInfo::with_device_id("dev1"))
        .await
        .unwrap();
    sso.login_with_device("alice", "alice_pass", &DeviceInfo::with_device_id("dev2"))
        .await
        .unwrap();
    sso.login_with_device("alice", "alice_pass", &DeviceInfo::with_device_id("dev3"))
        .await
        .unwrap();

    assert_eq!(sso.list_devices(1).await.unwrap().len(), 3);

    sso.revoke_all(1).await.unwrap();

    assert!(sso.list_devices(1).await.unwrap().is_empty());
}

#[tokio::test]
async fn integration_device_heartbeat_updates_last_active() {
    let sso = make_sso_service_with_device_store();

    sso.login_with_device("alice", "alice_pass", &DeviceInfo::with_device_id("dev1"))
        .await
        .unwrap();

    let before = sso.list_devices(1).await.unwrap()[0].last_active;

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    sso.update_device_active(1, "dev1").await.unwrap();

    let after = sso.list_devices(1).await.unwrap()[0].last_active;
    assert!(after > before);
}

#[tokio::test]
async fn integration_lru_eviction_on_max_devices() {
    let mut sso = make_sso_service_with_device_store();
    sso.with_device_store(
        Arc::new(MemoryDeviceSessionStore::new()),
        DeviceSessionConfig { max_devices: 2 },
    );

    let r1 = sso
        .login_with_device("alice", "alice_pass", &DeviceInfo::with_device_id("dev1"))
        .await
        .unwrap();
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    let r2 = sso
        .login_with_device("alice", "alice_pass", &DeviceInfo::with_device_id("dev2"))
        .await
        .unwrap();
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    let _r3 = sso
        .login_with_device("alice", "alice_pass", &DeviceInfo::with_device_id("dev3"))
        .await
        .unwrap();

    assert_eq!(sso.list_devices(1).await.unwrap().len(), 2);

    let result = sso.validate(&r1.tokens.access_token).await;
    assert!(result.is_err());

    sso.validate(&r2.tokens.access_token).await.unwrap();
}

#[tokio::test]
async fn integration_same_device_relogin_overwrites() {
    let sso = make_sso_service_with_device_store();

    let _r1 = sso
        .login_with_device("alice", "alice_pass", &DeviceInfo::with_device_id("dev1"))
        .await
        .unwrap();
    let r2 = sso
        .login_with_device("alice", "alice_pass", &DeviceInfo::with_device_id("dev1"))
        .await
        .unwrap();

    assert_eq!(sso.list_devices(1).await.unwrap().len(), 1);

    sso.validate(&r2.tokens.access_token).await.unwrap();
}

#[tokio::test]
async fn integration_cleanup_expired_devices() {
    let sso = make_sso_service_with_device_store();

    sso.login_with_device("alice", "alice_pass", &DeviceInfo::with_device_id("dev1"))
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    let count = sso.cleanup_expired_devices(1, 1).await.unwrap();
    assert_eq!(count, 1);
    assert!(sso.list_devices(1).await.unwrap().is_empty());
}

// ── Token 降级集成测试 ──

fn make_sso_service_with_degradation_store() -> SsoService {
    let mut sso = make_sso_service();
    sso.with_degradation_store(Arc::new(MemoryDegradationStore::new()));
    sso
}

#[tokio::test]
async fn integration_degradation_full_flow() {
    let sso = make_sso_service_with_degradation_store();

    let login = sso.login("alice", "alice_pass").await.unwrap();
    let claims = sso.validate(&login.tokens.access_token).await.unwrap();
    assert_eq!(claims.permissions.len(), 2);

    sso.degrade_user(1, vec!["admin".to_string()], vec!["read".to_string()], 3600)
        .await
        .unwrap();

    let degraded_claims = sso.validate(&login.tokens.access_token).await.unwrap();
    assert_eq!(degraded_claims.permissions, vec!["read".to_string()]);

    sso.clear_degradation(1).await.unwrap();
    let restored_claims = sso.validate(&login.tokens.access_token).await.unwrap();
    assert_eq!(restored_claims.permissions.len(), 2);
}

#[tokio::test]
async fn integration_device_degradation_priority() {
    let mut sso = make_sso_service_with_degradation_store();
    sso.with_device_store(
        Arc::new(MemoryDeviceSessionStore::new()),
        DeviceSessionConfig::default(),
    );

    let login = sso
        .login_with_device("alice", "alice_pass", &DeviceInfo::with_device_id("dev1"))
        .await
        .unwrap();

    sso.degrade_user(1, vec!["admin".to_string()], vec!["read".to_string()], 3600)
        .await
        .unwrap();
    sso.degrade_device(1, "dev1", vec![], vec![], 3600)
        .await
        .unwrap();

    let claims = sso.validate(&login.tokens.access_token).await.unwrap();
    assert!(claims.permissions.is_empty());
    assert!(claims.roles.is_empty());
}

#[tokio::test]
async fn integration_degradation_ttl_expired() {
    let sso = make_sso_service_with_degradation_store();

    let login = sso.login("alice", "alice_pass").await.unwrap();

    sso.degrade_user(1, vec!["admin".to_string()], vec!["read".to_string()], 1)
        .await
        .unwrap();

    let degraded_claims = sso.validate(&login.tokens.access_token).await.unwrap();
    assert_eq!(degraded_claims.permissions, vec!["read".to_string()]);

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    let restored_claims = sso.validate(&login.tokens.access_token).await.unwrap();
    assert_eq!(restored_claims.permissions.len(), 2);
}
