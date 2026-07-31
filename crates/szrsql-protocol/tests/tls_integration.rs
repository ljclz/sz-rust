//! Phase 4.5 端到端集成测试 — TLS 1.3（rustls 集成）。
//!
//! 完整覆盖进度表 Phase 4.5 验收标准：
//! > `psql "sslmode=require"` 连接 → 验证 TLS 握手
//! > `sslmode=verify-full` → 证书验证
//! > TLS 连接成功，Wireshark 抓包确认加密
//!
//! # 测试矩阵
//!
//! | 场景 | 服务器 TLS | 客户端 SSLRequest | 客户端验证 | 预期 |
//! |------|-----------|-------------------|-----------|------|
//! | SSLRequest 被拒绝 | None | 是 | - | 服务器回 'N'，回退明文 |
//! | sslmode=require | Some | 是 | 跳过 | TLS 握手成功，pgwire 正常 |
//! | sslmode=verify-full | Some | 是 | 信任根证书 | TLS 握手成功，pgwire 正常 |
//! | verify-full 不信任 | Some | 是 | 空根证书库 | TLS 握手失败 |
//! | 服务器仍接受明文 | Some | 否 | - | 直接 Startup，pgwire 正常 |

use std::sync::Arc;
use std::time::Duration;
use szrsql_protocol::pgwire::{
    message::{
        MSG_AUTHENTICATION, MSG_BACKEND_KEY_DATA, MSG_COMMAND_COMPLETE, MSG_DATA_ROW,
        MSG_ERROR_RESPONSE, MSG_PARAMETER_STATUS, MSG_READY_FOR_QUERY, MSG_ROW_DESCRIPTION,
    },
    server::{PgwireConfig, PgwireServer},
    startup::{
        encode_special_request, encode_startup_message, StartupParams, PROTOCOL_SSL_REQUEST,
    },
    tls::TlsConfig,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

// =====================================================================
//  辅助函数
// =====================================================================

/// 寻找可用端口：从给定起始端口开始尝试。
async fn find_free_port(start: u16) -> u16 {
    for port in start..start + 50 {
        if tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
            .await
            .is_ok()
        {
            return port;
        }
    }
    panic!("no free port found in {start}..{}", start + 50);
}

/// 启动一个不带 TLS 的测试服务器，返回其监听端口。
async fn spawn_plain_server(port: u16) -> tokio::task::JoinHandle<()> {
    let config = PgwireConfig::new()
        .with_host("127.0.0.1")
        .with_port(port)
        .with_server_version("14.0-test");
    let server = PgwireServer::new(config);
    tokio::spawn(async move {
        let _ = server.serve().await;
    })
}

/// 启动一个带 TLS 的测试服务器，返回其监听端口。
async fn spawn_tls_server(port: u16, tls: TlsConfig) -> tokio::task::JoinHandle<()> {
    let config = PgwireConfig::new()
        .with_host("127.0.0.1")
        .with_port(port)
        .with_server_version("14.0-test")
        .with_tls(tls);
    let server = PgwireServer::new(config);
    tokio::spawn(async move {
        let _ = server.serve().await;
    })
}

/// 启动一个带 TLS 且强制 TLS（require_tls=true）的测试服务器。
///
/// `require_tls=true` 时，客户端必须先发送 SSLRequest 升级为 TLS 才能继续握手；
/// 直接发送明文 StartupMessage 将被服务器拒绝。
async fn spawn_tls_server_with_require_tls(
    port: u16,
    tls: TlsConfig,
) -> tokio::task::JoinHandle<()> {
    let config = PgwireConfig::new()
        .with_host("127.0.0.1")
        .with_port(port)
        .with_server_version("14.0-test")
        .with_tls(tls)
        .with_require_tls(true);
    let server = PgwireServer::new(config);
    tokio::spawn(async move {
        let _ = server.serve().await;
    })
}

/// 等待服务器就绪（可连接）。
async fn wait_for_server(port: u16) {
    for _ in 0..50 {
        if TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("server did not become ready on port {port}");
}

/// 使用 rcgen 生成自签名证书和私钥（PEM 格式），CN=localhost。
fn generate_self_signed_pem() -> (Vec<u8>, Vec<u8>) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("rcgen: failed to generate self-signed cert");
    let cert_pem = cert.cert.pem().as_bytes().to_vec();
    let key_pem = cert.key_pair.serialize_pem().as_bytes().to_vec();
    (cert_pem, key_pem)
}

/// 构造一个跳过证书验证的 `TlsConnector`（模拟 `sslmode=require`）。
fn tls_connector_no_verify() -> TlsConnector {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::{ClientConfig, DigitallySignedStruct};

    /// 永远通过证书验证的 verifier（仅用于测试，模拟 sslmode=require）。
    #[derive(Debug)]
    struct NoVerify;

    impl ServerCertVerifier for NoVerify {
        fn verify_server_cert(
            &self,
            _end_entity: &rustls::pki_types::CertificateDer<'_>,
            _intermediates: &[rustls::pki_types::CertificateDer<'_>],
            _server_name: &rustls::pki_types::ServerName<'_>,
            _ocsp_response: &[u8],
            _now: rustls::pki_types::UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &rustls::pki_types::CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &rustls::pki_types::CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            vec![
                rustls::SignatureScheme::RSA_PSS_SHA256,
                rustls::SignatureScheme::RSA_PSS_SHA384,
                rustls::SignatureScheme::RSA_PSS_SHA512,
                rustls::SignatureScheme::ED25519,
                rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
                rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            ]
        }
    }

    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerify))
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}

