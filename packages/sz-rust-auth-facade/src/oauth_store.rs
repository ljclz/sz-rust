// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! OAuth2 Token 存储模块 — 需启用 `redis-store` feature
//!
//! 提供 [`OAuth2TokenStore`] trait 和 [`RedisOAuth2TokenStore`] 实现，
//! 用于持久化 OAuth2 token 响应，支持跨请求 token 共享与自动刷新。

use crate::oauth::{OAuth2AuditEvent, OAuth2AuditLogger, OAuth2Error, TokenResponse};
use async_trait::async_trait;
use redis::AsyncCommands;
use std::sync::Arc;

// ============================================================================
// OAuth2TokenStore trait
// ============================================================================

/// OAuth2 token 存储 trait — 异步持久化 token 响应
///
/// 实现者保证 `Send + Sync`，存储操作为 best-effort（失败不阻塞 token 发放）。
#[async_trait]
pub trait OAuth2TokenStore: Send + Sync {
    /// 存储 token 响应（key = client_id，TTL = expires_in）
    ///
    /// 使用 SETNX 原子操作防止并发刷新竞态。
    async fn store_token(&self, client_id: &str, token: &TokenResponse) -> Result<(), OAuth2Error>;

    /// 获取 token 响应（key = client_id）
    async fn get_token(&self, client_id: &str) -> Result<Option<TokenResponse>, OAuth2Error>;

    /// 删除 token 响应（key = client_id）
    async fn delete_token(&self, client_id: &str) -> Result<(), OAuth2Error>;
}

// ============================================================================
// MemoryOAuth2TokenStore — 测试用内存实现
// ============================================================================

use parking_lot::Mutex;
use std::collections::HashMap;

/// 内存 token 存储 — 用于测试和开发环境
#[derive(Default)]
pub struct MemoryOAuth2TokenStore {
    tokens: Mutex<HashMap<String, TokenResponse>>,
}

impl MemoryOAuth2TokenStore {
    /// 创建新的内存 token 存储
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl OAuth2TokenStore for MemoryOAuth2TokenStore {
    async fn store_token(&self, client_id: &str, token: &TokenResponse) -> Result<(), OAuth2Error> {
        self.tokens
            .lock()
            .insert(client_id.to_string(), token.clone());
        Ok(())
    }

    async fn get_token(&self, client_id: &str) -> Result<Option<TokenResponse>, OAuth2Error> {
        Ok(self.tokens.lock().get(client_id).cloned())
    }

    async fn delete_token(&self, client_id: &str) -> Result<(), OAuth2Error> {
        self.tokens.lock().remove(client_id);
        Ok(())
    }
}

// ============================================================================
// RedisOAuth2TokenStore — Redis 持久化实现
// ============================================================================

/// Redis token 存储 — 生产环境使用
///
/// key 格式：`oauth2:token:{client_id}`，TTL = `expires_in` 秒。
pub struct RedisOAuth2TokenStore {
    /// Redis 客户端
    client: redis::Client,
    /// 审计日志（可选）
    audit_logger: Option<Arc<dyn OAuth2AuditLogger>>,
}

impl RedisOAuth2TokenStore {
    /// 创建 Redis token 存储
    ///
    /// # 参数
    ///
    /// - `client`: Redis 客户端（`redis::Client`）
    pub fn new(client: redis::Client) -> Self {
        Self {
            client,
            audit_logger: None,
        }
    }

    /// 注入审计日志
    pub fn with_audit_logger(mut self, logger: Arc<dyn OAuth2AuditLogger>) -> Self {
        self.audit_logger = Some(logger);
        self
    }

    /// 生成 Redis key
    fn key(client_id: &str) -> String {
        format!("oauth2:token:{client_id}")
    }

