// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Redis 存储后端 — RedisRefreshTokenStore + RedisTokenBlacklist
//!
//! 对齐 spec.md FR-1 ~ FR-6，design.md §2.1 ~ §2.6。
//!
//! ## 核心组件
//!
//! - [`RedisConfig`]：Redis 连接配置（URL + key 前缀 + 超时），Debug 脱敏密码
//! - [`RedisRefreshTokenStore`]：实现 [`RefreshTokenStore`] trait（GET / INCR）
//! - [`RedisTokenBlacklist`]：实现 [`TokenBlacklist`] trait（EXISTS / SETEX）
//! - [`create_redis_stores`]：便捷工厂，一次创建 Store + Blacklist 共享 ConnectionManager

use crate::refresh::{
    AuditEvent, AuditStore, DegradationEntry, DegradationStore, DeviceInfo, DeviceSession,
    DeviceSessionStore, RefreshTokenError, RefreshTokenStore, SsoTicket, TicketStore,
    TokenBlacklist,
};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::fmt;
use std::time::Duration;

// ── TlsConfig ──

/// TLS 配置错误
#[derive(Debug, thiserror::Error)]
pub enum TlsConfigError {
    /// 生产环境要求 TLS 连接
    #[error("生产环境要求 Redis TLS 连接 — 请使用 rediss:// 协议或设置 enable_tls=true")]
    RedisTlsRequired,
    /// TLS 证书无效
    #[error("TLS 证书无效: {0}")]
    TlsCertInvalid(String),
    /// CA 证书读取失败
    #[error("CA 证书读取失败: {0}")]
    CaCertReadError(String),
    /// 生产环境禁止跳过证书校验
    #[error("生产环境禁止 accept_invalid_cert=true")]
    AcceptInvalidForbiddenInProduction,
}

/// Redis TLS 配置
#[derive(Clone)]
pub struct TlsConfig {
    /// CA 证书路径（PEM 格式）
    pub ca_cert_path: String,
    /// 客户端证书路径（mTLS，可选）
    pub client_cert_path: Option<String>,
    /// 客户端私钥路径（mTLS，可选）
    pub client_key_path: Option<String>,
    /// SNI 主机名（可选）
    pub sni: Option<String>,
    /// 是否接受无效证书（仅开发环境，默认 false）
    pub accept_invalid_cert: bool,
}

impl fmt::Debug for TlsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsConfig")
            .field("ca_cert_path", &self.ca_cert_path)
            .field("client_cert_path", &self.client_cert_path)
            .field("client_key_path", &self.client_key_path)
            .field("sni", &self.sni)
            .field("accept_invalid_cert", &self.accept_invalid_cert)
            .finish()
    }
}

impl TlsConfig {
    /// 从环境变量读取 TLS 配置
    ///
    /// 环境变量：
    /// - `SZ300_REDIS_CA_CERT_PATH`（必填，缺失则返回 None）
    /// - `SZ300_REDIS_CLIENT_CERT_PATH`（可选）
    /// - `SZ300_REDIS_CLIENT_KEY_PATH`（可选）
    /// - `SZ300_REDIS_SNI`（可选）
    /// - `SZ300_REDIS_ACCEPT_INVALID_CERT`（默认 false）
    pub fn from_env() -> Option<Self> {
        let ca_cert_path = std::env::var("SZ300_REDIS_CA_CERT_PATH").ok()?;
        Some(Self {
            ca_cert_path,
            client_cert_path: std::env::var("SZ300_REDIS_CLIENT_CERT_PATH").ok(),
            client_key_path: std::env::var("SZ300_REDIS_CLIENT_KEY_PATH").ok(),
            sni: std::env::var("SZ300_REDIS_SNI").ok(),
            accept_invalid_cert: std::env::var("SZ300_REDIS_ACCEPT_INVALID_CERT")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
        })
    }

    /// 校验 CA 证书（异步读取文件，禁止 std::fs）
    pub async fn validate_ca_cert(&self) -> Result<(), TlsConfigError> {
        let content = tokio::fs::read_to_string(&self.ca_cert_path)
            .await
            .map_err(|e| TlsConfigError::CaCertReadError(e.to_string()))?;
        if !content.contains("BEGIN CERTIFICATE") {
            return Err(TlsConfigError::TlsCertInvalid(
                "文件内容不是有效的 PEM 格式证书".to_string(),
            ));
        }
        Ok(())
    }
}

// ── RedisConfig ──

/// Redis 存储配置
///
/// 对齐 design.md §2.2。URL 中的密码在 Debug 输出时自动脱敏。
#[derive(Clone)]
pub struct RedisConfig {
    /// Redis 连接 URL（如 `redis://:password@127.0.0.1:6379/0`）
    pub url: String,
    /// 版本号 key 前缀（默认 `sso:ver`）
    pub key_prefix_ver: String,
    /// 黑名单 key 前缀（默认 `sso:bl`）
    pub key_prefix_bl: String,
    /// 设备会话 key 前缀（默认 `sso:sessions`）
    pub key_prefix_sessions: String,
    /// 降级 key 前缀（默认 `sso:deg`）
    pub key_prefix_deg: String,
    /// 审计 key 前缀（默认 `sso:audit`）
    pub key_prefix_audit: String,
    /// Ticket key 前缀（默认 `sso:ticket`）
    pub key_prefix_ticket: String,
    /// 连接超时（默认 3s）
    pub connection_timeout: Duration,
    /// 命令超时（默认 2s）
    pub command_timeout: Duration,
    /// 是否启用 TLS（默认 false，`rediss://` URL 自动启用）
    pub enable_tls: bool,
    /// TLS 配置（enable_tls=true 时必填）
    pub tls_config: Option<TlsConfig>,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: "redis://127.0.0.1:6379".to_string(),
            key_prefix_ver: "sso:ver".to_string(),
            key_prefix_bl: "sso:bl".to_string(),
            key_prefix_sessions: "sso:sessions".to_string(),
            key_prefix_deg: "sso:deg".to_string(),
            key_prefix_audit: "sso:audit".to_string(),
            key_prefix_ticket: "sso:ticket".to_string(),
            connection_timeout: Duration::from_secs(3),
            command_timeout: Duration::from_secs(2),
            enable_tls: false,
            tls_config: None,
        }
    }
}

