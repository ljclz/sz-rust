// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use serde::Deserialize;

/// 应用根配置（聚合服务器与数据库配置）
#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    /// HTTP 服务器配置
    pub server: ServerConfig,
    /// MySQL 数据库配置
    pub database: DatabaseConfig,
}

/// HTTP 服务器配置
#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    /// 监听端口
    pub port: u16,
    /// 监听地址
    pub host: String,
}

/// MySQL 数据库配置
#[derive(Clone, Deserialize)]
pub struct DatabaseConfig {
    /// 数据库主机地址
    pub host: String,
    /// 数据库端口
    pub port: u16,
    /// 数据库名
    pub database: String,
    /// 数据库用户名
    pub username: String,
    /// 数据库密码（脱敏：禁止序列化输出）
    #[serde(skip_serializing)]
    pub password: String,
}

impl std::fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

/// PostgreSQL 数据库配置
#[derive(Clone, Deserialize)]
pub struct PgDatabaseConfig {
    /// 数据库主机地址
    pub host: String,
    /// 数据库端口
    pub port: u16,
    /// 数据库名
    pub database: String,
    /// 数据库用户名
    pub username: String,
    /// 数据库密码（脱敏：禁止序列化输出）
    #[serde(skip_serializing)]
    pub password: String,
}

impl std::fmt::Debug for PgDatabaseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgDatabaseConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

