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
#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    /// 数据库主机地址
    pub host: String,
    /// 数据库端口
    pub port: u16,
    /// 数据库名
    pub database: String,
    /// 数据库用户名
    pub username: String,
    /// 数据库密码
    pub password: String,
}

/// PostgreSQL 数据库配置
#[derive(Debug, Deserialize, Clone)]
pub struct PgDatabaseConfig {
    /// 数据库主机地址
    pub host: String,
    /// 数据库端口
    pub port: u16,
    /// 数据库名
    pub database: String,
    /// 数据库用户名
    pub username: String,
    /// 数据库密码
    pub password: String,
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
    let db_password = std::env::var("SZ300_DB_PASSWORD")
        .map_err(|_| anyhow::anyhow!("SZ300_DB_PASSWORD 环境变量未设置 — 请在启动前设置数据库密码"))?;

    Ok(AppConfig {
        server: ServerConfig {
            port: std::env::var("SZ300_SERVER_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(8300),
            host: std::env::var("SZ300_SERVER_HOST")
                .unwrap_or_else(|_| "0.0.0.0".into()),
        },
        database: DatabaseConfig {
            host: std::env::var("SZ300_DB_HOST")
                .unwrap_or_else(|_| "127.0.0.1".into()),
            port: std::env::var("SZ300_DB_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3306),
            database: std::env::var("SZ300_DB_NAME")
                .unwrap_or_else(|_| "sz300".into()),
            username: std::env::var("SZ300_DB_USER")
                .unwrap_or_else(|_| "root".into()),
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
    let pg_password = std::env::var("SZ300_PG_PASSWORD")
        .map_err(|_| anyhow::anyhow!("SZ300_PG_PASSWORD 环境变量未设置 — 请在启动前设置 PostgreSQL 密码"))?;

    Ok(PgDatabaseConfig {
        host: std::env::var("SZ300_PG_HOST")
            .unwrap_or_else(|_| "127.0.0.1".into()),
        port: std::env::var("SZ300_PG_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5432),
        database: std::env::var("SZ300_PG_NAME")
            .unwrap_or_else(|_| "sz300".into()),
        username: std::env::var("SZ300_PG_USER")
            .unwrap_or_else(|_| "postgres".into()),
        password: pg_password,
    })
}
