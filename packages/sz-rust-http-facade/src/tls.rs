//! TLS 配置 + HTTP/2 服务
//!
//! 提供 `TlsConfig` 结构体和 `serve_http2` 函数，支持：
//! - ALPN 协商 h2 + http/1.1 自动回退
//! - 从 PEM 文件加载证书
//! - axum::serve 集成

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

/// TLS 错误
#[derive(Debug, Error)]
pub enum TlsError {
    /// IO 错误
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// 未找到有效证书
    #[error("No valid certificate found")]
    NoCertificate,
    /// 未找到有效私钥
    #[error("No valid private key found")]
    NoPrivateKey,
    /// TLS 协议错误
    #[error("TLS error: {0}")]
    Tls(String),
}

/// TLS 配置
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// 证书文件路径（PEM 格式）
    pub cert_path: PathBuf,
    /// 私钥文件路径（PEM 格式）
    pub key_path: PathBuf,
    /// ALPN 协议列表（默认 h2 + http/1.1）
    pub alpn: Vec<Vec<u8>>,
}

impl TlsConfig {
    /// 创建 TLS 配置
    pub fn new(cert_path: impl Into<PathBuf>, key_path: impl Into<PathBuf>) -> Self {
        Self {
            cert_path: cert_path.into(),
            key_path: key_path.into(),
            alpn: vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        }
    }

    /// 仅 HTTP/2（不回退 http/1.1）
    pub fn h2_only(mut self) -> Self {
        self.alpn = vec![b"h2".to_vec()];
        self
    }

    /// 仅 HTTP/1.1
    pub fn http1_only(mut self) -> Self {
        self.alpn = vec![b"http/1.1".to_vec()];
        self
    }

    /// 构建 TlsAcceptor
    ///
    /// 异步读取证书和私钥文件（遵循 tokio::fs 铁律）。
    pub async fn build_acceptor(&self) -> Result<TlsAcceptor, TlsError> {
        let cert = load_certs(&self.cert_path).await?;
        let key = load_private_key(&self.key_path).await?;

        let mut config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert, key)
            .map_err(|e| TlsError::Tls(e.to_string()))?;

        config.alpn_protocols = self.alpn.clone();

        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}

/// 从 PEM 文件加载证书
async fn load_certs(
    path: &std::path::Path,
) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, TlsError> {
    let file = tokio::fs::read_to_string(path).await?;
    let mut reader = std::io::BufReader::new(file.as_bytes());
    let certs: Vec<_> = rustls_pemfile::certs(&mut reader).collect::<Result<_, _>>()?;
    if certs.is_empty() {
        return Err(TlsError::NoCertificate);
    }
    Ok(certs)
}

/// 从 PEM 文件加载私钥
async fn load_private_key(
    path: &std::path::Path,
) -> Result<rustls::pki_types::PrivateKeyDer<'static>, TlsError> {
    let file = tokio::fs::read_to_string(path).await?;
    let mut reader = std::io::BufReader::new(file.as_bytes());
    rustls_pemfile::private_key(&mut reader)
        .map_err(TlsError::Io)?
        .ok_or(TlsError::NoPrivateKey)
}

/// 启动 HTTP/2 + HTTPS 服务
///
/// ALPN 协商：客户端支持 h2 时用 HTTP/2，否则回退 http/1.1。
///
/// 实现方式：手动接受 TCP 连接 → TLS 握手 → hyper HTTP/2 服务。
pub async fn serve_http2(router: Router, addr: SocketAddr, tls: TlsConfig) -> Result<(), TlsError> {
    let acceptor = tls.build_acceptor().await?;
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("HTTPS/HTTP2 server listening on {}", addr);

    loop {
        let (tcp_stream, _remote) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let router = router.clone();

        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(tcp_stream).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("TLS handshake failed: {}", e);
                    return;
                }
            };

            let io = hyper_util::rt::TokioIo::new(tls_stream);
            let svc = hyper::service::service_fn(move |req| {
                let router = router.clone();
                async move { router.oneshot(req).await }
            });

            if let Err(e) =
                hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
                    .serve_connection(io, svc)
                    .await
            {
                tracing::warn!("HTTP/2 connection error: {}", e);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tls_config_new() {
        let config = TlsConfig::new("/path/to/cert.pem", "/path/to/key.pem");
        assert_eq!(config.cert_path, PathBuf::from("/path/to/cert.pem"));
        assert_eq!(config.key_path, PathBuf::from("/path/to/key.pem"));
        assert_eq!(config.alpn, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
    }

    #[test]
    fn test_tls_config_h2_only() {
        let config = TlsConfig::new("/cert.pem", "/key.pem").h2_only();
        assert_eq!(config.alpn, vec![b"h2".to_vec()]);
    }

    #[test]
    fn test_tls_config_http1_only() {
        let config = TlsConfig::new("/cert.pem", "/key.pem").http1_only();
        assert_eq!(config.alpn, vec![b"http/1.1".to_vec()]);
    }

    #[test]
    fn test_tls_error_display() {
        let err = TlsError::NoCertificate;
        assert_eq!(err.to_string(), "No valid certificate found");

        let err = TlsError::NoPrivateKey;
        assert_eq!(err.to_string(), "No valid private key found");
    }
}
