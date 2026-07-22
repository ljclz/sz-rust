use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PgDatabaseConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,
}

pub fn load_config() -> anyhow::Result<AppConfig> {
    Ok(AppConfig {
        server: ServerConfig {
            port: 8300,
            host: "0.0.0.0".into(),
        },
        database: DatabaseConfig {
            host: "127.0.0.1".into(),
            port: 3306,
            database: "sz300".into(),
            username: "root".into(),
            password: "test123".into(),
        },
    })
}

/// PostgreSQL 连接配置（本机开发用）
pub fn pg_config() -> PgDatabaseConfig {
    PgDatabaseConfig {
        host: "127.0.0.1".into(),
        port: 5432,
        database: "sz300".into(),
        username: "postgres".into(),
        password: "test123".into(),
    }
}