/// 构造一个使用给定根证书的 `TlsConnector`（模拟 `sslmode=verify-full`）。
fn tls_connector_with_root(root_cert_pem: &[u8]) -> TlsConnector {
    use rustls::pki_types::pem::PemObject;
    use rustls::{ClientConfig, RootCertStore};

    let mut root_store = RootCertStore::empty();
    let roots: Vec<_> = rustls::pki_types::CertificateDer::pem_slice_iter(root_cert_pem)
        .collect::<Result<_, _>>()
        .expect("failed to parse root cert PEM");
    for root in roots {
        root_store.add(root).expect("failed to add root cert");
    }

    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}

/// 构造一个空根证书库的 `TlsConnector`（模拟不信任任何证书，用于负面测试）。
fn tls_connector_empty_root() -> TlsConnector {
    use rustls::{ClientConfig, RootCertStore};

    let root_store = RootCertStore::empty();
    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}

/// 读取直到收到 ReadyForQuery 消息，返回所有收到的字节。
async fn read_until_ready_for_query<R: AsyncReadExt + Unpin>(stream: &mut R) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await.expect("read should succeed");
        if n == 0 {
            panic!("connection closed before ReadyForQuery");
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_last_message_start(&buf, b'Z') {
            if buf.len() >= pos + 6 {
                let length =
                    i32::from_be_bytes([buf[pos + 1], buf[pos + 2], buf[pos + 3], buf[pos + 4]]);
                if length == 5 && buf.len() >= pos + 1 + length as usize {
                    return buf;
                }
            }
        }
    }
}

/// 在缓冲区中反向查找指定类型的消息起始位置。
fn find_last_message_start(buf: &[u8], msg_type: u8) -> Option<usize> {
    if buf.len() < 6 {
        return None;
    }
    (0..=buf.len() - 6).rev().find(|&i| buf[i] == msg_type)
}

/// 解析后端响应字节流，返回按顺序的消息类型列表。
fn parse_message_types(buf: &[u8]) -> Vec<u8> {
    let mut types = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let msg_type = buf[i];
        let msg_len = i32::from_be_bytes([buf[i + 1], buf[i + 2], buf[i + 3], buf[i + 4]]) as usize;
        types.push(msg_type);
        i += 1 + msg_len;
    }
    types
}

/// 发送 StartupMessage。
async fn send_startup<W: AsyncWriteExt + Unpin>(stream: &mut W) {
    let params = StartupParams::new()
        .with("user", "test_user")
        .with("database", "test_db");
    let startup_bytes = encode_startup_message(&params);
    stream
        .write_all(&startup_bytes)
        .await
        .expect("write startup");
    stream.flush().await.expect("flush");
}

/// 发送 Query: SELECT 1。
async fn send_select_one<W: AsyncWriteExt + Unpin>(stream: &mut W) {
    let sql = "SELECT 1";
    let mut query_msg = Vec::new();
    query_msg.push(b'Q');
    query_msg.extend_from_slice(&(sql.len() as i32 + 4 + 1).to_be_bytes());
    query_msg.extend_from_slice(sql.as_bytes());
    query_msg.push(0);
    stream.write_all(&query_msg).await.expect("write query");
    stream.flush().await.expect("flush");
}

// =====================================================================
//  端到端测试
// =====================================================================

