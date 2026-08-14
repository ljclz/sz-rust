use std::sync::Arc;
use sz_orm_sqlx::{
    MySqlPoolHandle, PgPoolHandle, SqlxMySqlConnectionFactory, SqlxPgConnectionFactory,
};
use sz_rust_core::orm::{Pool, PoolConfigBuilder};

/// 初始化 MySQL 连接池
pub async fn init_pool(config: &crate::config::AppConfig) -> anyhow::Result<Pool> {
    let conn_str = format!(
        "mysql://{}:{}@{}:{}/{}",
        config.database.username,
        config.database.password,
        config.database.host,
        config.database.port,
        config.database.database,
    );

    // SQLx 池 max_connections=20：与下方 sz-orm Pool max_size(20) 对齐。
    // 修复两层池容量不匹配缺陷（SQLx 默认 10 < max_size 20，并发 acquire 第 11 个起超时）
    let sqlx_pool = sqlx::pool::PoolOptions::<sqlx::MySql>::new()
        .max_connections(20)
        .acquire_timeout(std::time::Duration::from_secs(30))
        .connect(&conn_str)
        .await?;
    let factory = SqlxMySqlConnectionFactory::new(Arc::new(MySqlPoolHandle::from_pool(sqlx_pool)));

    let mut pool_cfg = PoolConfigBuilder::new().max_size(20).min_idle(10).build()?;
    pool_cfg.connection_timeout = std::time::Duration::from_secs(10);

    let pool = Pool::new(pool_cfg, Arc::new(factory))?;
    Ok(pool)
}

/// 初始化 PostgreSQL 连接池
pub async fn init_pg_pool(config: &crate::config::PgDatabaseConfig) -> anyhow::Result<Pool> {
    let conn_str = format!(
        "postgres://{}:{}@{}:{}/{}",
        config.username, config.password, config.host, config.port, config.database,
    );

    // P2-11: SQLx 池 max_connections=10，与 sz-orm Pool max_size(10) 对齐
    // 修复 PostgreSQL 池使用默认配置（max_connections=10, acquire_timeout=30s）的不一致问题
    let sqlx_pool = sqlx::pool::PoolOptions::<sqlx::Postgres>::new()
        .max_connections(10)
        .acquire_timeout(std::time::Duration::from_secs(30))
        .connect(&conn_str)
        .await?;
    let factory = SqlxPgConnectionFactory::new(Arc::new(PgPoolHandle::from_pool(sqlx_pool)));

    // P3-7：min_idle 提升至 max_size 的 50%，避免突发流量下冷连接建立延迟
    let mut pool_cfg = PoolConfigBuilder::new().max_size(10).min_idle(5).build()?;
    pool_cfg.connection_timeout = std::time::Duration::from_secs(10);

    let pool = Pool::new(pool_cfg, Arc::new(factory))?;
    Ok(pool)
}