impl RedisConfig {
    /// 从 URL 创建配置，其余字段使用默认值
    pub fn from_url(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Default::default()
        }
    }

    /// 构造版本号 key：`{prefix}:{user_id}`
    fn ver_key(&self, user_id: i64) -> String {
        format!("{}:{}", self.key_prefix_ver, user_id)
    }

    /// 构造黑名单 key：`{prefix}:{jti}`
    fn bl_key(&self, jti: &str) -> String {
        format!("{}:{}", self.key_prefix_bl, jti)
    }

    /// 构造设备会话 key：`{prefix}:{user_id}`
    fn sessions_key(&self, user_id: i64) -> String {
        format!("{}:{}", self.key_prefix_sessions, user_id)
    }

    /// 构造用户级降级 key：`{prefix}:user:{user_id}`
    fn deg_user_key(&self, user_id: i64) -> String {
        format!("{}:user:{}", self.key_prefix_deg, user_id)
    }

    /// 构造设备级降级 key：`{prefix}:device:{user_id}:{device_id}`
    fn deg_device_key(&self, user_id: i64, device_id: &str) -> String {
        format!("{}:device:{}:{}", self.key_prefix_deg, user_id, device_id)
    }

    /// 构造设备级降级 SCAN pattern：`{prefix}:device:{user_id}:*`
    fn deg_device_pattern(&self, user_id: i64) -> String {
        format!("{}:device:{}:*", self.key_prefix_deg, user_id)
    }

    /// 构造审计用户 key：`{prefix}:user:{user_id}`
    fn audit_user_key(&self, user_id: i64) -> String {
        format!("{}:user:{}", self.key_prefix_audit, user_id)
    }

    /// 构造审计全局 key：`{prefix}:all`
    fn audit_all_key(&self) -> String {
        format!("{}:all", self.key_prefix_audit)
    }

    /// 构造 Ticket key：`{prefix}:{ticket}`
    fn ticket_key(&self, ticket: &str) -> String {
        format!("{}:{}", self.key_prefix_ticket, ticket)
    }

    /// 判断是否启用 TLS
    ///
    /// `rediss://` URL 协议或 `enable_tls=true` 均视为启用
    pub fn is_tls_enabled(&self) -> bool {
        self.url.starts_with("rediss://") || self.enable_tls
    }

    /// 校验生产环境 TLS 配置
    ///
    /// `SZ_ENV=production` 且未启用 TLS → 返回 `RedisTlsRequired` 错误
    pub fn validate_production_tls(&self, env: &str) -> Result<(), TlsConfigError> {
        if env != "production" {
            return Ok(());
        }
        if !self.is_tls_enabled() {
            return Err(TlsConfigError::RedisTlsRequired);
        }
        if let Some(ref tls) = self.tls_config {
            if tls.accept_invalid_cert {
                return Err(TlsConfigError::AcceptInvalidForbiddenInProduction);
            }
        }
        Ok(())
    }
}

/// Debug 实现脱敏 URL 中的密码
impl fmt::Debug for RedisConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_url = redact_redis_url(&self.url);
        f.debug_struct("RedisConfig")
            .field("url", &redacted_url)
            .field("key_prefix_ver", &self.key_prefix_ver)
            .field("key_prefix_bl", &self.key_prefix_bl)
            .field("key_prefix_sessions", &self.key_prefix_sessions)
            .field("key_prefix_deg", &self.key_prefix_deg)
            .field("key_prefix_audit", &self.key_prefix_audit)
            .field("key_prefix_ticket", &self.key_prefix_ticket)
            .field("connection_timeout", &self.connection_timeout)
            .field("command_timeout", &self.command_timeout)
            .field("enable_tls", &self.enable_tls)
            .field("tls_config", &self.tls_config)
            .finish()
    }
}

/// 脱敏 Redis URL 中的密码部分
///
/// `redis://:secret@host:port` → `redis://[REDACTED]@host:port`
fn redact_redis_url(url: &str) -> String {
    if let Some(at_pos) = url.find('@') {
        if let Some(scheme_end) = url.find("://") {
            let password_start = scheme_end + 3;
            if at_pos > password_start {
                let (before, after) = url.split_at(at_pos);
                let scheme = &before[..password_start];
                return format!("{}[REDACTED]{}", scheme, after);
            }
        }
    }
    url.to_string()
}

// ── RedisRefreshTokenStore ──

/// Redis 版本号存储
///
/// 实现 [`RefreshTokenStore`] trait，使用 Redis `GET` / `INCR` 命令。
/// key 格式：`{key_prefix_ver}:{user_id}`，不存在时返回 0（与 Memory 行为一致）。
pub struct RedisRefreshTokenStore {
    conn: ConnectionManager,
    config: RedisConfig,
}

impl RedisRefreshTokenStore {
    /// 创建 Redis 版本号存储
    ///
    /// 内部建立 `ConnectionManager`（自动重连 + 连接池复用）。
    pub async fn new(config: RedisConfig) -> Result<Self, RefreshTokenError> {
        let client = redis::Client::open(config.url.as_str())
            .map_err(|e| RefreshTokenError::Cache(format!("redis client open failed: {e}")))?;

        let conn = tokio::time::timeout(config.connection_timeout, client.get_connection_manager())
            .await
            .map_err(|_| RefreshTokenError::ServiceUnavailable)?
            .map_err(|e| RefreshTokenError::Cache(format!("redis connect failed: {e}")))?;

        Ok(Self { conn, config })
    }
}