/// 验收场景 1：服务器未配置 TLS，客户端发送 SSLRequest，服务器应回 'N' 后回退明文。
#[tokio::test]
async fn test_e2e_ssl_request_refused_without_tls_returns_n() {
    let port = find_free_port(16032).await;
    let _server = spawn_plain_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // 发送 SSLRequest
    let ssl_bytes = encode_special_request(PROTOCOL_SSL_REQUEST);
    stream
        .write_all(&ssl_bytes)
        .await
        .expect("write SSLRequest");
    stream.flush().await.expect("flush");

    // 服务器应回单字节 'N' 表示不支持 SSL
    let mut resp = [0u8; 1];
    stream.read_exact(&mut resp).await.expect("read 'N'");
    assert_eq!(
        resp[0], b'N',
        "server without TLS should respond with 'N' to SSLRequest"
    );

    // 客户端回退明文，继续发送 StartupMessage
    send_startup(&mut stream).await;
    let response = read_until_ready_for_query(&mut stream).await;
    let types = parse_message_types(&response);

    // 验证明文握手成功
    assert_eq!(types[0], MSG_AUTHENTICATION);
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}

/// 验收场景 2：sslmode=require — 服务器配置 TLS，客户端发送 SSLRequest，服务器回 'S'，
/// 执行 TLS 握手，握手成功后通过加密通道完成 pgwire 启动握手。
#[tokio::test]
async fn test_e2e_sslmode_require_tls_handshake_success() {
    let port = find_free_port(16132).await;
    let (cert_pem, key_pem) = generate_self_signed_pem();
    let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem, None).expect("TlsConfig from PEM");
    let _server = spawn_tls_server(port, tls_config).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // 发送 SSLRequest
    let ssl_bytes = encode_special_request(PROTOCOL_SSL_REQUEST);
    stream
        .write_all(&ssl_bytes)
        .await
        .expect("write SSLRequest");
    stream.flush().await.expect("flush");

    // 服务器应回 'S' 表示支持 SSL
    let mut resp = [0u8; 1];
    stream.read_exact(&mut resp).await.expect("read 'S'");
    assert_eq!(resp[0], b'S', "server with TLS should respond with 'S'");

    // 执行 TLS 握手（sslmode=require，跳过证书验证）
    let connector = tls_connector_no_verify();
    let server_name =
        rustls::pki_types::ServerName::try_from("localhost").expect("valid server name");
    let mut tls_stream = connector
        .connect(server_name, stream)
        .await
        .expect("TLS handshake should succeed");

    // 通过加密通道发送 StartupMessage
    send_startup(&mut tls_stream).await;
    let response = read_until_ready_for_query(&mut tls_stream).await;
    let types = parse_message_types(&response);

    // 验证加密通道上的握手成功
    assert_eq!(
        types[0], MSG_AUTHENTICATION,
        "first message should be AuthenticationOk"
    );
    for t in &types[1..types.len() - 2] {
        assert_eq!(*t, MSG_PARAMETER_STATUS, "expected ParameterStatus");
    }
    assert_eq!(types[types.len() - 2], MSG_BACKEND_KEY_DATA);
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}

/// 验收场景 3：sslmode=require 完整流程 — TLS 握手 → Startup → SELECT 1 → 结果集。
#[tokio::test]
async fn test_e2e_sslmode_require_select_one_returns_result_set() {
    let port = find_free_port(16232).await;
    let (cert_pem, key_pem) = generate_self_signed_pem();
    let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem, None).expect("TlsConfig from PEM");
    let _server = spawn_tls_server(port, tls_config).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // SSLRequest → 'S' → TLS 握手
    let ssl_bytes = encode_special_request(PROTOCOL_SSL_REQUEST);
    stream
        .write_all(&ssl_bytes)
        .await
        .expect("write SSLRequest");
    stream.flush().await.expect("flush");
    let mut resp = [0u8; 1];
    stream.read_exact(&mut resp).await.expect("read 'S'");
    assert_eq!(resp[0], b'S');

    let connector = tls_connector_no_verify();
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let mut tls_stream = connector
        .connect(server_name, stream)
        .await
        .expect("TLS handshake");

    // Startup 握手
    send_startup(&mut tls_stream).await;
    let _handshake = read_until_ready_for_query(&mut tls_stream).await;

    // 通过加密通道发送 SELECT 1
    send_select_one(&mut tls_stream).await;
    let response = read_until_ready_for_query(&mut tls_stream).await;
    let types = parse_message_types(&response);

    // 验证结果集：RowDescription + DataRow + CommandComplete + ReadyForQuery
    assert_eq!(types[0], MSG_ROW_DESCRIPTION);
    assert_eq!(types[1], MSG_DATA_ROW);
    assert_eq!(types[2], MSG_COMMAND_COMPLETE);
    assert_eq!(types[3], MSG_READY_FOR_QUERY);
}

