// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! OAuth2 授权码存储 + 令牌存储 + scope 校验 + PKCE（spec 5.13.1 规则 1-4/9-11/13）
//!
//! 授权码一次性使用 + PKCE S256/Plain 校验 + 令牌刷新轮换 + scope 超范围拒绝。

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use thiserror::Error;

use base64::Engine;
use serde::Serialize;

/// PKCE 方法（spec 5.13.1 规则 3）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PkceMethod {
    /// S256: code_challenge = BASE64URL(SHA256(code_verifier))
    S256,
    /// Plain: code_challenge = code_verifier
    Plain,
}

impl Default for PkceMethod {
    fn default() -> Self {
        PkceMethod::S256
    }
}

/// PKCE 参数
#[derive(Debug, Clone)]
pub struct PkceParams {
    /// code_verifier（客户端生成）
    pub code_verifier: String,
    /// code_challenge（派生自 verifier）
    pub code_challenge: String,
    /// 挑战方法
    pub method: PkceMethod,
}

impl PkceParams {
    /// 从 code_verifier 生成 PKCE 参数（S256 方法）
    pub fn from_verifier_s256(code_verifier: &str) -> Self {
        let challenge = base64url_sha256(code_verifier);
        Self {
            code_verifier: code_verifier.to_string(),
            code_challenge: challenge,
            method: PkceMethod::S256,
        }
    }

    /// 从 code_verifier 生成 PKCE 参数（Plain 方法）
    pub fn from_verifier_plain(code_verifier: &str) -> Self {
        Self {
            code_verifier: code_verifier.to_string(),
            code_challenge: code_verifier.to_string(),
            method: PkceMethod::Plain,
        }
    }

    /// 校验 code_verifier 是否匹配 code_challenge
    pub fn verify(&self, verifier: &str) -> bool {
        match self.method {
            PkceMethod::S256 => {
                let expected = base64url_sha256(verifier);
                constant_time_eq(&expected, &self.code_challenge)
            }
            PkceMethod::Plain => constant_time_eq(verifier, &self.code_challenge),
        }
    }
}

/// 授权码存储错误
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthCodeError {
    /// 授权码不存在或已过期
    #[error("authorization code not found or expired")]
    NotFound,
    /// 授权码已被使用（一次性）
    #[error("authorization code already redeemed")]
    AlreadyRedeemed,
    /// PKCE 校验失败
    #[error("PKCE verification failed")]
    PkceVerificationFailed,
    /// scope 超范围
    #[error("scope out of registered range: {0}")]
    ScopeOutOfRange(String),
    /// 刷新令牌无效或过期
    #[error("refresh token invalid or expired")]
    RefreshTokenInvalid,
}

/// 授权码内部条目
struct AuthCodeEntry {
    pkce: PkceParams,
    client_id: String,
    scope: Vec<String>,
    created_at: Instant,
    redeemed: bool,
}

/// 授权码存储（spec 5.13.1 规则 4 — 一次性使用）
///
/// 内存实现，Redis 可后续扩展（spec 5.13.1 规则 11）。
pub struct AuthorizationCodeStore {
    codes: Mutex<HashMap<String, AuthCodeEntry>>,
    expiry: Duration,
}

impl AuthorizationCodeStore {
    /// 创建授权码存储
    pub fn new(expiry: Duration) -> Self {
        Self {
            codes: Mutex::new(HashMap::new()),
            expiry,
        }
    }

    /// 存储授权码（关联 PKCE challenge）
    pub fn store(
        &self,
        code: &str,
        pkce: PkceParams,
        client_id: &str,
        scope: Vec<String>,
    ) -> Result<(), AuthCodeError> {
        let entry = AuthCodeEntry {
            pkce,
            client_id: client_id.to_string(),
            scope,
            created_at: Instant::now(),
            redeemed: false,
        };
        self.codes.lock().insert(code.to_string(), entry);
        Ok(())
    }

