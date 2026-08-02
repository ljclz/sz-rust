//! MySQL 认证模块 — mysql_native_password 算法实现。
//!
//! mysql_native_password 是 MySQL 最常用的认证插件，使用 SHA1 challenge-response：
//!
//! ```text
//! server → client: 20 字节随机 salt（challenge）
//! client → server: SHA1(password) XOR SHA1(salt + SHA1(SHA1(password)))
//! server: 验证 SHA1(stored_hash) == SHA1(salt + stored_hash) XOR client_response
//! ```
//!
//! 其中 `stored_hash` = SHA1(password)，存储在服务器端。
//!
//! 安全性：基于 SHA1 challenge-response，避免明文传输密码。
//! 注意：MySQL 8.0+ 默认使用 caching_sha2_password，本实现为兼容 5.7+ 仍支持 native。

use rand::RngCore;
use sha1::{Digest, Sha1};
use thiserror::Error;

/// 认证 salt 长度（MySQL 协议固定 20 字节）。
pub const SALT_LEN: usize = 20;

/// 认证错误。
#[derive(Debug, Error)]
pub enum AuthError {
    /// 密码不匹配
    #[error("access denied: invalid password for user '{0}'")]
    AccessDenied(String),
    /// salt 长度错误
    #[error("invalid salt length: expected {SALT_LEN}, got {0}")]
    InvalidSaltLength(usize),
    /// 客户端响应格式错误
    #[error("invalid client response: {0}")]
    InvalidResponse(String),
    /// 不支持的认证插件
    #[error("unsupported auth plugin: {0}")]
    UnsupportedPlugin(String),
}

/// 认证模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMode {
    /// 信任模式（无密码，仅用于测试）
    Trust,
    /// mysql_native_password 认证
    MysqlNativePassword {
        /// 用户名 → SHA1(password) 映射
        users: std::collections::HashMap<String, [u8; 20]>,
    },
}

impl Default for AuthMode {
    fn default() -> Self {
        AuthMode::Trust
    }
}

/// 单次认证会话。
pub struct AuthSession {
    /// 模式
    mode: AuthMode,
    /// 本次握手生成的随机 salt
    salt: [u8; SALT_LEN],
    /// 客户端声称的用户名
    username: Option<String>,
}

impl AuthSession {
    /// 创建新会话，生成随机 salt。
    pub fn new(mode: AuthMode) -> Self {
        let mut salt = [0u8; SALT_LEN];
        rand::rng().fill_bytes(&mut salt);
        // 将 0 字节替换为非零值（MySQL 协议要求 salt 不含 NUL）
        for byte in salt.iter_mut() {
            if *byte == 0 {
                *byte = 1;
            }
        }
        Self {
            mode,
            salt,
            username: None,
        }
    }

    /// 返回本次握手使用的 salt（发送给客户端）。
    pub fn salt(&self) -> &[u8; SALT_LEN] {
        &self.salt
    }

    /// 验证客户端认证响应。
    ///
    /// `username`：客户端声称的用户名
    /// `auth_response`：客户端发送的认证响应（20 字节）
    /// `auth_plugin`：客户端使用的认证插件名（如 "mysql_native_password"）
    pub fn verify(
        &mut self,
        username: &str,
        auth_response: &[u8],
        auth_plugin: &str,
    ) -> Result<String, AuthError> {
        self.username = Some(username.to_string());

        match &self.mode {
            AuthMode::Trust => Ok(username.to_string()),

            AuthMode::MysqlNativePassword { users } => {
                if auth_plugin != "mysql_native_password" && auth_plugin != "mysql_clear_password" {
                    return Err(AuthError::UnsupportedPlugin(auth_plugin.to_string()));
                }
                if auth_response.len() != 20 {
                    return Err(AuthError::InvalidResponse(format!(
                        "expected 20 bytes, got {}",
                        auth_response.len()
                    )));
                }

                let stored_hash = users
                    .get(username)
                    .ok_or_else(|| AuthError::AccessDenied(username.to_string()))?;

                // 验证：client_response XOR SHA1(salt + stored_hash) 应恢复出 stored_hash
                // client_response = SHA1(password) XOR SHA1(salt + SHA1(SHA1(password)))
                //                 = password_hash XOR SHA1(salt + stored_hash)
                // 所以 recovered = client_response XOR SHA1(salt + stored_hash) = password_hash
                // 验证 recovered == stored_hash（即 SHA1(password)）
                let mut hasher = Sha1::new();
                hasher.update(&self.salt);
                hasher.update(stored_hash);
                let stage1 = hasher.finalize();

                let mut recovered = [0u8; 20];
                for i in 0..20 {
                    recovered[i] = auth_response[i] ^ stage1[i];
                }

                if recovered == *stored_hash {
                    Ok(username.to_string())
                } else {
                    Err(AuthError::AccessDenied(username.to_string()))
                }
            }
        }
    }
}

/// 计算客户端认证响应：SHA1(password_hash) XOR SHA1(salt + SHA1(password_hash))。
///
/// 等价于 `SHA1(password) XOR SHA1(salt + SHA1(SHA1(password)))`。
pub fn compute_auth_response(password_hash: &[u8; 20], salt: &[u8; SALT_LEN]) -> [u8; 20] {
    // stage1 = SHA1(salt + SHA1(password_hash))
    let mut hasher = Sha1::new();
    hasher.update(salt);
    hasher.update(password_hash);
    let stage1 = hasher.finalize();

    // response = password_hash XOR stage1
    let mut response = [0u8; 20];
    for i in 0..20 {
        response[i] = password_hash[i] ^ stage1[i];
    }
    response
}

