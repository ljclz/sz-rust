// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 文件上传增强集成测试（v1.8.0 P2-2.4）
//!
//! 验证 `v18-upload` feature gate 下：
//! 1. MultipartConfig 校验（大小/扩展名/MIME）
//! 2. MultipartFile 文件名安全校验（路径遍历/控制字符）
//! 3. StorageBackend + LocalStorage 存储操作
//! 4. 上传大小限制中间件（Content-Length 检查）
//! 5. 增强上传 handler（端到端 multipart 上传）

#![cfg(feature = "v18-upload")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware;
use axum::routing::post;
use axum::Router;
use http_body_util::BodyExt;
use sz_rust_sz300::middleware::upload_limit::upload_limit_middleware;
use sz_rust_upload::multipart::{MultipartConfig, MultipartFile};
use sz_rust_upload::storage::{LocalStorage, StorageAdapter, StorageBackend};
use tower::ServiceExt;

// ============================================================================
// MultipartConfig 单元测试
// ============================================================================

#[test]
fn test_config_validate_size_ok() {
    let cfg = MultipartConfig::new().with_max_file_size(1024);
    assert!(cfg.validate_size(512).is_ok());
    assert!(cfg.validate_size(1024).is_ok());
}

#[test]
fn test_config_validate_size_exceeded() {
    let cfg = MultipartConfig::new().with_max_file_size(100);
    let err = cfg.validate_size(200).unwrap_err();
    assert!(matches!(
        err,
        sz_rust_upload::UploadError::SizeExceeded(200, 100)
    ));
}

#[test]
fn test_config_validate_extension_allowed() {
    let cfg = MultipartConfig::new()
        .with_extension("png")
        .with_extension("jpg")
        .with_extension("txt");
    assert!(cfg.validate_extension("photo.png").is_ok());
    assert!(cfg.validate_extension("photo.jpg").is_ok());
    assert!(cfg.validate_extension("doc.txt").is_ok());
}

#[test]
fn test_config_validate_extension_rejected() {
    let cfg = MultipartConfig::new().with_extension("png");
    assert!(cfg.validate_extension("photo.gif").is_err());
    assert!(cfg.validate_extension("photo.exe").is_err());
    assert!(cfg.validate_extension("noext").is_err());
}

#[test]
fn test_config_validate_extension_unrestricted() {
    let cfg = MultipartConfig::new();
    assert!(cfg.validate_extension("anything.xyz").is_ok());
    assert!(cfg.validate_extension("noext").is_ok());
}

#[test]
fn test_config_validate_comprehensive() {
    let cfg = MultipartConfig::new()
        .with_max_file_size(1024)
        .with_extension("txt")
        .with_mime_type("text/plain");
    assert!(cfg.validate("file.txt", 100, "text/plain").is_ok());
    assert!(cfg.validate("file.txt", 2048, "text/plain").is_err());
    assert!(cfg.validate("file.png", 100, "text/plain").is_err());
    assert!(cfg.validate("file.txt", 100, "image/png").is_err());
}

// ============================================================================
// MultipartFile 文件名安全校验
// ============================================================================

#[test]
fn test_multipart_file_valid_name() {
    let file = MultipartFile::new("test.txt", "text/plain", vec![1, 2, 3]);
    assert!(file.validate_file_name().is_ok());
    assert_eq!(file.size, 3);
}

#[test]
fn test_multipart_file_reject_path_traversal() {
    let file = MultipartFile::new("../etc/passwd", "text/plain", vec![]);
    assert!(file.validate_file_name().is_err());
}

#[test]
fn test_multipart_file_reject_empty_name() {
    let file = MultipartFile::new("", "text/plain", vec![]);
    assert!(file.validate_file_name().is_err());
}

#[test]
fn test_multipart_file_reject_control_chars() {
    let file = MultipartFile::new("test\n.txt", "text/plain", vec![]);
    assert!(file.validate_file_name().is_err());
}