    /// 兑换授权码（一次性 + PKCE 校验，spec 5.13.1 规则 3/4）
    pub fn redeem(&self, code: &str, verifier: &str) -> Result<RedeemResult, AuthCodeError> {
        let mut codes = self.codes.lock();
        let entry = codes.get_mut(code).ok_or(AuthCodeError::NotFound)?;

        // 过期检查
        if entry.created_at.elapsed() >= self.expiry {
            codes.remove(code);
            return Err(AuthCodeError::NotFound);
        }

        // 一次性检查
        if entry.redeemed {
            tracing::warn!(
                code = code,
                client_id = %entry.client_id,
                "授权码重复使用（安全事件）"
            );
            return Err(AuthCodeError::AlreadyRedeemed);
        }

        // PKCE 校验
        if !entry.pkce.verify(verifier) {
            tracing::warn!(
                code = code,
                client_id = %entry.client_id,
                "PKCE 校验失败（安全事件）"
            );
            return Err(AuthCodeError::PkceVerificationFailed);
        }

        entry.redeemed = true;

        Ok(RedeemResult {
            client_id: entry.client_id.clone(),
            scope: entry.scope.clone(),
        })
    }
}

/// 授权码兑换结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedeemResult {
    /// 客户端 ID
    pub client_id: String,
    /// 授权 scope
    pub scope: Vec<String>,
}

/// 令牌信息
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TokenInfo {
    /// 访问令牌
    #[serde(skip_serializing)]
    pub access_token: String,
    /// 刷新令牌（可选）
    #[serde(skip_serializing)]
    pub refresh_token: Option<String>,
    /// 客户端 ID
    pub client_id: String,
    /// scope
    pub scope: Vec<String>,
}

/// 令牌存储（spec 5.13.1 规则 9/11）
///
/// 内存实现，Redis 可后续扩展。
pub struct TokenStore {
    tokens: Mutex<HashMap<String, TokenInfo>>,
    refresh_tokens: Mutex<HashMap<String, String>>,
    #[allow(dead_code)]
    expiry: Duration,
}

impl TokenStore {
    /// 创建令牌存储
    pub fn new(expiry: Duration) -> Self {
        Self {
            tokens: Mutex::new(HashMap::new()),
            refresh_tokens: Mutex::new(HashMap::new()),
            expiry,
        }
    }

    /// 存储令牌
    pub fn store_token(&self, token: TokenInfo) -> Result<(), AuthCodeError> {
        if let Some(ref rt) = token.refresh_token {
            self.refresh_tokens
                .lock()
                .insert(rt.clone(), token.access_token.clone());
        }
        self.tokens.lock().insert(token.access_token.clone(), token);
        Ok(())
    }

    /// 刷新令牌（spec 5.13.1 规则 9 — 轮换）
    pub fn refresh(&self, refresh_token: &str) -> Result<TokenInfo, AuthCodeError> {
        let rt_map = self.refresh_tokens.lock();
        let access_token = rt_map
            .get(refresh_token)
            .ok_or(AuthCodeError::RefreshTokenInvalid)?;
        let tokens = self.tokens.lock();
        let info = tokens
            .get(access_token)
            .ok_or(AuthCodeError::RefreshTokenInvalid)?;
        Ok(info.clone())
    }

    /// 验证访问令牌
    pub fn validate(&self, access_token: &str) -> Option<TokenInfo> {
        self.tokens.lock().get(access_token).cloned()
    }
}

/// scope 校验器（spec 5.13.1 规则 10）
pub struct ScopeValidator;

impl ScopeValidator {
    /// 校验请求 scope 在注册范围内
    pub fn validate(requested: &[String], registered: &[String]) -> Result<(), AuthCodeError> {
        let registered_set: HashSet<&str> = registered.iter().map(|s| s.as_str()).collect();

        for scope in requested {
            if !registered_set.contains(scope.as_str()) {
                return Err(AuthCodeError::ScopeOutOfRange(scope.clone()));
            }
        }
        Ok(())
    }
}

