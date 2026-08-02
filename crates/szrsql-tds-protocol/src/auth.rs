//! TDS 认证模块 — SQL Server 认证（NTLM / 明文）。
//!
//! TDS 协议认证流程：
//! ```text
//! 1. Pre-Login 协商（可选，确定 ENCRYPTION 等）
//! 2. Login7：客户端发送用户名 + 密码（XOR 0xA5 + 高低 nibble 交换混淆）
//! 3. 服务器验证后返回 Token Stream（包含 LOGINACK / ERROR）
//! ```
//!
//! ## 认证模式
//!
//! - `Trust`：信任模式（无密码，仅用于测试）
//! - `Ntlm`：使用 SQL Server 自带密码混淆（XOR 0xA5）+ 用户表比对
//!
//! 注意：完整的 NTLM SSPI 需要 MD4/HMAC-MD5 等 crypto，超出本 crate 依赖范围，
//! 因此 `Ntlm` 模式实际执行 SQL Server 自带的密码混淆校验（即"明文密码经混淆后比对"）。

use std::collections::HashMap;
use thiserror::Error;

/// Login7 密码混淆常量：每字节与 0xA5 异或，然后高低 nibble 交换。
pub const PASSWORD_XOR_MASK: u8 = 0xA5;

/// 认证错误。
#[derive(Debug, Error)]
pub enum AuthError {
    /// 密码不匹配
    #[error("access denied: invalid password for user '{0}'")]
    AccessDenied(String),
    /// 客户端响应格式错误
    #[error("invalid client response: {0}")]
    InvalidResponse(String),
    /// 不支持的认证模式
    #[error("unsupported auth mode: {0}")]
    UnsupportedMode(String),
    /// Login7 缺少必要字段
    #[error("missing login field: {0}")]
    MissingField(String),
}

/// 认证模式。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AuthMode {
    /// 信任模式（无密码，仅用于测试）
    #[default]
    Trust,
    /// SQL Server 密码混淆认证（Ntlm 命名沿用任务约定）
    Ntlm {
        /// 用户名 → 明文密码 映射
        users: HashMap<String, String>,
    },
}

/// 单次认证会话。
pub struct AuthSession {
    /// 模式
    mode: AuthMode,
    /// 客户端声称的用户名
    username: Option<String>,
}

impl AuthSession {
    /// 创建新会话。
    pub fn new(mode: AuthMode) -> Self {
        Self {
            mode,
            username: None,
        }
    }

    /// 返回当前模式引用。
    pub fn mode(&self) -> &AuthMode {
        &self.mode
    }

    /// 验证客户端 Login7 提交的凭据。
    ///
    /// `username`：客户端声称的用户名
    /// `obfuscated_password`：Login7 中经 XOR 0xA5 + nibble swap 混淆的密码（UTF-16LE 字节）
    pub fn verify(
        &mut self,
        username: &str,
        obfuscated_password: &[u8],
    ) -> Result<String, AuthError> {
        self.username = Some(username.to_string());

        match &self.mode {
            AuthMode::Trust => Ok(username.to_string()),

            AuthMode::Ntlm { users } => {
                let expected_password = users
                    .get(username)
                    .ok_or_else(|| AuthError::AccessDenied(username.to_string()))?;

                // 反向计算：先反 nibble swap，再 XOR 0xA5，得到 UTF-16LE 密码字节
                let deobfuscated = deobfuscate_password(obfuscated_password);
                let received =
                    String::from_utf16_lossy(deobfuscated_to_utf16(&deobfuscated).as_slice());

                if received == *expected_password {
                    Ok(username.to_string())
                } else {
                    Err(AuthError::AccessDenied(username.to_string()))
                }
            }
        }
    }

    /// 取回已认证的用户名（验证成功后调用）。
    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }
}

/// 将明文密码（UTF-16LE 字节）按 Login7 规则混淆：每字节 XOR 0xA5，然后高低 nibble 交换。
///
/// 用于客户端发送 Login7 时构造 password 字段；服务端可对收到的字段做反向运算还原。
pub fn obfuscate_password(plain_password_utf16: &[u8]) -> Vec<u8> {
    plain_password_utf16
        .iter()
        .map(|&b| {
            let xored = b ^ PASSWORD_XOR_MASK;
            ((xored & 0x0F) << 4) | ((xored & 0xF0) >> 4)
        })
        .collect()
}