/// 从环境变量加载配置（生产安全要求：密钥不硬编码）
///
/// 环境变量：
/// - `SZ300_DB_HOST` (默认 127.0.0.1)
/// - `SZ300_DB_PORT` (默认 3306)
/// - `SZ300_DB_NAME` (默认 sz300)
/// - `SZ300_DB_USER` (默认 root)
/// - `SZ300_DB_PASSWORD` (必填)
/// - `SZ300_SERVER_HOST` (默认 0.0.0.0)
/// - `SZ300_SERVER_PORT` (默认 8300)
pub fn load_config() -> anyhow::Result<AppConfig> {
    let db_password = std::env::var("SZ300_DB_PASSWORD").map_err(|_| {
        anyhow::anyhow!("SZ300_DB_PASSWORD 环境变量未设置 — 请在启动前设置数据库密码")
    })?;

    Ok(AppConfig {
        server: ServerConfig {
            port: std::env::var("SZ300_SERVER_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(8300),
            host: std::env::var("SZ300_SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".into()),
        },
        database: DatabaseConfig {
            host: std::env::var("SZ300_DB_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
            port: std::env::var("SZ300_DB_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3306),
            database: std::env::var("SZ300_DB_NAME").unwrap_or_else(|_| "sz300".into()),
            username: std::env::var("SZ300_DB_USER").unwrap_or_else(|_| "root".into()),
            password: db_password,
        },
    })
}

/// PostgreSQL 连接配置（从环境变量读取）
///
/// 环境变量：
/// - `SZ300_PG_HOST` (默认 127.0.0.1)
/// - `SZ300_PG_PORT` (默认 5432)
/// - `SZ300_PG_NAME` (默认 sz300)
/// - `SZ300_PG_USER` (默认 postgres)
/// - `SZ300_PG_PASSWORD` (必填)
pub fn pg_config() -> anyhow::Result<PgDatabaseConfig> {
    let pg_password = std::env::var("SZ300_PG_PASSWORD").map_err(|_| {
        anyhow::anyhow!("SZ300_PG_PASSWORD 环境变量未设置 — 请在启动前设置 PostgreSQL 密码")
    })?;

    Ok(PgDatabaseConfig {
        host: std::env::var("SZ300_PG_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
        port: std::env::var("SZ300_PG_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5432),
        database: std::env::var("SZ300_PG_NAME").unwrap_or_else(|_| "sz300".into()),
        username: std::env::var("SZ300_PG_USER").unwrap_or_else(|_| "postgres".into()),
        password: pg_password,
    })
}
/// 安全头配置（v1.8.0，feature gate `v18-security-headers`）
///
/// 从环境变量读取安全头参数，未设置时使用安全默认值。
///
/// 环境变量：
/// - `SZ300_CSP` — CSP 策略（默认：default-src 'self'; script-src 'self'; ...）
/// - `SZ300_HSTS_MAX_AGE` — HSTS max-age（默认 31536000，即 1 年）
/// - `SZ300_HSTS_PRELOAD` — 是否启用 preload（默认 true，设为 "false" 禁用）
/// - `SZ300_X_FRAME_OPTIONS` — X-Frame-Options（默认 DENY，可选 SAMEORIGIN）
#[cfg(feature = "v18-security-headers")]
pub fn security_headers_config() -> sz_rust_security_headers::SecurityHeadersConfig {
    use sz_rust_security_headers::{SecurityHeadersConfig, XFrameOptions};

    SecurityHeadersConfig {
        csp: std::env::var("SZ300_CSP").unwrap_or_else(|_| {
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'"
                .to_string()
        }),
        hsts_max_age: std::env::var("SZ300_HSTS_MAX_AGE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(31536000),
        hsts_preload: std::env::var("SZ300_HSTS_PRELOAD")
            .map(|v| v != "false")
            .unwrap_or(true),
        x_frame_options: std::env::var("SZ300_X_FRAME_OPTIONS")
            .map(|v| match v.to_uppercase().as_str() {
                "SAMEORIGIN" => XFrameOptions::SameOrigin,
                _ => XFrameOptions::Deny,
            })
            .unwrap_or(XFrameOptions::Deny),
        ..Default::default()
    }
}
/// 数据脱敏引擎（v1.8.0，feature gate `v18-data-mask`）
///
/// 创建默认脱敏引擎，注册常用字段脱敏规则：
/// - `phone` / `contact_phone` → 手机脱敏 `138****5678`
/// - `bank_account` → 银行卡脱敏 `6222****7890`
/// - `password_hash` → 完全脱敏 `****`
/// - `id_card` → 身份证脱敏 `110***********1234`
/// - `email` → 邮箱脱敏 `u**r@example.com`
#[cfg(feature = "v18-data-mask")]
pub fn mask_engine() -> std::sync::Arc<sz_rust_data_mask::MaskEngine> {
    std::sync::Arc::new(sz_rust_data_mask::default_mask_engine())
}
/// v1.8.0 上传配置
#[cfg(feature = "v18-upload")]
#[derive(Debug, Clone)]
pub struct UploadConfig {
    /// 存储后端
    pub backend: sz_rust_upload::storage::StorageBackend,
    /// 最大文件大小（字节）
    pub max_size: u64,
    /// 允许的文件扩展名
    pub allowed_extensions: Vec<String>,
}

/// v1.8.0 加载上传配置
///
/// 环境变量：
/// - `SZ300_UPLOAD_BACKEND`：local / s3 / minio（默认 local）
/// - `SZ300_UPLOAD_ROOT`：本地存储根目录（默认 ./uploads）
/// - `SZ300_UPLOAD_MAX_SIZE`：最大文件大小字节（默认 10MB）
/// - `SZ300_UPLOAD_S3_ENDPOINT`：S3 端点
/// - `SZ300_UPLOAD_S3_BUCKET`：S3 桶名
#[cfg(feature = "v18-upload")]
pub fn upload_config() -> UploadConfig {
    use sz_rust_upload::storage::StorageBackend;

    let backend_name =
        std::env::var("SZ300_UPLOAD_BACKEND").unwrap_or_else(|_| "local".to_string());
    let max_size = std::env::var("SZ300_UPLOAD_MAX_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10 * 1024 * 1024);

    let backend = match backend_name.as_str() {
        "s3" => {
            let endpoint = std::env::var("SZ300_UPLOAD_S3_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:9000".to_string());
            let bucket = std::env::var("SZ300_UPLOAD_S3_BUCKET")
                .unwrap_or_else(|_| "sz300-uploads".to_string());
            StorageBackend::s3(endpoint, bucket)
        }
        "minio" => {
            let endpoint = std::env::var("SZ300_UPLOAD_S3_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:9000".to_string());
            let bucket = std::env::var("SZ300_UPLOAD_S3_BUCKET")
                .unwrap_or_else(|_| "sz300-uploads".to_string());
            StorageBackend::minio(endpoint, bucket)
        }
        _ => {
            let root =
                std::env::var("SZ300_UPLOAD_ROOT").unwrap_or_else(|_| "./uploads".to_string());
            StorageBackend::local(root)
        }
    };

    UploadConfig {
        backend,
        max_size,
        allowed_extensions: vec![
            "jpg".to_string(),
            "jpeg".to_string(),
            "png".to_string(),
            "gif".to_string(),
            "pdf".to_string(),
            "doc".to_string(),
            "docx".to_string(),
            "xls".to_string(),
            "xlsx".to_string(),
            "txt".to_string(),
        ],
    }
}
