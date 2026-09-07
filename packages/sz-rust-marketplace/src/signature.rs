// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Ed25519 签名与 SHA256 完整性校验

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use sha2::{Digest, Sha256};

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};

use crate::error::{MarketplaceError, MarketplaceResult};

/// 签名服务
pub struct SignatureService;

impl SignatureService {
    /// Ed25519 签名（Base64 编码）
    pub fn sign(package: &[u8], private_key: &SigningKey) -> String {
        let signature = private_key.sign(package);
        B64.encode(signature.to_bytes())
    }

    /// Ed25519 签名校验
    pub fn verify(
        package: &[u8],
        signature: &str,
        public_key: &VerifyingKey,
    ) -> MarketplaceResult<()> {
        let sig_bytes = B64
            .decode(signature)
            .map_err(|e| MarketplaceError::InvalidSignature(format!("Base64 解码失败: {e}")))?;
        let sig_arr: [u8; 64] = sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| MarketplaceError::InvalidSignature("签名长度非 64 字节".to_string()))?;
        let signature = ed25519_dalek::Signature::from_bytes(&sig_arr);
        public_key
            .verify(package, &signature)
            .map_err(|_| MarketplaceError::InvalidSignature("签名校验失败".to_string()))
    }

    /// SHA256 校验和（64 位十六进制）
    pub fn sha256_checksum(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        let mut hex = String::with_capacity(64);
        for byte in result {
            hex.push_str(&format!("{byte:02x}"));
        }
        hex
    }

    /// 公钥 SHA256 指纹
    pub fn pubkey_fingerprint(public_key: &VerifyingKey) -> String {
        Self::sha256_checksum(&public_key.to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn test_keypair() -> SigningKey {
        let seed = [42u8; 32];
        SigningKey::from_bytes(&seed)
    }

    #[test]
    fn test_sign_verify_roundtrip() {
        let keypair = test_keypair();
        let public_key = keypair.verifying_key();

        let package = b"hello marketplace";
        let signature = SignatureService::sign(package, &keypair);
        SignatureService::verify(package, &signature, &public_key).unwrap();
    }

    #[test]
    fn test_verify_tampered_package() {
        let keypair = test_keypair();
        let public_key = keypair.verifying_key();

        let package = b"original content";
        let signature = SignatureService::sign(package, &keypair);

        let tampered = b"tampered content";
        let result = SignatureService::verify(tampered, &signature, &public_key);
        assert!(result.is_err());
    }

    #[test]
    fn test_sha256_checksum() {
        let data = b"test data";
        let checksum = SignatureService::sha256_checksum(data);
        assert_eq!(checksum.len(), 64);
        let checksum2 = SignatureService::sha256_checksum(data);
        assert_eq!(checksum, checksum2);

        let different = SignatureService::sha256_checksum(b"different data");
        assert_ne!(checksum, different);
    }

    #[test]
    fn test_pubkey_fingerprint() {
        let keypair = test_keypair();
        let public_key = keypair.verifying_key();
        let fingerprint = SignatureService::pubkey_fingerprint(&public_key);
        assert_eq!(fingerprint.len(), 64);
    }

    #[test]
    fn test_verify_invalid_base64() {
        let keypair = test_keypair();
        let public_key = keypair.verifying_key();
        let result = SignatureService::verify(b"data", "!!!invalid base64!!!", &public_key);
        assert!(result.is_err());
    }
}
