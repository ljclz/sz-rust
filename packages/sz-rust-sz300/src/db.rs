// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use std::sync::Arc;
use sz_orm_sqlx::{
    MySqlPoolHandle, PgPoolHandle, SqlxMySqlConnectionFactory, SqlxPgConnectionFactory,
};
use sz_rust_core::orm::{Pool, PoolConfigBuilder};

/// v1.9.0 连接池配置（spec §5.8.1 规则 2）
///
/// 暴露连接池参数供外部调优，acquire P99 ≤ 1ms 目标。
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// 最大连接数
    pub max_connections: u32,
    /// 最小空闲连接数
    pub min_idle: u32,
    /// 连接获取超时（秒），超时返回 503（spec §5.8.3 异常 2）
    pub acquire_timeout_secs: u64,
    /// 连接建立超时（秒）
    pub connection_timeout_secs: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 20,
            min_idle: 10,
            acquire_timeout_secs: 5,
            connection_timeout_secs: 10,
        }
    }
}

impl PoolConfig {
    /// 从环境变量加载连接池配置
    ///
    /// 环境变量：
    /// - `SZ300_POOL_MAX`（默认 20）
    /// - `SZ300_POOL_MIN_IDLE`（默认 10）
    /// - `SZ300_POOL_ACQUIRE_TIMEOUT`（默认 5 秒）
    pub fn from_env() -> Self {
        Self {
            max_connections: std::env::var("SZ300_POOL_MAX")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(20),
            min_idle: std::env::var("SZ300_POOL_MIN_IDLE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10),
            acquire_timeout_secs: std::env::var("SZ300_POOL_ACQUIRE_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
            connection_timeout_secs: 10,
        }
    }
}

/// 连接池耗尽错误（spec §5.8.3 异常 2）
///
/// 映射到 HTTP 503 + "服务繁忙，请稍后重试"
#[derive(Debug, thiserror::Error)]
#[error("连接池耗尽：acquire 超时（{timeout_secs}s），当前 max_connections={max_connections}")]
pub struct PoolExhaustedError {
    /// 超时秒数
    pub timeout_secs: u64,
    /// 最大连接数
    pub max_connections: u32,
}

/// 初始化 MySQL 连接池
pub async fn init_pool(config: &crate::config::AppConfig) -> anyhow::Result<Pool> {
    init_pool_with_config(config, PoolConfig::from_env()).await
}

/// v1.9.0 使用自定义 PoolConfig 初始化 MySQL 连接池（spec §5.8.1 规则 2）
pub async fn init_pool_with_config(
    config: &crate::config::AppConfig,
    pool_config: PoolConfig,
) -> anyhow::Result<Pool> {
    let conn_str = format!(
        "mysql://{}:{}@{}:{}/{}",
        config.database.username,
        config.database.password,
        config.database.host,
        config.database.port,
        config.database.database,
    );

    let sqlx_pool = sqlx::pool::PoolOptions::<sqlx::MySql>::new()
        .max_connections(pool_config.max_connections)
        .acquire_timeout(std::time::Duration::from_secs(
            pool_config.acquire_timeout_secs,
        ))
        .connect(&conn_str)
        .await?;
    let factory = SqlxMySqlConnectionFactory::new(Arc::new(MySqlPoolHandle::from_pool(sqlx_pool)));

    let mut pool_cfg = PoolConfigBuilder::new()
        .max_size(pool_config.max_connections)
        .min_idle(pool_config.min_idle)
        .build()?;
    pool_cfg.connection_timeout =
        std::time::Duration::from_secs(pool_config.connection_timeout_secs);

    let pool = Pool::new(pool_cfg, Arc::new(factory))?;
    Ok(pool)
}

/// 初始化 PostgreSQL 连接池
pub async fn init_pg_pool(config: &crate::config::PgDatabaseConfig) -> anyhow::Result<Pool> {
    let conn_str = format!(
        "postgres://{}:{}@{}:{}/{}",
        config.username, config.password, config.host, config.port, config.database,
    );

    let sqlx_pool = PgPoolHandle::connect(&conn_str).await?;
    let factory = SqlxPgConnectionFactory::new(Arc::new(sqlx_pool));

    // P3-7：min_idle 提升至 max_size 的 50%，避免突发流量下冷连接建立延迟
    let mut pool_cfg = PoolConfigBuilder::new().max_size(10).min_idle(5).build()?;
    pool_cfg.connection_timeout = std::time::Duration::from_secs(10);

    let pool = Pool::new(pool_cfg, Arc::new(factory))?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_config_default() {
        let cfg = PoolConfig::default();
        assert_eq!(cfg.max_connections, 20);
        assert_eq!(cfg.min_idle, 10);
        assert_eq!(cfg.acquire_timeout_secs, 5);
        assert_eq!(cfg.connection_timeout_secs, 10);
    }

    #[test]
    fn test_pool_config_from_env() {
        std::env::set_var("SZ300_POOL_MAX", "50");
        std::env::set_var("SZ300_POOL_MIN_IDLE", "25");
        std::env::set_var("SZ300_POOL_ACQUIRE_TIMEOUT", "3");
        let cfg = PoolConfig::from_env();
        assert_eq!(cfg.max_connections, 50);
        assert_eq!(cfg.min_idle, 25);
        assert_eq!(cfg.acquire_timeout_secs, 3);
        std::env::remove_var("SZ300_POOL_MAX");
        std::env::remove_var("SZ300_POOL_MIN_IDLE");
        std::env::remove_var("SZ300_POOL_ACQUIRE_TIMEOUT");
    }

    #[test]
    fn test_pool_exhausted_error_display() {
        let err = PoolExhaustedError {
            timeout_secs: 5,
            max_connections: 20,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("连接池耗尽"));
        assert!(msg.contains("5"));
        assert!(msg.contains("20"));
    }
}
