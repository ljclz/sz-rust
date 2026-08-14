//! SSO 认证中心
//!
//! 对齐 spec.md FR-5 ~ FR-7，design.md §3。
//!
//! ## 核心组件
//!
//! - [`UserAuthService`]：用户认证后端 trait 抽象
//! - [`SsoService`]：SSO 认证中心核心逻辑
//! - [`UserInfo`]：用户信息
//!
//! ## axum 集成（需启用 `axum` feature）
//!
//! ```ignore
//! use sz_rust_auth_facade::sso::axum_routes::sso_routes;
//! ```

use crate::refresh::{
    RefreshTokenError, RefreshTokenIssuer, RefreshTokenRevoker, RefreshTokenVerifier, SsoClaims,
    TokenPair,
};
use std::sync::Arc;

pub use crate::refresh::{AuditEvent, AuditEventType, AuditStore, MemoryAuditStore};
pub use crate::refresh::{DegradationEntry, DegradationStore, MemoryDegradationStore};
pub use crate::refresh::{DeviceInfo, DeviceSession, DeviceSessionConfig, DeviceSessionStore};
pub use crate::refresh::{MemoryTicketStore, SsoTicket, TicketStore};
pub use crate::refresh::{RenewalConfig, RenewedToken};

// ── 用户信息 ──

/// 用户信息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserInfo {
    /// 用户 ID
    pub user_id: i64,
    /// 用户名
    pub username: String,
    /// 角色
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    /// 权限
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<String>,
}

// ── UserAuthService trait ──

/// 用户认证后端 trait 抽象
///
/// 实现者负责具体的用户名密码校验（如 DB 查询 + bcrypt）。
#[async_trait::async_trait]
pub trait UserAuthService: Send + Sync {
    /// 用户名密码认证，返回用户信息
    async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<UserInfo, RefreshTokenError>;
    /// 按 user_id 获取用户信息
    async fn get_user_info(&self, user_id: i64) -> Result<UserInfo, RefreshTokenError>;
}

// ── LoginResponse ──

/// 登录响应
#[derive(Debug, Clone, serde::Serialize)]
pub struct LoginResponse {
    /// 双 Token
    #[serde(flatten)]
    pub tokens: TokenPair,
    /// 用户 ID
    pub user_id: i64,
    /// 用户名
    pub username: String,
}

// ── SsoService ──

/// SSO 认证中心核心服务
///
/// 组合 Issuer/Verifier/Revoker + UserAuthService，
/// 提供 login/refresh/revoke/validate/me 五大操作。
pub struct SsoService {
    issuer: RefreshTokenIssuer,
    verifier: RefreshTokenVerifier,
    revoker: RefreshTokenRevoker,
    user_auth: Arc<dyn UserAuthService>,
    renewal_config: RenewalConfig,
    device_store: Option<Arc<dyn DeviceSessionStore>>,
    device_config: DeviceSessionConfig,
    degradation_store: Option<Arc<dyn DegradationStore>>,
    ticket_store: Option<Arc<dyn TicketStore>>,
    audit_store: Option<Arc<dyn AuditStore>>,
}

impl SsoService {
    /// 创建 SSO 服务
    pub fn new(
        issuer: RefreshTokenIssuer,
        verifier: RefreshTokenVerifier,
        revoker: RefreshTokenRevoker,
        user_auth: Arc<dyn UserAuthService>,
    ) -> Self {
        Self {
            issuer,
            verifier,
            revoker,
            user_auth,
            renewal_config: RenewalConfig::default(),
            device_store: None,
            device_config: DeviceSessionConfig::default(),
            degradation_store: None,
            ticket_store: None,
            audit_store: None,
        }
    }

    /// 设置续期配置（链式调用）
    pub fn with_renewal_config(&mut self, config: RenewalConfig) -> &mut Self {
        self.renewal_config = config;
        self
    }

    /// 设置设备会话存储（链式调用）
    pub fn with_device_store(
        &mut self,
        store: Arc<dyn DeviceSessionStore>,
        config: DeviceSessionConfig,
    ) -> &mut Self {
        self.device_store = Some(store);
        self.device_config = config;
        self
    }

    /// 设置降级存储（链式调用）
    pub fn with_degradation_store(&mut self, store: Arc<dyn DegradationStore>) -> &mut Self {
        self.degradation_store = Some(store);
        self
    }

    /// 设置 Ticket 存储（链式调用）
    pub fn with_ticket_store(&mut self, store: Arc<dyn TicketStore>) -> &mut Self {
        self.ticket_store = Some(store);
        self
    }

    /// 设置审计存储（链式调用）
    pub fn with_audit_store(&mut self, store: Arc<dyn AuditStore>) -> &mut Self {
        self.audit_store = Some(store);
        self
    }

    /// 登录：用户名密码认证 → 签发双 Token
    #[tracing::instrument(skip(self, password), fields(username = username))]
    pub async fn login(
        &self,
        username: &str,
        password: &str,
    ) -> Result<LoginResponse, RefreshTokenError> {
        if username.is_empty() || password.is_empty() {
            return Err(RefreshTokenError::InvalidCredentials);
        }

        let user_info = self.user_auth.authenticate(username, password).await?;
        let tokens = self
            .issuer
            .issue_with_roles(
                user_info.user_id,
                &user_info.username,
                user_info.roles.clone(),
                user_info.permissions.clone(),
            )
            .await?;

        self.record_audit(AuditEventType::Login, Some(user_info.user_id), None, None)
            .await;

        Ok(LoginResponse {
            tokens,
            user_id: user_info.user_id,
            username: user_info.username,
        })
    }

    /// 登录并绑定设备：用户名密码认证 → 签发绑定设备的双 Token → 注册设备会话
    #[tracing::instrument(skip(self, password), fields(username = username, device_id = device_info.device_id))]
    pub async fn login_with_device(
        &self,
        username: &str,
        password: &str,
        device_info: &DeviceInfo,
    ) -> Result<LoginResponse, RefreshTokenError> {
        if username.is_empty() || password.is_empty() {
            return Err(RefreshTokenError::InvalidCredentials);
        }

        let user_info = self.user_auth.authenticate(username, password).await?;
        let (tokens, jti, access_jti) = self
            .issuer
            .issue_with_device_and_jti(
                user_info.user_id,
                &user_info.username,
                &device_info.device_id,
                user_info.roles.clone(),
                user_info.permissions.clone(),
            )
            .await?;

        if let Some(ref store) = self.device_store {
            let sessions = store
                .get_sessions(user_info.user_id)
                .await
                .unwrap_or_default();
            if sessions.len() >= self.device_config.max_devices {
                let mut sorted = sessions;
                sorted.sort_by_key(|s| s.last_active);
                let to_evict = sorted.len() + 1 - self.device_config.max_devices;
                for session in sorted.iter().take(to_evict) {
                    self.revoker.revoke_by_jti(&session.jti).await.ok();
                    self.revoker.revoke_by_jti(&session.access_jti).await.ok();
                    store
                        .revoke_session(user_info.user_id, &session.device_id)
                        .await
                        .ok();
                    tracing::info!(user_id = user_info.user_id, device_id = %session.device_id, reason = "lru", "device session evicted");
                }
            }

            store
                .register_session(
                    user_info.user_id,
                    &device_info.device_id,
                    device_info,
                    &jti,
                    &access_jti,
                )
                .await?;

            tracing::info!(
                user_id = user_info.user_id,
                device_id = %device_info.device_id,
                device_type = ?device_info.device_type,
                ip = ?device_info.ip,
                "device session registered"
            );
        }

        Ok(LoginResponse {
            tokens,
            user_id: user_info.user_id,
            username: user_info.username,
        })
    }

