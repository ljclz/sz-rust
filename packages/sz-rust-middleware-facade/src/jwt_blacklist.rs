// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! @REVIEW_REQUIRED（铁律 R12）：人类必须审查此文件
//!
//! 审查要点：
//! - 黑名单 TTL 与 JWT 过期时间的一致性（防止已注销 token 过期前被复用）
//! - 并发添加/查询黑名单的线程安全性
//! - 内存泄漏风险（黑名单无限增长）
//!
//! 审查者签名：__________  日期：__________  结论：__________
//!
//! JWT 黑名单 / 注销列表
//!
//! 对齐 PHP `addons\BaseController::initialize` 中的注销逻辑：
//! ```php
//! if (Cache::store('delete_token')->get($token)) {
//!     throw new BaseException(['msg' => 'token 已注销']);
//! }
//! ```
//!
//! ## 设计
//!
//! 基于 [`Cache`] 层存储已注销的 JWT，key 为 `jwt:blacklist:<sha256(token)>`。
//! - 注销时写入 cache，TTL = JWT 剩余有效期（避免无限增长）
//! - 校验时先查 cache，命中则拒绝

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use sz_rust_cache_facade::{Cache, MemoryCacheDriver};

/// JWT 黑名单默认 cache key 前缀
pub const DEFAULT_KEY_PREFIX: &str = "jwt:blacklist:";

/// JWT 黑名单配置
#[derive(Debug, Clone)]
pub struct JwtBlacklistConfig {
    /// Cache key 前缀
    pub key_prefix: String,
    /// 默认 TTL（None = 永久存储，不推荐）
    pub default_ttl: Option<Duration>,
}

impl Default for JwtBlacklistConfig {
    fn default() -> Self {
        Self {
            key_prefix: DEFAULT_KEY_PREFIX.to_string(),
            default_ttl: Some(Duration::from_secs(3600 * 24 * 30)),
        }
    }
}

impl JwtBlacklistConfig {
    /// 设置 cache key 前缀
    pub fn with_key_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.key_prefix = prefix.into();
        self
    }

    /// 设置默认 TTL
    pub fn with_default_ttl(mut self, ttl: Option<Duration>) -> Self {
        self.default_ttl = ttl;
        self
    }
}

/// JWT 黑名单错误类型
#[derive(Debug, thiserror::Error)]
pub enum JwtBlacklistError {
    /// Cache 操作失败
    #[error("cache error: {0}")]
    Cache(String),
}

/// JWT 黑名单 / 注销列表
#[derive(Clone)]
pub struct JwtBlacklist {
    cache: Arc<Cache>,
    config: JwtBlacklistConfig,
    lock: Arc<Mutex<()>>,
}

