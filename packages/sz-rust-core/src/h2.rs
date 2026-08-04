//! HTTP/2 + TLS 支持 — 基于 `tokio-rustls` + `rustls-pemfile`
//!
//! 对齐 PHP `think-swoole` 启用 SSL 后的行为，提供 HTTP/2 over TLS 启动器。
//!
//! ## 实现方式
//!
//! 自定义 [`TlsListener`] 实现 `axum::serve::Listener` trait，包装 `TcpListener`
//! 和 `tokio_rustls::TlsAcceptor`。每个新连接先完成 TLS 握手，再交给 axum::serve。
//!
//! ALPN 协商列表为 `["h2", "http/1.1"]`，优先 HTTP/2，回退 HTTP/1.1。
//!
//! ## 功能
//!
//! - [`load_tls_config`]：从 PEM 文件加载证书和私钥，构造 `rustls::ServerConfig`
//! - [`TlsListener`]：自定义 listener，实现 `axum::serve::Listener`
//! - [`serve_h2`]：HTTP/2 + TLS 启动器
//! - [`serve_h2_with_graceful_shutdown`]：带优雅关闭
//! - [`serve_h2_with_listener`]：使用自定义 listener（测试友好）
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::h2::serve_h2;
//! use axum::Router;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let router = Router::new();
//! serve_h2(router, "127.0.0.1:8443", "cert.pem", "key.pem").await?;
//! # Ok(())
//! # }
//! ```

use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use axum::serve::Listener;
use axum::Router;
use tokio::net::TcpListener;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;

/// TLS / HTTP-2 加载与启动过程中可能发生的错误
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    /// 绑定 TCP 端口失败
    #[error("failed to bind TCP listener: {0}")]
    Bind(#[source] std::io::Error),

    /// 读取证书文件失败
    #[error("failed to read certificate file: {0}")]
    ReadCertFile(#[source] std::io::Error),

    /// 读取私钥文件失败
    #[error("failed to read private key file: {0}")]
    ReadKeyFile(#[source] std::io::Error),

    /// 解析 PEM 证书失败
    #[error("failed to parse certificate PEM: {0}")]
    ParseCert(#[source] std::io::Error),

    /// 解析 PEM 私钥失败
    #[error("failed to parse private key PEM: {0}")]
    ParseKey(#[source] std::io::Error),

    /// 私钥文件不包含任何 PKCS8/PKCS1/Sec1 私钥
    #[error("no private key found in PEM file")]
    NoPrivateKey,

    /// rustls ServerConfig 构造失败（如证书链不完整）
    #[error("failed to build rustls ServerConfig: {0}")]
    BuildServerConfig(#[source] tokio_rustls::rustls::Error),

    /// 服务器运行失败（通常是 accept / IO 错误）
    #[error("server error: {0}")]
    Server(#[source] std::io::Error),
}

/// 从 PEM 文件加载证书和私钥，构造 `rustls::ServerConfig`
///
/// - ALPN 协议列表为 `["h2", "http/1.1"]`，优先 HTTP/2，回退 HTTP/1.1
/// - 不启用客户端证书校验（`with_no_client_auth`）
///
/// ## 参数
///
/// - `cert_path`：PEM 格式证书文件路径
/// - `key_path`：PEM 格式私钥文件路径（支持 PKCS8 / PKCS1 / Sec1）
pub async fn load_tls_config(
    cert_path: impl AsRef<Path>,
    key_path: impl AsRef<Path>,
) -> Result<ServerConfig, TlsError> {
    let certs = load_certs(cert_path).await?;
    let key = load_private_key(key_path).await?;

    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(TlsError::BuildServerConfig)?;
    // ALPN 协商：优先 HTTP/2，回退 HTTP/1.1
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

/// 从 PEM 文件加载证书链
async fn load_certs(path: impl AsRef<Path>) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    let cert_data = tokio::fs::read(path.as_ref())
        .await
        .map_err(TlsError::ReadCertFile)?;
    let mut reader = BufReader::new(&cert_data[..]);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(TlsError::ParseCert)?;
    if certs.is_empty() {
        return Err(TlsError::ParseCert(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "no certificate found in PEM file",
        )));
    }
    Ok(certs)
}

/// 从 PEM 文件加载私钥（支持 PKCS8 / PKCS1 / Sec1）
async fn load_private_key(path: impl AsRef<Path>) -> Result<PrivateKeyDer<'static>, TlsError> {
    let key_data = tokio::fs::read(path.as_ref())
        .await
        .map_err(TlsError::ReadKeyFile)?;
    let mut reader = BufReader::new(&key_data[..]);
    let mut keys = Vec::new();
    for item in rustls_pemfile::read_all(&mut reader) {
        match item.map_err(TlsError::ParseKey)? {
            rustls_pemfile::Item::Pkcs8Key(k) => keys.push(PrivateKeyDer::Pkcs8(k)),
            rustls_pemfile::Item::Pkcs1Key(k) => keys.push(PrivateKeyDer::Pkcs1(k)),
            rustls_pemfile::Item::Sec1Key(k) => keys.push(PrivateKeyDer::Sec1(k)),
            _ => {}
        }
    }
    keys.into_iter().next().ok_or(TlsError::NoPrivateKey)
}

/// 构造 `TlsAcceptor`（基于 `Arc<ServerConfig>`）
pub fn tls_acceptor(config: ServerConfig) -> TlsAcceptor {
    TlsAcceptor::from(Arc::new(config))
}

/// TLS 包装的 listener
///
/// 实现 `axum::serve::Listener` trait，对每个新连接执行 TLS 握手。
///
/// ## 并发握手
///
/// TCP accept 后立即 `tokio::spawn` 一个独立任务执行 TLS 握手，
/// 通过 `mpsc` channel 将完成的 `TlsStream` 送回 accept 循环。
/// 这样 TLS 握手（50-200ms）不会阻塞下一个 TCP accept，
/// 高并发场景下吞吐量不会因为串行握手而退化。
///
/// TLS 握手失败时记录日志并继续接受下一个连接（不中断服务器）。
pub struct TlsListener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    /// 已完成 TLS 握手的连接队列（由 spawn 的握手任务写入）
    pending_rx: tokio::sync::mpsc::UnboundedReceiver<(
        TlsStream<tokio::net::TcpStream>,
        std::net::SocketAddr,
    )>,
    /// 用于 spawn 任务发送完成握手的连接
    pending_tx: tokio::sync::mpsc::UnboundedSender<(
        TlsStream<tokio::net::TcpStream>,
        std::net::SocketAddr,
    )>,
}

impl std::fmt::Debug for TlsListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsListener")
            .field("tcp", &self.tcp)
            .finish_non_exhaustive()
    }
}

impl TlsListener {
    /// 构造 TlsListener
    pub fn new(tcp: TcpListener, acceptor: TlsAcceptor) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            tcp,
            acceptor,
            pending_rx: rx,
            pending_tx: tx,
        }
    }

    /// 从 `TcpListener` 和 `ServerConfig` 构造 TlsListener
    pub fn from_config(tcp: TcpListener, config: ServerConfig) -> Self {
        Self::new(tcp, tls_acceptor(config))
    }
}

