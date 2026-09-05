// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Refresh Token 双 Token 机制
//!
//! 对齐 spec.md FR-1 ~ FR-4，design.md §2.1 ~ §2.6。
//!
//! ## 核心组件
//!
//! - [`SsoJwtCodec`]：JWT HS256 编解码（支持 token_type/jti/ver 自定义 claim）
//! - [`SsoClaims`]：JWT claims 结构体（JwtClaims 超集）
//! - [`RefreshTokenIssuer`]：签发 + 轮换（T4 实现）
//! - [`RefreshTokenVerifier`]：校验（T3 实现）
//! - [`RefreshTokenRevoker`]：撤销（T5 实现）
//! - [`RefreshTokenStore`]：存储抽象 trait（T2 实现）

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;
use subtle::ConstantTimeEq;

// ── 错误类型 ──

/// Refresh Token 错误类型
#[derive(Debug, thiserror::Error)]
pub enum RefreshTokenError {
    /// 用户名或密码为空
    #[error("invalid credentials")]
    InvalidCredentials,
    /// JWT 签名无效
    #[error("invalid signature")]
    InvalidSignature,
    /// Token 已过期
    #[error("token expired")]
    Expired,
    /// Token 类型不匹配（access vs refresh）
    #[error("wrong token type: expected {expected}, got {actual}")]
    WrongTokenType {
        /// 期望的 Token 类型
        expected: String,
        /// 实际的 Token 类型
        actual: String,
    },
    /// Token 已被撤销（在黑名单中）
    #[error("token revoked")]
    Revoked,
    /// JWT 签发人不匹配
    #[error("issuer mismatch: expected {expected}, got {actual}")]
    IssuerMismatch {
        /// 期望的签发人
        expected: String,
        /// 实际的签发人
        actual: String,
    },
    /// Token 版本不匹配（用户级撤销）
    #[error("token version mismatch: token ver={token_ver}, current ver={current_ver}")]
    VersionMismatch {
        /// Token 中的版本号
        token_ver: u64,
        /// 当前用户的版本号
        current_ver: u64,
    },
    /// Refresh Token 复用攻击检测
    #[error("refresh token reuse detected, all tokens for user revoked")]
    ReuseDetected,
    /// 服务不可用（DB/Cache 故障）
    #[error("service unavailable")]
    ServiceUnavailable,
    /// 缓存错误
    #[error("cache error: {0}")]
    Cache(String),
    /// JWT 编解码错误
    #[error("jwt error: {0}")]
    Jwt(String),
    /// 用户不存在
    #[error("user not found")]
    UserNotFound,
    /// 配置无效（如 reqwest::Client 构建失败）
    #[error("invalid config: {0}")]
    InvalidConfig(String),
}

// ── SsoClaims ──

fn default_token_type() -> String {
    "access".to_string()
}

/// SSO JWT claims — JwtClaims 的超集，新增 token_type / jti / ver
///
/// 对齐 design.md §2.1。`token_type` 区分 access/refresh，
/// `jti` 用于黑名单精确定位，`ver` 用于用户级撤销。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SsoClaims {
    /// 主体（用户名）
    pub sub: String,
    /// 过期时间（Unix 时间戳）
    pub exp: i64,
    /// 签发时间（Unix 时间戳）
    pub iat: i64,
    /// 签发人
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    /// 用户 ID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
    /// Token 类型：`"access"` 或 `"refresh"`
    #[serde(default = "default_token_type")]
    pub token_type: String,
    /// JWT ID（用于黑名单精确定位与审计）
    #[serde(default)]
    pub jti: String,
    /// Token 版本（用户级撤销，每次撤销所有 Token 时递增）
    #[serde(default)]
    pub ver: u64,
    /// 用户角色
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    /// 用户权限
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<String>,
    /// 设备 ID（多设备会话管理，None = 未绑定设备）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
}

impl SsoClaims {
    /// 创建 accessToken claims
    pub fn access(user_id: i64, username: &str, exp: i64, issuer: &str, ver: u64) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            sub: username.to_string(),
            exp,
            iat: now,
            iss: Some(issuer.to_string()),
            user_id: Some(user_id),
            token_type: "access".to_string(),
            jti: String::new(),
            ver,
            roles: Vec::new(),
            permissions: Vec::new(),
            device_id: None,
        }
    }

    /// 创建 refreshToken claims
    pub fn refresh(
        user_id: i64,
        username: &str,
        exp: i64,
        issuer: &str,
        ver: u64,
        jti: String,
    ) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            sub: username.to_string(),
            exp,
            iat: now,
            iss: Some(issuer.to_string()),
            user_id: Some(user_id),
            token_type: "refresh".to_string(),
            jti,
            ver,
            roles: Vec::new(),
            permissions: Vec::new(),
            device_id: None,
        }
    }

    /// 是否已过期
    pub fn is_expired(&self) -> bool {
        chrono::Utc::now().timestamp() >= self.exp
    }

    /// 是否为 accessToken
    pub fn is_access(&self) -> bool {
        self.token_type == "access"
    }

    /// 是否为 refreshToken
    pub fn is_refresh(&self) -> bool {
        self.token_type == "refresh"
    }
}

// ── SsoJwtCodec ──

type HmacSha256 = Hmac<Sha256>;

/// JWT header（固定 alg=HS256, typ=JWT）
const JWT_HEADER: &str = "{\"alg\":\"HS256\",\"typ\":\"JWT\"}";

/// SSO JWT 编解码器 — HS256 签名/验签
///
/// 复用 RustCrypto audited crate（hmac + sha2 + base64 + subtle），
/// 不修改上游 sz-orm-auth。支持 token_type/jti/ver 自定义 claim。
#[derive(Clone)]
pub struct SsoJwtCodec {
    secret: String,
}

impl SsoJwtCodec {
    /// 创建编解码器
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
        }
    }

    /// 编码 JWT
    ///
    /// 格式：`base64url(header).base64url(payload).base64url(signature)`
    pub fn encode(&self, claims: &SsoClaims) -> Result<String, RefreshTokenError> {
        let header_b64 = URL_SAFE_NO_PAD.encode(JWT_HEADER.as_bytes());
        let payload_json =
            serde_json::to_string(claims).map_err(|e| RefreshTokenError::Jwt(e.to_string()))?;
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());

        let signing_input = format!("{header_b64}.{payload_b64}");
        let mut mac = HmacSha256::new_from_slice(self.secret.as_bytes())
            .map_err(|e| RefreshTokenError::Jwt(e.to_string()))?;
        mac.update(signing_input.as_bytes());
        let sig = mac.finalize().into_bytes();
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig);

        Ok(format!("{signing_input}.{sig_b64}"))
    }

    /// 解码并验签 JWT
    ///
    /// 校验链：签名有效 → alg=HS256 → 过期检查
    pub fn decode(&self, token: &str) -> Result<SsoClaims, RefreshTokenError> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(RefreshTokenError::InvalidSignature);
        }

        // 验签
        let signing_input = format!("{}.{}", parts[0], parts[1]);
        let sig_bytes = URL_SAFE_NO_PAD
            .decode(parts[2])
            .map_err(|_| RefreshTokenError::InvalidSignature)?;
        let mut mac = HmacSha256::new_from_slice(self.secret.as_bytes())
            .map_err(|e| RefreshTokenError::Jwt(e.to_string()))?;
        mac.update(signing_input.as_bytes());
        let expected_sig = mac.finalize().into_bytes();
        if sig_bytes.ct_eq(&expected_sig).unwrap_u8() == 0 {
            return Err(RefreshTokenError::InvalidSignature);
        }

        // 校验 header alg
        let header_bytes = URL_SAFE_NO_PAD
            .decode(parts[0])
            .map_err(|_| RefreshTokenError::InvalidSignature)?;
        let header: serde_json::Value = serde_json::from_slice(&header_bytes)
            .map_err(|e| RefreshTokenError::Jwt(e.to_string()))?;
        let alg = header.get("alg").and_then(|v| v.as_str()).unwrap_or("");
        if alg != "HS256" {
            return Err(RefreshTokenError::InvalidSignature);
        }

        // 解析 payload
        let payload_bytes = URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|_| RefreshTokenError::InvalidSignature)?;
        let claims: SsoClaims = serde_json::from_slice(&payload_bytes)
            .map_err(|e| RefreshTokenError::Jwt(e.to_string()))?;

        // 过期检查
        if claims.is_expired() {
            return Err(RefreshTokenError::Expired);
        }

        Ok(claims)
    }
}

impl std::fmt::Debug for SsoJwtCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SsoJwtCodec")
            .field("secret", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

// ── TokenPair ──

/// 双 Token 响应
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct TokenPair {
    /// 短期 accessToken
    pub access_token: String,
    /// 长期 refreshToken
    pub refresh_token: String,
    /// accessToken 过期时间（Unix 时间戳）
    pub access_expires_at: i64,
    /// refreshToken 过期时间（Unix 时间戳）
    pub refresh_expires_at: i64,
}

impl std::fmt::Debug for TokenPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenPair")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("access_expires_at", &self.access_expires_at)
            .field("refresh_expires_at", &self.refresh_expires_at)
            .finish()
    }
}

// ── RefreshTokenConfig ──

/// Refresh Token 配置
#[derive(Debug, Clone)]
pub struct RefreshTokenConfig {
    /// accessToken 有效期（默认 900 秒 = 15 分钟）
    pub access_token_ttl: chrono::Duration,
    /// refreshToken 有效期（默认 604800 秒 = 7 天）
    pub refresh_token_ttl: chrono::Duration,
    /// JWT 签发人（默认 "sz-rust-sso"）
    pub issuer: String,
}

impl Default for RefreshTokenConfig {
    fn default() -> Self {
        Self {
            access_token_ttl: chrono::Duration::seconds(900),
            refresh_token_ttl: chrono::Duration::seconds(604800),
            issuer: "sz-rust-sso".to_string(),
        }
    }
}

// ── RenewalConfig ──