    /// 查询用户所有在线设备
    pub async fn list_devices(
        &self,
        user_id: i64,
    ) -> Result<Vec<DeviceSession>, RefreshTokenError> {
        let store = self.device_store.as_ref().ok_or_else(|| {
            RefreshTokenError::InvalidConfig("device session store not configured".to_string())
        })?;
        store.get_sessions(user_id).await
    }

    /// 撤销指定设备会话（不递增版本号，不影响其他设备）
    pub async fn revoke_device(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<(), RefreshTokenError> {
        let store = self.device_store.as_ref().ok_or_else(|| {
            RefreshTokenError::InvalidConfig("device session store not configured".to_string())
        })?;

        if let Some((refresh_jti, access_jti)) = store.revoke_session(user_id, device_id).await? {
            self.revoker.revoke_by_jti(&refresh_jti).await.ok();
            self.revoker.revoke_by_jti(&access_jti).await.ok();
            tracing::info!(
                user_id,
                device_id,
                reason = "manual",
                "device session revoked"
            );
        }

        if let Some(ref store) = self.degradation_store {
            if let Err(e) = store.clear_device_degradation(user_id, device_id).await {
                tracing::warn!(error = %e, "failed to clear device degradation on revoke_device");
            }
        }

        Ok(())
    }

    /// 更新设备活跃时间（心跳）
    pub async fn update_device_active(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<(), RefreshTokenError> {
        let store = self.device_store.as_ref().ok_or_else(|| {
            RefreshTokenError::InvalidConfig("device session store not configured".to_string())
        })?;
        store.update_last_active(user_id, device_id).await
    }

    /// 清理过期设备会话
    pub async fn cleanup_expired_devices(
        &self,
        user_id: i64,
        ttl_secs: i64,
    ) -> Result<usize, RefreshTokenError> {
        let store = self.device_store.as_ref().ok_or_else(|| {
            RefreshTokenError::InvalidConfig("device session store not configured".to_string())
        })?;
        let jti_list = store.cleanup_expired(user_id, ttl_secs).await?;
        for (refresh_jti, access_jti) in &jti_list {
            self.revoker.revoke_by_jti(refresh_jti).await.ok();
            self.revoker.revoke_by_jti(access_jti).await.ok();
        }
        Ok(jti_list.len())
    }

    /// 用户级权限降级
    pub async fn degrade_user(
        &self,
        user_id: i64,
        roles: Vec<String>,
        permissions: Vec<String>,
        ttl_secs: u64,
    ) -> Result<(), RefreshTokenError> {
        let store = self.degradation_store.as_ref().ok_or_else(|| {
            RefreshTokenError::InvalidConfig("degradation store not configured".to_string())
        })?;
        let entry = DegradationEntry {
            roles,
            permissions,
            expires_at: chrono::Utc::now().timestamp() + ttl_secs as i64,
        };
        store.set_user_degradation(user_id, entry).await?;
        tracing::info!(user_id, ttl_secs, "user degraded");
        self.record_audit(AuditEventType::Degrade, Some(user_id), None, None)
            .await;
        Ok(())
    }

    /// 清除用户降级（含设备级）
    pub async fn clear_degradation(&self, user_id: i64) -> Result<(), RefreshTokenError> {
        let store = self.degradation_store.as_ref().ok_or_else(|| {
            RefreshTokenError::InvalidConfig("degradation store not configured".to_string())
        })?;
        store.clear_all_degradations(user_id).await?;
        tracing::info!(user_id, "user degradation cleared");
        Ok(())
    }

    /// 查询用户降级状态
    pub async fn get_degradation(
        &self,
        user_id: i64,
    ) -> Result<Option<DegradationEntry>, RefreshTokenError> {
        let store = self.degradation_store.as_ref().ok_or_else(|| {
            RefreshTokenError::InvalidConfig("degradation store not configured".to_string())
        })?;
        store.get_user_degradation(user_id).await
    }

    /// 设备级权限降级
    pub async fn degrade_device(
        &self,
        user_id: i64,
        device_id: &str,
        roles: Vec<String>,
        permissions: Vec<String>,
        ttl_secs: u64,
    ) -> Result<(), RefreshTokenError> {
        let store = self.degradation_store.as_ref().ok_or_else(|| {
            RefreshTokenError::InvalidConfig("degradation store not configured".to_string())
        })?;
        let entry = DegradationEntry {
            roles,
            permissions,
            expires_at: chrono::Utc::now().timestamp() + ttl_secs as i64,
        };
        store
            .set_device_degradation(user_id, device_id, entry)
            .await?;
        tracing::info!(user_id, device_id, ttl_secs, "device degraded");
        Ok(())
    }

    /// 清除设备级降级
    pub async fn clear_device_degradation(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<(), RefreshTokenError> {
        let store = self.degradation_store.as_ref().ok_or_else(|| {
            RefreshTokenError::InvalidConfig("degradation store not configured".to_string())
        })?;
        store.clear_device_degradation(user_id, device_id).await?;
        tracing::info!(user_id, device_id, "device degradation cleared");
        Ok(())
    }

    /// 生成跨域 SSO Ticket（一次性，短期有效）
    pub async fn generate_ticket(
        &self,
        user_id: i64,
        redirect_uri: &str,
    ) -> Result<String, RefreshTokenError> {
        let store = self.ticket_store.as_ref().ok_or_else(|| {
            RefreshTokenError::InvalidConfig("ticket store not configured".to_string())
        })?;
        let user_info = self.user_auth.get_user_info(user_id).await?;
        let now = chrono::Utc::now().timestamp();
        let ticket = uuid::Uuid::new_v4().to_string();
        let sso_ticket = SsoTicket {
            ticket: ticket.clone(),
            user_id,
            username: user_info.username.clone(),
            redirect_uri: redirect_uri.to_string(),
            roles: user_info.roles.clone(),
            permissions: user_info.permissions.clone(),
            created_at: now,
            expires_at: now + 30,
        };
        store.save(sso_ticket).await?;
        tracing::info!(user_id, redirect_uri, "sso ticket generated");
        self.record_audit(
            AuditEventType::TicketGenerate,
            Some(user_id),
            None,
            Some(redirect_uri.to_string()),
        )
        .await;
        Ok(ticket)
    }

    /// 交换 Ticket 获取 TokenPair（一次性使用，交换后 ticket 失效）
    pub async fn exchange_ticket(&self, ticket: &str) -> Result<TokenPair, RefreshTokenError> {
        let store = self.ticket_store.as_ref().ok_or_else(|| {
            RefreshTokenError::InvalidConfig("ticket store not configured".to_string())
        })?;
        let sso_ticket = store.take(ticket).await?.ok_or_else(|| {
            RefreshTokenError::InvalidConfig("ticket not found or expired".to_string())
        })?;
        let tokens = self
            .issuer
            .issue_with_roles(
                sso_ticket.user_id,
                &sso_ticket.username,
                sso_ticket.roles,
                sso_ticket.permissions,
            )
            .await?;
        tracing::info!(user_id = sso_ticket.user_id, "sso ticket exchanged");
        self.record_audit(
            AuditEventType::TicketExchange,
            Some(sso_ticket.user_id),
            None,
            None,
        )
        .await;
        Ok(tokens)
    }

    /// 验证 Ticket（仅查看，不消费）
    pub async fn validate_ticket(
        &self,
        ticket: &str,
    ) -> Result<Option<SsoTicket>, RefreshTokenError> {
        let store = self.ticket_store.as_ref().ok_or_else(|| {
            RefreshTokenError::InvalidConfig("ticket store not configured".to_string())
        })?;
        store.peek(ticket).await
    }

    /// 刷新：旧 refreshToken → 新 TokenPair
    #[tracing::instrument(skip(self, refresh_token))]
    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenPair, RefreshTokenError> {
        self.issuer.rotate(refresh_token).await
    }

    /// 撤销单个 Token
    #[tracing::instrument(skip(self, token))]
    pub async fn revoke(&self, token: &str) -> Result<(), RefreshTokenError> {
        self.revoker.revoke(token).await
    }

    /// 撤销用户所有 Token
    #[tracing::instrument(skip(self), fields(user_id = user_id))]
    pub async fn revoke_all(&self, user_id: i64) -> Result<(), RefreshTokenError> {
        self.revoker.revoke_all(user_id).await?;

        if let Some(ref store) = self.device_store {
            match store.clear_user_sessions(user_id).await {
                Ok(jti_list) => {
                    for (refresh_jti, access_jti) in &jti_list {
                        self.revoker.revoke_by_jti(refresh_jti).await.ok();
                        self.revoker.revoke_by_jti(access_jti).await.ok();
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to clear device sessions on revoke_all");
                }
            }
        }

        if let Some(ref store) = self.degradation_store {
            if let Err(e) = store.clear_all_degradations(user_id).await {
                tracing::warn!(error = %e, "failed to clear degradations on revoke_all");
            }
        }

        self.record_audit(AuditEventType::RevokeAll, Some(user_id), None, None)
            .await;

        Ok(())
    }

    /// 校验 accessToken
    #[tracing::instrument(skip(self, access_token))]
    pub async fn validate(&self, access_token: &str) -> Result<SsoClaims, RefreshTokenError> {
        let mut claims = self.verifier.verify_access(access_token).await?;
        self.apply_degradation(&mut claims).await;
        self.best_effort_update_device_active(&claims).await;
        Ok(claims)
    }

    /// 校验 accessToken 并自动续期
    ///
    /// 如果 accessToken 校验通过且剩余 TTL 低于阈值，自动签发新 accessToken。
    /// 返回 `(claims, Option<RenewedToken>)`，`Some` 表示已续期。
    #[tracing::instrument(skip(self, access_token))]
    pub async fn validate_with_renewal(
        &self,
        access_token: &str,
    ) -> Result<(SsoClaims, Option<RenewedToken>), RefreshTokenError> {
        let mut claims = self.verifier.verify_access(access_token).await?;
        self.apply_degradation(&mut claims).await;
        self.best_effort_update_device_active(&claims).await;

        if !self.renewal_config.enabled {
            return Ok((claims, None));
        }

        let now = chrono::Utc::now().timestamp();
        let remaining_ttl = claims.exp - now;

        if self.renewal_config.should_renew(remaining_ttl) {
            let old_jti = claims.jti.clone();
            let old_exp = claims.exp;
            let (new_token, new_exp) = self.issuer.renew_access(&claims)?;
            let new_jti = uuid::Uuid::new_v4().to_string();

            tracing::debug!(
                user_id = claims.user_id,
                old_jti = %old_jti,
                new_jti = %new_jti,
                old_exp = old_exp,
                new_exp = new_exp,
                "access token renewed"
            );

            return Ok((
                claims,
                Some(RenewedToken {
                    access_token: new_token,
                    expires_at: new_exp,
                }),
            ));
        }

        Ok((claims, None))
    }

    /// Best-effort 更新设备活跃时间（失败仅 warn 不中断）
    async fn best_effort_update_device_active(&self, claims: &SsoClaims) {
        if let (Some(store), Some(device_id), Some(user_id)) = (
            self.device_store.as_ref(),
            claims.device_id.as_ref(),
            claims.user_id,
        ) {
            if let Err(e) = store.update_last_active(user_id, device_id).await {
                tracing::warn!(error = %e, "failed to update device active time");
            }
        }
    }

    /// 应用降级到 claims（best-effort，失败不阻断）
    async fn apply_degradation(&self, claims: &mut SsoClaims) {
        let Some(ref store) = self.degradation_store else {
            return;
        };
        let Some(user_id) = claims.user_id else {
            return;
        };

        let entry = if let Some(ref device_id) = claims.device_id {
            match store.get_device_degradation(user_id, device_id).await {
                Ok(Some(e)) => Some(e),
                Ok(None) => match store.get_user_degradation(user_id).await {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to query user degradation");
                        None
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "failed to query device degradation");
                    None
                }
            }
        } else {
            match store.get_user_degradation(user_id).await {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to query user degradation");
                    None
                }
            }
        };

        if let Some(entry) = entry {
            claims.roles.retain(|r| entry.roles.contains(r));
            claims.permissions.retain(|p| entry.permissions.contains(p));
        }
    }

    /// 记录审计事件（best-effort，失败不阻断）
    async fn record_audit(
        &self,
        event_type: AuditEventType,
        user_id: Option<i64>,
        device_id: Option<String>,
        detail: Option<String>,
    ) {
        if let Some(ref store) = self.audit_store {
            let event = AuditEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                event_type,
                user_id,
                device_id,
                timestamp: chrono::Utc::now().timestamp(),
                ip: None,
                detail,
            };
            if let Err(e) = store.record(event).await {
                tracing::warn!(error = %e, "failed to record audit event");
            }
        }
    }

    /// 查询用户审计事件
    pub async fn query_audit(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, RefreshTokenError> {
        let store = self.audit_store.as_ref().ok_or_else(|| {
            RefreshTokenError::InvalidConfig("audit store not configured".to_string())
        })?;
        store.query_by_user(user_id, limit).await
    }

    /// 获取用户信息
    #[tracing::instrument(skip(self), fields(user_id = user_id))]
    pub async fn me(&self, user_id: i64) -> Result<UserInfo, RefreshTokenError> {
        self.user_auth.get_user_info(user_id).await
    }
}

// ── axum HTTP 端点（feature = "axum"） ──

/// axum 路由集成
#[cfg(feature = "axum")]
pub mod axum_routes {
    use super::*;
    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::{delete, get, post};
    use axum::Json;
    use axum::Router;
    use serde::Deserialize;

    /// SSO 共享状态
    pub type SsoState = Arc<SsoService>;

    /// 构建 SSO 路由
    pub fn sso_routes() -> Router<SsoState> {
        Router::new()
            .route("/sso/login", post(login))
            .route("/sso/refresh", post(refresh))
            .route("/sso/revoke", post(revoke))
            .route("/sso/validate", get(validate))
            .route("/sso/me/:user_id", get(me))
            .route("/sso/devices/:user_id", get(list_devices_handler))
            .route("/sso/devices/revoke", post(revoke_device_handler))
            .route("/sso/devices/heartbeat", post(heartbeat_handler))
            .route("/sso/degrade/user", post(degrade_user_handler))
            .route("/sso/degrade/device", post(degrade_device_handler))
            .route(
                "/sso/degrade/user/:user_id",
                delete(clear_degradation_handler),
            )
            .route(
                "/sso/degrade/device/:user_id/:device_id",
                delete(clear_device_degradation_handler),
            )
            .route("/sso/degrade/:user_id", get(get_degradation_handler))
    }

    #[derive(Deserialize)]
    struct LoginRequest {
        username: String,
        password: String,
        #[serde(default)]
        device_info: Option<DeviceInfo>,
    }

    #[derive(Deserialize)]
    struct TokenRequest {
        token: String,
    }

    #[derive(serde::Serialize)]
    struct ErrorResponse {
        code: i32,
        msg: String,
    }

    #[derive(serde::Serialize)]
    struct SuccessResponse<T: serde::Serialize> {
        code: i32,
        msg: String,
        data: T,
    }

    #[derive(serde::Serialize)]
    struct ValidateResponse {
        valid: bool,
        user_id: i64,
        expires_at: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        new_access_token: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        new_access_expires_at: Option<i64>,
    }

    fn error_response(err: RefreshTokenError) -> Response {
        let (status, msg) = match &err {
            RefreshTokenError::InvalidCredentials => (StatusCode::UNAUTHORIZED, err.to_string()),
            RefreshTokenError::Expired | RefreshTokenError::Revoked => {
                (StatusCode::UNAUTHORIZED, err.to_string())
            }
            RefreshTokenError::WrongTokenType { .. } => (StatusCode::UNAUTHORIZED, err.to_string()),
            RefreshTokenError::IssuerMismatch { .. } => (StatusCode::UNAUTHORIZED, err.to_string()),
            RefreshTokenError::VersionMismatch { .. } => {
                (StatusCode::UNAUTHORIZED, err.to_string())
            }
            RefreshTokenError::ReuseDetected => (StatusCode::UNAUTHORIZED, err.to_string()),
            RefreshTokenError::InvalidSignature => (StatusCode::UNAUTHORIZED, err.to_string()),
            RefreshTokenError::UserNotFound => (StatusCode::NOT_FOUND, err.to_string()),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
        };
        (
            status,
            [("Cache-Control", "no-store"), ("Pragma", "no-cache")],
            Json(ErrorResponse { code: -1, msg }),
        )
            .into_response()
    }

    fn success_response<T: serde::Serialize>(data: T) -> Response {
        (
            StatusCode::OK,
            [("Cache-Control", "no-store"), ("Pragma", "no-cache")],
            Json(SuccessResponse {
                code: 0,
                msg: "success".to_string(),
                data,
            }),
        )
            .into_response()
    }

    /// POST /sso/login
    async fn login(State(sso): State<SsoState>, Json(req): Json<LoginRequest>) -> Response {
        let result = if let Some(ref device_info) = req.device_info {
            sso.login_with_device(&req.username, &req.password, device_info)
                .await
        } else {
            sso.login(&req.username, &req.password).await
        };
        match result {
            Ok(resp) => success_response(resp),
            Err(err) => error_response(err),
        }
    }

    /// POST /sso/refresh
    async fn refresh(State(sso): State<SsoState>, Json(req): Json<TokenRequest>) -> Response {
        match sso.refresh(&req.token).await {
            Ok(pair) => success_response(pair),
            Err(err) => error_response(err),
        }
    }

    /// POST /sso/revoke
    async fn revoke(State(sso): State<SsoState>, Json(req): Json<TokenRequest>) -> Response {
        match sso.revoke(&req.token).await {
            Ok(()) => success_response(serde_json::json!({ "revoked": true })),
            Err(err) => error_response(err),
        }
    }

    /// GET /sso/validate
    async fn validate(
        State(sso): State<SsoState>,
        axum::extract::Query(params): axum::extract::Query<ValidateQuery>,
    ) -> Response {
        match sso.validate_with_renewal(&params.token).await {
            Ok((claims, renewed)) => success_response(ValidateResponse {
                valid: true,
                user_id: claims.user_id.unwrap_or(0),
                expires_at: claims.exp,
                new_access_token: renewed.as_ref().map(|r| r.access_token.clone()),
                new_access_expires_at: renewed.map(|r| r.expires_at),
            }),
            Err(err) => error_response(err),
        }
    }

    #[derive(Deserialize)]
    struct ValidateQuery {
        token: String,
    }

    /// GET /sso/me/:user_id
    async fn me(State(sso): State<SsoState>, Path(user_id): Path<i64>) -> Response {
        match sso.me(user_id).await {
            Ok(info) => success_response(info),
            Err(err) => error_response(err),
        }
    }

    /// GET /sso/devices/:user_id
    async fn list_devices_handler(
        State(sso): State<SsoState>,
        Path(user_id): Path<i64>,
    ) -> Response {
        match sso.list_devices(user_id).await {
            Ok(devices) => success_response(serde_json::json!({
                "devices": devices,
                "count": devices.len(),
            })),
            Err(err) => error_response(err),
        }
    }

    /// POST /sso/devices/revoke
    #[derive(Deserialize)]
    struct DeviceRevokeRequest {
        user_id: i64,
        device_id: String,
    }

    async fn revoke_device_handler(
        State(sso): State<SsoState>,
        Json(req): Json<DeviceRevokeRequest>,
    ) -> Response {
        match sso.revoke_device(req.user_id, &req.device_id).await {
            Ok(()) => success_response(serde_json::json!({ "revoked": true })),
            Err(err) => error_response(err),
        }
    }

    /// POST /sso/devices/heartbeat
    #[derive(Deserialize)]
    struct DeviceHeartbeatRequest {
        user_id: i64,
        device_id: String,
    }

    async fn heartbeat_handler(
        State(sso): State<SsoState>,
        Json(req): Json<DeviceHeartbeatRequest>,
    ) -> Response {
        match sso.update_device_active(req.user_id, &req.device_id).await {
            Ok(()) => success_response(serde_json::json!({ "updated": true })),
            Err(err) => error_response(err),
        }
    }

    // ── 降级端点 ──

    #[derive(Deserialize)]
    struct DegradeUserRequest {
        user_id: i64,
        roles: Vec<String>,
        permissions: Vec<String>,
        ttl_secs: u64,
    }

    async fn degrade_user_handler(
        State(sso): State<SsoState>,
        Json(req): Json<DegradeUserRequest>,
    ) -> Response {
        match sso
            .degrade_user(req.user_id, req.roles, req.permissions, req.ttl_secs)
            .await
        {
            Ok(()) => success_response(serde_json::json!({ "degraded": true })),
            Err(err) => error_response(err),
        }
    }

    #[derive(Deserialize)]
    struct DegradeDeviceRequest {
        user_id: i64,
        device_id: String,
        roles: Vec<String>,
        permissions: Vec<String>,
        ttl_secs: u64,
    }

    async fn degrade_device_handler(
        State(sso): State<SsoState>,
        Json(req): Json<DegradeDeviceRequest>,
    ) -> Response {
        match sso
            .degrade_device(
                req.user_id,
                &req.device_id,
                req.roles,
                req.permissions,
                req.ttl_secs,
            )
            .await
        {
            Ok(()) => success_response(serde_json::json!({ "degraded": true })),
            Err(err) => error_response(err),
        }
    }

    async fn clear_degradation_handler(
        State(sso): State<SsoState>,
        Path(user_id): Path<i64>,
    ) -> Response {
        match sso.clear_degradation(user_id).await {
            Ok(()) => success_response(serde_json::json!({ "cleared": true })),
            Err(err) => error_response(err),
        }
    }

    async fn clear_device_degradation_handler(
        State(sso): State<SsoState>,
        Path((user_id, device_id)): Path<(i64, String)>,
    ) -> Response {
        match sso.clear_device_degradation(user_id, &device_id).await {
            Ok(()) => success_response(serde_json::json!({ "cleared": true })),
            Err(err) => error_response(err),
        }
    }

    async fn get_degradation_handler(
        State(sso): State<SsoState>,
        Path(user_id): Path<i64>,
    ) -> Response {
        match sso.get_degradation(user_id).await {
            Ok(entry) => success_response(serde_json::json!({ "degradation": entry })),
            Err(err) => error_response(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refresh::{
        MemoryRefreshTokenStore, MemoryTokenBlacklist, RefreshTokenConfig, RefreshTokenStore,
        SsoJwtCodec, TokenBlacklist,
    };

    struct MockUserAuth {
        users: parking_lot::RwLock<std::collections::HashMap<String, (String, UserInfo)>>,
    }

    impl MockUserAuth {
        fn new() -> Self {
            let users = std::collections::HashMap::from([(
                "user1".to_string(),
                (
                    "pass1".to_string(),
                    UserInfo {
                        user_id: 1,
                        username: "user1".to_string(),
                        roles: vec!["admin".to_string(), "user".to_string()],
                        permissions: vec!["read".to_string(), "write".to_string()],
                    },
                ),
            )]);
            Self {
                users: parking_lot::RwLock::new(users),
            }
        }
    }

    #[async_trait::async_trait]
    impl UserAuthService for MockUserAuth {
        async fn authenticate(
            &self,
            username: &str,
            password: &str,
        ) -> Result<UserInfo, RefreshTokenError> {
            let users = self.users.read();
            match users.get(username) {
                Some((stored_pass, info)) if stored_pass == password => Ok(info.clone()),
                _ => Err(RefreshTokenError::InvalidCredentials),
            }
        }

        async fn get_user_info(&self, user_id: i64) -> Result<UserInfo, RefreshTokenError> {
            let users = self.users.read();
            users
                .values()
                .find(|(_, info)| info.user_id == user_id)
                .map(|(_, info)| info.clone())
                .ok_or(RefreshTokenError::UserNotFound)
        }
    }

    fn make_sso_service() -> SsoService {
        let codec = SsoJwtCodec::new("test-secret");
        let blacklist: Arc<dyn TokenBlacklist> = Arc::new(MemoryTokenBlacklist::new());
        let store: Arc<dyn RefreshTokenStore> = Arc::new(MemoryRefreshTokenStore::new());
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
        let user_auth: Arc<dyn UserAuthService> = Arc::new(MockUserAuth::new());
        SsoService::new(issuer, verifier, revoker, user_auth)
    }

    #[tokio::test]
    async fn test_sso_login_success() {
        let sso = make_sso_service();
        let resp = sso.login("user1", "pass1").await.unwrap();
        assert_eq!(resp.user_id, 1);
        assert_eq!(resp.username, "user1");
        assert!(!resp.tokens.access_token.is_empty());
        assert!(!resp.tokens.refresh_token.is_empty());
    }

    #[tokio::test]
    async fn test_sso_login_wrong_password() {
        let sso = make_sso_service();
        let result = sso.login("user1", "wrong").await;
        assert!(matches!(result, Err(RefreshTokenError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn test_sso_login_empty_credentials() {
        let sso = make_sso_service();
        assert!(matches!(
            sso.login("", "pass").await,
            Err(RefreshTokenError::InvalidCredentials)
        ));
        assert!(matches!(
            sso.login("user", "").await,
            Err(RefreshTokenError::InvalidCredentials)
        ));
    }

    #[tokio::test]
    async fn test_sso_refresh_and_validate() {
        let sso = make_sso_service();
        let login_resp = sso.login("user1", "pass1").await.unwrap();

        let claims = sso.validate(&login_resp.tokens.access_token).await.unwrap();
        assert_eq!(claims.user_id, Some(1));

        let new_tokens = sso.refresh(&login_resp.tokens.refresh_token).await.unwrap();
        let new_claims = sso.validate(&new_tokens.access_token).await.unwrap();
        assert_eq!(new_claims.user_id, Some(1));
    }

    #[tokio::test]
    async fn test_sso_revoke_and_me() {
        let sso = make_sso_service();
        let login_resp = sso.login("user1", "pass1").await.unwrap();

        sso.revoke(&login_resp.tokens.refresh_token).await.unwrap();
        let result = sso.refresh(&login_resp.tokens.refresh_token).await;
        assert!(matches!(result, Err(RefreshTokenError::ReuseDetected)));

        let user_info = sso.me(1).await.unwrap();
        assert_eq!(user_info.username, "user1");
    }

    #[tokio::test]
    async fn test_sso_revoke_all() {
        let sso = make_sso_service();
        let login_resp = sso.login("user1", "pass1").await.unwrap();

        sso.revoke_all(1).await.unwrap();
        let result = sso.validate(&login_resp.tokens.access_token).await;
        assert!(matches!(
            result,
            Err(RefreshTokenError::VersionMismatch { .. })
        ));
    }

    // ── validate_with_renewal 单元测试 ──

    fn make_sso_service_with_short_ttl() -> SsoService {
        let codec = SsoJwtCodec::new("test-secret");
        let blacklist: Arc<dyn TokenBlacklist> = Arc::new(MemoryTokenBlacklist::new());
        let store: Arc<dyn RefreshTokenStore> = Arc::new(MemoryRefreshTokenStore::new());
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
        let user_auth: Arc<dyn UserAuthService> = Arc::new(MockUserAuth::new());
        let mut sso = SsoService::new(issuer, verifier, revoker, user_auth);
        sso.with_renewal_config(RenewalConfig {
            enabled: true,
            renewal_threshold: chrono::Duration::seconds(30),
            renewal_ratio: 0.2,
            access_token_ttl: chrono::Duration::seconds(60),
        });
        sso
    }

    fn make_sso_service_disabled_renewal() -> SsoService {
        let mut sso = make_sso_service();
        sso.with_renewal_config(RenewalConfig {
            enabled: false,
            ..Default::default()
        });
        sso
    }

    #[tokio::test]
    async fn test_validate_with_renewal_triggers() {
        let sso = make_sso_service_with_short_ttl();
        let login_resp = sso.login("user1", "pass1").await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(35)).await;

        let (claims, renewed) = sso
            .validate_with_renewal(&login_resp.tokens.access_token)
            .await
            .unwrap();
        assert_eq!(claims.user_id, Some(1));
        assert!(renewed.is_some());
        let renewed = renewed.unwrap();
        assert!(!renewed.access_token.is_empty());
        assert!(renewed.expires_at > chrono::Utc::now().timestamp());
    }

    #[tokio::test]
    async fn test_validate_with_renewal_no_trigger() {
        let sso = make_sso_service_with_short_ttl();
        let login_resp = sso.login("user1", "pass1").await.unwrap();

        let (claims, renewed) = sso
            .validate_with_renewal(&login_resp.tokens.access_token)
            .await
            .unwrap();
        assert_eq!(claims.user_id, Some(1));
        assert!(renewed.is_none());
    }

    #[tokio::test]
    async fn test_validate_with_renewal_disabled() {
        let sso = make_sso_service_disabled_renewal();
        let login_resp = sso.login("user1", "pass1").await.unwrap();

        let (claims, renewed) = sso
            .validate_with_renewal(&login_resp.tokens.access_token)
            .await
            .unwrap();
        assert_eq!(claims.user_id, Some(1));
        assert!(renewed.is_none());
    }

    #[tokio::test]
    async fn test_validate_with_renewal_invalid_token() {
        let sso = make_sso_service();
        let result = sso.validate_with_renewal("invalid.token.here").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_with_renewal_revoked_token() {
        let sso = make_sso_service();
        let login_resp = sso.login("user1", "pass1").await.unwrap();

        sso.revoke(&login_resp.tokens.access_token).await.unwrap();
        let result = sso
            .validate_with_renewal(&login_resp.tokens.access_token)
            .await;
        assert!(matches!(result, Err(RefreshTokenError::Revoked)));
    }

    #[tokio::test]
    async fn test_validate_with_renewal_version_mismatch() {
        let sso = make_sso_service();
        let login_resp = sso.login("user1", "pass1").await.unwrap();

        sso.revoke_all(1).await.unwrap();
        let result = sso
            .validate_with_renewal(&login_resp.tokens.access_token)
            .await;
        assert!(matches!(
            result,
            Err(RefreshTokenError::VersionMismatch { .. })
        ));
    }

    #[tokio::test]
    async fn test_validate_with_renewal_preserves_claims() {
        let sso = make_sso_service_with_short_ttl();
        let login_resp = sso.login("user1", "pass1").await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(35)).await;

        let (claims, renewed) = sso
            .validate_with_renewal(&login_resp.tokens.access_token)
            .await
            .unwrap();
        assert_eq!(claims.user_id, Some(1));
        assert_eq!(claims.sub, "user1");
        assert!(renewed.is_some());
    }

    #[tokio::test]
    async fn test_validate_unchanged() {
        let sso = make_sso_service();
        let login_resp = sso.login("user1", "pass1").await.unwrap();

        let claims = sso.validate(&login_resp.tokens.access_token).await.unwrap();
        assert_eq!(claims.user_id, Some(1));
        assert_eq!(claims.sub, "user1");
    }

    // ── 多设备会话管理服务层测试 ──

    use crate::refresh::{DeviceSessionStore, MemoryDeviceSessionStore};

    fn make_sso_service_with_device_store() -> SsoService {
        let codec = SsoJwtCodec::new("test-secret");
        let blacklist: Arc<dyn TokenBlacklist> = Arc::new(MemoryTokenBlacklist::new());
        let store: Arc<dyn RefreshTokenStore> = Arc::new(MemoryRefreshTokenStore::new());
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
        let user_auth: Arc<dyn UserAuthService> = Arc::new(MockUserAuth::new());
        let device_store: Arc<dyn DeviceSessionStore> = Arc::new(MemoryDeviceSessionStore::new());
        let mut sso = SsoService::new(issuer, verifier, revoker, user_auth);
        sso.with_device_store(device_store, DeviceSessionConfig::default());
        sso
    }

    fn make_sso_service_with_degradation_store() -> SsoService {
        let mut sso = make_sso_service_with_device_store();
        sso.with_degradation_store(Arc::new(MemoryDegradationStore::new()));
        sso
    }

    fn make_sso_service_with_ticket_store() -> SsoService {
        let mut sso = make_sso_service_with_degradation_store();
        sso.with_ticket_store(Arc::new(MemoryTicketStore::new()));
        sso
    }

    fn make_sso_service_with_audit_store() -> SsoService {
        let mut sso = make_sso_service_with_ticket_store();
        sso.with_audit_store(Arc::new(MemoryAuditStore::new()));
        sso
    }

    #[tokio::test]
    async fn test_login_with_device_token_has_device_id() {
        let sso = make_sso_service_with_device_store();
        let device_info = DeviceInfo::with_device_id("dev-iphone");
        let resp = sso
            .login_with_device("user1", "pass1", &device_info)
            .await
            .unwrap();
        let claims = sso.validate(&resp.tokens.access_token).await.unwrap();
        assert_eq!(claims.device_id, Some("dev-iphone".to_string()));
    }

    #[tokio::test]
    async fn test_login_token_no_device_id() {
        let sso = make_sso_service_with_device_store();
        let resp = sso.login("user1", "pass1").await.unwrap();
        let claims = sso.validate(&resp.tokens.access_token).await.unwrap();
        assert!(claims.device_id.is_none());
    }

    #[tokio::test]
    async fn test_list_devices_returns_all() {
        let sso = make_sso_service_with_device_store();

        sso.login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev1"))
            .await
            .unwrap();
        sso.login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev2"))
            .await
            .unwrap();

        let devices = sso.list_devices(1).await.unwrap();
        assert_eq!(devices.len(), 2);
    }

    #[tokio::test]
    async fn test_revoke_device_only_affects_target() {
        let sso = make_sso_service_with_device_store();

        let resp1 = sso
            .login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev1"))
            .await
            .unwrap();
        let resp2 = sso
            .login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev2"))
            .await
            .unwrap();

        sso.revoke_device(1, "dev1").await.unwrap();

        let result = sso.validate(&resp1.tokens.access_token).await;
        assert!(result.is_err());

        sso.validate(&resp2.tokens.access_token).await.unwrap();

        let devices = sso.list_devices(1).await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_id, "dev2");
    }

    #[tokio::test]
    async fn test_revoke_all_clears_device_sessions() {
        let sso = make_sso_service_with_device_store();

        sso.login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev1"))
            .await
            .unwrap();
        sso.login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev2"))
            .await
            .unwrap();

        sso.revoke_all(1).await.unwrap();

        let devices = sso.list_devices(1).await.unwrap();
        assert!(devices.is_empty());
    }

    #[tokio::test]
    async fn test_update_device_active_updates_last_active() {
        let sso = make_sso_service_with_device_store();

        sso.login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev1"))
            .await
            .unwrap();

        let before = sso.list_devices(1).await.unwrap()[0].last_active;
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        sso.update_device_active(1, "dev1").await.unwrap();
        let after = sso.list_devices(1).await.unwrap()[0].last_active;
        assert!(after > before);
    }

    #[tokio::test]
    async fn test_cleanup_expired_devices() {
        let sso = make_sso_service_with_device_store();

        sso.login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev1"))
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        let count = sso.cleanup_expired_devices(1, 1).await.unwrap();
        assert_eq!(count, 1);

        let devices = sso.list_devices(1).await.unwrap();
        assert!(devices.is_empty());
    }

    #[tokio::test]
    async fn test_lru_eviction_on_max_devices() {
        let codec = SsoJwtCodec::new("test-secret");
        let blacklist: Arc<dyn TokenBlacklist> = Arc::new(MemoryTokenBlacklist::new());
        let store: Arc<dyn RefreshTokenStore> = Arc::new(MemoryRefreshTokenStore::new());
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
        let user_auth: Arc<dyn UserAuthService> = Arc::new(MockUserAuth::new());
        let device_store: Arc<dyn DeviceSessionStore> = Arc::new(MemoryDeviceSessionStore::new());
        let mut sso = SsoService::new(issuer, verifier, revoker, user_auth);
        sso.with_device_store(device_store, DeviceSessionConfig::new(2));

        sso.login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev1"))
            .await
            .unwrap();
        sso.login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev2"))
            .await
            .unwrap();
        sso.login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev3"))
            .await
            .unwrap();

        let devices = sso.list_devices(1).await.unwrap();
        assert_eq!(devices.len(), 2);
    }

    #[tokio::test]
    async fn test_same_device_relogin_overwrites_session() {
        let sso = make_sso_service_with_device_store();

        sso.login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev1"))
            .await
            .unwrap();
        sso.login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev1"))
            .await
            .unwrap();

        let devices = sso.list_devices(1).await.unwrap();
        assert_eq!(devices.len(), 1);
    }

    #[tokio::test]
    async fn test_device_methods_without_store_return_err() {
        let sso = make_sso_service();
        let result = sso.list_devices(1).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_updates_device_active() {
        let sso = make_sso_service_with_device_store();

        sso.login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev1"))
            .await
            .unwrap();

        let devices_before = sso.list_devices(1).await.unwrap();
        let before = devices_before[0].last_active;

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let login_resp = sso
            .login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev1"))
            .await
            .unwrap();
        sso.validate(&login_resp.tokens.access_token).await.unwrap();

        let devices_after = sso.list_devices(1).await.unwrap();
        let after = devices_after[0].last_active;
        assert!(after > before);
    }

    // ── Token 降级测试 ──

    #[tokio::test]
    async fn test_degrade_user_applies_on_validate() {
        let sso = make_sso_service_with_degradation_store();
        let login = sso.login("user1", "pass1").await.unwrap();

        let claims = sso.validate(&login.tokens.access_token).await.unwrap();
        assert!(claims.roles.contains(&"admin".to_string()));

        sso.degrade_user(1, vec!["user".to_string()], vec!["read".to_string()], 3600)
            .await
            .unwrap();

        let claims = sso.validate(&login.tokens.access_token).await.unwrap();
        assert!(!claims.roles.contains(&"admin".to_string()));
        assert!(claims.roles.contains(&"user".to_string()));
        assert_eq!(claims.permissions, vec!["read".to_string()]);
    }

    #[tokio::test]
    async fn test_clear_degradation_restores() {
        let sso = make_sso_service_with_degradation_store();
        let login = sso.login("user1", "pass1").await.unwrap();

        sso.degrade_user(1, vec!["user".to_string()], vec!["read".to_string()], 3600)
            .await
            .unwrap();
        let claims = sso.validate(&login.tokens.access_token).await.unwrap();
        assert_eq!(claims.roles, vec!["user".to_string()]);

        sso.clear_degradation(1).await.unwrap();
        let claims = sso.validate(&login.tokens.access_token).await.unwrap();
        assert!(claims.roles.contains(&"admin".to_string()));
    }

    #[tokio::test]
    async fn test_degradation_cannot_escalate() {
        let sso = make_sso_service_with_degradation_store();
        let login = sso.login("user1", "pass1").await.unwrap();

        sso.degrade_user(1, vec!["superadmin".to_string()], vec![], 3600)
            .await
            .unwrap();

        let claims = sso.validate(&login.tokens.access_token).await.unwrap();
        assert!(claims.roles.is_empty());
    }

    #[tokio::test]
    async fn test_degradation_ttl_expired() {
        let sso = make_sso_service_with_degradation_store();
        let login = sso.login("user1", "pass1").await.unwrap();

        sso.degrade_user(1, vec!["user".to_string()], vec![], 1)
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        let claims = sso.validate(&login.tokens.access_token).await.unwrap();
        assert!(claims.roles.contains(&"admin".to_string()));
    }

    #[tokio::test]
    async fn test_revoke_all_clears_degradation() {
        let sso = make_sso_service_with_degradation_store();
        let _login = sso.login("user1", "pass1").await.unwrap();

        sso.degrade_user(1, vec!["user".to_string()], vec![], 3600)
            .await
            .unwrap();
        assert!(sso.get_degradation(1).await.unwrap().is_some());

        sso.revoke_all(1).await.unwrap();
        assert!(sso.get_degradation(1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_device_degradation_priority() {
        let sso = make_sso_service_with_degradation_store();
        let login = sso
            .login_with_device("user1", "pass1", &DeviceInfo::with_device_id("dev1"))
            .await
            .unwrap();

        sso.degrade_user(1, vec!["user".to_string()], vec![], 3600)
            .await
            .unwrap();
        sso.degrade_device(
            1,
            "dev1",
            vec!["admin".to_string(), "user".to_string()],
            vec![],
            3600,
        )
        .await
        .unwrap();

        let claims = sso.validate(&login.tokens.access_token).await.unwrap();
        assert_eq!(claims.roles, vec!["admin".to_string(), "user".to_string()]);

        sso.clear_device_degradation(1, "dev1").await.unwrap();
        let claims = sso.validate(&login.tokens.access_token).await.unwrap();
        assert_eq!(claims.roles, vec!["user".to_string()]);
    }

    #[tokio::test]
    async fn test_degradation_without_store_return_err() {
        let sso = make_sso_service();
        let result = sso.degrade_user(1, vec![], vec![], 3600).await;
        assert!(result.is_err());
    }

    // ── SSO 跨域 Ticket 测试 ──

    #[tokio::test]
    async fn test_generate_and_exchange_ticket() {
        let sso = make_sso_service_with_ticket_store();
        sso.login("user1", "pass1").await.unwrap();

        let ticket = sso
            .generate_ticket(1, "https://b.example.com/callback")
            .await
            .unwrap();
        assert!(!ticket.is_empty());

        let tokens = sso.exchange_ticket(&ticket).await.unwrap();
        assert!(!tokens.access_token.is_empty());
        assert!(!tokens.refresh_token.is_empty());
    }

    #[tokio::test]
    async fn test_ticket_one_time_use() {
        let sso = make_sso_service_with_ticket_store();
        sso.login("user1", "pass1").await.unwrap();

        let ticket = sso
            .generate_ticket(1, "https://b.example.com/callback")
            .await
            .unwrap();
        sso.exchange_ticket(&ticket).await.unwrap();

        let result = sso.exchange_ticket(&ticket).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ticket_ttl_expired() {
        let sso = make_sso_service_with_ticket_store();
        sso.login("user1", "pass1").await.unwrap();

        let ticket = sso
            .generate_ticket(1, "https://b.example.com/callback")
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_secs(31)).await;

        let result = sso.exchange_ticket(&ticket).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_ticket_without_consuming() {
        let sso = make_sso_service_with_ticket_store();
        sso.login("user1", "pass1").await.unwrap();

        let ticket = sso
            .generate_ticket(1, "https://b.example.com/callback")
            .await
            .unwrap();
        let result = sso.validate_ticket(&ticket).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().user_id, 1);

        let tokens = sso.exchange_ticket(&ticket).await.unwrap();
        assert!(!tokens.access_token.is_empty());
    }

    #[tokio::test]
    async fn test_ticket_without_store_return_err() {
        let sso = make_sso_service();
        let result = sso.generate_ticket(1, "https://b.example.com").await;
        assert!(result.is_err());
    }

    // ── 审计日志测试 ──

    #[tokio::test]
    async fn test_audit_records_login() {
        let sso = make_sso_service_with_audit_store();
        sso.login("user1", "pass1").await.unwrap();

        let events = sso.query_audit(1, 10).await.unwrap();
        assert!(events.iter().any(|e| e.event_type == AuditEventType::Login));
    }

    #[tokio::test]
    async fn test_audit_records_revoke_all() {
        let sso = make_sso_service_with_audit_store();
        sso.login("user1", "pass1").await.unwrap();
        sso.revoke_all(1).await.unwrap();

        let events = sso.query_audit(1, 10).await.unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == AuditEventType::RevokeAll));
    }

    #[tokio::test]
    async fn test_audit_records_degrade() {
        let sso = make_sso_service_with_audit_store();
        sso.login("user1", "pass1").await.unwrap();
        sso.degrade_user(1, vec!["user".to_string()], vec![], 3600)
            .await
            .unwrap();

        let events = sso.query_audit(1, 10).await.unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == AuditEventType::Degrade));
    }

    #[tokio::test]
    async fn test_audit_records_ticket_generate_and_exchange() {
        let sso = make_sso_service_with_audit_store();
        sso.login("user1", "pass1").await.unwrap();

        let ticket = sso
            .generate_ticket(1, "https://b.example.com")
            .await
            .unwrap();
        sso.exchange_ticket(&ticket).await.unwrap();

        let events = sso.query_audit(1, 10).await.unwrap();
        assert!(events
            .iter()
            .any(|e| e.event_type == AuditEventType::TicketGenerate));
        assert!(events
            .iter()
            .any(|e| e.event_type == AuditEventType::TicketExchange));
    }

    #[tokio::test]
    async fn test_audit_query_without_store_return_err() {
        let sso = make_sso_service();
        let result = sso.query_audit(1, 10).await;
        assert!(result.is_err());
    }
}
