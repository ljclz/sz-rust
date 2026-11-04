// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! API 签名验证中间件（spec 5.12）
//!
//! HMAC 签名验证 + nonce 防重放 + 多密钥轮换。

#![forbid(unsafe_code)]

pub mod key_manager;
pub mod nonce_store;

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sha2::{Digest, Sha256};

/// 签名配置
#[derive(Debug, Clone)]
pub struct SignatureConfig {
    /// 时间窗口（默认 300s，spec 6.9.3）
    pub time_window: Duration,
    /// 豁免路径
    pub exempt_paths: Vec<String>,
    /// API 密钥集
    pub keys: Vec<ApiKey>,
}

impl Default for SignatureConfig {
    fn default() -> Self {
        Self {
            time_window: Duration::from_secs(300),
            exempt_paths: Vec::new(),
            keys: Vec::new(),
        }
    }
}

/// API 密钥（spec 5.12.6 + 6.9.2）
#[derive(Clone, Serialize)]
pub struct ApiKey {
    /// 密钥标识
    pub key_id: String,
    /// 密钥内容（不明文记录日志）
    #[serde(skip_serializing)]
    pub secret: String,
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 密钥属于敏感凭据：Debug 输出一律脱敏，禁止进入日志
        f.debug_struct("ApiKey")
            .field("key_id", &self.key_id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl ApiKey {
    /// 创建密钥
    pub fn new(key_id: &str, secret: &str) -> Self {
        Self {
            key_id: key_id.to_string(),
            secret: secret.to_string(),
        }
    }
}

/// 签名验证结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyResult {
    /// 验证通过
    Ok,
    /// 签名不一致
    InvalidSignature,
    /// 缺少签名内容
    MissingSignature,
    /// 时间戳超窗口
    TimestampExpired,
    /// 重复 nonce
    NonceReplayed,
    /// 密钥不存在
    KeyNotFound,
    /// 豁免路径
    Exempt,
}

/// 签名验证器
pub struct SignatureVerifier {
    config: SignatureConfig,
    nonce_store: nonce_store::NonceStore,
    key_manager: key_manager::KeyManager,
}

impl SignatureVerifier {
    /// 创建验证器
    pub fn new(config: SignatureConfig) -> Self {
        let keys = config.keys.clone();
        Self {
            nonce_store: nonce_store::NonceStore::new(config.time_window),
            key_manager: key_manager::KeyManager::new(keys),
            config,
        }
    }

    /// 验证签名
    ///
    /// 流程（spec 5.12.2）：
    /// 1. 检查豁免路径
    /// 2. 提取签名参数
    /// 3. 查找密钥
    /// 4. 计算期望签名
    /// 5. 比对签名
    /// 6. 时间窗口校验
    /// 7. nonce 防重放
    // 参数数量为签名协议领域固有（方法/路径/时间戳/nonce/签名/正文），保持扁平签名以支持无堆分配调用
    #[allow(clippy::too_many_arguments)]
    pub fn verify(
        &mut self,
        method: &str,
        path: &str,
        key_id: &str,
        timestamp: u64,
        nonce: &str,
        signature: &str,
        body: &[u8],
    ) -> VerifyResult {
        // 豁免路径
        if self.config.exempt_paths.iter().any(|p| path.starts_with(p)) {
            return VerifyResult::Exempt;
        }

        // 查找密钥
        let key = match self.key_manager.find_key(key_id) {
            Some(k) => k,
            None => return VerifyResult::KeyNotFound,
        };

        // 时间窗口校验
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now.abs_diff(timestamp) > self.config.time_window.as_secs() {
            return VerifyResult::TimestampExpired;
        }

        // 计算期望签名
        let expected = compute_signature(method, path, timestamp, nonce, body, &key.secret);
        if expected != signature {
            return VerifyResult::InvalidSignature;
        }

        // nonce 防重放
        if !self.nonce_store.check_and_store(nonce) {
            return VerifyResult::NonceReplayed;
        }

        VerifyResult::Ok
    }