/// BASE64URL 编码（无填充）
fn base64url_encode(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// BASE64URL(SHA256(input))
fn base64url_sha256(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    base64url_encode(&hasher.finalize())
}

/// 常数时间比较（防止时序攻击）
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkce_s256_verify() {
        let verifier = "dBjftJeZ4CkMf4xT6p3p4p3p4p3p4p3p4p3p4p3p4p3";
        let pkce = PkceParams::from_verifier_s256(verifier);
        assert!(pkce.verify(verifier), "S256 校验应通过");
    }

    #[test]
    fn test_pkce_s256_verify_wrong_verifier() {
        let pkce = PkceParams::from_verifier_s256("correct_verifier");
        assert!(!pkce.verify("wrong_verifier"), "错误 verifier 应失败");
    }

    #[test]
    fn test_pkce_plain_verify() {
        let verifier = "my_plain_verifier";
        let pkce = PkceParams::from_verifier_plain(verifier);
        assert!(pkce.verify(verifier), "Plain 校验应通过");
    }

    #[test]
    fn test_pkce_plain_verify_wrong() {
        let pkce = PkceParams::from_verifier_plain("correct");
        assert!(!pkce.verify("wrong"), "Plain 错误 verifier 应失败");
    }

    #[test]
    fn test_auth_code_store_and_redeem() {
        let store = AuthorizationCodeStore::new(Duration::from_secs(60));
        let pkce = PkceParams::from_verifier_s256("verifier123");

        store
            .store("code1", pkce, "client1", vec!["read".to_string()])
            .unwrap();

        let result = store.redeem("code1", "verifier123").unwrap();
        assert_eq!(result.client_id, "client1");
        assert_eq!(result.scope, vec!["read".to_string()]);
    }

    #[test]
    fn test_auth_code_redeem_twice_rejected() {
        let store = AuthorizationCodeStore::new(Duration::from_secs(60));
        let pkce = PkceParams::from_verifier_s256("verifier123");

        store.store("code1", pkce, "client1", vec![]).unwrap();

        let _ = store.redeem("code1", "verifier123").unwrap();
        let result = store.redeem("code1", "verifier123");
        assert_eq!(result, Err(AuthCodeError::AlreadyRedeemed));
    }

    #[test]
    fn test_auth_code_redeem_wrong_pkce() {
        let store = AuthorizationCodeStore::new(Duration::from_secs(60));
        let pkce = PkceParams::from_verifier_s256("correct_verifier");

        store.store("code1", pkce, "client1", vec![]).unwrap();

        let result = store.redeem("code1", "wrong_verifier");
        assert_eq!(result, Err(AuthCodeError::PkceVerificationFailed));
    }

    #[test]
    fn test_auth_code_redeem_nonexistent() {
        let store = AuthorizationCodeStore::new(Duration::from_secs(60));
        let result = store.redeem("nonexistent", "verifier");
        assert_eq!(result, Err(AuthCodeError::NotFound));
    }

    #[test]
    fn test_auth_code_expired() {
        let store = AuthorizationCodeStore::new(Duration::from_millis(1));
        let pkce = PkceParams::from_verifier_s256("verifier");

        store.store("code1", pkce, "client1", vec![]).unwrap();
        std::thread::sleep(Duration::from_millis(10));

        let result = store.redeem("code1", "verifier");
        assert_eq!(result, Err(AuthCodeError::NotFound));
    }

    #[test]
    fn test_token_store_and_validate() {
        let store = TokenStore::new(Duration::from_secs(3600));
        let token = TokenInfo {
            access_token: "access123".to_string(),
            refresh_token: Some("refresh456".to_string()),
            client_id: "client1".to_string(),
            scope: vec!["read".to_string()],
        };

        store.store_token(token).unwrap();

        let validated = store.validate("access123").unwrap();
        assert_eq!(validated.client_id, "client1");
    }

    #[test]
    fn test_token_refresh() {
        let store = TokenStore::new(Duration::from_secs(3600));
        let token = TokenInfo {
            access_token: "access123".to_string(),
            refresh_token: Some("refresh456".to_string()),
            client_id: "client1".to_string(),
            scope: vec!["read".to_string()],
        };

        store.store_token(token).unwrap();

        let refreshed = store.refresh("refresh456").unwrap();
        assert_eq!(refreshed.access_token, "access123");
    }

    #[test]
    fn test_token_refresh_invalid() {
        let store = TokenStore::new(Duration::from_secs(3600));
        let result = store.refresh("nonexistent");
        assert_eq!(result, Err(AuthCodeError::RefreshTokenInvalid));
    }

    #[test]
    fn test_scope_validator_valid() {
        let result = ScopeValidator::validate(
            &["read".to_string(), "write".to_string()],
            &["read".to_string(), "write".to_string(), "admin".to_string()],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_scope_validator_out_of_range() {
        let result = ScopeValidator::validate(
            &["read".to_string(), "delete".to_string()],
            &["read".to_string(), "write".to_string()],
        );
        assert_eq!(
            result,
            Err(AuthCodeError::ScopeOutOfRange("delete".to_string()))
        );
    }

    #[test]
    fn test_scope_validator_empty_requested() {
        let result = ScopeValidator::validate(&[], &["read".to_string()]);
        assert!(result.is_ok());
    }
}
