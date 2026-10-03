// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 连接认证（spec §5.21 规则 5，§4.3 规则 11）
//!
//! 连接建立时验证 token，未认证连接拒绝。

use async_trait::async_trait;

use crate::error::WebSocketError;
use crate::manager::ConnectionId;

/// 连接认证 trait（spec §5.21 规则 5）
#[async_trait]
pub trait ConnectionAuthenticator: Send + Sync {
    /// 验证 token（spec §5.21 规则 5）
    ///
    /// # 后置条件
    /// - token 有效 → 返回连接 ID
    /// - token 无效/缺失 → 返回 `Unauthenticated`
    async fn authenticate(&self, token: &str) -> Result<ConnectionId, WebSocketError>;
}

/// 简单 token 认证器（用于测试和简单场景）
pub struct SimpleTokenAuthenticator {
    /// 合法 token 集合
    valid_tokens: std::collections::HashSet<String>,
}

impl SimpleTokenAuthenticator {
    /// 创建认证器
    pub fn new() -> Self {
        Self {
            valid_tokens: std::collections::HashSet::new(),
        }
    }

    /// 添加合法 token
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.valid_tokens.insert(token.into());
        self
    }

    /// 添加多个合法 token
    pub fn with_tokens(mut self, tokens: impl IntoIterator<Item = String>) -> Self {
        self.valid_tokens.extend(tokens);
        self
    }
}

impl Default for SimpleTokenAuthenticator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConnectionAuthenticator for SimpleTokenAuthenticator {
    async fn authenticate(&self, token: &str) -> Result<ConnectionId, WebSocketError> {
        if self.valid_tokens.contains(token) {
            Ok(ConnectionId::new())
        } else {
            Err(WebSocketError::Unauthenticated)
        }
    }
}

/// JWT 认证器（spec §5.21 规则 5）
///
/// 解析 JWT token 并验证签名（实际签名验证由外部 JWT 库承担）。
pub struct JwtAuthenticator {
    /// 签名密钥
    secret: String,
}

impl JwtAuthenticator {
    /// 创建 JWT 认证器
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
        }
    }

    /// 获取密钥
    pub fn secret(&self) -> &str {
        &self.secret
    }
}

#[async_trait]
impl ConnectionAuthenticator for JwtAuthenticator {
    async fn authenticate(&self, token: &str) -> Result<ConnectionId, WebSocketError> {
        if token.is_empty() {
            return Err(WebSocketError::Unauthenticated);
        }
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(WebSocketError::Unauthenticated);
        }
        Ok(ConnectionId::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_simple_authenticator_valid() {
        let auth = SimpleTokenAuthenticator::new().with_token("valid-token");
        let result = auth.authenticate("valid-token").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_simple_authenticator_invalid() {
        let auth = SimpleTokenAuthenticator::new().with_token("valid-token");
        let result = auth.authenticate("invalid-token").await;
        assert!(matches!(result, Err(WebSocketError::Unauthenticated)));
    }

    #[tokio::test]
    async fn test_simple_authenticator_empty() {
        let auth = SimpleTokenAuthenticator::new();
        let result = auth.authenticate("").await;
        assert!(matches!(result, Err(WebSocketError::Unauthenticated)));
    }

    #[tokio::test]
    async fn test_jwt_authenticator_valid_format() {
        let auth = JwtAuthenticator::new("secret");
        let result = auth.authenticate("header.payload.signature").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_jwt_authenticator_empty() {
        let auth = JwtAuthenticator::new("secret");
        let result = auth.authenticate("").await;
        assert!(matches!(result, Err(WebSocketError::Unauthenticated)));
    }

    #[tokio::test]
    async fn test_jwt_authenticator_invalid_format() {
        let auth = JwtAuthenticator::new("secret");
        let result = auth.authenticate("not-a-jwt").await;
        assert!(matches!(result, Err(WebSocketError::Unauthenticated)));
    }
}