/// 将 Login7 中的混淆密码字段还原为 UTF-16LE 字节。
///
/// 反向操作：先反 nibble swap，再 XOR 0xA5。
pub fn deobfuscate_password(obfuscated: &[u8]) -> Vec<u8> {
    obfuscated
        .iter()
        .map(|&b| {
            let unswapped = ((b & 0x0F) << 4) | ((b & 0xF0) >> 4);
            unswapped ^ PASSWORD_XOR_MASK
        })
        .collect()
}

/// 将 UTF-16LE 字节序列解码为 u16 数组。
pub fn deobfuscated_to_utf16(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect()
}

/// 将字符串编码为 UTF-16LE 字节序列。
pub fn encode_utf16_le(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 2);
    for unit in s.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_obfuscate_deobfuscate_roundtrip() {
        let plain = encode_utf16_le("Password123!");
        let obfuscated = obfuscate_password(&plain);
        let recovered = deobfuscate_password(&obfuscated);
        assert_eq!(recovered, plain);
    }

    #[test]
    fn test_obfuscate_each_byte_changed() {
        let plain = encode_utf16_le("sa");
        let obfuscated = obfuscate_password(&plain);
        // 至少不能与原始字节相同
        assert_ne!(plain, obfuscated);
    }

    #[test]
    fn test_auth_session_trust_mode() {
        let mut session = AuthSession::new(AuthMode::Trust);
        let result = session.verify("sa", &[]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "sa");
        assert_eq!(session.username(), Some("sa"));
    }

    #[test]
    fn test_auth_session_ntlm_success() {
        let mut users = HashMap::new();
        users.insert("admin".to_string(), "P@ssw0rd".to_string());
        let mode = AuthMode::Ntlm { users };
        let mut session = AuthSession::new(mode);

        let plain = encode_utf16_le("P@ssw0rd");
        let obfuscated = obfuscate_password(&plain);
        let result = session.verify("admin", &obfuscated);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "admin");
    }

    #[test]
    fn test_auth_session_ntlm_wrong_password() {
        let mut users = HashMap::new();
        users.insert("admin".to_string(), "correct".to_string());
        let mode = AuthMode::Ntlm { users };
        let mut session = AuthSession::new(mode);

        // 客户端发送错误密码
        let plain = encode_utf16_le("wrong");
        let obfuscated = obfuscate_password(&plain);
        let result = session.verify("admin", &obfuscated);
        assert!(matches!(result, Err(AuthError::AccessDenied(_))));
    }

    #[test]
    fn test_auth_session_ntlm_unknown_user() {
        let users = HashMap::new();
        let mode = AuthMode::Ntlm { users };
        let mut session = AuthSession::new(mode);

        let plain = encode_utf16_le("whatever");
        let obfuscated = obfuscate_password(&plain);
        let result = session.verify("ghost", &obfuscated);
        assert!(matches!(result, Err(AuthError::AccessDenied(_))));
    }

    #[test]
    fn test_auth_session_username_after_verify() {
        let mut session = AuthSession::new(AuthMode::Trust);
        assert_eq!(session.username(), None);
        let _ = session.verify("user1", &[]);
        assert_eq!(session.username(), Some("user1"));
    }

    #[test]
    fn test_encode_utf16_le_roundtrip() {
        let s = "中文密码";
        let bytes = encode_utf16_le(s);
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let recovered = String::from_utf16_lossy(&units);
        assert_eq!(recovered, s);
    }

    #[test]
    fn test_deobfuscate_to_utf16() {
        let plain = encode_utf16_le("abc");
        let obfuscated = obfuscate_password(&plain);
        let deobfuscated = deobfuscate_password(&obfuscated);
        let units = deobfuscated_to_utf16(&deobfuscated);
        let s = String::from_utf16_lossy(&units);
        assert_eq!(s, "abc");
    }

    #[test]
    fn test_ntlm_chinese_password() {
        let mut users = HashMap::new();
        users.insert("sa".to_string(), "中文密码".to_string());
        let mode = AuthMode::Ntlm { users };
        let mut session = AuthSession::new(mode);

        let plain = encode_utf16_le("中文密码");
        let obfuscated = obfuscate_password(&plain);
        let result = session.verify("sa", &obfuscated);
        assert!(result.is_ok());
    }

    #[test]
    fn test_password_xor_mask_constant() {
        // 0xA5 = 165
        assert_eq!(PASSWORD_XOR_MASK, 0xA5);
        assert_eq!(0x12 ^ PASSWORD_XOR_MASK, 0xB7);
    }

    #[test]
    fn test_default_auth_mode_is_trust() {
        let mode = AuthMode::default();
        assert_eq!(mode, AuthMode::Trust);
    }
}
