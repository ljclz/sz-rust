//! Phase 4.5 — pgwire TLS 1.3 加密（rustls 集成）。
//!
//! # 概述
//!
//! 基于 rustls 0.23 + tokio-rustls 0.26 实现 PostgreSQL 协议的 SSL 协商：
//!
//! 1. 客户端连接后发送 `SSLRequest`（特殊协议版本号 80877103）
//! 2. 服务器根据 TLS 配置：
//!    - 已配置 TLS：回复单字节 `'S'`，随后对 TCP 流执行 TLS 握手
//!    - 未配置 TLS：回复单字节 `'N'`，客户端应回退到明文
//! 3. TLS 握手成功后，连接升级为加密流，后续 pgwire 协议在加密层上传输
//!
//! # TLS 1.3
//!
//! 本模块通过 `ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])`
//! 显式限定仅启用 TLS 1.3。
//!
//! # 证书加载
//!
//! - `TlsConfig::from_pem`：从 PEM 字节流加载证书链 + 私钥
//! - `TlsConfig::from_files`：从文件路径加载（生产环境）
//! - 测试用例通过 `rcgen` 生成自签名证书，由 `from_pem` 加载
//!
//! 参考文档：
//! - PostgreSQL SSL 支持: <https://www.postgresql.org/docs/current/ssl-tcp.html>
//! - pgwire SSLRequest: <https://www.postgresql.org/docs/current/protocol-flow.html#PROTOCOL-STARTUP-SSL>
//! - rustls: <https://docs.rs/rustls>
//! - tokio-rustls: <https://docs.rs/tokio-rustls>

use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::version::TLS13;
use rustls::ServerConfig;
use std::io;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;

// =====================================================================
//  TlsError
// =====================================================================

/// TLS 配置或握手错误。
#[derive(Debug, Error)]
pub enum TlsError {
    /// 证书文件读取/解析失败。
    #[error("certificate error: {0}")]
    Certificate(String),

    /// 私钥文件读取/解析失败。
    #[error("private key error: {0}")]
    PrivateKey(String),

    /// IO 错误。
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// rustls 配置构造失败。
    #[error("rustls config error: {0}")]
    Rustls(String),
}

// =====================================================================
//  TlsConfig
// =====================================================================

/// pgwire TLS 配置。
///
/// 持有 rustls 的 `ServerConfig`（Arc 共享），可在多连接间复用。
#[derive(Clone)]
pub struct TlsConfig {
    server_config: Arc<ServerConfig>,
}

impl std::fmt::Debug for TlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsConfig").finish_non_exhaustive()
    }
}

impl TlsConfig {
    /// 从 PEM 字节流加载证书链与私钥，构造强制 TLS 1.3 的 `ServerConfig`。
    ///
    /// - `cert_pem`：PEM 编码的证书链（含服务器证书 + 中间证书）
    /// - `key_pem`：PEM 编码的 PKCS#8 / PKCS#1 / Sec1 私钥
    pub fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<Self, TlsError> {
        let certs = parse_certificate_chain(cert_pem)?;
        let key = parse_private_key(key_pem)?;

        // 强制仅启用 TLS 1.3
        let server_config = ServerConfig::builder_with_protocol_versions(&[&TLS13])
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| TlsError::Rustls(e.to_string()))?;

        Ok(Self {
            server_config: Arc::new(server_config),
        })
    }

    /// 从文件路径加载证书和私钥。
    pub fn from_files<P: AsRef<Path>>(cert_path: P, key_path: P) -> Result<Self, TlsError> {
        let cert_pem = std::fs::read(cert_path)?;
        let key_pem = std::fs::read(key_path)?;
        Self::from_pem(&cert_pem, &key_pem)
    }

    /// 返回内部 `ServerConfig` 引用（Arc 共享）。
    pub fn server_config(&self) -> Arc<ServerConfig> {
        Arc::clone(&self.server_config)
    }
}

// =====================================================================
//  PEM 解析辅助
// =====================================================================

/// 解析 PEM 编码的证书链为 `CertificateDer` 列表。
///
/// 委托给 `rustls::pki_types::pem::PemObject::pem_slice_iter`，自动识别
/// `-----BEGIN CERTIFICATE-----` / `-----END CERTIFICATE-----` 块。
fn parse_certificate_chain(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(pem)
        .collect::<Result<_, _>>()
        .map_err(|e| TlsError::Certificate(e.to_string()))?;
    if certs.is_empty() {
        return Err(TlsError::Certificate("no certificate found in PEM".into()));
    }
    Ok(certs)
}

/// 解析 PEM 编码的私钥为 `PrivateKeyDer`。
///
/// 使用 `PemObject::pem_slice_iter` 迭代所有 PEM 块，返回第一个匹配的私钥。
/// `PrivateKeyDer` 的 `PemObject` 实现会自动识别 PKCS#8 / PKCS#1 RSA / Sec1 EC 格式。
fn parse_private_key(pem: &[u8]) -> Result<PrivateKeyDer<'static>, TlsError> {
    let mut iter = PrivateKeyDer::pem_slice_iter(pem);
    if let Some(item) = iter.next() {
        return item.map_err(|e| TlsError::PrivateKey(e.to_string()));
    }
    Err(TlsError::PrivateKey("no private key found in PEM".into()))
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 使用 rcgen 生成自签名证书和私钥（PEM 格式）。
    fn generate_self_signed_pem() -> (Vec<u8>, Vec<u8>) {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("rcgen: failed to generate self-signed cert");
        let cert_pem = cert.cert.pem().as_bytes().to_vec();
        let key_pem = cert.key_pair.serialize_pem().as_bytes().to_vec();
        (cert_pem, key_pem)
    }

    #[test]
    fn test_tls_config_from_pem_self_signed() {
        let (cert_pem, key_pem) = generate_self_signed_pem();
        let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem)
            .expect("should construct TlsConfig from self-signed PEM");
        // 验证可获取 ServerConfig（隐式验证构造成功）
        let _ = tls_config.server_config();
    }

    #[test]
    fn test_parse_certificate_chain_single() {
        let (cert_pem, _) = generate_self_signed_pem();
        let certs = parse_certificate_chain(&cert_pem).expect("should parse cert chain");
        assert_eq!(certs.len(), 1);
        assert!(!certs[0].as_ref().is_empty());
    }

    #[test]
    fn test_parse_certificate_chain_empty_errors() {
        let empty: Vec<u8> = Vec::new();
        let err = parse_certificate_chain(&empty).unwrap_err();
        assert!(matches!(err, TlsError::Certificate(_)));
    }

    #[test]
    fn test_parse_private_key_pkcs8() {
        let (_, key_pem) = generate_self_signed_pem();
        let key = parse_private_key(&key_pem).expect("should parse private key");
        // rcgen 默认输出 PKCS#8
        assert!(!key.secret_der().is_empty());
    }

    #[test]
    fn test_parse_private_key_empty_errors() {
        let empty: Vec<u8> = Vec::new();
        let err = parse_private_key(&empty).unwrap_err();
        assert!(matches!(err, TlsError::PrivateKey(_)));
    }

    #[test]
    fn test_tls_config_debug_does_not_leak_secrets() {
        let (cert_pem, key_pem) = generate_self_signed_pem();
        let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem).unwrap();
        let debug_str = format!("{tls_config:?}");
        assert!(debug_str.contains("TlsConfig"));
        // Debug 输出不应包含私钥内容
        assert!(!debug_str.contains("PRIVATE KEY"));
    }
}