#[async_trait::async_trait]
impl RefreshTokenStore for RedisRefreshTokenStore {
    async fn get_version(&self, user_id: i64) -> Result<u64, RefreshTokenError> {
        let key = self.config.ver_key(user_id);
        let mut conn = self.conn.clone();
        let result: Option<u64> = tokio::time::timeout(
            self.config.command_timeout,
            conn.get::<&str, Option<u64>>(&key),
        )
        .await
        .map_err(|_| RefreshTokenError::ServiceUnavailable)?
        .map_err(|e| RefreshTokenError::Cache(format!("redis GET failed: {e}")))?;

        Ok(result.unwrap_or(0))
    }

    async fn increment_version(&self, user_id: i64) -> Result<u64, RefreshTokenError> {
        let key = self.config.ver_key(user_id);
        let mut conn = self.conn.clone();
        let new_version: u64 = tokio::time::timeout(
            self.config.command_timeout,
            conn.incr::<&str, u64, u64>(&key, 1),
        )
        .await
        .map_err(|_| RefreshTokenError::ServiceUnavailable)?
        .map_err(|e| RefreshTokenError::Cache(format!("redis INCR failed: {e}")))?;

        Ok(new_version)
    }
}

// ── RedisTokenBlacklist ──

/// Redis Token 黑名单
///
/// 实现 [`TokenBlacklist`] trait，使用 Redis `EXISTS` / `SETEX` 命令。
/// key 格式：`{key_prefix_bl}:{jti}`，TTL 由调用方传入（Token 剩余有效期）。
pub struct RedisTokenBlacklist {
    conn: ConnectionManager,
    config: RedisConfig,
}

impl RedisTokenBlacklist {
    /// 创建 Redis Token 黑名单
    pub async fn new(config: RedisConfig) -> Result<Self, RefreshTokenError> {
        let client = redis::Client::open(config.url.as_str())
            .map_err(|e| RefreshTokenError::Cache(format!("redis client open failed: {e}")))?;

        let conn = tokio::time::timeout(config.connection_timeout, client.get_connection_manager())
            .await
            .map_err(|_| RefreshTokenError::ServiceUnavailable)?
            .map_err(|e| RefreshTokenError::Cache(format!("redis connect failed: {e}")))?;

        Ok(Self { conn, config })
    }
}

#[async_trait::async_trait]
impl TokenBlacklist for RedisTokenBlacklist {
    async fn revoke(&self, jti: &str, ttl_secs: u64) -> Result<(), RefreshTokenError> {
        if ttl_secs == 0 {
            return Ok(());
        }
        let key = self.config.bl_key(jti);
        let mut conn = self.conn.clone();
        tokio::time::timeout(
            self.config.command_timeout,
            conn.set_ex::<&str, &str, ()>(&key, "1", ttl_secs),
        )
        .await
        .map_err(|_| RefreshTokenError::ServiceUnavailable)?
        .map_err(|e| RefreshTokenError::Cache(format!("redis SETEX failed: {e}")))?;

        Ok(())
    }

    async fn is_revoked(&self, jti: &str) -> Result<bool, RefreshTokenError> {
        let key = self.config.bl_key(jti);
        let mut conn = self.conn.clone();
        let exists: bool =
            tokio::time::timeout(self.config.command_timeout, conn.exists::<&str, bool>(&key))
                .await
                .map_err(|_| RefreshTokenError::ServiceUnavailable)?
                .map_err(|e| RefreshTokenError::Cache(format!("redis EXISTS failed: {e}")))?;

        Ok(exists)
    }
}

// ── RedisDeviceSessionStore ──

/// Redis 设备会话存储
///
/// 实现 [`DeviceSessionStore`] trait，使用 Redis Hash 命令。
/// key 格式：`{key_prefix_sessions}:{user_id}`，field 为 `{device_id}`，
/// value 为 `serde_json(DeviceSession)`。
pub struct RedisDeviceSessionStore {
    conn: ConnectionManager,
    config: RedisConfig,
}

impl RedisDeviceSessionStore {
    /// 创建 Redis 设备会话存储
    pub async fn new(config: RedisConfig) -> Result<Self, RefreshTokenError> {
        let client = redis::Client::open(config.url.as_str())
            .map_err(|e| RefreshTokenError::Cache(format!("redis client open failed: {e}")))?;

        let conn = tokio::time::timeout(config.connection_timeout, client.get_connection_manager())
            .await
            .map_err(|_| RefreshTokenError::ServiceUnavailable)?
            .map_err(|e| RefreshTokenError::Cache(format!("redis connect failed: {e}")))?;

        Ok(Self { conn, config })
    }

    /// 从已有 ConnectionManager 创建（共享连接池）
    pub fn from_conn(conn: ConnectionManager, config: RedisConfig) -> Self {
        Self { conn, config }
    }
}