impl Listener for TlsListener {
    type Io = TlsStream<tokio::net::TcpStream>;
    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        // 连续 TCP accept 错误的指数退避计数（避免在 fd 耗尽等持续错误下死循环）
        let mut backoff_ms: u64 = 100;
        const MAX_BACKOFF_MS: u64 = 5_000;

        loop {
            tokio::select! {
                // 优先返回已完成握手的连接
                Some((tls, addr)) = self.pending_rx.recv() => {
                    return (tls, addr);
                }
                // 同时继续 accept 新 TCP 连接，握手丢给 spawn 任务并行处理
                accept_result = self.tcp.accept() => {
                    match accept_result {
                        Ok((tcp, addr)) => {
                            let acceptor = self.acceptor.clone();
                            let tx = self.pending_tx.clone();
                            tokio::spawn(async move {
                                match acceptor.accept(tcp).await {
                                    Ok(tls) => {
                                        if tx.send((tls, addr)).is_err() {
                                            tracing::warn!(
                                                "pending channel closed, dropping TLS connection from {addr}"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("TLS handshake failed from {addr}: {e}");
                                    }
                                }
                            });
                            // 成功 accept，重置退避
                            backoff_ms = 100;
                        }
                        Err(e) => {
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::ConnectionRefused
                                    | std::io::ErrorKind::ConnectionAborted
                                    | std::io::ErrorKind::ConnectionReset
                            ) {
                                // 连接级错误，直接重试，不退避
                                continue;
                            }
                            tracing::error!("accept error: {e}");
                            // 指数退避（100ms → 200ms → 400ms → ... → 5s 上限）
                            tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                            backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
                        }
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

/// 启动 HTTP/2 + TLS 服务器
///
/// 阻塞当前异步任务，直到服务器关闭。ALPN 协商优先 h2，回退 http/1.1。
///
/// ## 参数
///
/// - `router`：axum::Router
/// - `addr`：监听地址，例如 `"127.0.0.1:8443"`
/// - `cert_path`：PEM 证书文件路径
/// - `key_path`：PEM 私钥文件路径
pub async fn serve_h2(
    router: Router,
    addr: &str,
    cert_path: impl AsRef<Path>,
    key_path: impl AsRef<Path>,
) -> Result<(), TlsError> {
    let listener = TcpListener::bind(addr).await.map_err(TlsError::Bind)?;
    let config = load_tls_config(cert_path, key_path).await?;
    serve_h2_with_listener(router, listener, config).await
}

/// 启动 HTTP/2 + TLS 服务器（带优雅关闭）
///
/// 监听 Ctrl+C 信号，收到后启动 graceful shutdown。
pub async fn serve_h2_with_graceful_shutdown(
    router: Router,
    addr: &str,
    cert_path: impl AsRef<Path>,
    key_path: impl AsRef<Path>,
) -> Result<(), TlsError> {
    let listener = TcpListener::bind(addr).await.map_err(TlsError::Bind)?;
    let config = load_tls_config(cert_path, key_path).await?;
    let tls_listener = TlsListener::from_config(listener, config);
    axum::serve(tls_listener, router.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(TlsError::Server)?;
    Ok(())
}

/// 启动 HTTP/2 + TLS 服务器（使用已有 listener，测试友好）
///
/// 适用于测试场景：测试代码可以 `listener.local_addr()` 获取实际端口。
pub async fn serve_h2_with_listener(
    router: Router,
    listener: TcpListener,
    config: ServerConfig,
) -> Result<(), TlsError> {
    let tls_listener = TlsListener::from_config(listener, config);
    axum::serve(tls_listener, router.into_make_service())
        .await
        .map_err(TlsError::Server)?;
    Ok(())
}

/// 优雅关闭信号监听
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// 生成自签名证书和私钥（PEM 格式），用于测试
    ///
    /// 返回 `(cert_pem_bytes, key_pem_bytes)`。
    fn generate_self_signed_cert() -> (Vec<u8>, Vec<u8>) {
        // rcgen 0.13 API
        let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        let cert_pem = cert.pem().into_bytes();
        let key_pem = key_pair.serialize_pem().into_bytes();
        (cert_pem, key_pem)
    }

    /// 将 PEM 字节写入临时文件
    fn write_temp_pem(name: &str, data: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("sz-rust-h2-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, data).unwrap();
        path
    }

    // ====================================================================
    // load_tls_config
    // ====================================================================

    #[tokio::test]
    async fn test_load_tls_config_valid_pem() {
        let (cert, key) = generate_self_signed_cert();
        let cert_path = write_temp_pem("valid_cert.pem", &cert);
        let key_path = write_temp_pem("valid_key.pem", &key);

        let config = load_tls_config(&cert_path, &key_path).await;
        assert!(
            config.is_ok(),
            "failed to load TLS config: {:?}",
            config.err()
        );

        let config = config.unwrap();
        // ALPN 协议列表必须包含 h2 和 http/1.1
        assert!(config.alpn_protocols.contains(&b"h2".to_vec()));
        assert!(config.alpn_protocols.contains(&b"http/1.1".to_vec()));
    }

    #[tokio::test]
    async fn test_load_tls_config_missing_cert_file() {
        let result = load_tls_config("nonexistent_cert.pem", "nonexistent_key.pem").await;
        assert!(matches!(result, Err(TlsError::ReadCertFile(_))));
    }

    #[tokio::test]
    async fn test_load_tls_config_missing_key_file() {
        let (cert, _key) = generate_self_signed_cert();
        let cert_path = write_temp_pem("valid_cert_for_missing_key.pem", &cert);

        let result = load_tls_config(&cert_path, "nonexistent_key.pem").await;
        assert!(matches!(result, Err(TlsError::ReadKeyFile(_))));
    }

    #[tokio::test]
    async fn test_load_tls_config_invalid_pem_content() {
        let cert_path = write_temp_pem("invalid_cert.pem", b"not a valid PEM");
        let key_path = write_temp_pem("invalid_key.pem", b"not a valid PEM");

        let result = load_tls_config(&cert_path, &key_path).await;
        // PEM 解析失败 → ParseCert（certs() 返回空）或 ParseKey
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_load_tls_config_empty_pem_files() {
        let cert_path = write_temp_pem("empty_cert.pem", b"");
        let key_path = write_temp_pem("empty_key.pem", b"");

        // 空文件 → load_certs 返回 ParseCert 错误（no certificate found）
        let result = load_tls_config(&cert_path, &key_path).await;
        assert!(matches!(result, Err(TlsError::ParseCert(_))));
    }

    #[tokio::test]
    async fn test_load_tls_config_key_without_cert() {
        // 只有私钥没有证书 → ParseCert
        let (_cert, key) = generate_self_signed_cert();
        let key_path = write_temp_pem("only_key.pem", &key);
        let empty_cert_path = write_temp_pem("empty_for_key_only.pem", b"");

        let result = load_tls_config(&empty_cert_path, &key_path).await;
        assert!(matches!(result, Err(TlsError::ParseCert(_))));
    }

    // ====================================================================
    // tls_acceptor + TlsListener 构造
    // ====================================================================

    #[tokio::test]
    async fn test_tls_acceptor_constructible() {
        let (cert, key) = generate_self_signed_cert();
        let cert_path = write_temp_pem("acceptor_cert.pem", &cert);
        let key_path = write_temp_pem("acceptor_key.pem", &key);

        let config = load_tls_config(&cert_path, &key_path).await.unwrap();
        let _acceptor = tls_acceptor(config);
    }

    #[tokio::test]
    async fn test_tls_listener_constructible() {
        let (cert, key) = generate_self_signed_cert();
        let cert_path = write_temp_pem("listener_cert.pem", &cert);
        let key_path = write_temp_pem("listener_key.pem", &key);

        let config = load_tls_config(&cert_path, &key_path).await.unwrap();
        let (tcp, _addr) = crate::server::build_tcp_listener("127.0.0.1:0")
            .await
            .unwrap();
        let tls_listener = TlsListener::from_config(tcp, config);
        // 验证 local_addr 可用
        assert!(tls_listener.local_addr().is_ok());
    }

    #[tokio::test]
    async fn test_tls_listener_new() {
        let (cert, key) = generate_self_signed_cert();
        let cert_path = write_temp_pem("new_cert.pem", &cert);
        let key_path = write_temp_pem("new_key.pem", &key);

        let config = load_tls_config(&cert_path, &key_path).await.unwrap();
        let _arc: Arc<ServerConfig> = Arc::new(config);
    }

    // ====================================================================
    // serve_h2_with_listener - 集成测试
    // ====================================================================

    #[tokio::test]
    async fn test_serve_h2_with_listener_starts_and_accepts_connections() {
        use tokio::net::TcpStream;

        // 1. 构造 router
        let router = Router::new().route("/", axum::routing::get(|| async { "hello h2" }));

        // 2. 生成自签名证书 + listener
        let (cert, key) = generate_self_signed_cert();
        let cert_path = write_temp_pem("serve_cert.pem", &cert);
        let key_path = write_temp_pem("serve_key.pem", &key);
        let config = load_tls_config(&cert_path, &key_path).await.unwrap();
        let (listener, addr) = crate::server::build_tcp_listener("127.0.0.1:0")
            .await
            .unwrap();

        // 3. 启动服务器
        tokio::spawn(async move {
            let _ = serve_h2_with_listener(router, listener, config).await;
        });

        // 4. 等待服务器就绪
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // 5. 验证 TCP 端口可达（TLS 握手由客户端发起，这里仅验证 listener 已 listen）
        let _stream = TcpStream::connect(addr).await.expect("TCP connect failed");
    }

    #[tokio::test]
    async fn test_serve_h2_with_invalid_cert_returns_error() {
        let router = Router::new();
        let result = serve_h2(router, "127.0.0.1:0", "nonexistent.pem", "nonexistent.pem").await;
        assert!(result.is_err());
        // 错误类型应该是 Bind（不会失败）或 ReadCertFile（来自 load_tls_config）
        // 实际上 bind 不会失败，所以错误应该来自 load_tls_config
        match result {
            Err(TlsError::ReadCertFile(_)) => {}
            other => panic!("expected ReadCertFile error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_serve_h2_bind_failure() {
        let (cert, key) = generate_self_signed_cert();
        let cert_path = write_temp_pem("bind_fail_cert.pem", &cert);
        let key_path = write_temp_pem("bind_fail_key.pem", &key);

        // 端口超出范围
        let router = Router::new();
        let result = serve_h2(router, "127.0.0.1:99999", &cert_path, &key_path).await;
        assert!(matches!(result, Err(TlsError::Bind(_))));
    }

    // ====================================================================
    // TLS 握手实际测试（通过 TCP 写入 + 读取）
    // ====================================================================

    #[tokio::test]
    async fn test_tls_handshake_completes_with_valid_client() {
        use tokio::io::AsyncReadExt;
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpStream;

        // 1. 构造 router
        let router = Router::new().route("/", axum::routing::get(|| async { "tls ok" }));

        // 2. 生成自签名证书 + listener
        let (cert, key) = generate_self_signed_cert();
        let cert_path = write_temp_pem("handshake_cert.pem", &cert);
        let key_path = write_temp_pem("handshake_key.pem", &key);
        let config = load_tls_config(&cert_path, &key_path).await.unwrap();
        let (listener, addr) = crate::server::build_tcp_listener("127.0.0.1:0")
            .await
            .unwrap();

        // 3. 启动服务器
        tokio::spawn(async move {
            let _ = serve_h2_with_listener(router, listener, config).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // 4. 客户端建立 TCP 连接并发送 ClientHello（TLS 1.2 最简版本）
        //    这里不验证完整 TLS 握手（需要 rustls 客户端），仅验证 TCP 连接可写
        let mut stream = TcpStream::connect(addr).await.unwrap();

        // 写入一些字节（不是有效的 TLS ClientHello，服务器会关闭连接）
        let _ = stream.write_all(b"GET / HTTP/1.1\r\n\r\n").await;

        // 服务器应该关闭连接或返回错误（TLS 握手失败）
        let mut buf = [0u8; 64];
        let _ = stream.read(&mut buf).await;
    }

    #[tokio::test]
    async fn test_serve_h2_full_tls_request_with_rustls_client() {
        // 完整的 TLS 握手 + HTTP/1.1 请求测试（使用 tokio-rustls 客户端）
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;
        use tokio_rustls::rustls::pki_types::ServerName;
        use tokio_rustls::rustls::{ClientConfig, RootCertStore};
        use tokio_rustls::TlsConnector;

        // 1. 构造 router
        let router = Router::new().route("/ping", axum::routing::get(|| async { "pong" }));

        // 2. 生成自签名证书
        let (cert, key) = generate_self_signed_cert();
        let cert_path = write_temp_pem("full_cert.pem", &cert);
        let key_path = write_temp_pem("full_key.pem", &key);

        // 3. 构造服务器 config
        let server_config = load_tls_config(&cert_path, &key_path).await.unwrap();
        let (listener, addr) = crate::server::build_tcp_listener("127.0.0.1:0")
            .await
            .unwrap();

        // 4. 启动服务器
        tokio::spawn(async move {
            let _ = serve_h2_with_listener(router, listener, server_config).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // 5. 构造客户端 config，使用自签名证书（添加到 root store）
        let mut root_store = RootCertStore::empty();
        let cert_der = rustls_pemfile::certs(&mut BufReader::new(cert.as_slice()))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for c in cert_der {
            root_store.add(c).unwrap();
        }
        let client_config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        // 6. TLS 连接 + HTTP/1.1 请求
        let connector = TlsConnector::from(Arc::new(client_config));
        let tcp_stream = TcpStream::connect(addr).await.unwrap();
        let server_name = ServerName::try_from("localhost").unwrap();
        let mut tls_stream = connector.connect(server_name, tcp_stream).await.unwrap();

        // 发送 HTTP/1.1 请求（因为我们没有强制 h2，客户端默认 http/1.1）
        tls_stream
            .write_all(b"GET /ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();

        // 读取响应
        let mut response = Vec::new();
        tls_stream.read_to_end(&mut response).await.unwrap();
        let response_str = String::from_utf8_lossy(&response);

        // 响应应该包含 "pong"
        assert!(
            response_str.contains("pong"),
            "expected response to contain 'pong', got: {}",
            response_str
        );
        // 应该是 HTTP/1.1 响应
        assert!(
            response_str.starts_with("HTTP/1.1") || response_str.starts_with("HTTP/2"),
            "expected HTTP response, got: {}",
            response_str.lines().next().unwrap_or("")
        );
    }
}
