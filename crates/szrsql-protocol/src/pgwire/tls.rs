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
//! - `TlsConfig::from_pem`：从 PEM 字节流加载证书链 + 私钥（可选客户端 CA）
//! - `TlsConfig::from_files`：从文件路径加载（生产环境）
//! - `TlsConfig::from_files_with_client_auth`：从文件加载并启用 mutual TLS
//! - 测试用例通过 `rcgen` 生成自签名证书，由 `from_pem` 加载
//!
//! # Mutual TLS（双向认证）
//!
//! 当 `require_client_cert=true` 且提供了 `client_ca` 时，服务器在 TLS 握手阶段
//! 要求客户端提供证书，并使用 `WebPkiClientVerifier` 验证证书是否由指定 CA 签名。
//! 适用于 Navicat `sslmode=verify-full` 等高安全场景。
//!
//! 参考文档：
//! - PostgreSQL SSL 支持: <https://www.postgresql.org/docs/current/ssl-tcp.html>
//! - pgwire SSLRequest: <https://www.postgresql.org/docs/current/protocol-flow.html#PROTOCOL-STARTUP-SSL>
//! - rustls: <https://docs.rs/rustls>
//! - tokio-rustls: <https://docs.rs/tokio-rustls>

use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::version::TLS13;
use rustls::{RootCertStore, ServerConfig};
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
/// 持有服务器证书链、私钥及可选的客户端 CA 证书池（用于 mutual TLS）。
/// `server_config()` 按需构造 `Arc<ServerConfig>`，可在多连接间复用。
///
/// # Mutual TLS
///
/// 通过 `with_require_client_cert(true)` + `with_client_ca(Some(store))` 启用，
/// 或直接使用 `from_files_with_client_auth` / `from_pem(..., Some(ca_pem))`。
pub struct TlsConfig {
    /// 服务器证书链（PEM 解析后的 DER 列表）。
    server_certs: Vec<CertificateDer<'static>>,
    /// 服务器私钥（DER 格式）。
    server_key: PrivateKeyDer<'static>,
    /// 是否要求客户端证书（mutual TLS）。
    require_client_cert: bool,
    /// 客户端 CA 证书池（`require_client_cert=true` 时必须提供）。
    client_ca: Option<Arc<RootCertStore>>,
}

impl Clone for TlsConfig {
    fn clone(&self) -> Self {
        Self {
            server_certs: self.server_certs.clone(),
            // rustls 0.23+ 中 PrivateKeyDer 不再实现 Clone，使用 clone_key() 拷贝私钥
            server_key: self.server_key.clone_key(),
            require_client_cert: self.require_client_cert,
            client_ca: self.client_ca.clone(),
        }
    }
}

impl std::fmt::Debug for TlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsConfig")
            .field("require_client_cert", &self.require_client_cert)
            .field("has_client_ca", &self.client_ca.is_some())
            .finish_non_exhaustive()
    }
}

impl TlsConfig {
    /// 从 PEM 字节流加载证书链与私钥，构造强制 TLS 1.3 的配置。
    ///
    /// - `cert_pem`：PEM 编码的证书链（含服务器证书 + 中间证书）
    /// - `key_pem`：PEM 编码的 PKCS#8 / PKCS#1 / Sec1 私钥
    /// - `client_ca_pem`：可选的客户端 CA 证书 PEM（提供时自动启用 mutual TLS）
    ///
    /// 当 `client_ca_pem` 为 `Some` 时，自动设置 `require_client_cert=true`。
    pub fn from_pem(
        cert_pem: &[u8],
        key_pem: &[u8],
        client_ca_pem: Option<&[u8]>,
    ) -> Result<Self, TlsError> {
        let server_certs = parse_certificate_chain(cert_pem)?;
        let server_key = parse_private_key(key_pem)?;

        // 提供客户端 CA 时自动启用 mutual TLS
        let (client_ca, require_client_cert) = if let Some(ca_pem) = client_ca_pem {
            let store = parse_root_cert_store(ca_pem)?;
            (Some(Arc::new(store)), true)
        } else {
            (None, false)
        };

        Ok(Self {
            server_certs,
            server_key,
            require_client_cert,
            client_ca,
        })
    }

    /// 从文件路径加载证书和私钥（可选客户端 CA）。
    ///
    /// - `cert_path`：服务器证书文件路径
    /// - `key_path`：服务器私钥文件路径
    /// - `client_ca_path`：客户端 CA 文件路径（`Some` 时启用 mutual TLS）
    pub fn from_files<P: AsRef<Path>>(
        cert_path: P,
        key_path: P,
        client_ca_path: Option<P>,
    ) -> Result<Self, TlsError> {
        let cert_pem = std::fs::read(cert_path)?;
        let key_pem = std::fs::read(key_path)?;
        let client_ca_pem = match client_ca_path {
            Some(p) => Some(std::fs::read(p)?),
            None => None,
        };
        Self::from_pem(&cert_pem, &key_pem, client_ca_pem.as_deref())
    }