#[test]
fn test_multipart_file_reject_backslash() {
    let file = MultipartFile::new("..\\windows\\system32", "text/plain", vec![]);
    assert!(file.validate_file_name().is_err());
}

// ============================================================================
// StorageBackend + LocalStorage 测试
// ============================================================================

#[test]
fn test_storage_backend_names() {
    assert_eq!(StorageBackend::local("/tmp").backend_name(), "local");
    assert_eq!(StorageBackend::s3("ep", "bk").backend_name(), "s3");
    assert_eq!(StorageBackend::minio("ep", "bk").backend_name(), "minio");
    assert_eq!(StorageBackend::oss("ep", "bk").backend_name(), "oss");
}

#[tokio::test]
async fn test_local_storage_store_and_merge() {
    let temp_dir = std::env::temp_dir().join(format!(
        "sz-upload-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let storage = LocalStorage::new(&temp_dir);

    storage.store_chunk("upload1", 0, b"hello ").await.unwrap();
    storage.store_chunk("upload1", 1, b"world").await.unwrap();
    storage.store_chunk("upload1", 2, b"!").await.unwrap();

    let merged = storage.merge_chunks("upload1", 3).await.unwrap();
    let content = tokio::fs::read_to_string(&merged).await.unwrap();
    assert_eq!(content, "hello world!");

    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
}

#[tokio::test]
async fn test_local_storage_exists_and_delete() {
    let temp_dir = std::env::temp_dir().join(format!(
        "sz-upload-test-exists-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let storage = LocalStorage::new(&temp_dir);
    storage.store_chunk("upload1", 0, b"data").await.unwrap();
    let chunk_path = temp_dir.join("upload1").join("chunk_0");
    assert!(storage.exists(&chunk_path).await);

    storage.delete(&chunk_path).await.unwrap();
    assert!(!storage.exists(&chunk_path).await);

    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
}

// ============================================================================
// 上传大小限制中间件测试
// ============================================================================

fn make_limit_router(max_size: u64) -> Router {
    let state = Arc::new(max_size);
    Router::new()
        .route("/api/v1/upload", post(|| async { "ok" }))
        .layer(middleware::from_fn_with_state(
            state,
            upload_limit_middleware,
        ))
}

#[tokio::test]
async fn test_upload_limit_under_limit() {
    let router = make_limit_router(1024);
    let body = Body::from(vec![0u8; 512]);
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/upload")
        .header("content-length", "512")
        .body(body)
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_upload_limit_exceeded() {
    let router = make_limit_router(100);
    let body = Body::from(vec![0u8; 200]);
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/upload")
        .header("content-length", "200")
        .body(body)
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn test_upload_limit_no_content_length() {
    let router = make_limit_router(100);
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/upload")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_upload_limit_exact_boundary() {
    let router = make_limit_router(100);
    let body = Body::from(vec![0u8; 100]);
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/upload")
        .header("content-length", "100")
        .body(body)
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ============================================================================
// 增强上传 handler 端到端测试（需要 MySQL）
// ============================================================================

use sz_rust_core::orm::Pool;
use sz_rust_sz300::config::UploadConfig;
use sz_rust_sz300::controllers::file::upload_enhanced;
use sz_rust_sz300::state::AppState;
use sz_rust_sz300::{config, db};

fn mysql_test_config() -> config::AppConfig {
    config::AppConfig {
        server: config::ServerConfig {
            host: "0.0.0.0".to_string(),
            port: 8300,
        },
        database: config::DatabaseConfig {
            host: "127.0.0.1".to_string(),
            port: 3306,
            username: "root".to_string(),
            password: "test123".to_string(),
            database: "sz_orm_test".to_string(),
        },
    }
}

async fn ensure_mysql() -> Option<Pool> {
    let cfg = mysql_test_config();
    match db::init_pool(&cfg).await {
        Ok(pool) => match pool.acquire().await {
            Ok(mut conn) => match conn.query("SELECT 1").await {
                Ok(_) => Some(pool),
                Err(_) => {
                    eprintln!("⚠️ MySQL 查询失败，跳过上传 handler 测试");
                    pool.close_all().await;
                    None
                }
            },
            Err(_) => {
                eprintln!("⚠️ MySQL 不可达，跳过上传 handler 测试");
                pool.close_all().await;
                None
            }
        },
        Err(_) => {
            eprintln!("⚠️ MySQL 连接池初始化失败，跳过上传 handler 测试");
            None
        }
    }
}

fn make_upload_state(pool: Pool) -> AppState {
    AppState {
        db_pool: Arc::new(pool),
        pg_pool: None,
        metrics_registry: Arc::new(sz_rust_observability::MetricsRegistry::new()),
        #[cfg(feature = "v18-rbac")]
        rbac_engine: Arc::new(sz_rust_sz300::rbac::roles::init_rbac_engine()),
        #[cfg(feature = "v18-key-rotation")]
        key_manager: sz_rust_sz300::services::auth_service::default_key_manager_for_tests(),
        #[cfg(feature = "v18-audit-chain")]
        chain_auditor: Arc::new(sz_rust_middleware_facade::audit_chain::ChainHashAuditor::new()),
        upload_config: UploadConfig {
            backend: StorageBackend::local("./test_uploads"),
            max_size: 1024,
            allowed_extensions: vec!["txt".to_string(), "png".to_string()],
        },
        #[cfg(feature = "v18-graphql")]
        graphql_schema: sz_rust_sz300::graphql::build_schema(),
        #[cfg(feature = "v18-websocket")]
        ws_manager: std::sync::Arc::new(
            sz_rust_websocket::manager::ConnectionManager::with_defaults(),
        ),
        #[cfg(feature = "v18-websocket")]
        ws_rooms: std::sync::Arc::new(sz_rust_websocket::room::RoomManager::new()),
        #[cfg(feature = "v18-sse")]
        sse_service: sz_rust_sz300::services::sse_service::SseService::with_defaults(),
    }
}

fn make_upload_router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/file/upload_enhanced", post(upload_enhanced))
        .with_state(state)
}

fn multipart_body(boundary: &str, filename: &str, content_type: &str, data: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(data);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

async fn collect_body(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

#[tokio::test]
async fn test_enhanced_upload_valid_file() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_upload_state(pool);
    let router = make_upload_router(state);

    let boundary = "----testboundary123";
    let body = multipart_body(boundary, "test.txt", "text/plain", b"hello world");
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/file/upload_enhanced")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_str = collect_body(resp).await;
    assert!(body_str.contains("\"code\":0"), "应返回成功: {body_str}");
    assert!(body_str.contains("test.txt"), "应包含文件名: {body_str}");

    let _ = tokio::fs::remove_dir_all("./test_uploads").await;
}

#[tokio::test]
async fn test_enhanced_upload_invalid_extension() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_upload_state(pool);
    let router = make_upload_router(state);

    let boundary = "----testboundary456";
    let body = multipart_body(boundary, "malware.exe", "application/octet-stream", b"fake");
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/file/upload_enhanced")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_str = collect_body(resp).await;
    assert!(body_str.contains("\"code\":1"), "应返回错误: {body_str}");
    assert!(body_str.contains("exe"), "应提示扩展名错误: {body_str}");

    let _ = tokio::fs::remove_dir_all("./test_uploads").await;
}

#[tokio::test]
async fn test_enhanced_upload_path_traversal() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_upload_state(pool);
    let router = make_upload_router(state);

    let boundary = "----testboundary789";
    let body = multipart_body(boundary, "../etc/passwd", "text/plain", b"bad");
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/file/upload_enhanced")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_str = collect_body(resp).await;
    assert!(
        body_str.contains("\"code\":1"),
        "应拒绝路径遍历: {body_str}"
    );

    let _ = tokio::fs::remove_dir_all("./test_uploads").await;
}
