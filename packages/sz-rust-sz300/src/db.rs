use sz_orm_core::{Pool, PoolConfigBuilder};
use sz_orm_sqlx::{SqlxMySqlConnectionFactory, MySqlPoolHandle, SqlxPgConnectionFactory, PgPoolHandle};
use std::sync::Arc;

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

    let sqlx_pool = MySqlPoolHandle::connect(&conn_str).await?;
    let factory = SqlxMySqlConnectionFactory::new(Arc::new(sqlx_pool));

    let mut pool_cfg = PoolConfigBuilder::new()
        .max_size(20)
        .min_idle(2)
        .build()?;
    pool_cfg.connection_timeout = std::time::Duration::from_secs(10);

    let pool = Pool::new(pool_cfg, Arc::new(factory))?;
    Ok(pool)
}

/// 初始化 PostgreSQL 连接池
pub async fn init_pg_pool(config: &crate::config::PgDatabaseConfig) -> anyhow::Result<Pool> {
    let conn_str = format!(
        "postgres://{}:{}@{}:{}/{}",
        config.username,
        config.password,
        config.host,
        config.port,
        config.database,
    );

    let sqlx_pool = PgPoolHandle::connect(&conn_str).await?;
    let factory = SqlxPgConnectionFactory::new(Arc::new(sqlx_pool));

    let mut pool_cfg = PoolConfigBuilder::new()
        .max_size(10)
        .min_idle(1)
        .build()?;
    pool_cfg.connection_timeout = std::time::Duration::from_secs(10);

    let pool = Pool::new(pool_cfg, Arc::new(factory))?;
    Ok(pool)
}