impl JwtBlacklist {
    /// 使用指定 Cache 实例和配置创建黑名单
    pub fn new(cache: Arc<Cache>, config: JwtBlacklistConfig) -> Self {
        Self {
            cache,
            config,
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// 使用独立内存 Cache 和默认配置创建黑名单
    pub fn with_default_cache(config: JwtBlacklistConfig) -> Self {
        let cache = Arc::new(Cache::new());
        cache.register_default(MemoryCacheDriver::new());
        Self::new(cache, config)
    }

    /// 使用默认配置创建黑名单
    pub fn default_with_memory_cache() -> Self {
        Self::with_default_cache(JwtBlacklistConfig::default())
    }

    /// 获取配置引用
    pub fn config(&self) -> &JwtBlacklistConfig {
        &self.config
    }

    /// 注销 JWT（加入黑名单）
    pub fn revoke(&self, token: &str, ttl: Option<Duration>) -> Result<bool, JwtBlacklistError> {
        let _guard = self.lock.lock();
        let cache_key = self.make_cache_key(token);
        if self
            .cache
            .has(&cache_key)
            .map_err(|e| JwtBlacklistError::Cache(e.to_string()))?
        {
            return Ok(false);
        }
        let effective_ttl = ttl.or(self.config.default_ttl);
        self.cache
            .set(&cache_key, true, effective_ttl)
            .map_err(|e| JwtBlacklistError::Cache(e.to_string()))?;
        Ok(true)
    }

    /// 检查 JWT 是否已注销（在黑名单中）
    pub fn is_revoked(&self, token: &str) -> Result<bool, JwtBlacklistError> {
        let cache_key = self.make_cache_key(token);
        self.cache
            .has(&cache_key)
            .map_err(|e| JwtBlacklistError::Cache(e.to_string()))
    }

    /// 从黑名单中移除 JWT（取消注销）
    pub fn unrevoke(&self, token: &str) -> Result<bool, JwtBlacklistError> {
        let cache_key = self.make_cache_key(token);
        let existed = self
            .cache
            .has(&cache_key)
            .map_err(|e| JwtBlacklistError::Cache(e.to_string()))?;
        self.cache
            .delete(&cache_key)
            .map_err(|e| JwtBlacklistError::Cache(e.to_string()))?;
        Ok(existed)
    }

    /// 清空所有黑名单条目
    pub fn clear(&self) -> Result<(), JwtBlacklistError> {
        self.cache
            .clear()
            .map_err(|e| JwtBlacklistError::Cache(e.to_string()))
    }

    /// 生成 cache key：`<prefix><sha256(token)>`
    fn make_cache_key(&self, token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let hash = hasher.finalize();
        let hash_hex: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
        format!("{}{}", self.config.key_prefix, hash_hex)
    }
}

impl Default for JwtBlacklist {
    fn default() -> Self {
        Self::default_with_memory_cache()
    }
}

impl std::fmt::Debug for JwtBlacklist {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtBlacklist")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_revoke_and_is_revoked() {
        let blacklist = JwtBlacklist::default();
        let token = "eyJhbGciOiJIUzI1NiJ9.test.payload";
        assert!(!blacklist.is_revoked(token).unwrap());
        let revoked = blacklist.revoke(token, None).unwrap();
        assert!(revoked);
        assert!(blacklist.is_revoked(token).unwrap());
    }

    #[test]
    fn test_revoke_duplicate_returns_false() {
        let blacklist = JwtBlacklist::default();
        let token = "duplicate_token";
        assert!(blacklist.revoke(token, None).unwrap());
        assert!(!blacklist.revoke(token, None).unwrap());
    }

    #[test]
    fn test_unrevoke() {
        let blacklist = JwtBlacklist::default();
        let token = "to_be_unrevoked";
        blacklist.revoke(token, None).unwrap();
        assert!(blacklist.is_revoked(token).unwrap());
        let removed = blacklist.unrevoke(token).unwrap();
        assert!(removed);
        assert!(!blacklist.is_revoked(token).unwrap());
        let removed_again = blacklist.unrevoke(token).unwrap();
        assert!(!removed_again);
    }

    #[test]
    fn test_clear() {
        let blacklist = JwtBlacklist::default();
        blacklist.revoke("token1", None).unwrap();
        blacklist.revoke("token2", None).unwrap();
        assert!(blacklist.is_revoked("token1").unwrap());
        assert!(blacklist.is_revoked("token2").unwrap());
        blacklist.clear().unwrap();
        assert!(!blacklist.is_revoked("token1").unwrap());
        assert!(!blacklist.is_revoked("token2").unwrap());
    }

    #[test]
    fn test_ttl_expiry() {
        let blacklist = JwtBlacklist::default();
        let token = "short_lived_token";
        blacklist
            .revoke(token, Some(Duration::from_secs(1)))
            .unwrap();
        assert!(blacklist.is_revoked(token).unwrap());
        std::thread::sleep(Duration::from_millis(1100));
        assert!(!blacklist.is_revoked(token).unwrap());
    }

    #[test]
    fn test_different_tokens_independent() {
        let blacklist = JwtBlacklist::default();
        blacklist.revoke("token_one", None).unwrap();
        assert!(blacklist.is_revoked("token_one").unwrap());
        assert!(!blacklist.is_revoked("token_two").unwrap());
    }

    #[test]
    fn test_config_builder() {
        let config = JwtBlacklistConfig::default()
            .with_key_prefix("custom:bl:")
            .with_default_ttl(Some(Duration::from_secs(3600)));
        assert_eq!(config.key_prefix, "custom:bl:");
        assert_eq!(config.default_ttl, Some(Duration::from_secs(3600)));
    }

    #[test]
    fn test_cache_key_is_hash_not_plain_token() {
        let blacklist = JwtBlacklist::default();
        let token = "secret_jwt_token";
        let key1 = blacklist.make_cache_key(token);
        let key2 = blacklist.make_cache_key(token);
        assert_eq!(key1, key2);
        assert!(!key1.contains(token));
        assert!(key1.starts_with(DEFAULT_KEY_PREFIX));
        assert_eq!(key1.len(), DEFAULT_KEY_PREFIX.len() + 64);
    }

    #[test]
    fn test_with_custom_cache() {
        let cache = Arc::new(Cache::new());
        cache.register_default(MemoryCacheDriver::new());
        let blacklist1 = JwtBlacklist::new(cache.clone(), JwtBlacklistConfig::default());
        let blacklist2 = JwtBlacklist::new(cache, JwtBlacklistConfig::default());
        blacklist1.revoke("shared_token", None).unwrap();
        assert!(blacklist2.is_revoked("shared_token").unwrap());
    }
}