    /// 记录审计事件（best-effort）
    fn log_audit(
        &self,
        grant_type: &str,
        result: &str,
        alert_code: Option<&str>,
        message: Option<&str>,
    ) {
        if let Some(logger) = &self.audit_logger {
            let event = OAuth2AuditEvent {
                client_id: String::new(),
                grant_type: grant_type.to_string(),
                result: result.to_string(),
                timestamp: chrono::Utc::now().timestamp(),
                alert_code: alert_code.map(|s| s.to_string()),
                message: message.map(|s| s.to_string()),
            };
            logger.log_event(&event);
        }
    }
}

#[async_trait]
impl OAuth2TokenStore for RedisOAuth2TokenStore {
    async fn store_token(&self, client_id: &str, token: &TokenResponse) -> Result<(), OAuth2Error> {
        let key = Self::key(client_id);
        let value =
            serde_json::to_string(token).map_err(|err| OAuth2Error::Serialize(err.to_string()))?;

        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|err| {
                self.log_audit(
                    "token_store",
                    "failure",
                    Some("OAUTH2_TOKEN_STORE_FAILED"),
                    Some(&err.to_string()),
                );
                OAuth2Error::HttpTransport(err.to_string())
            })?;

        // SETNX 原子操作：仅当 key 不存在时设置（防止并发刷新竞态）
        let ttl = token.expires_in.unwrap_or(3600).max(1) as u64;
        let result: redis::RedisResult<()> = redis::pipe()
            .atomic()
            .cmd("SET")
            .arg(&key)
            .arg(&value)
            .arg("NX")
            .arg("EX")
            .arg(ttl)
            .query_async(&mut conn)
            .await;

        match result {
            Ok(()) => {
                self.log_audit("token_store", "success", None, None);
                Ok(())
            }
            Err(err) => {
                self.log_audit(
                    "token_store",
                    "failure",
                    Some("OAUTH2_TOKEN_STORE_FAILED"),
                    Some(&err.to_string()),
                );
                Err(OAuth2Error::HttpTransport(err.to_string()))
            }
        }
    }

    async fn get_token(&self, client_id: &str) -> Result<Option<TokenResponse>, OAuth2Error> {
        let key = Self::key(client_id);
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|err| OAuth2Error::HttpTransport(err.to_string()))?;

        let value: Option<String> = conn
            .get(&key)
            .await
            .map_err(|err| OAuth2Error::HttpTransport(err.to_string()))?;

        match value {
            Some(s) => {
                let token: TokenResponse = serde_json::from_str(&s)
                    .map_err(|err| OAuth2Error::Serialize(err.to_string()))?;
                Ok(Some(token))
            }
            None => Ok(None),
        }
    }

    async fn delete_token(&self, client_id: &str) -> Result<(), OAuth2Error> {
        let key = Self::key(client_id);
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|err| OAuth2Error::HttpTransport(err.to_string()))?;

        let _: () = conn
            .del(&key)
            .await
            .map_err(|err| OAuth2Error::HttpTransport(err.to_string()))?;
        Ok(())
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试 MemoryOAuth2TokenStore store + get
    #[tokio::test]
    async fn test_memory_token_store_set_get() {
        let store = MemoryOAuth2TokenStore::new();
        let token = TokenResponse {
            access_token: "token123".into(),
            token_type: Some("Bearer".into()),
            expires_in: Some(3600),
            scope: Some("read".into()),
            refresh_token: Some("refresh456".into()),
        };

        store
            .store_token("client1", &token)
            .await
            .expect("store_token 失败");

        let retrieved = store
            .get_token("client1")
            .await
            .expect("get_token 失败")
            .expect("应查到 token");

        assert_eq!(retrieved.access_token, "token123");
        assert_eq!(retrieved.token_type.as_deref(), Some("Bearer"));
        assert_eq!(retrieved.expires_in, Some(3600));
    }

    /// 测试 MemoryOAuth2TokenStore delete
    #[tokio::test]
    async fn test_memory_token_store_delete() {
        let store = MemoryOAuth2TokenStore::new();
        let token = TokenResponse {
            access_token: "token123".into(),
            token_type: None,
            expires_in: Some(3600),
            scope: None,
            refresh_token: None,
        };

        store
            .store_token("client1", &token)
            .await
            .expect("store_token 失败");
        store
            .delete_token("client1")
            .await
            .expect("delete_token 失败");

        let result = store.get_token("client1").await.expect("get_token 失败");
        assert!(result.is_none(), "删除后应查不到 token");
    }

    /// 测试 MemoryOAuth2TokenStore get 不存在的 key
    #[tokio::test]
    async fn test_memory_token_store_get_nonexistent() {
        let store = MemoryOAuth2TokenStore::new();
        let result = store
            .get_token("nonexistent")
            .await
            .expect("get_token 失败");
        assert!(result.is_none());
    }
}