/// Token 自动续期配置
///
/// 在 `validate`（校验 accessToken）时，如果剩余 TTL 低于阈值，
/// 自动签发新 accessToken 并随响应返回，客户端无需主动调用 refresh 端点。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RenewalConfig {
    /// 是否启用自动续期
    pub enabled: bool,
    /// 续期阈值（剩余 TTL < 此值时触发续期），默认 300 秒 = 5 分钟
    pub renewal_threshold: chrono::Duration,
    /// 续期比例（剩余 TTL < access_token_ttl * ratio 时触发续期），默认 0.2
    pub renewal_ratio: f64,
    /// accessToken 有效期（用于计算 ratio 阈值），默认 900 秒 = 15 分钟
    pub access_token_ttl: chrono::Duration,
}

impl Default for RenewalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            renewal_threshold: chrono::Duration::seconds(300),
            renewal_ratio: 0.2,
            access_token_ttl: chrono::Duration::seconds(900),
        }
    }
}

impl RenewalConfig {
    /// 判定是否需要续期
    ///
    /// 算法：
    /// - `enabled=false` → false
    /// - `threshold_secs==0` → `remaining_ttl > 0`（未过期即续期）
    /// - 否则 → `remaining_ttl < max(threshold_secs, access_token_ttl * ratio)`
    pub fn should_renew(&self, remaining_ttl: i64) -> bool {
        if !self.enabled {
            return false;
        }
        let threshold_secs = self.renewal_threshold.num_seconds();
        if threshold_secs == 0 {
            return remaining_ttl > 0;
        }
        let ratio_secs = (self.access_token_ttl.num_seconds() as f64 * self.renewal_ratio) as i64;
        let effective_threshold = threshold_secs.max(ratio_secs);
        remaining_ttl < effective_threshold
    }
}

// ── RenewedToken ──

/// 续期结果载体
#[derive(Clone)]
pub struct RenewedToken {
    /// 新签发的 accessToken
    pub access_token: String,
    /// 新 accessToken 的过期时间（Unix 时间戳）
    pub expires_at: i64,
}

impl std::fmt::Debug for RenewedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenewedToken")
            .field("access_token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

// ── DeviceInfo / DeviceSession / DeviceSessionStore ──

/// 设备信息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct DeviceInfo {
    /// 设备唯一标识（UUID v4）
    pub device_id: String,
    /// 设备类型（web / ios / android / pc）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_type: Option<String>,
    /// 浏览器/客户端 User-Agent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// 登录 IP
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// 设备名称（如 "iPhone 15 Pro"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
}

impl DeviceInfo {
    /// 创建设备信息（自动生成 UUID v4 作为 device_id）
    pub fn new() -> Self {
        Self {
            device_id: uuid::Uuid::new_v4().to_string(),
            device_type: None,
            user_agent: None,
            ip: None,
            device_name: None,
        }
    }

    /// 显式指定 device_id
    pub fn with_device_id(device_id: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            device_type: None,
            user_agent: None,
            ip: None,
            device_name: None,
        }
    }
}

impl Default for DeviceInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// 设备会话
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct DeviceSession {
    /// 设备 ID
    pub device_id: String,
    /// 设备信息
    pub device_info: DeviceInfo,
    /// refreshToken 的 JWT ID（用于撤销时精确定位）
    pub jti: String,
    /// accessToken 的 JWT ID（用于撤销设备时同时拉黑 access token）
    pub access_jti: String,
    /// 会话创建时间（Unix 时间戳）
    pub created_at: i64,
    /// 最后活跃时间（Unix 时间戳）
    pub last_active: i64,
}

/// 设备会话配置
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceSessionConfig {
    /// 最大设备数量（默认 10，超出时 LRU 淘汰）
    pub max_devices: usize,
}

impl Default for DeviceSessionConfig {
    fn default() -> Self {
        Self { max_devices: 10 }
    }
}

impl DeviceSessionConfig {
    /// 创建配置，max_devices clamp 到 [1, 100]
    pub fn new(max_devices: usize) -> Self {
        let clamped = max_devices.clamp(1, 100);
        if clamped != max_devices {
            tracing::warn!(
                requested = max_devices,
                clamped,
                "max_devices clamped to [1, 100]"
            );
        }
        Self {
            max_devices: clamped,
        }
    }
}

/// 设备会话存储抽象
///
/// 维护 `user_id → {device_id → DeviceSession}` 映射。
/// 实现者需保证线程安全（`Send + Sync`）。
#[async_trait::async_trait]
pub trait DeviceSessionStore: Send + Sync {
    /// 注册设备会话（upsert 语义，覆盖同 device_id 旧会话）
    async fn register_session(
        &self,
        user_id: i64,
        device_id: &str,
        device_info: &DeviceInfo,
        jti: &str,
        access_jti: &str,
    ) -> Result<(), RefreshTokenError>;

    /// 查询用户所有在线设备
    async fn get_sessions(&self, user_id: i64) -> Result<Vec<DeviceSession>, RefreshTokenError>;