    /// 从文件路径加载并启用 mutual TLS（双向认证）。
    ///
    /// 等价于 `from_files(server_cert, server_key, Some(client_ca))`。
    ///
    /// - `server_cert_path`：服务器证书文件路径
    /// - `server_key_path`：服务器私钥文件路径
    /// - `client_ca_path`：客户端 CA 证书文件路径
    pub fn from_files_with_client_auth<P: AsRef<Path>>(
        server_cert_path: P,
        server_key_path: P,
        client_ca_path: P,
    ) -> Result<Self, TlsError> {
        Self::from_files(server_cert_path, server_key_path, Some(client_ca_path))
    }

    /// 设置是否要求客户端证书（mutual TLS）。
    ///
    /// 设为 `true` 时，`server_config()` 会使用 `WebPkiClientVerifier` 验证客户端证书。
    /// 需同时通过 [`with_client_ca`](Self::with_client_ca) 设置客户端 CA。
    pub fn with_require_client_cert(mut self, require: bool) -> Self {
        self.require_client_cert = require;
        self
    }

    /// 设置客户端 CA 证书池（用于 mutual TLS）。
    ///
    /// 传入 `Some(store)` 设置 CA 证书池；传入 `None` 清除。
    /// 仅当 `require_client_cert=true` 时生效。
    pub fn with_client_ca(mut self, client_ca: Option<RootCertStore>) -> Self {
        self.client_ca = client_ca.map(Arc::new);
        self
    }

    /// 按需构造 `Arc<ServerConfig>`。
    ///
    /// - `require_client_cert=false`：使用 `with_no_client_auth()`（单向 TLS）
    /// - `require_client_cert=true`：使用 `with_client_cert_verifier()`（mutual TLS）
    ///
    /// 当 `require_client_cert=true` 但 `client_ca` 未设置时返回错误。
    pub fn server_config(&self) -> Result<Arc<ServerConfig>, TlsError> {
        if self.require_client_cert {
            // mutual TLS：要求并验证客户端证书
            let client_ca = self.client_ca.as_ref().ok_or_else(|| {
                TlsError::Rustls("require_client_cert=true but client_ca is not set".into())
            })?;
            let verifier = WebPkiClientVerifier::builder(Arc::clone(client_ca))
                .build()
                .map_err(|e| TlsError::Rustls(e.to_string()))?;
            let config = ServerConfig::builder_with_protocol_versions(&[&TLS13])
                .with_client_cert_verifier(verifier)
                .with_single_cert(self.server_certs.clone(), self.server_key.clone_key())
                .map_err(|e| TlsError::Rustls(e.to_string()))?;
            Ok(Arc::new(config))
        } else {
            // 单向 TLS：不验证客户端证书
            let config = ServerConfig::builder_with_protocol_versions(&[&TLS13])
                .with_no_client_auth()
                .with_single_cert(self.server_certs.clone(), self.server_key.clone_key())
                .map_err(|e| TlsError::Rustls(e.to_string()))?;
            Ok(Arc::new(config))
        }
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

/// 解析 PEM 编码的 CA 证书为 `RootCertStore`。
///
/// 将 PEM 中的所有证书添加到信任库，用于 mutual TLS 中验证客户端证书。
fn parse_root_cert_store(ca_pem: &[u8]) -> Result<RootCertStore, TlsError> {
    let mut store = RootCertStore::empty();
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(ca_pem)
        .collect::<Result<_, _>>()
        .map_err(|e| TlsError::Certificate(e.to_string()))?;
    if certs.is_empty() {
        return Err(TlsError::Certificate(
            "no CA certificate found in PEM".into(),
        ));
    }
    for cert in certs {
        store
            .add(cert)
            .map_err(|e| TlsError::Certificate(e.to_string()))?;
    }
    Ok(store)
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

    /// 生成自签名 CA 证书（PEM 格式），用于 mutual TLS 测试。
    ///
    /// 使用 rcgen 的 `generate_simple_self_signed` 生成自签名证书作为测试 CA。
    /// 注：实际生产环境应使用 `IsCa::Ca` 的 CA 证书，此处仅用于测试 `server_config()` 构造。
    fn generate_ca_pem() -> Vec<u8> {
        let cert = rcgen::generate_simple_self_signed(vec!["szrsql-test-ca".to_string()])
            .expect("rcgen: failed to generate self-signed cert for CA");
        cert.cert.pem().as_bytes().to_vec()
    }

    #[test]
    fn test_tls_config_from_pem_self_signed() {
        let (cert_pem, key_pem) = generate_self_signed_pem();
        let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem, None)
            .expect("should construct TlsConfig from self-signed PEM");
        // 验证可获取 ServerConfig（隐式验证构造成功）
        let _ = tls_config
            .server_config()
            .expect("server_config should succeed");
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
        let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem, None).unwrap();
        let debug_str = format!("{tls_config:?}");
        assert!(debug_str.contains("TlsConfig"));
        // Debug 输出不应包含私钥内容
        assert!(!debug_str.contains("PRIVATE KEY"));
    }