    /// 获取配置
    pub fn config(&self) -> &SignatureConfig {
        &self.config
    }
}

/// 计算 HMAC-SHA256 签名
///
/// 签名内容：method + path + timestamp + nonce + body_digest（spec 5.12.2）
pub fn compute_signature(
    method: &str,
    path: &str,
    timestamp: u64,
    nonce: &str,
    body: &[u8],
    secret: &str,
) -> String {
    let body_digest = {
        let mut hasher = Sha256::new();
        hasher.update(body);
        hex::encode(hasher.finalize())
    };

    let message = format!("{}{}{}{}{}", method, path, timestamp, nonce, body_digest);

    let mut hasher = Sha256::new();
    hasher.update(message.as_bytes());
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> SignatureConfig {
        SignatureConfig {
            time_window: Duration::from_secs(300),
            exempt_paths: vec!["/health".to_string()],
            keys: vec![ApiKey::new("key1", "secret1")],
        }
    }

    fn now_ts() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn test_verify_valid_signature() {
        let mut verifier = SignatureVerifier::new(make_config());
        let ts = now_ts();
        let sig = compute_signature("GET", "/api/data", ts, "nonce1", b"", "secret1");

        let result = verifier.verify("GET", "/api/data", "key1", ts, "nonce1", &sig, b"");
        assert_eq!(result, VerifyResult::Ok);
    }

    #[test]
    fn test_verify_invalid_signature() {
        let mut verifier = SignatureVerifier::new(make_config());
        let ts = now_ts();

        let result = verifier.verify("GET", "/api/data", "key1", ts, "nonce1", "invalid_sig", b"");
        assert_eq!(result, VerifyResult::InvalidSignature);
    }

    #[test]
    fn test_verify_key_not_found() {
        let mut verifier = SignatureVerifier::new(make_config());
        let ts = now_ts();

        let result = verifier.verify("GET", "/api/data", "unknown_key", ts, "nonce1", "sig", b"");
        assert_eq!(result, VerifyResult::KeyNotFound);
    }

    #[test]
    fn test_verify_exempt_path() {
        let mut verifier = SignatureVerifier::new(make_config());
        let ts = now_ts();

        let result = verifier.verify("GET", "/health", "key1", ts, "nonce1", "sig", b"");
        assert_eq!(result, VerifyResult::Exempt);
    }

    #[test]
    fn test_verify_timestamp_expired() {
        let mut verifier = SignatureVerifier::new(make_config());
        let old_ts = now_ts() - 600; // 10 分钟前
        let sig = compute_signature("GET", "/api/data", old_ts, "nonce1", b"", "secret1");

        let result = verifier.verify("GET", "/api/data", "key1", old_ts, "nonce1", &sig, b"");
        assert_eq!(result, VerifyResult::TimestampExpired);
    }

    #[test]
    fn test_verify_nonce_replay() {
        let mut verifier = SignatureVerifier::new(make_config());
        let ts = now_ts();
        let sig = compute_signature("GET", "/api/data", ts, "nonce1", b"", "secret1");

        // 第一次验证通过
        let r1 = verifier.verify("GET", "/api/data", "key1", ts, "nonce1", &sig, b"");
        assert_eq!(r1, VerifyResult::Ok);

        // 第二次相同 nonce 应拒绝
        let r2 = verifier.verify("GET", "/api/data", "key1", ts, "nonce1", &sig, b"");
        assert_eq!(r2, VerifyResult::NonceReplayed);
    }

    #[test]
    fn test_api_key_debug_redacts_secret() {
        let key = ApiKey::new("key1", "super_secret_value");

        let debug = format!("{key:?}");
        assert!(
            !debug.contains("super_secret_value"),
            "Debug 输出不得包含密钥明文"
        );
        assert!(debug.contains("key1"));
        assert!(debug.contains("<redacted>"));
    }
}
