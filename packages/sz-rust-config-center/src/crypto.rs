// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! AES-GCM 敏感配置加解密（T018）
//!
//! 存储密文，内存中解密使用。

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;

use crate::ConfigCenterError;

/// AES-GCM 加密器
pub struct ConfigCrypto {
    cipher: Aes256Gcm,
}

impl ConfigCrypto {
    /// 从 32 字节密钥创建加密器
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: Aes256Gcm::new(key.into()),
        }
    }

    /// 加密明文，返回 Base64 编码的 `nonce || ciphertext`
    pub fn encrypt(&self, plaintext: &str) -> Result<String, ConfigCenterError> {
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| ConfigCenterError::Crypto(e.to_string()))?;

        let mut combined = Vec::with_capacity(12 + ciphertext.len());
        combined.extend_from_slice(&nonce_bytes);
        combined.extend_from_slice(&ciphertext);

        Ok(base64_encode(&combined))
    }

    /// 解密 `nonce || ciphertext`，返回明文
    pub fn decrypt(&self, encoded: &str) -> Result<String, ConfigCenterError> {
        let combined = base64_decode(encoded).map_err(ConfigCenterError::Crypto)?;

        if combined.len() < 12 {
            return Err(ConfigCenterError::Crypto(
                "ciphertext too short".to_string(),
            ));
        }

        let nonce = Nonce::from_slice(&combined[..12]);
        let ciphertext = &combined[12..];

        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| ConfigCenterError::Crypto(e.to_string()))?;

        String::from_utf8(plaintext).map_err(|e| ConfigCenterError::Crypto(e.to_string()))
    }
}

fn base64_encode(data: &[u8]) -> String {
    use base64::{engine::general_purpose, Engine};
    general_purpose::STANDARD.encode(data)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::{engine::general_purpose, Engine};
    general_purpose::STANDARD
        .decode(s)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = i as u8;
        }
        key
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let crypto = ConfigCrypto::new(&make_key());
        let plaintext = "my-secret-password-123";
        let encrypted = crypto.encrypt(plaintext).unwrap();
        let decrypted = crypto.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_different_each_time() {
        let crypto = ConfigCrypto::new(&make_key());
        let e1 = crypto.encrypt("same").unwrap();
        let e2 = crypto.encrypt("same").unwrap();
        assert_ne!(e1, e2, "nonce should make each encryption unique");
    }

    #[test]
    fn test_decrypt_invalid_data() {
        let crypto = ConfigCrypto::new(&make_key());
        assert!(crypto.decrypt("invalid-base64!!!").is_err());
    }

    #[test]
    fn test_decrypt_too_short() {
        let crypto = ConfigCrypto::new(&make_key());
        let short = base64_encode(&[1, 2, 3]);
        assert!(crypto.decrypt(&short).is_err());
    }
}