#[async_trait::async_trait]
impl DeviceSessionStore for RedisDeviceSessionStore {
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
        let key = self.config.sessions_key(user_id);
        let value = serde_json::to_string(&session)
            .map_err(|e| RefreshTokenError::Cache(format!("json serialize failed: {e}")))?;
        let mut conn = self.conn.clone();
        tokio::time::timeout(
            self.config.command_timeout,
            conn.hset::<&str, &str, &str, ()>(&key, device_id, &value),
        )
        .await
        .map_err(|_| RefreshTokenError::ServiceUnavailable)?
        .map_err(|e| RefreshTokenError::Cache(format!("redis HSET failed: {e}")))?;
        Ok(())
    }

    async fn get_sessions(&self, user_id: i64) -> Result<Vec<DeviceSession>, RefreshTokenError> {
        let key = self.config.sessions_key(user_id);
        let mut conn = self.conn.clone();
        let map: std::collections::HashMap<String, String> = tokio::time::timeout(
            self.config.command_timeout,
            conn.hgetall::<&str, std::collections::HashMap<String, String>>(&key),
        )
        .await
        .map_err(|_| RefreshTokenError::ServiceUnavailable)?
        .map_err(|e| RefreshTokenError::Cache(format!("redis HGETALL failed: {e}")))?;

        let mut sessions = Vec::with_capacity(map.len());
        for (_, v) in map {
            let session: DeviceSession = serde_json::from_str(&v)
                .map_err(|e| RefreshTokenError::Cache(format!("json deserialize failed: {e}")))?;
            sessions.push(session);
        }
        Ok(sessions)
    }

    async fn get_session(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<Option<DeviceSession>, RefreshTokenError> {
        let key = self.config.sessions_key(user_id);
        let mut conn = self.conn.clone();
        let value: Option<String> = tokio::time::timeout(
            self.config.command_timeout,
            conn.hget::<&str, &str, Option<String>>(&key, device_id),
        )
        .await
        .map_err(|_| RefreshTokenError::ServiceUnavailable)?
        .map_err(|e| RefreshTokenError::Cache(format!("redis HGET failed: {e}")))?;

        match value {
            Some(v) => {
                let session: DeviceSession = serde_json::from_str(&v).map_err(|e| {
                    RefreshTokenError::Cache(format!("json deserialize failed: {e}"))
                })?;
                Ok(Some(session))
            }
            None => Ok(None),
        }
    }

    async fn revoke_session(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<Option<(String, String)>, RefreshTokenError> {
        let key = self.config.sessions_key(user_id);
        let mut conn = self.conn.clone();

        let value: Option<String> = tokio::time::timeout(
            self.config.command_timeout,
            conn.hget::<&str, &str, Option<String>>(&key, device_id),
        )
        .await
        .map_err(|_| RefreshTokenError::ServiceUnavailable)?
        .map_err(|e| RefreshTokenError::Cache(format!("redis HGET failed: {e}")))?;

        match value {
            Some(v) => {
                let session: DeviceSession = serde_json::from_str(&v).map_err(|e| {
                    RefreshTokenError::Cache(format!("json deserialize failed: {e}"))
                })?;
                tokio::time::timeout(
                    self.config.command_timeout,
                    conn.hdel::<&str, &str, ()>(&key, device_id),
                )
                .await
                .map_err(|_| RefreshTokenError::ServiceUnavailable)?
                .map_err(|e| RefreshTokenError::Cache(format!("redis HDEL failed: {e}")))?;
                Ok(Some((session.jti, session.access_jti)))
            }
            None => Ok(None),
        }
    }

    async fn update_last_active(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<(), RefreshTokenError> {
        let key = self.config.sessions_key(user_id);
        let mut conn = self.conn.clone();

        let value: Option<String> = tokio::time::timeout(
            self.config.command_timeout,
            conn.hget::<&str, &str, Option<String>>(&key, device_id),
        )
        .await
        .map_err(|_| RefreshTokenError::ServiceUnavailable)?
        .map_err(|e| RefreshTokenError::Cache(format!("redis HGET failed: {e}")))?;

        match value {
            Some(v) => {
                let mut session: DeviceSession = serde_json::from_str(&v).map_err(|e| {
                    RefreshTokenError::Cache(format!("json deserialize failed: {e}"))
                })?;
                session.last_active = chrono::Utc::now().timestamp();
                let new_value = serde_json::to_string(&session)
                    .map_err(|e| RefreshTokenError::Cache(format!("json serialize failed: {e}")))?;
                tokio::time::timeout(
                    self.config.command_timeout,
                    conn.hset::<&str, &str, &str, ()>(&key, device_id, &new_value),
                )
                .await
                .map_err(|_| RefreshTokenError::ServiceUnavailable)?
                .map_err(|e| RefreshTokenError::Cache(format!("redis HSET failed: {e}")))?;
                Ok(())
            }
            None => Ok(()),
        }
    }

    async fn update_session_jti(
        &self,
        user_id: i64,
        device_id: &str,
        new_jti: &str,
    ) -> Result<(), RefreshTokenError> {
        let key = self.config.sessions_key(user_id);
        let mut conn = self.conn.clone();

        let value: Option<String> = tokio::time::timeout(
            self.config.command_timeout,
            conn.hget::<&str, &str, Option<String>>(&key, device_id),
        )
        .await
        .map_err(|_| RefreshTokenError::ServiceUnavailable)?
        .map_err(|e| RefreshTokenError::Cache(format!("redis HGET failed: {e}")))?;

        match value {
            Some(v) => {
                let mut session: DeviceSession = serde_json::from_str(&v).map_err(|e| {
                    RefreshTokenError::Cache(format!("json deserialize failed: {e}"))
                })?;
                session.jti = new_jti.to_string();
                session.last_active = chrono::Utc::now().timestamp();
                let new_value = serde_json::to_string(&session)
                    .map_err(|e| RefreshTokenError::Cache(format!("json serialize failed: {e}")))?;
                tokio::time::timeout(
                    self.config.command_timeout,
                    conn.hset::<&str, &str, &str, ()>(&key, device_id, &new_value),
                )
                .await
                .map_err(|_| RefreshTokenError::ServiceUnavailable)?
                .map_err(|e| RefreshTokenError::Cache(format!("redis HSET failed: {e}")))?;
                Ok(())
            }
            None => Ok(()),
        }
    }

    async fn cleanup_expired(
        &self,
        user_id: i64,
        ttl_secs: i64,
    ) -> Result<Vec<(String, String)>, RefreshTokenError> {
        let key = self.config.sessions_key(user_id);
        let mut conn = self.conn.clone();

        let map: std::collections::HashMap<String, String> = tokio::time::timeout(
            self.config.command_timeout,
            conn.hgetall::<&str, std::collections::HashMap<String, String>>(&key),
        )
        .await
        .map_err(|_| RefreshTokenError::ServiceUnavailable)?
        .map_err(|e| RefreshTokenError::Cache(format!("redis HGETALL failed: {e}")))?;

        let now = chrono::Utc::now().timestamp();
        let mut expired_fields = Vec::new();
        let mut jti_list = Vec::new();

        for (field, v) in map {
            let session: DeviceSession = serde_json::from_str(&v)
                .map_err(|e| RefreshTokenError::Cache(format!("json deserialize failed: {e}")))?;
            if session.last_active + ttl_secs < now {
                jti_list.push((session.jti.clone(), session.access_jti.clone()));
                expired_fields.push(field);
            }
        }

        if !expired_fields.is_empty() {
            for field in &expired_fields {
                tokio::time::timeout(
                    self.config.command_timeout,
                    conn.hdel::<&str, &str, ()>(&key, field),
                )
                .await
                .map_err(|_| RefreshTokenError::ServiceUnavailable)?
                .map_err(|e| RefreshTokenError::Cache(format!("redis HDEL failed: {e}")))?;
            }
            tracing::debug!(
                user_id,
                count = expired_fields.len(),
                "expired sessions cleaned"
            );
        }

        Ok(jti_list)
    }

    async fn clear_user_sessions(
        &self,
        user_id: i64,
    ) -> Result<Vec<(String, String)>, RefreshTokenError> {
        let key = self.config.sessions_key(user_id);
        let mut conn = self.conn.clone();

        let map: std::collections::HashMap<String, String> = tokio::time::timeout(
            self.config.command_timeout,
            conn.hgetall::<&str, std::collections::HashMap<String, String>>(&key),
        )
        .await
        .map_err(|_| RefreshTokenError::ServiceUnavailable)?
        .map_err(|e| RefreshTokenError::Cache(format!("redis HGETALL failed: {e}")))?;

        let mut jti_list = Vec::with_capacity(map.len());
        for (_, v) in map {
            let session: DeviceSession = serde_json::from_str(&v)
                .map_err(|e| RefreshTokenError::Cache(format!("json deserialize failed: {e}")))?;
            jti_list.push((session.jti, session.access_jti));
        }

        tokio::time::timeout(self.config.command_timeout, conn.del::<&str, ()>(&key))
            .await
            .map_err(|_| RefreshTokenError::ServiceUnavailable)?
            .map_err(|e| RefreshTokenError::Cache(format!("redis DEL failed: {e}")))?;

        Ok(jti_list)
    }
}

// ── 便捷工厂 ──

/// 一次创建 Redis Store + Blacklist，共享同一 ConnectionManager
///
/// 对齐 design.md §2.5。返回 `(Store, Blacklist)`，两者各自持有独立的
/// `ConnectionManager` clone（内部 Arc 共享连接池）。
pub async fn create_redis_stores(
    config: RedisConfig,
) -> Result<
    (
        std::sync::Arc<dyn RefreshTokenStore>,
        std::sync::Arc<dyn TokenBlacklist>,
    ),
    RefreshTokenError,
> {
    let store = RedisRefreshTokenStore::new(config.clone()).await?;
    let blacklist = RedisTokenBlacklist::new(config).await?;
    Ok((std::sync::Arc::new(store), std::sync::Arc::new(blacklist)))
}

/// 一次创建 Redis Store + Blacklist + DeviceSessionStore，共享同一 ConnectionManager
///
/// 对齐 multi-device-session design.md §6.3。返回三元组，
/// 三者各自持有独立的 `ConnectionManager` clone（内部 Arc 共享连接池）。
pub async fn create_redis_stores_with_devices(
    config: RedisConfig,
) -> Result<
    (
        std::sync::Arc<dyn RefreshTokenStore>,
        std::sync::Arc<dyn TokenBlacklist>,
        std::sync::Arc<dyn DeviceSessionStore>,
    ),
    RefreshTokenError,
> {
    let store = RedisRefreshTokenStore::new(config.clone()).await?;
    let blacklist = RedisTokenBlacklist::new(config.clone()).await?;
    let device_store = RedisDeviceSessionStore::new(config).await?;
    Ok((
        std::sync::Arc::new(store),
        std::sync::Arc::new(blacklist),
        std::sync::Arc::new(device_store),
    ))
}

// ── RedisDegradationStore ──

/// Redis 降级存储
///
/// 实现 [`DegradationStore`] trait，使用 Redis `SET EX` / `GET` / `DEL` 命令。
/// key 格式：`{key_prefix_deg}:user:{user_id}`（用户级），
/// `{key_prefix_deg}:device:{user_id}:{device_id}`（设备级）。
/// TTL = `expires_at - now`，过期后 Redis 自动清除。
pub struct RedisDegradationStore {
    conn: ConnectionManager,
    config: RedisConfig,
}

impl RedisDegradationStore {
    /// 创建 Redis 降级存储
    pub async fn new(config: RedisConfig) -> Result<Self, RefreshTokenError> {
        let client = redis::Client::open(config.url.as_str())
            .map_err(|e| RefreshTokenError::Cache(format!("redis client open failed: {e}")))?;
        let conn = tokio::time::timeout(config.connection_timeout, client.get_connection_manager())
            .await
            .map_err(|_| RefreshTokenError::ServiceUnavailable)?
            .map_err(|e| RefreshTokenError::Cache(format!("redis connect failed: {e}")))?;
        Ok(Self { conn, config })
    }

    /// 从已有 ConnectionManager 创建（共享连接池）
    pub fn from_conn(conn: ConnectionManager, config: RedisConfig) -> Self {
        Self { conn, config }
    }

    /// 执行带超时的 Redis 命令
    async fn with_timeout<F, T>(&self, f: F) -> Result<T, RefreshTokenError>
    where
        F: std::future::Future<Output = Result<T, redis::RedisError>>,
    {
        tokio::time::timeout(self.config.command_timeout, f)
            .await
            .map_err(|_| RefreshTokenError::ServiceUnavailable)?
            .map_err(|e| RefreshTokenError::Cache(format!("redis command failed: {e}")))
    }
}

#[async_trait::async_trait]
impl DegradationStore for RedisDegradationStore {
    async fn set_user_degradation(
        &self,
        user_id: i64,
        entry: DegradationEntry,
    ) -> Result<(), RefreshTokenError> {
        let key = self.config.deg_user_key(user_id);
        let value = serde_json::to_string(&entry)
            .map_err(|e| RefreshTokenError::Cache(format!("serialize failed: {e}")))?;
        let now = chrono::Utc::now().timestamp();
        let ttl = (entry.expires_at - now).max(1) as u64;
        let mut conn = self.conn.clone();
        self.with_timeout(async { conn.set_ex::<&str, &str, ()>(&key, &value, ttl).await })
            .await
    }

    async fn get_user_degradation(
        &self,
        user_id: i64,
    ) -> Result<Option<DegradationEntry>, RefreshTokenError> {
        let key = self.config.deg_user_key(user_id);
        let mut conn = self.conn.clone();
        let result: Option<String> = self
            .with_timeout(async { conn.get::<&str, Option<String>>(&key).await })
            .await?;
        match result {
            Some(s) => {
                let entry: DegradationEntry = serde_json::from_str(&s)
                    .map_err(|e| RefreshTokenError::Cache(format!("deserialize failed: {e}")))?;
                if entry.expires_at <= chrono::Utc::now().timestamp() {
                    Ok(None)
                } else {
                    Ok(Some(entry))
                }
            }
            None => Ok(None),
        }
    }

    async fn clear_user_degradation(&self, user_id: i64) -> Result<(), RefreshTokenError> {
        let key = self.config.deg_user_key(user_id);
        let mut conn = self.conn.clone();
        self.with_timeout(async { conn.del::<&str, ()>(&key).await })
            .await
    }

    async fn set_device_degradation(
        &self,
        user_id: i64,
        device_id: &str,
        entry: DegradationEntry,
    ) -> Result<(), RefreshTokenError> {
        let key = self.config.deg_device_key(user_id, device_id);
        let value = serde_json::to_string(&entry)
            .map_err(|e| RefreshTokenError::Cache(format!("serialize failed: {e}")))?;
        let now = chrono::Utc::now().timestamp();
        let ttl = (entry.expires_at - now).max(1) as u64;
        let mut conn = self.conn.clone();
        self.with_timeout(async { conn.set_ex::<&str, &str, ()>(&key, &value, ttl).await })
            .await
    }

    async fn get_device_degradation(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<Option<DegradationEntry>, RefreshTokenError> {
        let key = self.config.deg_device_key(user_id, device_id);
        let mut conn = self.conn.clone();
        let result: Option<String> = self
            .with_timeout(async { conn.get::<&str, Option<String>>(&key).await })
            .await?;
        match result {
            Some(s) => {
                let entry: DegradationEntry = serde_json::from_str(&s)
                    .map_err(|e| RefreshTokenError::Cache(format!("deserialize failed: {e}")))?;
                if entry.expires_at <= chrono::Utc::now().timestamp() {
                    Ok(None)
                } else {
                    Ok(Some(entry))
                }
            }
            None => Ok(None),
        }
    }

    async fn clear_device_degradation(
        &self,
        user_id: i64,
        device_id: &str,
    ) -> Result<(), RefreshTokenError> {
        let key = self.config.deg_device_key(user_id, device_id);
        let mut conn = self.conn.clone();
        self.with_timeout(async { conn.del::<&str, ()>(&key).await })
            .await
    }

    async fn clear_all_degradations(&self, user_id: i64) -> Result<(), RefreshTokenError> {
        let user_key = self.config.deg_user_key(user_id);
        let pattern = self.config.deg_device_pattern(user_id);
        let mut conn = self.conn.clone();

        // 删除用户级降级
        self.with_timeout(async { conn.del::<&str, ()>(&user_key).await })
            .await?;

        // SCAN + DEL 删除所有设备级降级
        let mut cursor: u64 = 0;
        loop {
            let (next_cursor, keys): (u64, Vec<String>) = self
                .with_timeout(async {
                    redis::cmd("SCAN")
                        .arg(cursor)
                        .arg("MATCH")
                        .arg(&pattern)
                        .arg("COUNT")
                        .arg(100)
                        .query_async(&mut conn)
                        .await
                })
                .await?;
            if !keys.is_empty() {
                self.with_timeout::<_, ()>(async {
                    redis::cmd("DEL").arg(&keys).query_async(&mut conn).await
                })
                .await?;
            }
            cursor = next_cursor;
            if cursor == 0 {
                break;
            }
        }
        Ok(())
    }
}

// ── RedisAuditStore ──

/// Redis 审计存储
///
/// 实现 [`AuditStore`] trait，使用 Redis Sorted Set（`ZADD` / `ZREVRANGE` / `ZRANGEBYSCORE`）。
/// key 格式：`{key_prefix_audit}:user:{user_id}`（用户级）和 `{key_prefix_audit}:all`（全局）。
/// score 为事件时间戳，member 为 `serde_json(AuditEvent)`。
pub struct RedisAuditStore {
    conn: ConnectionManager,
    config: RedisConfig,
}

impl RedisAuditStore {
    /// 创建 Redis 审计存储
    pub async fn new(config: RedisConfig) -> Result<Self, RefreshTokenError> {
        let client = redis::Client::open(config.url.as_str())
            .map_err(|e| RefreshTokenError::Cache(format!("redis client open failed: {e}")))?;
        let conn = tokio::time::timeout(config.connection_timeout, client.get_connection_manager())
            .await
            .map_err(|_| RefreshTokenError::ServiceUnavailable)?
            .map_err(|e| RefreshTokenError::Cache(format!("redis connect failed: {e}")))?;
        Ok(Self { conn, config })
    }

    /// 从已有 ConnectionManager 创建（共享连接池）
    pub fn from_conn(conn: ConnectionManager, config: RedisConfig) -> Self {
        Self { conn, config }
    }

    async fn with_timeout<F, T>(&self, f: F) -> Result<T, RefreshTokenError>
    where
        F: std::future::Future<Output = Result<T, redis::RedisError>>,
    {
        tokio::time::timeout(self.config.command_timeout, f)
            .await
            .map_err(|_| RefreshTokenError::ServiceUnavailable)?
            .map_err(|e| RefreshTokenError::Cache(format!("redis command failed: {e}")))
    }
}

#[async_trait::async_trait]
impl AuditStore for RedisAuditStore {
    async fn record(&self, event: AuditEvent) -> Result<(), RefreshTokenError> {
        let value = serde_json::to_string(&event)
            .map_err(|e| RefreshTokenError::Cache(format!("serialize failed: {e}")))?;
        let score = event.timestamp as f64;
        let mut conn = self.conn.clone();

        // 写入全局 Sorted Set
        let all_key = self.config.audit_all_key();
        self.with_timeout(async {
            redis::cmd("ZADD")
                .arg(&all_key)
                .arg(score)
                .arg(&value)
                .query_async::<()>(&mut conn)
                .await
        })
        .await?;

        // 写入用户级 Sorted Set
        if let Some(user_id) = event.user_id {
            let user_key = self.config.audit_user_key(user_id);
            self.with_timeout(async {
                redis::cmd("ZADD")
                    .arg(&user_key)
                    .arg(score)
                    .arg(&value)
                    .query_async::<()>(&mut conn)
                    .await
            })
            .await?;
        }
        Ok(())
    }

    async fn query_by_user(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, RefreshTokenError> {
        let key = self.config.audit_user_key(user_id);
        let mut conn = self.conn.clone();
        let results: Vec<String> = self
            .with_timeout(async {
                redis::cmd("ZREVRANGE")
                    .arg(&key)
                    .arg(0)
                    .arg(limit.saturating_sub(1) as i64)
                    .query_async(&mut conn)
                    .await
            })
            .await?;
        let mut events = Vec::with_capacity(results.len());
        for s in results {
            if let Ok(e) = serde_json::from_str::<AuditEvent>(&s) {
                events.push(e);
            }
        }
        Ok(events)
    }

    async fn query_by_time_range(
        &self,
        start: i64,
        end: i64,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, RefreshTokenError> {
        let key = self.config.audit_all_key();
        let mut conn = self.conn.clone();
        let results: Vec<String> = self
            .with_timeout(async {
                redis::cmd("ZRANGEBYSCORE")
                    .arg(&key)
                    .arg(start)
                    .arg(end)
                    .arg("LIMIT")
                    .arg(0)
                    .arg(limit as i64)
                    .query_async(&mut conn)
                    .await
            })
            .await?;
        let mut events = Vec::with_capacity(results.len());
        for s in results {
            if let Ok(e) = serde_json::from_str::<AuditEvent>(&s) {
                events.push(e);
            }
        }
        Ok(events)
    }
}

// ── RedisTicketStore ──

/// Redis Ticket 存储
///
/// 实现 [`TicketStore`] trait，使用 Redis `SET EX` / `GET` / `DEL` 命令。
/// key 格式：`{key_prefix_ticket}:{ticket}`。
/// TTL = `expires_at - now`，过期后 Redis 自动清除。
/// `take` 使用 `GETDEL`（Redis 6.2+）实现原子删除，回退到 `GET` + `DEL` pipeline。
pub struct RedisTicketStore {
    conn: ConnectionManager,
    config: RedisConfig,
}

impl RedisTicketStore {
    /// 创建 Redis Ticket 存储
    pub async fn new(config: RedisConfig) -> Result<Self, RefreshTokenError> {
        let client = redis::Client::open(config.url.as_str())
            .map_err(|e| RefreshTokenError::Cache(format!("redis client open failed: {e}")))?;
        let conn = tokio::time::timeout(config.connection_timeout, client.get_connection_manager())
            .await
            .map_err(|_| RefreshTokenError::ServiceUnavailable)?
            .map_err(|e| RefreshTokenError::Cache(format!("redis connect failed: {e}")))?;
        Ok(Self { conn, config })
    }

    /// 从已有 ConnectionManager 创建（共享连接池）
    pub fn from_conn(conn: ConnectionManager, config: RedisConfig) -> Self {
        Self { conn, config }
    }

    async fn with_timeout<F, T>(&self, f: F) -> Result<T, RefreshTokenError>
    where
        F: std::future::Future<Output = Result<T, redis::RedisError>>,
    {
        tokio::time::timeout(self.config.command_timeout, f)
            .await
            .map_err(|_| RefreshTokenError::ServiceUnavailable)?
            .map_err(|e| RefreshTokenError::Cache(format!("redis command failed: {e}")))
    }
}

#[async_trait::async_trait]
impl TicketStore for RedisTicketStore {
    async fn save(&self, ticket: SsoTicket) -> Result<(), RefreshTokenError> {
        let key = self.config.ticket_key(&ticket.ticket);
        let value = serde_json::to_string(&ticket)
            .map_err(|e| RefreshTokenError::Cache(format!("serialize failed: {e}")))?;
        let now = chrono::Utc::now().timestamp();
        let ttl = (ticket.expires_at - now).max(1) as u64;
        let mut conn = self.conn.clone();
        self.with_timeout(async { conn.set_ex::<&str, &str, ()>(&key, &value, ttl).await })
            .await
    }

    async fn take(&self, ticket: &str) -> Result<Option<SsoTicket>, RefreshTokenError> {
        let key = self.config.ticket_key(ticket);
        let mut conn = self.conn.clone();

        // 使用 pipeline 原子执行 GET + DEL
        let (value, _): (Option<String>, ()) = self
            .with_timeout(async {
                redis::pipe()
                    .cmd("GET")
                    .arg(&key)
                    .cmd("DEL")
                    .arg(&key)
                    .query_async(&mut conn)
                    .await
            })
            .await?;

        match value {
            Some(s) => {
                let ticket: SsoTicket = serde_json::from_str(&s)
                    .map_err(|e| RefreshTokenError::Cache(format!("deserialize failed: {e}")))?;
                if ticket.expires_at <= chrono::Utc::now().timestamp() {
                    Ok(None)
                } else {
                    Ok(Some(ticket))
                }
            }
            None => Ok(None),
        }
    }

    async fn peek(&self, ticket: &str) -> Result<Option<SsoTicket>, RefreshTokenError> {
        let key = self.config.ticket_key(ticket);
        let mut conn = self.conn.clone();
        let result: Option<String> = self
            .with_timeout(async { conn.get::<&str, Option<String>>(&key).await })
            .await?;
        match result {
            Some(s) => {
                let ticket: SsoTicket = serde_json::from_str(&s)
                    .map_err(|e| RefreshTokenError::Cache(format!("deserialize failed: {e}")))?;
                if ticket.expires_at <= chrono::Utc::now().timestamp() {
                    Ok(None)
                } else {
                    Ok(Some(ticket))
                }
            }
            None => Ok(None),
        }
    }
}

// ── 单元测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redis_config_default() {
        let config = RedisConfig::default();
        assert_eq!(config.url, "redis://127.0.0.1:6379");
        assert_eq!(config.key_prefix_ver, "sso:ver");
        assert_eq!(config.key_prefix_bl, "sso:bl");
        assert_eq!(config.connection_timeout, Duration::from_secs(3));
        assert_eq!(config.command_timeout, Duration::from_secs(2));
    }

    #[test]
    fn test_redis_config_from_url() {
        let config = RedisConfig::from_url("redis://localhost:6380/1");
        assert_eq!(config.url, "redis://localhost:6380/1");
        assert_eq!(config.key_prefix_ver, "sso:ver");
    }

    #[test]
    fn test_redis_config_debug_redacts_password() {
        let config = RedisConfig::from_url("redis://:secret_pass@127.0.0.1:6379");
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("[REDACTED]"));
        assert!(!debug_str.contains("secret_pass"));
    }

    #[test]
    fn test_redis_config_debug_no_password() {
        let config = RedisConfig::from_url("redis://127.0.0.1:6379");
        let debug_str = format!("{:?}", config);
        assert!(!debug_str.contains("[REDACTED]"));
        assert!(debug_str.contains("127.0.0.1:6379"));
    }

    #[test]
    fn test_ver_key_format() {
        let config = RedisConfig::default();
        assert_eq!(config.ver_key(1), "sso:ver:1");
        assert_eq!(config.ver_key(42), "sso:ver:42");
    }

    #[test]
    fn test_bl_key_format() {
        let config = RedisConfig::default();
        assert_eq!(config.bl_key("abc123"), "sso:bl:abc123");
    }

    #[test]
    fn test_redact_redis_url_with_password() {
        let redacted = redact_redis_url("redis://:mypassword@host:6379/0");
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("mypassword"));
        assert!(redacted.contains("host:6379"));
    }

    #[test]
    fn test_redact_redis_url_without_password() {
        let redacted = redact_redis_url("redis://127.0.0.1:6379");
        assert_eq!(redacted, "redis://127.0.0.1:6379");
    }

    #[test]
    fn test_redact_redis_url_with_user_and_password() {
        let redacted = redact_redis_url("redis://user:pass@host:6379");
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("pass"));
    }

    // ── P1-4~6 新增测试 ──

    #[test]
    fn test_deg_user_key_format() {
        let config = RedisConfig::default();
        assert_eq!(config.deg_user_key(1), "sso:deg:user:1");
        assert_eq!(config.deg_user_key(42), "sso:deg:user:42");
    }

    #[test]
    fn test_deg_device_key_format() {
        let config = RedisConfig::default();
        assert_eq!(
            config.deg_device_key(1, "dev-abc"),
            "sso:deg:device:1:dev-abc"
        );
    }

    #[test]
    fn test_deg_device_pattern_format() {
        let config = RedisConfig::default();
        assert_eq!(config.deg_device_pattern(1), "sso:deg:device:1:*");
    }

    #[test]
    fn test_audit_user_key_format() {
        let config = RedisConfig::default();
        assert_eq!(config.audit_user_key(1), "sso:audit:user:1");
    }

    #[test]
    fn test_audit_all_key_format() {
        let config = RedisConfig::default();
        assert_eq!(config.audit_all_key(), "sso:audit:all");
    }

    #[test]
    fn test_ticket_key_format() {
        let config = RedisConfig::default();
        assert_eq!(config.ticket_key("abc-123-def"), "sso:ticket:abc-123-def");
    }

    #[test]
    fn test_redis_config_new_prefixes() {
        let config = RedisConfig::default();
        assert_eq!(config.key_prefix_deg, "sso:deg");
        assert_eq!(config.key_prefix_audit, "sso:audit");
        assert_eq!(config.key_prefix_ticket, "sso:ticket");
    }

    #[test]
    fn test_degradation_entry_serialize_deserialize() {
        let entry = crate::refresh::DegradationEntry {
            roles: vec!["viewer".to_string()],
            permissions: vec!["read".to_string()],
            expires_at: 1000000,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: crate::refresh::DegradationEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry.roles, deserialized.roles);
        assert_eq!(entry.permissions, deserialized.permissions);
        assert_eq!(entry.expires_at, deserialized.expires_at);
    }

    #[test]
    fn test_audit_event_serialize_deserialize() {
        let event = crate::refresh::AuditEvent {
            event_id: "test-uuid".to_string(),
            event_type: crate::refresh::AuditEventType::Login,
            user_id: Some(1),
            device_id: Some("dev-1".to_string()),
            timestamp: 1000000,
            ip: Some("127.0.0.1".to_string()),
            detail: Some("test detail".to_string()),
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: crate::refresh::AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event.event_id, deserialized.event_id);
        assert_eq!(event.timestamp, deserialized.timestamp);
    }

    #[test]
    fn test_sso_ticket_serialize_deserialize() {
        let ticket = crate::refresh::SsoTicket {
            ticket: "test-ticket-uuid".to_string(),
            user_id: 1,
            username: "testuser".to_string(),
            redirect_uri: "https://example.com/cb".to_string(),
            roles: vec!["user".to_string()],
            permissions: vec!["read".to_string()],
            created_at: 1000000,
            expires_at: 1000030,
        };
        let json = serde_json::to_string(&ticket).unwrap();
        let deserialized: crate::refresh::SsoTicket = serde_json::from_str(&json).unwrap();
        assert_eq!(ticket.ticket, deserialized.ticket);
        assert_eq!(ticket.expires_at, deserialized.expires_at);
    }
}