    // ---- mutual TLS 测试 ----

    #[test]
    fn test_tls_config_with_client_ca_enables_mutual_tls() {
        let (cert_pem, key_pem) = generate_self_signed_pem();
        let ca_pem = generate_ca_pem();
        // 提供客户端 CA 时自动启用 mutual TLS
        let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem, Some(&ca_pem))
            .expect("should construct TlsConfig with client CA");
        assert!(tls_config.require_client_cert);
        assert!(tls_config.client_ca.is_some());
        // 验证 server_config 使用 client_cert_verifier 构造成功
        let _ = tls_config
            .server_config()
            .expect("server_config with mutual TLS should succeed");
    }

    #[test]
    fn test_from_files_with_client_auth() {
        // 写入临时文件测试 from_files_with_client_auth
        let (cert_pem, key_pem) = generate_self_signed_pem();
        let ca_pem = generate_ca_pem();
        let dir = std::env::temp_dir();
        let cert_path = dir.join("szrsql_test_tls_cert.pem");
        let key_path = dir.join("szrsql_test_tls_key.pem");
        let ca_path = dir.join("szrsql_test_tls_ca.pem");
        std::fs::write(&cert_path, &cert_pem).unwrap();
        std::fs::write(&key_path, &key_pem).unwrap();
        std::fs::write(&ca_path, &ca_pem).unwrap();

        let tls_config = TlsConfig::from_files_with_client_auth(&cert_path, &key_path, &ca_path)
            .expect("from_files_with_client_auth should succeed");
        assert!(tls_config.require_client_cert);
        assert!(tls_config.client_ca.is_some());

        // 清理临时文件
        let _ = std::fs::remove_file(&cert_path);
        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_file(&ca_path);
    }

    #[test]
    fn test_with_require_client_cert_builder() {
        let (cert_pem, key_pem) = generate_self_signed_pem();
        let ca_pem = generate_ca_pem();
        // 先构造不带 mutual TLS 的配置
        let tls_config =
            TlsConfig::from_pem(&cert_pem, &key_pem, None).expect("from_pem without client CA");
        assert!(!tls_config.require_client_cert);

        // 通过 builder 设置 client_ca 和 require_client_cert
        let mut store = RootCertStore::empty();
        let ca_certs: Vec<_> = CertificateDer::pem_slice_iter(&ca_pem)
            .collect::<Result<_, _>>()
            .expect("parse CA PEM");
        for c in ca_certs {
            store.add(c).expect("add CA cert");
        }
        let tls_config = tls_config
            .with_client_ca(Some(store))
            .with_require_client_cert(true);
        assert!(tls_config.require_client_cert);
        assert!(tls_config.client_ca.is_some());
        // server_config 应成功（mutual TLS）
        let _ = tls_config
            .server_config()
            .expect("server_config with builder-set mutual TLS should succeed");
    }

    #[test]
    fn test_server_config_errors_when_require_without_ca() {
        let (cert_pem, key_pem) = generate_self_signed_pem();
        // require_client_cert=true 但未设置 client_ca
        let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem, None)
            .expect("from_pem")
            .with_require_client_cert(true);
        let err = tls_config.server_config().unwrap_err();
        assert!(matches!(err, TlsError::Rustls(_)));
        assert!(err.to_string().contains("client_ca is not set"));
    }

    #[test]
    fn test_with_require_client_cert_false_disables_mutual_tls() {
        let (cert_pem, key_pem) = generate_self_signed_pem();
        let ca_pem = generate_ca_pem();
        // 提供客户端 CA（自动启用 mutual TLS），然后用 builder 关闭
        let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem, Some(&ca_pem))
            .expect("from_pem with client CA")
            .with_require_client_cert(false);
        assert!(!tls_config.require_client_cert);
        // server_config 应退化为单向 TLS（不验证客户端证书）
        let _ = tls_config
            .server_config()
            .expect("server_config should succeed with single-direction TLS");
    }

    #[test]
    fn test_parse_root_cert_store_empty_errors() {
        let empty: Vec<u8> = Vec::new();
        let err = parse_root_cert_store(&empty).unwrap_err();
        assert!(matches!(err, TlsError::Certificate(_)));
    }

    #[test]
    fn test_parse_root_cert_store_single() {
        let ca_pem = generate_ca_pem();
        let store = parse_root_cert_store(&ca_pem).expect("should parse CA cert store");
        assert!(!store.is_empty());
    }

    #[test]
    fn test_server_config_caches_independently_per_call() {
        // 验证多次调用 server_config 返回独立的 Arc（不共享缓存）
        let (cert_pem, key_pem) = generate_self_signed_pem();
        let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem, None).unwrap();
        let config1 = tls_config.server_config().unwrap();
        let config2 = tls_config.server_config().unwrap();
        // 两次调用应返回不同的 Arc 实例（按需构造，不缓存）
        assert!(!Arc::ptr_eq(&config1, &config2));
    }
}