/// 验收场景 4：sslmode=verify-full — 客户端使用自签名证书作为根证书，
/// 验证服务器证书链，握手成功。
#[tokio::test]
async fn test_e2e_sslmode_verify_full_with_trusted_root_succeeds() {
    let port = find_free_port(16332).await;
    let (cert_pem, key_pem) = generate_self_signed_pem();
    let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem, None).expect("TlsConfig from PEM");
    let _server = spawn_tls_server(port, tls_config).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // SSLRequest → 'S'
    let ssl_bytes = encode_special_request(PROTOCOL_SSL_REQUEST);
    stream
        .write_all(&ssl_bytes)
        .await
        .expect("write SSLRequest");
    stream.flush().await.expect("flush");
    let mut resp = [0u8; 1];
    stream.read_exact(&mut resp).await.expect("read 'S'");
    assert_eq!(resp[0], b'S');

    // 使用同一自签名证书作为根证书进行验证（verify-full）
    let connector = tls_connector_with_root(&cert_pem);
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let mut tls_stream = connector
        .connect(server_name, stream)
        .await
        .expect("TLS handshake with verify-full should succeed when root is trusted");

    // 验证加密通道上的 pgwire 协议正常工作
    send_startup(&mut tls_stream).await;
    let response = read_until_ready_for_query(&mut tls_stream).await;
    let types = parse_message_types(&response);
    assert_eq!(types[0], MSG_AUTHENTICATION);
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}

/// 验收场景 5：sslmode=verify-full 拒绝不信任的证书 — 客户端使用空根证书库，
/// 服务器证书无法验证，TLS 握手应失败。
#[tokio::test]
async fn test_e2e_sslmode_verify_full_rejects_untrusted_cert() {
    let port = find_free_port(16432).await;
    let (cert_pem, key_pem) = generate_self_signed_pem();
    let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem, None).expect("TlsConfig from PEM");
    let _server = spawn_tls_server(port, tls_config).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // SSLRequest → 'S'
    let ssl_bytes = encode_special_request(PROTOCOL_SSL_REQUEST);
    stream
        .write_all(&ssl_bytes)
        .await
        .expect("write SSLRequest");
    stream.flush().await.expect("flush");
    let mut resp = [0u8; 1];
    stream.read_exact(&mut resp).await.expect("read 'S'");
    assert_eq!(resp[0], b'S');

    // 使用空根证书库进行验证，握手应失败
    let connector = tls_connector_empty_root();
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let result = connector.connect(server_name, stream).await;
    assert!(
        result.is_err(),
        "TLS handshake should fail when server cert is not trusted"
    );
}

/// 验收场景 6：服务器配置了 TLS 但客户端不发送 SSLRequest，直接发送 StartupMessage，
/// 服务器应接受明文连接（兼容旧客户端）。
#[tokio::test]
async fn test_e2e_tls_server_accepts_plaintext_startup() {
    let port = find_free_port(16532).await;
    let (cert_pem, key_pem) = generate_self_signed_pem();
    let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem, None).expect("TlsConfig from PEM");
    let _server = spawn_tls_server(port, tls_config).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // 直接发送 StartupMessage（不经过 SSLRequest）
    send_startup(&mut stream).await;
    let response = read_until_ready_for_query(&mut stream).await;
    let types = parse_message_types(&response);

    // 验证明文握手成功
    assert_eq!(types[0], MSG_AUTHENTICATION);
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);

    // 验证明文查询也正常
    send_select_one(&mut stream).await;
    let response = read_until_ready_for_query(&mut stream).await;
    let types = parse_message_types(&response);
    assert_eq!(types[0], MSG_ROW_DESCRIPTION);
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}

/// 辅助测试：验证 `TlsStream<TcpStream>` 类型可被泛型 stream 函数接受。
///
/// 这确保 `handle_full_connection<S: AsyncRead + AsyncWrite + Unpin>` 能正确接受
/// TLS 升级后的 stream 类型。
#[tokio::test]
async fn test_tls_stream_satisfies_async_read_write_unpin() {
    // 类型断言：TlsStream<TcpStream> 实现 AsyncRead + AsyncWrite + Unpin
    fn assert_stream<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(_: S) {}

    let port = find_free_port(16632).await;
    let (cert_pem, key_pem) = generate_self_signed_pem();
    let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem, None).expect("TlsConfig from PEM");
    let _server = spawn_tls_server(port, tls_config).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    let ssl_bytes = encode_special_request(PROTOCOL_SSL_REQUEST);
    stream
        .write_all(&ssl_bytes)
        .await
        .expect("write SSLRequest");
    stream.flush().await.expect("flush");
    let mut resp = [0u8; 1];
    stream.read_exact(&mut resp).await.expect("read 'S'");

    let connector = tls_connector_no_verify();
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let tls_stream = connector
        .connect(server_name, stream)
        .await
        .expect("TLS handshake");

    // 类型断言：如果 TlsStream<TcpStream> 不满足约束，编译会失败
    assert_stream(tls_stream);
}