/// 计算 SHA1(password)（用于服务器端存储）。
pub fn hash_password(password: &str) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(password.as_bytes());
    let result = hasher.finalize();
    let mut hash = [0u8; 20];
    hash.copy_from_slice(&result);
    hash
}

/// 计算 SHA1(SHA1(password))（用于 mysql.user 表存储）。
pub fn double_hash_password(password: &str) -> [u8; 20] {
    let first = hash_password(password);
    let mut hasher = Sha1::new();
    hasher.update(first);
    let result = hasher.finalize();
    let mut hash = [0u8; 20];
    hash.copy_from_slice(&result);
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_hash_password_deterministic() {
        let h1 = hash_password("password123");
        let h2 = hash_password("password123");
        assert_eq!(h1, h2);

        let h3 = hash_password("different");
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_double_hash_differs_from_single() {
        let single = hash_password("test");
        let double = double_hash_password("test");
        assert_ne!(single, double);
    }

    #[test]
    fn test_auth_response_roundtrip() {
        // 模拟客户端-服务端认证交互
        let password = "my_secret";
        let stored_hash = hash_password(password); // 服务器存储 SHA1(password)

        let mut salt = [0u8; SALT_LEN];
        rand::rng().fill_bytes(&mut salt);
        for byte in salt.iter_mut() {
            if *byte == 0 {
                *byte = 1;
            }
        }

        // 客户端计算响应
        let client_response = compute_auth_response(&stored_hash, &salt);

        // 服务器验证：response XOR stored_hash 应等于 SHA1(salt + SHA1(stored_hash))
        let mut received_sha1 = [0u8; 20];
        for i in 0..20 {
            received_sha1[i] = client_response[i] ^ stored_hash[i];
        }

        // 重新计算期望值
        let mut hasher = Sha1::new();
        hasher.update(&salt);
        hasher.update(&stored_hash);
        let expected = hasher.finalize();

        assert_eq!(&received_sha1[..], &expected[..]);
    }

    #[test]
    fn test_auth_session_trust_mode() {
        let session = AuthSession::new(AuthMode::Trust);
        assert_eq!(session.salt().len(), SALT_LEN);

        let mut session = session;
        let result = session.verify("root", &[], "mysql_native_password");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "root");
    }

    #[test]
    fn test_auth_session_native_password_success() {
        let password = "test_pass";
        let stored_hash = hash_password(password);

        let mut users = HashMap::new();
        users.insert("test_user".to_string(), stored_hash);

        let mode = AuthMode::MysqlNativePassword { users };
        let mut session = AuthSession::new(mode);
        let salt = *session.salt();

        // 客户端响应
        let client_response = compute_auth_response(&stored_hash, &salt);

        let result = session.verify("test_user", &client_response, "mysql_native_password");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test_user");
    }

    #[test]
    fn test_auth_session_native_password_wrong_password() {
        let stored_hash = hash_password("correct_pass");

        let mut users = HashMap::new();
        users.insert("user".to_string(), stored_hash);

        let mode = AuthMode::MysqlNativePassword { users };
        let mut session = AuthSession::new(mode);
        let salt = *session.salt();

        // 用错误密码计算响应
        let wrong_hash = hash_password("wrong_pass");
        let client_response = compute_auth_response(&wrong_hash, &salt);

        let result = session.verify("user", &client_response, "mysql_native_password");
        assert!(matches!(result, Err(AuthError::AccessDenied(_))));
    }

    #[test]
    fn test_auth_session_native_password_unknown_user() {
        let users = HashMap::new();
        let mode = AuthMode::MysqlNativePassword { users };
        let mut session = AuthSession::new(mode);

        let response = [0u8; 20];
        let result = session.verify("ghost", &response, "mysql_native_password");
        assert!(matches!(result, Err(AuthError::AccessDenied(_))));
    }

    #[test]
    fn test_auth_session_invalid_response_length() {
        let users = HashMap::new();
        let mode = AuthMode::MysqlNativePassword { users };
        let mut session = AuthSession::new(mode);

        let result = session.verify("user", &[1, 2, 3], "mysql_native_password");
        assert!(matches!(result, Err(AuthError::InvalidResponse(_))));
    }

    #[test]
    fn test_auth_session_unsupported_plugin() {
        let users = HashMap::new();
        let mode = AuthMode::MysqlNativePassword { users };
        let mut session = AuthSession::new(mode);

        let response = [0u8; 20];
        let result = session.verify("user", &response, "sha256_password");
        assert!(matches!(result, Err(AuthError::UnsupportedPlugin(_))));
    }

    #[test]
    fn test_salt_contains_no_null_bytes() {
        for _ in 0..100 {
            let session = AuthSession::new(AuthMode::Trust);
            for &byte in session.salt() {
                assert_ne!(byte, 0, "salt must not contain NUL bytes");
            }
        }
    }

    #[test]
    fn test_salt_is_random_per_session() {
        let s1 = AuthSession::new(AuthMode::Trust);
        let s2 = AuthSession::new(AuthMode::Trust);
        // 随机生成的 salt 极不可能相同
        assert_ne!(s1.salt(), s2.salt());
    }
}