    /// 查询特定设备会话
    async fn get_session(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<Option<DeviceSession>, RefreshTokenError>;

    /// 撤销设备会话，返回被撤销会话的 (refresh_jti, access_jti)（用于加入黑名单）
    async fn revoke_session(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<Option<(String, String)>, RefreshTokenError>;

    /// 更新设备最后活跃时间
    async fn update_last_active(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<(), RefreshTokenError>;

    /// 更新会话 jti（refresh 轮换后调用）
    async fn update_session_jti(
        &self,
        user_id: i64,
        device_id: &str,
        new_jti: &str,
    ) -> Result<(), RefreshTokenError>;

    /// 清理过期会话，返回被清理会话的 (refresh_jti, access_jti) 列表
    async fn cleanup_expired(
        &self,
        user_id: i64,
        ttl_secs: i64,
    ) -> Result<Vec<(String, String)>, RefreshTokenError>;

    /// 清空用户所有会话，返回被清理会话的 (refresh_jti, access_jti) 列表
    async fn clear_user_sessions(
        &self,
        user_id: i64,
    ) -> Result<Vec<(String, String)>, RefreshTokenError>;
}

/// 内存设备会话存储（单进程，测试用）
pub struct MemoryDeviceSessionStore {
    inner: Arc<parking_lot::RwLock<std::collections::HashMap<(i64, String), DeviceSession>>>,
}

impl MemoryDeviceSessionStore {
    /// 创建空存储
    pub fn new() -> Self {
        Self {
            inner: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        }
    }
}

impl Default for MemoryDeviceSessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl DeviceSessionStore for MemoryDeviceSessionStore {
    async fn register_session(
        &self,
        user_id: i64,
        device_id: &str,
        device_info: &DeviceInfo,
        jti: &str,
        access_jti: &str,
    ) -> Result<(), RefreshTokenError> {
        let now = chrono::Utc::now().timestamp();
        let session = DeviceSession {
            device_id: device_id.to_string(),
            device_info: device_info.clone(),
            jti: jti.to_string(),
            access_jti: access_jti.to_string(),
            created_at: now,
            last_active: now,
        };
        self.inner
            .write()
            .insert((user_id, device_id.to_string()), session);
        Ok(())
    }

    async fn get_sessions(&self, user_id: i64) -> Result<Vec<DeviceSession>, RefreshTokenError> {
        let sessions: Vec<_> = self
            .inner
            .read()
            .iter()
            .filter(|((uid, _), _)| *uid == user_id)
            .map(|(_, s)| s.clone())
            .collect();
        Ok(sessions)
    }

    async fn get_session(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<Option<DeviceSession>, RefreshTokenError> {
        Ok(self
            .inner
            .read()
            .get(&(user_id, device_id.to_string()))
            .cloned())
    }

    async fn revoke_session(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<Option<(String, String)>, RefreshTokenError> {
        Ok(self
            .inner
            .write()
            .remove(&(user_id, device_id.to_string()))
            .map(|s| (s.jti, s.access_jti)))
    }

    async fn update_last_active(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<(), RefreshTokenError> {
        let now = chrono::Utc::now().timestamp();
        if let Some(session) = self
            .inner
            .write()
            .get_mut(&(user_id, device_id.to_string()))
        {
            session.last_active = now;
        }
        Ok(())
    }

    async fn update_session_jti(
        &self,
        user_id: i64,
        device_id: &str,
        new_jti: &str,
    ) -> Result<(), RefreshTokenError> {
        let now = chrono::Utc::now().timestamp();
        if let Some(session) = self
            .inner
            .write()
            .get_mut(&(user_id, device_id.to_string()))
        {
            session.jti = new_jti.to_string();
            session.last_active = now;
        }
        Ok(())
    }

    async fn cleanup_expired(
        &self,
        user_id: i64,
        ttl_secs: i64,
    ) -> Result<Vec<(String, String)>, RefreshTokenError> {
        let now = chrono::Utc::now().timestamp();
        let mut removed = Vec::new();
        let mut store = self.inner.write();
        let expired_keys: Vec<_> = store
            .iter()
            .filter(|((uid, _), s)| *uid == user_id && now - s.last_active > ttl_secs)
            .map(|(k, _)| k.clone())
            .collect();
        for key in expired_keys {
            if let Some(session) = store.remove(&key) {
                removed.push((session.jti, session.access_jti));
            }
        }
        Ok(removed)
    }

    async fn clear_user_sessions(
        &self,
        user_id: i64,
    ) -> Result<Vec<(String, String)>, RefreshTokenError> {
        let mut removed = Vec::new();
        let mut store = self.inner.write();
        let keys: Vec<_> = store
            .keys()
            .filter(|(uid, _)| *uid == user_id)
            .cloned()
            .collect();
        for key in keys {
            if let Some(session) = store.remove(&key) {
                removed.push((session.jti, session.access_jti));
            }
        }
        Ok(removed)
    }
}

// ── RefreshTokenStore trait ──

/// Refresh Token 存储抽象
///
/// 职责：维护 `user_id → token_version`（用于用户级撤销，O(1) 撤销所有 Token）。
/// 实现者需保证线程安全（`Send + Sync`）。
#[async_trait::async_trait]
pub trait RefreshTokenStore: Send + Sync {
    /// 获取用户当前 Token 版本
    async fn get_version(&self, user_id: i64) -> Result<u64, RefreshTokenError>;
    /// 递增用户 Token 版本（撤销该用户所有 Token）
    async fn increment_version(&self, user_id: i64) -> Result<u64, RefreshTokenError>;
}

/// 内存实现（单进程，测试用）
pub struct MemoryRefreshTokenStore {
    inner: Arc<parking_lot::RwLock<std::collections::HashMap<i64, u64>>>,
}

impl MemoryRefreshTokenStore {
    /// 创建空存储
    pub fn new() -> Self {
        Self {
            inner: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        }
    }
}

impl Default for MemoryRefreshTokenStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl RefreshTokenStore for MemoryRefreshTokenStore {
    async fn get_version(&self, user_id: i64) -> Result<u64, RefreshTokenError> {
        Ok(self.inner.read().get(&user_id).copied().unwrap_or(0))
    }

    async fn increment_version(&self, user_id: i64) -> Result<u64, RefreshTokenError> {
        let mut guard = self.inner.write();
        let new_ver = guard.entry(user_id).and_modify(|v| *v += 1).or_insert(1);
        Ok(*new_ver)
    }
}

// ── TokenBlacklist trait ──

/// Token 黑名单抽象
///
/// 职责：存储已撤销的 Token jti，支持快速查询。
/// 在 sso_middleware 中可适配 `JwtBlacklist` 实现此 trait。
#[async_trait::async_trait]
pub trait TokenBlacklist: Send + Sync {
    /// 将 jti 加入黑名单，ttl 为存活秒数
    async fn revoke(&self, jti: &str, ttl_secs: u64) -> Result<(), RefreshTokenError>;
    /// 检查 jti 是否在黑名单中
    async fn is_revoked(&self, jti: &str) -> Result<bool, RefreshTokenError>;
}

/// 内存黑名单实现（单进程，测试用）
pub struct MemoryTokenBlacklist {
    inner: Arc<parking_lot::RwLock<std::collections::HashMap<String, i64>>>,
}

impl MemoryTokenBlacklist {
    /// 创建空黑名单
    pub fn new() -> Self {
        Self {
            inner: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        }
    }
}

impl Default for MemoryTokenBlacklist {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl TokenBlacklist for MemoryTokenBlacklist {
    async fn revoke(&self, jti: &str, ttl_secs: u64) -> Result<(), RefreshTokenError> {
        let expires_at = chrono::Utc::now().timestamp() + ttl_secs as i64;
        self.inner.write().insert(jti.to_string(), expires_at);
        Ok(())
    }

    async fn is_revoked(&self, jti: &str) -> Result<bool, RefreshTokenError> {
        let now = chrono::Utc::now().timestamp();
        let guard = self.inner.read();
        match guard.get(jti) {
            Some(&expires_at) if expires_at > now => Ok(true),
            _ => Ok(false),
        }
    }
}

// ── RefreshTokenVerifier ──

/// Refresh Token 校验器
///
/// 校验链：JWT 签名 → 过期 → token_type → 黑名单 → 签发人 → 版本
pub struct RefreshTokenVerifier {
    codec: SsoJwtCodec,
    blacklist: Arc<dyn TokenBlacklist>,
    store: Arc<dyn RefreshTokenStore>,
    issuer: String,
}

impl RefreshTokenVerifier {
    /// 创建校验器
    pub fn new(
        codec: SsoJwtCodec,
        blacklist: Arc<dyn TokenBlacklist>,
        store: Arc<dyn RefreshTokenStore>,
        issuer: impl Into<String>,
    ) -> Self {
        Self {
            codec,
            blacklist,
            store,
            issuer: issuer.into(),
        }
    }

    /// 校验 accessToken
    pub async fn verify_access(&self, token: &str) -> Result<SsoClaims, RefreshTokenError> {
        self.verify(token, "access").await
    }

    /// 校验 refreshToken
    pub async fn verify_refresh(&self, token: &str) -> Result<SsoClaims, RefreshTokenError> {
        self.verify(token, "refresh").await
    }

    async fn verify(
        &self,
        token: &str,
        expected_type: &str,
    ) -> Result<SsoClaims, RefreshTokenError> {
        let claims = self.codec.decode(token)?;

        if claims.token_type != expected_type {
            return Err(RefreshTokenError::WrongTokenType {
                expected: expected_type.to_string(),
                actual: claims.token_type,
            });
        }

        if !claims.jti.is_empty() && self.blacklist.is_revoked(&claims.jti).await? {
            return Err(RefreshTokenError::Revoked);
        }

        if let Some(ref iss) = claims.iss {
            if iss != &self.issuer {
                return Err(RefreshTokenError::IssuerMismatch {
                    expected: self.issuer.clone(),
                    actual: iss.clone(),
                });
            }
        }

        if let Some(user_id) = claims.user_id {
            let current_ver = self.store.get_version(user_id).await?;
            if claims.ver != current_ver {
                return Err(RefreshTokenError::VersionMismatch {
                    token_ver: claims.ver,
                    current_ver,
                });
            }
        }

        Ok(claims)
    }
}

// ── Token 降级机制（P3）──

/// 降级条目
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DegradationEntry {
    /// 降级后的角色列表
    pub roles: Vec<String>,
    /// 降级后的权限列表
    pub permissions: Vec<String>,
    /// 降级过期时间（Unix 时间戳）
    pub expires_at: i64,
}

/// 降级存储 trait
///
/// 维护用户级和设备级权限降级映射。
/// 实现者需保证线程安全（`Send + Sync`）。
#[async_trait::async_trait]
pub trait DegradationStore: Send + Sync {
    /// 设置用户级降级
    async fn set_user_degradation(
        &self,
        user_id: i64,
        entry: DegradationEntry,
    ) -> Result<(), RefreshTokenError>;

    /// 获取用户级降级（已过期返回 None）
    async fn get_user_degradation(
        &self,
        user_id: i64,
    ) -> Result<Option<DegradationEntry>, RefreshTokenError>;

    /// 清除用户级降级
    async fn clear_user_degradation(&self, user_id: i64) -> Result<(), RefreshTokenError>;

    /// 设置设备级降级
    async fn set_device_degradation(
        &self,
        user_id: i64,
        device_id: &str,
        entry: DegradationEntry,
    ) -> Result<(), RefreshTokenError>;

    /// 获取设备级降级
    async fn get_device_degradation(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<Option<DegradationEntry>, RefreshTokenError>;

    /// 清除设备级降级
    async fn clear_device_degradation(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<(), RefreshTokenError>;

    /// 清除用户所有降级（含设备级）
    async fn clear_all_degradations(&self, user_id: i64) -> Result<(), RefreshTokenError>;
}

/// 内存降级存储（单进程，测试用）
pub struct MemoryDegradationStore {
    user_entries: Arc<parking_lot::RwLock<std::collections::HashMap<i64, DegradationEntry>>>,
    device_entries:
        Arc<parking_lot::RwLock<std::collections::HashMap<(i64, String), DegradationEntry>>>,
}

impl MemoryDegradationStore {
    /// 创建空存储
    pub fn new() -> Self {
        Self {
            user_entries: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
            device_entries: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        }
    }
}

impl Default for MemoryDegradationStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl DegradationStore for MemoryDegradationStore {
    async fn set_user_degradation(
        &self,
        user_id: i64,
        entry: DegradationEntry,
    ) -> Result<(), RefreshTokenError> {
        self.user_entries.write().insert(user_id, entry);
        Ok(())
    }

    async fn get_user_degradation(
        &self,
        user_id: i64,
    ) -> Result<Option<DegradationEntry>, RefreshTokenError> {
        let now = chrono::Utc::now().timestamp();
        let guard = self.user_entries.read();
        match guard.get(&user_id) {
            Some(e) if e.expires_at > now => Ok(Some(e.clone())),
            _ => Ok(None),
        }
    }

    async fn clear_user_degradation(&self, user_id: i64) -> Result<(), RefreshTokenError> {
        self.user_entries.write().remove(&user_id);
        Ok(())
    }

    async fn set_device_degradation(
        &self,
        user_id: i64,
        device_id: &str,
        entry: DegradationEntry,
    ) -> Result<(), RefreshTokenError> {
        self.device_entries
            .write()
            .insert((user_id, device_id.to_string()), entry);
        Ok(())
    }

    async fn get_device_degradation(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<Option<DegradationEntry>, RefreshTokenError> {
        let now = chrono::Utc::now().timestamp();
        let guard = self.device_entries.read();
        match guard.get(&(user_id, device_id.to_string())) {
            Some(e) if e.expires_at > now => Ok(Some(e.clone())),
            _ => Ok(None),
        }
    }

    async fn clear_device_degradation(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<(), RefreshTokenError> {
        self.device_entries
            .write()
            .remove(&(user_id, device_id.to_string()));
        Ok(())
    }

    async fn clear_all_degradations(&self, user_id: i64) -> Result<(), RefreshTokenError> {
        self.user_entries.write().remove(&user_id);
        let mut device_store = self.device_entries.write();
        let keys: Vec<_> = device_store
            .keys()
            .filter(|(uid, _)| *uid == user_id)
            .cloned()
            .collect();
        for key in keys {
            device_store.remove(&key);
        }
        Ok(())
    }
}

// ── SSO 跨域 Ticket（P4）──

/// SSO 一次性 Ticket（跨域单点登录）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SsoTicket {
    /// Ticket 字符串（UUID v4）
    pub ticket: String,
    /// 用户 ID
    pub user_id: i64,
    /// 用户名
    pub username: String,
    /// 重定向 URI
    pub redirect_uri: String,
    /// 用户角色
    pub roles: Vec<String>,
    /// 用户权限
    pub permissions: Vec<String>,
    /// 创建时间（Unix 时间戳）
    pub created_at: i64,
    /// 过期时间（Unix 时间戳）
    pub expires_at: i64,
}

/// Ticket 存储 trait
#[async_trait::async_trait]
pub trait TicketStore: Send + Sync {
    /// 保存 ticket
    async fn save(&self, ticket: SsoTicket) -> Result<(), RefreshTokenError>;

    /// 取出并删除 ticket（一次性使用）
    async fn take(&self, ticket: &str) -> Result<Option<SsoTicket>, RefreshTokenError>;

    /// 仅查看 ticket（不删除）
    async fn peek(&self, ticket: &str) -> Result<Option<SsoTicket>, RefreshTokenError>;
}

/// 内存 Ticket 存储
pub struct MemoryTicketStore {
    inner: Arc<parking_lot::RwLock<std::collections::HashMap<String, SsoTicket>>>,
}

impl MemoryTicketStore {
    /// 创建空存储
    pub fn new() -> Self {
        Self {
            inner: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        }
    }
}

impl Default for MemoryTicketStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl TicketStore for MemoryTicketStore {
    async fn save(&self, ticket: SsoTicket) -> Result<(), RefreshTokenError> {
        self.inner.write().insert(ticket.ticket.clone(), ticket);
        Ok(())
    }

    async fn take(&self, ticket: &str) -> Result<Option<SsoTicket>, RefreshTokenError> {
        let mut store = self.inner.write();
        let entry = store.remove(ticket);
        if let Some(ref t) = entry {
            if t.expires_at <= chrono::Utc::now().timestamp() {
                return Ok(None);
            }
        }
        Ok(entry)
    }

    async fn peek(&self, ticket: &str) -> Result<Option<SsoTicket>, RefreshTokenError> {
        let now = chrono::Utc::now().timestamp();
        let guard = self.inner.read();
        match guard.get(ticket) {
            Some(t) if t.expires_at > now => Ok(Some(t.clone())),
            _ => Ok(None),
        }
    }
}

// ── 审计日志（P5）──

/// 审计事件类型
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum AuditEventType {
    /// 用户登录
    Login,
    /// 用户登出
    Logout,
    /// 撤销单个 Token
    Revoke,
    /// 撤销用户所有 Token
    RevokeAll,
    /// 撤销设备会话
    RevokeDevice,
    /// 权限降级
    Degrade,
    /// 清除降级
    ClearDegradation,
    /// 生成跨域 Ticket
    TicketGenerate,
    /// 交换跨域 Ticket
    TicketExchange,
    /// Refresh Token 轮换
    RefreshRotated,
    /// 复用攻击检测
    ReuseDetected,
    /// 设备会话注册
    DeviceRegistered,
    /// 设备会话淘汰（LRU）
    DeviceEvicted,
}

/// 审计事件
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuditEvent {
    /// 事件 ID（UUID v4）
    pub event_id: String,
    /// 事件类型
    pub event_type: AuditEventType,
    /// 用户 ID
    pub user_id: Option<i64>,
    /// 设备 ID
    pub device_id: Option<String>,
    /// 时间戳（Unix）
    pub timestamp: i64,
    /// 来源 IP
    pub ip: Option<String>,
    /// 详情（JSON）
    pub detail: Option<String>,
}

/// 审计存储 trait
#[async_trait::async_trait]
pub trait AuditStore: Send + Sync {
    /// 记录审计事件
    async fn record(&self, event: AuditEvent) -> Result<(), RefreshTokenError>;

    /// 查询用户审计事件（按时间倒序，限制数量）
    async fn query_by_user(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, RefreshTokenError>;

    /// 查询指定时间范围内的审计事件
    async fn query_by_time_range(
        &self,
        start: i64,
        end: i64,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, RefreshTokenError>;
}

/// 内存审计存储
pub struct MemoryAuditStore {
    inner: Arc<parking_lot::RwLock<Vec<AuditEvent>>>,
}

impl MemoryAuditStore {
    /// 创建空存储
    pub fn new() -> Self {
        Self {
            inner: Arc::new(parking_lot::RwLock::new(Vec::new())),
        }
    }
}

impl Default for MemoryAuditStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AuditStore for MemoryAuditStore {
    async fn record(&self, event: AuditEvent) -> Result<(), RefreshTokenError> {
        self.inner.write().push(event);
        Ok(())
    }

    async fn query_by_user(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, RefreshTokenError> {
        let guard = self.inner.read();
        let mut events: Vec<AuditEvent> = guard
            .iter()
            .filter(|e| e.user_id == Some(user_id))
            .cloned()
            .collect();
        events.sort_by_key(|b| std::cmp::Reverse(b.timestamp));
        events.truncate(limit);
        Ok(events)
    }

    async fn query_by_time_range(
        &self,
        start: i64,
        end: i64,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, RefreshTokenError> {
        let guard = self.inner.read();
        let mut events: Vec<AuditEvent> = guard
            .iter()
            .filter(|e| e.timestamp >= start && e.timestamp <= end)
            .cloned()
            .collect();
        events.sort_by_key(|b| std::cmp::Reverse(b.timestamp));
        events.truncate(limit);
        Ok(events)
    }
}

// ── RefreshTokenIssuer ──

/// Refresh Token 签发器
///
/// 职责：签发双 Token（issue）、轮换 Token（rotate）
pub struct RefreshTokenIssuer {
    codec: SsoJwtCodec,
    blacklist: Arc<dyn TokenBlacklist>,
    store: Arc<dyn RefreshTokenStore>,
    config: RefreshTokenConfig,
}

impl RefreshTokenIssuer {
    /// 创建签发器
    pub fn new(
        codec: SsoJwtCodec,
        blacklist: Arc<dyn TokenBlacklist>,
        store: Arc<dyn RefreshTokenStore>,
        config: RefreshTokenConfig,
    ) -> Self {
        Self {
            codec,
            blacklist,
            store,
            config,
        }
    }

    /// 签发双 Token
    #[tracing::instrument(skip(self), fields(user_id = user_id))]
    pub async fn issue(
        &self,
        user_id: i64,
        username: &str,
    ) -> Result<TokenPair, RefreshTokenError> {
        self.issue_inner(user_id, username, None, Vec::new(), Vec::new())
            .await
    }

    /// 签发携带 roles/permissions 的双 Token
    pub async fn issue_with_roles(
        &self,
        user_id: i64,
        username: &str,
        roles: Vec<String>,
        permissions: Vec<String>,
    ) -> Result<TokenPair, RefreshTokenError> {
        self.issue_inner(user_id, username, None, roles, permissions)
            .await
    }

    /// 签发绑定设备的双 Token
    #[tracing::instrument(skip(self), fields(user_id = user_id, device_id = device_id))]
    pub async fn issue_with_device(
        &self,
        user_id: i64,
        username: &str,
        device_id: &str,
    ) -> Result<TokenPair, RefreshTokenError> {
        self.issue_inner(user_id, username, Some(device_id), Vec::new(), Vec::new())
            .await
    }

    /// 签发绑定设备的双 Token，并返回 refresh_token 的 jti
    pub async fn issue_with_device_and_jti(
        &self,
        user_id: i64,
        username: &str,
        device_id: &str,
        roles: Vec<String>,
        permissions: Vec<String>,
    ) -> Result<(TokenPair, String, String), RefreshTokenError> {
        let now = chrono::Utc::now();
        let access_exp = (now + self.config.access_token_ttl).timestamp();
        let refresh_exp = (now + self.config.refresh_token_ttl).timestamp();
        let ver = self.store.get_version(user_id).await?;
        let jti = uuid::Uuid::new_v4().to_string();

        let mut access_claims =
            SsoClaims::access(user_id, username, access_exp, &self.config.issuer, ver);
        let access_jti = uuid::Uuid::new_v4().to_string();
        access_claims.jti = access_jti.clone();
        access_claims.device_id = Some(device_id.to_string());
        access_claims.roles = roles;
        access_claims.permissions = permissions;

        let mut refresh_claims = SsoClaims::refresh(
            user_id,
            username,
            refresh_exp,
            &self.config.issuer,
            ver,
            jti.clone(),
        );
        refresh_claims.device_id = Some(device_id.to_string());

        let access_token = self.codec.encode(&access_claims)?;
        let refresh_token = self.codec.encode(&refresh_claims)?;

        Ok((
            TokenPair {
                access_token,
                refresh_token,
                access_expires_at: access_exp,
                refresh_expires_at: refresh_exp,
            },
            jti,
            access_jti,
        ))
    }

    /// 内部签发实现
    async fn issue_inner(
        &self,
        user_id: i64,
        username: &str,
        device_id: Option<&str>,
        roles: Vec<String>,
        permissions: Vec<String>,
    ) -> Result<TokenPair, RefreshTokenError> {
        let now = chrono::Utc::now();
        let access_exp = (now + self.config.access_token_ttl).timestamp();
        let refresh_exp = (now + self.config.refresh_token_ttl).timestamp();
        let ver = self.store.get_version(user_id).await?;
        let jti = uuid::Uuid::new_v4().to_string();

        let mut access_claims =
            SsoClaims::access(user_id, username, access_exp, &self.config.issuer, ver);
        access_claims.jti = uuid::Uuid::new_v4().to_string();
        access_claims.device_id = device_id.map(|s| s.to_string());
        access_claims.roles = roles;
        access_claims.permissions = permissions;

        let mut refresh_claims = SsoClaims::refresh(
            user_id,
            username,
            refresh_exp,
            &self.config.issuer,
            ver,
            jti,
        );
        refresh_claims.device_id = device_id.map(|s| s.to_string());

        let access_token = self.codec.encode(&access_claims)?;
        let refresh_token = self.codec.encode(&refresh_claims)?;

        Ok(TokenPair {
            access_token,
            refresh_token,
            access_expires_at: access_exp,
            refresh_expires_at: refresh_exp,
        })
    }

    /// 续期 accessToken（仅签发新 accessToken，不签发新 refreshToken）
    ///
    /// 从 `old_claims` 复制 `sub / iss / user_id / ver / roles / permissions`，
    /// 更新 `exp / iat / jti / token_type`，签发新 accessToken。
    ///
    /// **安全约束**：
    /// - 不调用 `store.get_version` / `store.increment_version`（不递增版本号）
    /// - 不调用 `blacklist.revoke`（不撤销旧 accessToken）
    /// - 不签发新 refreshToken
    pub fn renew_access(&self, old_claims: &SsoClaims) -> Result<(String, i64), RefreshTokenError> {
        let now = chrono::Utc::now().timestamp();
        let new_exp = now + self.config.access_token_ttl.num_seconds();
        let new_jti = uuid::Uuid::new_v4().to_string();

        let new_claims = SsoClaims {
            sub: old_claims.sub.clone(),
            exp: new_exp,
            iat: now,
            iss: old_claims.iss.clone(),
            user_id: old_claims.user_id,
            token_type: "access".to_string(),
            jti: new_jti,
            ver: old_claims.ver,
            roles: old_claims.roles.clone(),
            permissions: old_claims.permissions.clone(),
            device_id: old_claims.device_id.clone(),
        };

        let new_token = self.codec.encode(&new_claims)?;
        Ok((new_token, new_exp))
    }

    /// 轮换 Token（旧 refreshToken → 新 TokenPair）
    ///
    /// 1. 校验旧 refreshToken
    /// 2. 将旧 refreshToken 的 jti 加入黑名单
    /// 3. 签发新的 TokenPair
    #[tracing::instrument(skip(self, old_refresh_token), fields(jti))]
    pub async fn rotate(&self, old_refresh_token: &str) -> Result<TokenPair, RefreshTokenError> {
        // 先解码 Token 获取 claims（用于复用攻击检测）
        let old_claims = self.codec.decode(old_refresh_token)?;

        // 校验 token_type 为 refresh
        if !old_claims.is_refresh() {
            return Err(RefreshTokenError::WrongTokenType {
                expected: "refresh".to_string(),
                actual: old_claims.token_type,
            });
        }

        // 复用攻击检测：jti 已在黑名单 → 撤销用户所有 Token + 告警
        if !old_claims.jti.is_empty() && self.blacklist.is_revoked(&old_claims.jti).await? {
            if let Some(user_id) = old_claims.user_id {
                tracing::warn!(
                    user_id,
                    jti = %old_claims.jti,
                    "refresh token reuse detected, revoking all tokens for user"
                );
                self.store.increment_version(user_id).await?;
            }
            return Err(RefreshTokenError::ReuseDetected);
        }

        // 正常校验剩余字段（过期、签发人、版本）
        let verifier = RefreshTokenVerifier::new(
            self.codec.clone(),
            self.blacklist.clone(),
            self.store.clone(),
            self.config.issuer.clone(),
        );
        let old_claims = verifier.verify_refresh(old_refresh_token).await?;

        if old_claims.jti.is_empty() {
            return Err(RefreshTokenError::InvalidSignature);
        }

        let user_id = old_claims.user_id.ok_or(RefreshTokenError::UserNotFound)?;
        let username = &old_claims.sub;

        let remaining_ttl = old_claims.exp - chrono::Utc::now().timestamp();
        if remaining_ttl > 0 {
            self.blacklist
                .revoke(&old_claims.jti, remaining_ttl as u64)
                .await?;
        }

        self.issue(user_id, username).await
    }
}

// ── RefreshTokenRevoker ──

/// Refresh Token 撤销器
///
/// 职责：撤销单个 Token（revoke）、撤销用户所有 Token（revoke_all）
pub struct RefreshTokenRevoker {
    codec: SsoJwtCodec,
    blacklist: Arc<dyn TokenBlacklist>,
    store: Arc<dyn RefreshTokenStore>,
}

impl RefreshTokenRevoker {
    /// 创建撤销器
    pub fn new(
        codec: SsoJwtCodec,
        blacklist: Arc<dyn TokenBlacklist>,
        store: Arc<dyn RefreshTokenStore>,
    ) -> Self {
        Self {
            codec,
            blacklist,
            store,
        }
    }

    /// 撤销单个 Token（通过 jti 加入黑名单）
    pub async fn revoke(&self, token: &str) -> Result<(), RefreshTokenError> {
        let claims = self.codec.decode(token)?;

        if claims.jti.is_empty() {
            return Ok(());
        }

        let remaining_ttl = claims.exp - chrono::Utc::now().timestamp();
        if remaining_ttl > 0 {
            self.blacklist
                .revoke(&claims.jti, remaining_ttl as u64)
                .await?;
        }

        Ok(())
    }

    /// 通过 jti 撤销 Token（直接加入黑名单，无需解码）
    pub async fn revoke_by_jti(&self, jti: &str) -> Result<(), RefreshTokenError> {
        if jti.is_empty() {
            return Ok(());
        }
        self.blacklist.revoke(jti, 604800).await?;
        Ok(())
    }

    /// 撤销用户所有 Token（递增版本号，O(1)）
    pub async fn revoke_all(&self, user_id: i64) -> Result<(), RefreshTokenError> {
        self.store.increment_version(user_id).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sso_jwt_codec_encode_decode_roundtrip() {
        let codec = SsoJwtCodec::new("test-secret");
        let claims = SsoClaims::access(1, "user1", chrono::Utc::now().timestamp() + 900, "iss", 0);
        let token = codec.encode(&claims).unwrap();
        let decoded = codec.decode(&token).unwrap();
        assert_eq!(decoded, claims);
    }

    #[test]
    fn test_sso_jwt_codec_rejects_wrong_secret() {
        let codec_a = SsoJwtCodec::new("secret-a");
        let codec_b = SsoJwtCodec::new("secret-b");
        let claims = SsoClaims::access(1, "user1", chrono::Utc::now().timestamp() + 900, "iss", 0);
        let token = codec_a.encode(&claims).unwrap();
        let result = codec_b.decode(&token);
        assert!(matches!(result, Err(RefreshTokenError::InvalidSignature)));
    }

    #[test]
    fn test_sso_jwt_codec_rejects_expired() {
        let codec = SsoJwtCodec::new("test-secret");
        let claims = SsoClaims::access(1, "user1", chrono::Utc::now().timestamp() - 1, "iss", 0);
        let token = codec.encode(&claims).unwrap();
        let result = codec.decode(&token);
        assert!(matches!(result, Err(RefreshTokenError::Expired)));
    }

    #[test]
    fn test_sso_jwt_codec_rejects_malformed_token() {
        let codec = SsoJwtCodec::new("test-secret");
        assert!(matches!(
            codec.decode("not.a.valid.token"),
            Err(RefreshTokenError::InvalidSignature)
        ));
        assert!(matches!(
            codec.decode("onlytwo.parts"),
            Err(RefreshTokenError::InvalidSignature)
        ));
    }

    #[test]
    fn test_sso_jwt_codec_debug_redacts_secret() {
        let codec = SsoJwtCodec::new("super-secret-value");
        let debug_str = format!("{:?}", codec);
        assert!(!debug_str.contains("super-secret-value"));
        assert!(debug_str.contains("[REDACTED]"));
    }

    #[test]
    fn test_sso_claims_access_vs_refresh() {
        let access = SsoClaims::access(1, "user1", 9999, "iss", 0);
        assert!(access.is_access());
        assert!(!access.is_refresh());

        let refresh = SsoClaims::refresh(1, "user1", 9999, "iss", 0, "jti-123".to_string());
        assert!(!refresh.is_access());
        assert!(refresh.is_refresh());
        assert_eq!(refresh.jti, "jti-123");
    }

    #[test]
    fn test_sso_claims_default_token_type() {
        let json = r#"{"sub":"user1","exp":9999,"iat":0}"#;
        let claims: SsoClaims = serde_json::from_str(json).unwrap();
        assert_eq!(claims.token_type, "access");
        assert_eq!(claims.ver, 0);
        assert!(claims.jti.is_empty());
    }

    #[test]
    fn test_sso_claims_is_expired() {
        let past = SsoClaims::access(1, "u", chrono::Utc::now().timestamp() - 100, "i", 0);
        assert!(past.is_expired());

        let future = SsoClaims::access(1, "u", chrono::Utc::now().timestamp() + 100, "i", 0);
        assert!(!future.is_expired());
    }

    #[test]
    fn test_token_pair_serialization() {
        let pair = TokenPair {
            access_token: "at".to_string(),
            refresh_token: "rt".to_string(),
            access_expires_at: 100,
            refresh_expires_at: 200,
        };
        let json = serde_json::to_string(&pair).unwrap();
        let decoded: TokenPair = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.access_token, "at");
        assert_eq!(decoded.refresh_token, "rt");
    }

    #[test]
    fn test_refresh_token_config_default() {
        let config = RefreshTokenConfig::default();
        assert_eq!(config.access_token_ttl, chrono::Duration::seconds(900));
        assert_eq!(config.refresh_token_ttl, chrono::Duration::seconds(604800));
        assert_eq!(config.issuer, "sz-rust-sso");
    }

    // ── T2: Store + Blacklist 测试 ──

    #[tokio::test]
    async fn test_memory_store_get_version_default() {
        let store = MemoryRefreshTokenStore::new();
        assert_eq!(store.get_version(1).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_memory_store_increment() {
        let store = MemoryRefreshTokenStore::new();
        assert_eq!(store.increment_version(1).await.unwrap(), 1);
        assert_eq!(store.increment_version(1).await.unwrap(), 2);
        assert_eq!(store.get_version(1).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_memory_store_different_users() {
        let store = MemoryRefreshTokenStore::new();
        store.increment_version(1).await.unwrap();
        store.increment_version(2).await.unwrap();
        store.increment_version(2).await.unwrap();
        assert_eq!(store.get_version(1).await.unwrap(), 1);
        assert_eq!(store.get_version(2).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_memory_blacklist_revoke_and_check() {
        let blacklist = MemoryTokenBlacklist::new();
        assert!(!blacklist.is_revoked("jti-1").await.unwrap());

        blacklist.revoke("jti-1", 3600).await.unwrap();
        assert!(blacklist.is_revoked("jti-1").await.unwrap());
        assert!(!blacklist.is_revoked("jti-2").await.unwrap());
    }

    #[tokio::test]
    async fn test_memory_blacklist_expired_entry() {
        let blacklist = MemoryTokenBlacklist::new();
        // TTL=0 means expires immediately
        blacklist.revoke("jti-expired", 0).await.unwrap();
        // Should be expired (not revoked) since expires_at <= now
        assert!(!blacklist.is_revoked("jti-expired").await.unwrap());
    }

    // ── T3+T4+T5: Verifier + Issuer + Revoker 测试 ──

    fn make_issuer() -> (
        RefreshTokenIssuer,
        RefreshTokenVerifier,
        RefreshTokenRevoker,
    ) {
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
        (issuer, verifier, revoker)
    }

    #[tokio::test]
    async fn test_issuer_issue_token_pair() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();
        assert!(!pair.access_token.is_empty());
        assert!(!pair.refresh_token.is_empty());
        assert!(pair.access_expires_at < pair.refresh_expires_at);

        let access_claims = verifier.verify_access(&pair.access_token).await.unwrap();
        assert!(access_claims.is_access());
        assert_eq!(access_claims.user_id, Some(1));

        let refresh_claims = verifier.verify_refresh(&pair.refresh_token).await.unwrap();
        assert!(refresh_claims.is_refresh());
        assert!(!refresh_claims.jti.is_empty());
    }

    #[tokio::test]
    async fn test_verifier_rejects_wrong_token_type() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();

        // Use access token as refresh
        let result = verifier.verify_refresh(&pair.access_token).await;
        assert!(matches!(
            result,
            Err(RefreshTokenError::WrongTokenType { .. })
        ));

        // Use refresh token as access
        let result = verifier.verify_access(&pair.refresh_token).await;
        assert!(matches!(
            result,
            Err(RefreshTokenError::WrongTokenType { .. })
        ));
    }

    #[tokio::test]
    async fn test_issuer_rotate_token() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();

        // Rotate
        let new_pair = issuer.rotate(&pair.refresh_token).await.unwrap();
        assert_ne!(new_pair.access_token, pair.access_token);
        assert_ne!(new_pair.refresh_token, pair.refresh_token);

        // Old refresh token should be revoked
        let result = verifier.verify_refresh(&pair.refresh_token).await;
        assert!(matches!(result, Err(RefreshTokenError::Revoked)));

        // New tokens should be valid
        verifier
            .verify_access(&new_pair.access_token)
            .await
            .unwrap();
        verifier
            .verify_refresh(&new_pair.refresh_token)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_revoker_revoke_single_token() {
        let (issuer, verifier, revoker) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();

        revoker.revoke(&pair.refresh_token).await.unwrap();

        let result = verifier.verify_refresh(&pair.refresh_token).await;
        assert!(matches!(result, Err(RefreshTokenError::Revoked)));
    }

    #[tokio::test]
    async fn test_revoker_revoke_all() {
        let (issuer, verifier, revoker) = make_issuer();
        let pair1 = issuer.issue(1, "user1").await.unwrap();

        revoker.revoke_all(1).await.unwrap();

        // All tokens for user 1 should be invalid (version mismatch)
        let result = verifier.verify_access(&pair1.access_token).await;
        assert!(matches!(
            result,
            Err(RefreshTokenError::VersionMismatch { .. })
        ));

        let result = verifier.verify_refresh(&pair1.refresh_token).await;
        assert!(matches!(
            result,
            Err(RefreshTokenError::VersionMismatch { .. })
        ));
    }

    #[tokio::test]
    async fn test_verifier_rejects_issuer_mismatch() {
        let codec = SsoJwtCodec::new("test-secret");
        let blacklist: Arc<dyn TokenBlacklist> = Arc::new(MemoryTokenBlacklist::new());
        let store: Arc<dyn RefreshTokenStore> = Arc::new(MemoryRefreshTokenStore::new());

        // Issue with issuer "aaa"
        let config_a = RefreshTokenConfig {
            issuer: "aaa".to_string(),
            ..Default::default()
        };
        let issuer =
            RefreshTokenIssuer::new(codec.clone(), blacklist.clone(), store.clone(), config_a);
        let pair = issuer.issue(1, "user1").await.unwrap();

        // Verify with issuer "bbb"
        let verifier = RefreshTokenVerifier::new(codec, blacklist, store, "bbb");
        let result = verifier.verify_access(&pair.access_token).await;
        assert!(matches!(
            result,
            Err(RefreshTokenError::IssuerMismatch { .. })
        ));
    }

    #[tokio::test]
    async fn test_revoker_revoke_idempotent() {
        let (issuer, _, revoker) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();

        revoker.revoke(&pair.refresh_token).await.unwrap();
        // Second revoke should also succeed (idempotent)
        revoker.revoke(&pair.refresh_token).await.unwrap();
    }

    // ── T10: 边界测试 ──

    #[tokio::test]
    async fn test_verifier_empty_token() {
        let (_, verifier, _) = make_issuer();
        let result = verifier.verify_access("").await;
        assert!(matches!(result, Err(RefreshTokenError::InvalidSignature)));
        let result = verifier.verify_refresh("").await;
        assert!(matches!(result, Err(RefreshTokenError::InvalidSignature)));
    }

    #[tokio::test]
    async fn test_verifier_tampered_signature() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();

        // 篡改签名部分：翻转最后一个字符
        let mut tampered = pair.access_token.clone();
        let last_idx = tampered.len() - 1;
        let last_char = tampered.as_bytes()[last_idx];
        tampered.replace_range(last_idx.., if last_char == b'A' { "B" } else { "A" });

        let result = verifier.verify_access(&tampered).await;
        assert!(matches!(result, Err(RefreshTokenError::InvalidSignature)));
    }

    #[tokio::test]
    async fn test_verifier_tampered_payload() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();

        // 篡改 payload 部分：修改第二段第一个字符
        let parts: Vec<&str> = pair.access_token.split('.').collect();
        let mut payload = parts[1].to_string();
        let first_byte = payload.as_bytes()[0];
        payload.replace_range(0..1, if first_byte == b'e' { "f" } else { "e" });
        let tampered = format!("{}.{}.{}", parts[0], payload, parts[2]);

        let result = verifier.verify_access(&tampered).await;
        assert!(matches!(result, Err(RefreshTokenError::InvalidSignature)));
    }

    #[tokio::test]
    async fn test_verifier_expired_by_one_second() {
        let codec = SsoJwtCodec::new("test-secret");
        let blacklist: Arc<dyn TokenBlacklist> = Arc::new(MemoryTokenBlacklist::new());
        let store: Arc<dyn RefreshTokenStore> = Arc::new(MemoryRefreshTokenStore::new());

        // 构造已过期 1 秒的 claims
        let claims =
            SsoClaims::access(1, "user1", chrono::Utc::now().timestamp() - 1, "sz-rust", 0);
        let token = codec.encode(&claims).unwrap();

        let verifier = RefreshTokenVerifier::new(codec, blacklist, store, "sz-rust");
        let result = verifier.verify_access(&token).await;
        assert!(matches!(result, Err(RefreshTokenError::Expired)));
    }

    #[tokio::test]
    async fn test_verifier_token_type_missing_defaults_to_access() {
        // token_type 有 #[serde(default = "default_token_type")]，缺失时默认 "access"
        let codec = SsoJwtCodec::new("test-secret");
        let blacklist: Arc<dyn TokenBlacklist> = Arc::new(MemoryTokenBlacklist::new());
        let store: Arc<dyn RefreshTokenStore> = Arc::new(MemoryRefreshTokenStore::new());

        // 手工构造缺失 token_type 的 payload
        let now = chrono::Utc::now().timestamp();
        let payload_json = format!(
            r#"{{"sub":"user1","exp":{},"iat":{},"iss":"sz-rust","user_id":1,"jti":"","ver":0}}"#,
            now + 900,
            now
        );
        let header_b64 = URL_SAFE_NO_PAD.encode(JWT_HEADER.as_bytes());
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        let signing_input = format!("{}.{}", header_b64, payload_b64);
        let mut mac = <HmacSha256 as Mac>::new_from_slice(b"test-secret").unwrap();
        mac.update(signing_input.as_bytes());
        let sig = mac.finalize().into_bytes();
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
        let token = format!("{}.{}.{}", header_b64, payload_b64, sig_b64);

        let verifier = RefreshTokenVerifier::new(codec, blacklist, store, "sz-rust");
        // 缺失 token_type 默认 "access"，verify_access 应通过，verify_refresh 应失败
        let result = verifier.verify_access(&token).await;
        assert!(result.is_ok());
        let result = verifier.verify_refresh(&token).await;
        assert!(matches!(
            result,
            Err(RefreshTokenError::WrongTokenType { .. })
        ));
    }

    #[tokio::test]
    async fn test_reuse_detected_on_blacklisted_refresh() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();

        // 第一次轮换：成功，旧 refresh 入黑名单
        let _new_pair = issuer.rotate(&pair.refresh_token).await.unwrap();

        // 第二次用同一旧 refresh 轮换：应检测到复用攻击
        let result = issuer.rotate(&pair.refresh_token).await;
        assert!(matches!(result, Err(RefreshTokenError::ReuseDetected)));

        // 复用攻击后，用户所有 Token 应已撤销（版本递增）
        let verify_result = verifier.verify_access(&_new_pair.access_token).await;
        assert!(matches!(
            verify_result,
            Err(RefreshTokenError::VersionMismatch { .. })
        ));
    }

    #[tokio::test]
    async fn test_concurrent_rotate_different_tokens() {
        // 并发轮换不同用户的 Token：应互不干扰
        let (issuer, verifier, _) = make_issuer();
        let pair1 = issuer.issue(1, "user1").await.unwrap();
        let pair2 = issuer.issue(2, "user2").await.unwrap();

        let (r1, r2) = tokio::join!(
            issuer.rotate(&pair1.refresh_token),
            issuer.rotate(&pair2.refresh_token),
        );

        let new1 = r1.unwrap();
        let new2 = r2.unwrap();

        // 两个新 Token 都应有效
        verifier.verify_access(&new1.access_token).await.unwrap();
        verifier.verify_access(&new2.access_token).await.unwrap();

        // 两个旧 refresh 都应失效（黑名单）
        let result = verifier.verify_refresh(&pair1.refresh_token).await;
        assert!(matches!(result, Err(RefreshTokenError::Revoked)));
        let result = verifier.verify_refresh(&pair2.refresh_token).await;
        assert!(matches!(result, Err(RefreshTokenError::Revoked)));
    }

    #[tokio::test]
    async fn test_verifier_malformed_token_various() {
        let (_, verifier, _) = make_issuer();

        // 只有两段（缺少签名）
        let result = verifier.verify_access("a.b").await;
        assert!(matches!(result, Err(RefreshTokenError::InvalidSignature)));

        // 四段（多余部分）
        let result = verifier.verify_access("a.b.c.d").await;
        assert!(matches!(result, Err(RefreshTokenError::InvalidSignature)));

        // 非 base64 字符
        let result = verifier.verify_access("@@@.@@@.@@@").await;
        assert!(matches!(result, Err(RefreshTokenError::InvalidSignature)));

        // 空段
        let result = verifier.verify_access("..").await;
        assert!(matches!(result, Err(RefreshTokenError::InvalidSignature)));
    }

    #[tokio::test]
    async fn test_codec_empty_secret() {
        // 空密钥应能正常工作（虽然不安全，但不应 panic）
        let codec = SsoJwtCodec::new("");
        let claims = SsoClaims::access(1, "u", chrono::Utc::now().timestamp() + 60, "iss", 0);
        let token = codec.encode(&claims).unwrap();
        let decoded = codec.decode(&token).unwrap();
        assert_eq!(decoded, claims);
    }

    #[tokio::test]
    async fn test_verifier_very_long_token() {
        let (issuer, verifier, _) = make_issuer();
        // 超长用户名（10KB）— 应正常签发与校验
        let long_name = "u".repeat(10_000);
        let pair = issuer.issue(1, &long_name).await.unwrap();
        let claims = verifier.verify_access(&pair.access_token).await.unwrap();
        assert_eq!(claims.sub, long_name);
    }

    #[tokio::test]
    async fn test_rotate_chain_multiple_times() {
        // 连续轮换 5 次，每次新 Token 都应有效，旧 Token 都应失效
        let (issuer, verifier, _) = make_issuer();
        let mut current = issuer.issue(1, "user1").await.unwrap();

        for i in 0..5 {
            let prev = current;
            current = issuer.rotate(&prev.refresh_token).await.unwrap();
            verifier.verify_access(&current.access_token).await.unwrap();
            verifier
                .verify_refresh(&current.refresh_token)
                .await
                .unwrap();

            // 旧 refresh 应已黑名单
            let result = verifier.verify_refresh(&prev.refresh_token).await;
            assert!(
                matches!(result, Err(RefreshTokenError::Revoked)),
                "iter {}",
                i
            );
        }
    }

    #[tokio::test]
    async fn test_revoke_all_does_not_affect_other_users() {
        let (issuer, verifier, revoker) = make_issuer();
        let pair1 = issuer.issue(1, "user1").await.unwrap();
        let pair2 = issuer.issue(2, "user2").await.unwrap();

        // 撤销 user1 所有 Token
        revoker.revoke_all(1).await.unwrap();

        // user1 的 Token 应失效
        let result = verifier.verify_access(&pair1.access_token).await;
        assert!(matches!(
            result,
            Err(RefreshTokenError::VersionMismatch { .. })
        ));

        // user2 的 Token 应仍然有效
        verifier.verify_access(&pair2.access_token).await.unwrap();
        verifier.verify_refresh(&pair2.refresh_token).await.unwrap();
    }

    // ── RenewalConfig 单元测试 ──

    #[test]
    fn test_renewal_config_default() {
        let config = RenewalConfig::default();
        assert!(config.enabled);
        assert_eq!(config.renewal_threshold.num_seconds(), 300);
        assert!((config.renewal_ratio - 0.2).abs() < f64::EPSILON);
        assert_eq!(config.access_token_ttl.num_seconds(), 900);
    }

    #[test]
    fn test_should_renew_disabled() {
        let config = RenewalConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(!config.should_renew(10));
        assert!(!config.should_renew(0));
    }

    #[test]
    fn test_should_renew_threshold_zero() {
        let config = RenewalConfig {
            renewal_threshold: chrono::Duration::seconds(0),
            ..Default::default()
        };
        assert!(config.should_renew(1));
        assert!(!config.should_renew(0));
        assert!(!config.should_renew(-1));
    }

    #[test]
    fn test_should_renew_ratio_zero() {
        let config = RenewalConfig {
            renewal_ratio: 0.0,
            ..Default::default()
        };
        assert!(config.should_renew(299));
        assert!(!config.should_renew(300));
    }

    #[test]
    fn test_should_renew_ratio_one() {
        let config = RenewalConfig {
            renewal_ratio: 1.0,
            ..Default::default()
        };
        assert!(config.should_renew(899));
        assert!(!config.should_renew(900));
    }

    #[test]
    fn test_should_renew_below_threshold() {
        let config = RenewalConfig::default();
        assert!(config.should_renew(299));
        assert!(config.should_renew(100));
        assert!(config.should_renew(1));
    }

    #[test]
    fn test_should_renew_above_threshold() {
        let config = RenewalConfig::default();
        assert!(!config.should_renew(301));
        assert!(!config.should_renew(600));
        assert!(!config.should_renew(900));
    }

    #[test]
    fn test_should_renew_at_exact_threshold() {
        let config = RenewalConfig::default();
        assert!(!config.should_renew(300));
    }

    #[test]
    fn test_should_renew_ratio_dominant() {
        let config = RenewalConfig {
            renewal_threshold: chrono::Duration::seconds(100),
            renewal_ratio: 0.5,
            access_token_ttl: chrono::Duration::seconds(900),
            ..Default::default()
        };
        assert!(config.should_renew(449));
        assert!(!config.should_renew(450));
    }

    #[test]
    fn test_should_renew_threshold_dominant() {
        let config = RenewalConfig {
            renewal_threshold: chrono::Duration::seconds(400),
            renewal_ratio: 0.1,
            access_token_ttl: chrono::Duration::seconds(900),
            ..Default::default()
        };
        assert!(config.should_renew(399));
        assert!(!config.should_renew(400));
    }

    // ── renew_access 单元测试 ──

    #[tokio::test]
    async fn test_renew_access_preserves_user_id() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(42, "alice").await.unwrap();
        let claims = verifier.verify_access(&pair.access_token).await.unwrap();
        let (new_token, _) = issuer.renew_access(&claims).unwrap();
        let new_claims = verifier.verify_access(&new_token).await.unwrap();
        assert_eq!(new_claims.user_id, Some(42));
    }

    #[tokio::test]
    async fn test_renew_access_preserves_ver() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();
        let claims = verifier.verify_access(&pair.access_token).await.unwrap();
        let original_ver = claims.ver;
        let (new_token, _) = issuer.renew_access(&claims).unwrap();
        let new_claims = verifier.verify_access(&new_token).await.unwrap();
        assert_eq!(new_claims.ver, original_ver);
    }

    #[tokio::test]
    async fn test_renew_access_preserves_roles_permissions() {
        let codec = SsoJwtCodec::new("test-secret");
        let blacklist: Arc<dyn TokenBlacklist> = Arc::new(MemoryTokenBlacklist::new());
        let store: Arc<dyn RefreshTokenStore> = Arc::new(MemoryRefreshTokenStore::new());
        let config = RefreshTokenConfig::default();
        let issuer = RefreshTokenIssuer::new(codec.clone(), blacklist, store, config);

        let mut claims = SsoClaims::access(
            1,
            "user1",
            chrono::Utc::now().timestamp() + 900,
            "sz-rust-sso",
            0,
        );
        claims.roles = vec!["admin".to_string(), "user".to_string()];
        claims.permissions = vec!["read".to_string(), "write".to_string()];
        let (new_token, _) = issuer.renew_access(&claims).unwrap();
        let new_claims = codec.decode(&new_token).unwrap();
        assert_eq!(
            new_claims.roles,
            vec!["admin".to_string(), "user".to_string()]
        );
        assert_eq!(
            new_claims.permissions,
            vec!["read".to_string(), "write".to_string()]
        );
    }

    #[tokio::test]
    async fn test_renew_access_new_jti() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();
        let claims = verifier.verify_access(&pair.access_token).await.unwrap();
        let old_jti = claims.jti.clone();
        let (new_token, _) = issuer.renew_access(&claims).unwrap();
        let new_claims = verifier.verify_access(&new_token).await.unwrap();
        assert_ne!(new_claims.jti, old_jti);
        assert!(!new_claims.jti.is_empty());
    }

    #[tokio::test]
    async fn test_renew_access_new_exp() {
        let (issuer, _, _) = make_issuer();
        let codec = SsoJwtCodec::new("test-secret");
        let claims = SsoClaims::access(
            1,
            "user1",
            chrono::Utc::now().timestamp() + 60,
            "sz-rust-sso",
            0,
        );
        let now = chrono::Utc::now().timestamp();
        let (new_token, new_exp) = issuer.renew_access(&claims).unwrap();
        let new_claims = codec.decode(&new_token).unwrap();
        assert!(new_exp > now + 850);
        assert!(new_exp < now + 950);
        assert_eq!(new_claims.exp, new_exp);
    }

    #[tokio::test]
    async fn test_renew_access_token_type_access() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();
        let claims = verifier.verify_access(&pair.access_token).await.unwrap();
        let (new_token, _) = issuer.renew_access(&claims).unwrap();
        let new_claims = verifier.verify_access(&new_token).await.unwrap();
        assert_eq!(new_claims.token_type, "access");
    }

    #[tokio::test]
    async fn test_renew_access_no_store_call() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();
        let claims = verifier.verify_access(&pair.access_token).await.unwrap();
        let ver_before = claims.ver;
        let (new_token, _) = issuer.renew_access(&claims).unwrap();
        let new_claims = verifier.verify_access(&new_token).await.unwrap();
        assert_eq!(new_claims.ver, ver_before);
    }

    #[tokio::test]
    async fn test_renew_access_no_blacklist_call() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();
        let claims = verifier.verify_access(&pair.access_token).await.unwrap();
        let (new_token, _) = issuer.renew_access(&claims).unwrap();
        verifier.verify_access(&pair.access_token).await.unwrap();
        verifier.verify_access(&new_token).await.unwrap();
    }

    #[tokio::test]
    async fn test_renew_access_new_token_valid() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();
        let claims = verifier.verify_access(&pair.access_token).await.unwrap();
        let (new_token, _) = issuer.renew_access(&claims).unwrap();
        verifier.verify_access(&new_token).await.unwrap();
    }

    #[tokio::test]
    async fn test_renew_access_old_token_still_valid() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();
        let claims = verifier.verify_access(&pair.access_token).await.unwrap();
        let _ = issuer.renew_access(&claims).unwrap();
        verifier.verify_access(&pair.access_token).await.unwrap();
    }

    // ── 边界组合测试 ──

    #[test]
    fn test_boundary_threshold_zero_ratio_zero() {
        let config = RenewalConfig {
            renewal_threshold: chrono::Duration::seconds(0),
            renewal_ratio: 0.0,
            ..Default::default()
        };
        assert!(config.should_renew(1));
        assert!(!config.should_renew(0));
        assert!(!config.should_renew(-1));
    }

    #[test]
    fn test_boundary_threshold_zero_ratio_one() {
        let config = RenewalConfig {
            renewal_threshold: chrono::Duration::seconds(0),
            renewal_ratio: 1.0,
            ..Default::default()
        };
        assert!(config.should_renew(1));
        assert!(!config.should_renew(0));
    }

    #[test]
    fn test_boundary_ratio_one_always_renews() {
        let config = RenewalConfig {
            renewal_ratio: 1.0,
            access_token_ttl: chrono::Duration::seconds(900),
            ..Default::default()
        };
        assert!(config.should_renew(899));
        assert!(!config.should_renew(900));
    }

    #[test]
    fn test_boundary_ttl_exact_threshold_strict_less() {
        let config = RenewalConfig::default();
        assert!(!config.should_renew(300));
        assert!(config.should_renew(299));
    }

    #[test]
    fn test_boundary_renewal_config_serde_roundtrip() {
        let config = RenewalConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let decoded: RenewalConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.enabled, config.enabled);
        assert_eq!(decoded.renewal_threshold, config.renewal_threshold);
        assert!((decoded.renewal_ratio - config.renewal_ratio).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_boundary_renewed_token_decodable() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();
        let claims = verifier.verify_access(&pair.access_token).await.unwrap();
        let (new_token, new_exp) = issuer.renew_access(&claims).unwrap();

        let codec = SsoJwtCodec::new("test-secret");
        let new_claims = codec.decode(&new_token).unwrap();
        assert_eq!(new_claims.exp, new_exp);
        assert_eq!(new_claims.token_type, "access");
        assert!(!new_claims.jti.is_empty());
        assert_ne!(new_claims.jti, claims.jti);
    }

    // ── DeviceInfo / DeviceSession / MemoryDeviceSessionStore 单元测试 ──

    #[test]
    fn test_device_info_new_generates_uuid() {
        let info = DeviceInfo::new();
        assert!(!info.device_id.is_empty());
        assert!(uuid::Uuid::parse_str(&info.device_id).is_ok());
    }

    #[test]
    fn test_device_info_with_device_id() {
        let info = DeviceInfo::with_device_id("custom-device-id");
        assert_eq!(info.device_id, "custom-device-id");
    }

    #[test]
    fn test_device_info_serde_skip_none() {
        let info = DeviceInfo::with_device_id("dev1");
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("device_type"));
        assert!(!json.contains("user_agent"));
        assert!(!json.contains("ip"));
        assert!(!json.contains("device_name"));
    }

    #[test]
    fn test_sso_claims_device_id_default_none() {
        let json = r#"{"sub":"user1","exp":9999,"iat":0,"token_type":"access","jti":"","ver":0}"#;
        let claims: SsoClaims = serde_json::from_str(json).unwrap();
        assert!(claims.device_id.is_none());
    }

    #[test]
    fn test_sso_claims_device_id_roundtrip() {
        let mut claims = SsoClaims::access(1, "user1", 9999, "iss", 0);
        claims.device_id = Some("dev-123".to_string());
        let json = serde_json::to_string(&claims).unwrap();
        let decoded: SsoClaims = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.device_id, Some("dev-123".to_string()));
    }

    #[test]
    fn test_device_session_config_default() {
        let config = DeviceSessionConfig::default();
        assert_eq!(config.max_devices, 10);
    }

    #[test]
    fn test_device_session_config_clamp() {
        let config = DeviceSessionConfig::new(200);
        assert_eq!(config.max_devices, 100);
        let config = DeviceSessionConfig::new(0);
        assert_eq!(config.max_devices, 1);
    }

    #[tokio::test]
    async fn test_memory_store_register_get_revoke() {
        let store = MemoryDeviceSessionStore::new();
        let device_info = DeviceInfo::with_device_id("dev1");

        store
            .register_session(1, "dev1", &device_info, "jti-123", "access-jti-123")
            .await
            .unwrap();

        let session = store.get_session(1, "dev1").await.unwrap();
        assert!(session.is_some());
        assert_eq!(session.unwrap().jti, "jti-123");

        let sessions = store.get_sessions(1).await.unwrap();
        assert_eq!(sessions.len(), 1);

        let jti = store.revoke_session(1, "dev1").await.unwrap();
        assert_eq!(
            jti,
            Some(("jti-123".to_string(), "access-jti-123".to_string()))
        );

        let session = store.get_session(1, "dev1").await.unwrap();
        assert!(session.is_none());
    }

    #[tokio::test]
    async fn test_memory_store_cleanup_expired() {
        let store = MemoryDeviceSessionStore::new();
        let device_info = DeviceInfo::with_device_id("dev1");

        store
            .register_session(1, "dev1", &device_info, "jti-old", "access-jti-old")
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        let removed = store.cleanup_expired(1, 1).await.unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(
            removed[0],
            ("jti-old".to_string(), "access-jti-old".to_string())
        );

        let sessions = store.get_sessions(1).await.unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_memory_store_clear_user_sessions() {
        let store = MemoryDeviceSessionStore::new();

        store
            .register_session(
                1,
                "dev1",
                &DeviceInfo::with_device_id("dev1"),
                "jti1",
                "access-jti1",
            )
            .await
            .unwrap();
        store
            .register_session(
                1,
                "dev2",
                &DeviceInfo::with_device_id("dev2"),
                "jti2",
                "access-jti2",
            )
            .await
            .unwrap();
        store
            .register_session(
                2,
                "dev3",
                &DeviceInfo::with_device_id("dev3"),
                "jti3",
                "access-jti3",
            )
            .await
            .unwrap();

        let removed = store.clear_user_sessions(1).await.unwrap();
        assert_eq!(removed.len(), 2);

        let sessions = store.get_sessions(1).await.unwrap();
        assert!(sessions.is_empty());

        let sessions = store.get_sessions(2).await.unwrap();
        assert_eq!(sessions.len(), 1);
    }

    #[tokio::test]
    async fn test_memory_store_update_session_jti() {
        let store = MemoryDeviceSessionStore::new();
        let device_info = DeviceInfo::with_device_id("dev1");

        store
            .register_session(1, "dev1", &device_info, "jti-old", "access-jti-old")
            .await
            .unwrap();
        store
            .update_session_jti(1, "dev1", "jti-new")
            .await
            .unwrap();

        let session = store.get_session(1, "dev1").await.unwrap().unwrap();
        assert_eq!(session.jti, "jti-new");
    }

    // ── MemoryDegradationStore 测试 ──

    #[tokio::test]
    async fn test_degradation_store_user_crud() {
        let store = MemoryDegradationStore::new();
        let entry = DegradationEntry {
            roles: vec!["user".to_string()],
            permissions: vec!["read".to_string()],
            expires_at: chrono::Utc::now().timestamp() + 3600,
        };

        store.set_user_degradation(1, entry).await.unwrap();
        let got = store.get_user_degradation(1).await.unwrap();
        assert!(got.is_some());
        assert_eq!(got.unwrap().roles, vec!["user".to_string()]);

        store.clear_user_degradation(1).await.unwrap();
        assert!(store.get_user_degradation(1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_degradation_store_device_crud() {
        let store = MemoryDegradationStore::new();
        let entry = DegradationEntry {
            roles: vec!["guest".to_string()],
            permissions: vec![],
            expires_at: chrono::Utc::now().timestamp() + 3600,
        };

        store
            .set_device_degradation(1, "dev1", entry)
            .await
            .unwrap();
        let got = store.get_device_degradation(1, "dev1").await.unwrap();
        assert!(got.is_some());
        assert_eq!(got.unwrap().roles, vec!["guest".to_string()]);

        store.clear_device_degradation(1, "dev1").await.unwrap();
        assert!(store
            .get_device_degradation(1, "dev1")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_degradation_store_ttl_expired() {
        let store = MemoryDegradationStore::new();
        let entry = DegradationEntry {
            roles: vec!["user".to_string()],
            permissions: vec![],
            expires_at: chrono::Utc::now().timestamp() - 1,
        };

        store.set_user_degradation(1, entry).await.unwrap();
        assert!(store.get_user_degradation(1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_degradation_store_clear_all() {
        let store = MemoryDegradationStore::new();
        let entry = DegradationEntry {
            roles: vec!["user".to_string()],
            permissions: vec![],
            expires_at: chrono::Utc::now().timestamp() + 3600,
        };

        store.set_user_degradation(1, entry.clone()).await.unwrap();
        store
            .set_device_degradation(1, "dev1", entry.clone())
            .await
            .unwrap();
        store
            .set_device_degradation(1, "dev2", entry)
            .await
            .unwrap();
        store
            .set_user_degradation(
                2,
                DegradationEntry {
                    roles: vec!["admin".to_string()],
                    permissions: vec![],
                    expires_at: chrono::Utc::now().timestamp() + 3600,
                },
            )
            .await
            .unwrap();

        store.clear_all_degradations(1).await.unwrap();

        assert!(store.get_user_degradation(1).await.unwrap().is_none());
        assert!(store
            .get_device_degradation(1, "dev1")
            .await
            .unwrap()
            .is_none());
        assert!(store
            .get_device_degradation(1, "dev2")
            .await
            .unwrap()
            .is_none());
        assert!(store.get_user_degradation(2).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_issue_with_device_token_has_device_id() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer
            .issue_with_device(1, "user1", "dev-123")
            .await
            .unwrap();
        let claims = verifier.verify_access(&pair.access_token).await.unwrap();
        assert_eq!(claims.device_id, Some("dev-123".to_string()));
    }

    #[tokio::test]
    async fn test_issue_without_device_token_no_device_id() {
        let (issuer, verifier, _) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();
        let claims = verifier.verify_access(&pair.access_token).await.unwrap();
        assert!(claims.device_id.is_none());
    }

    #[tokio::test]
    async fn test_issue_with_device_and_jti() {
        let (issuer, _, _) = make_issuer();
        let (pair, jti, access_jti) = issuer
            .issue_with_device_and_jti(1, "user1", "dev-1", Vec::new(), Vec::new())
            .await
            .unwrap();
        assert!(!pair.access_token.is_empty());
        assert!(!pair.refresh_token.is_empty());
        assert!(!jti.is_empty());
        assert!(!access_jti.is_empty());
    }

    #[tokio::test]
    async fn test_revoke_by_jti() {
        let (issuer, verifier, revoker) = make_issuer();
        let pair = issuer.issue(1, "user1").await.unwrap();
        let claims = verifier.verify_access(&pair.access_token).await.unwrap();

        revoker.revoke_by_jti(&claims.jti).await.unwrap();

        let result = verifier.verify_access(&pair.access_token).await;
        assert!(matches!(result, Err(RefreshTokenError::Revoked)));
    }
}