// =====================================================================
//  require_tls 集成测试
// =====================================================================

/// 验收场景 7：require_tls=true 时，客户端直接发送明文 StartupMessage（不经过
/// SSLRequest），服务器应回复 ErrorResponse（'E'）并包含 "SSLRequired" 文案，
/// 随后关闭连接，拒绝明文降级。
#[tokio::test]
async fn test_e2e_require_tls_rejects_plaintext_startup() {
    let port = find_free_port(16732).await;
    let (cert_pem, key_pem) = generate_self_signed_pem();
    let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem, None).expect("TlsConfig from PEM");
    let _server = spawn_tls_server_with_require_tls(port, tls_config).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // 直接发送明文 StartupMessage（不发送 SSLRequest）
    send_startup(&mut stream).await;

    // 读取服务器响应：应为 ErrorResponse（'E'）
    let mut buf = [0u8; 256];
    let n = stream.read(&mut buf).await.expect("read should succeed");
    assert!(n > 0, "server should respond with ErrorResponse before closing");

    // 首字节应为 'E'（MSG_ERROR_RESPONSE）
    assert_eq!(
        buf[0], MSG_ERROR_RESPONSE,
        "require_tls=true should respond with ErrorResponse to plaintext StartupMessage"
    );

    // 解析消息长度（紧跟 type 后的 4 字节 i32）
    let length = i32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    assert!(length >= 4, "error response length should be at least 4");

    // 整个消息体应在一次读取中（type=1 + length=4 + payload）
    let total_len = 1 + length;
    assert!(
        n >= total_len,
        "expected at least {total_len} bytes, got {n}"
    );

    // payload 中应包含 "SSLRequired" 文案（ASCII 字节匹配）
    let payload = &buf[5..total_len];
    let payload_str = String::from_utf8_lossy(payload);
    assert!(
        payload_str.contains("SSLRequired"),
        "error response should contain 'SSLRequired', got: {payload_str}"
    );

    // 继续读取，服务器应关闭连接（返回 0 表示 EOF）
    let n2 = stream.read(&mut buf).await.expect("read after error");
    assert_eq!(n2, 0, "server should close connection after sending error");
}

/// 验收场景 8：require_tls=true 时，客户端正常发送 SSLRequest → 'S' → TLS 握手，
/// 服务器应接受 TLS 连接并完成 pgwire 启动握手，证明 require_tls 仅拒绝明文降级，
/// 不影响合法的 TLS 客户端。
#[tokio::test]
async fn test_e2e_require_tls_accepts_tls_handshake() {
    let port = find_free_port(16832).await;
    let (cert_pem, key_pem) = generate_self_signed_pem();
    let tls_config = TlsConfig::from_pem(&cert_pem, &key_pem, None).expect("TlsConfig from PEM");
    let _server = spawn_tls_server_with_require_tls(port, tls_config).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // 发送 SSLRequest
    let ssl_bytes = encode_special_request(PROTOCOL_SSL_REQUEST);
    stream
        .write_all(&ssl_bytes)
        .await
        .expect("write SSLRequest");
    stream.flush().await.expect("flush");

    // 服务器应回 'S' 表示支持 SSL
    let mut resp = [0u8; 1];
    stream.read_exact(&mut resp).await.expect("read 'S'");
    assert_eq!(resp[0], b'S', "server with require_tls should accept SSLRequest");

    // 执行 TLS 握手（sslmode=require，跳过证书验证）
    let connector = tls_connector_no_verify();
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let mut tls_stream = connector
        .connect(server_name, stream)
        .await
        .expect("TLS handshake should succeed even with require_tls=true");

    // 通过加密通道发送 StartupMessage，握手应成功
    send_startup(&mut tls_stream).await;
    let response = read_until_ready_for_query(&mut tls_stream).await;
    let types = parse_message_types(&response);

    assert_eq!(types[0], MSG_AUTHENTICATION, "first message should be AuthenticationOk");
    assert_eq!(
        types[types.len() - 1], MSG_READY_FOR_QUERY,
        "last message should be ReadyForQuery"
    );

    // 验证加密通道上的查询也正常
    send_select_one(&mut tls_stream).await;
    let response = read_until_ready_for_query(&mut tls_stream).await;
    let types = parse_message_types(&response);
    assert_eq!(types[0], MSG_ROW_DESCRIPTION);
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}
