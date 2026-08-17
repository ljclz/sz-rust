use std::sync::Arc;
use std::time::Duration;
use sz_orm_sqlx::{
    MySqlPoolHandle, PgPoolHandle, SqlxMySqlConnectionFactory, SqlxPgConnectionFactory,
};
use sz_rust_core::orm::{Pool, PoolConfigBuilder, SqlxPoolConfig};

/// MySQL 池默认容量（与既有默认一致：max=20, min_idle=10）
const MYSQL_POOL_MAX: u32 = 20;
const MYSQL_POOL_MIN_IDLE: u32 = 10;
/// PostgreSQL 池默认容量（P2-11 / P3-7 既有默认：max=10, min_idle=5）
const PG_POOL_MAX: u32 = 10;
const PG_POOL_MIN_IDLE: u32 = 5;

/// 从环境变量读取连接池配置（`DB_POOL_*`），未设置时保持 sz300 既有默认值。
///
/// 对齐 sz-rust-orm-facade 的 `SqlxPoolConfig::from_env()`（L2 调优层），
/// 但默认值取 sz300 现状（20/10/30s/600s/1800s），避免 facade 默认（10/0）改变行为。
pub fn mysql_pool_config_from_env() -> SqlxPoolConfig {
    let from_env = SqlxPoolConfig::from_env();
    SqlxPoolConfig {
        max_connections: env_u32("DB_POOL_MAX").unwrap_or(MYSQL_POOL_MAX),
        min_connections: env_u32("DB_POOL_MIN").unwrap_or(MYSQL_POOL_MIN_IDLE),
        ..from_env
    }
}

/// PostgreSQL 池固定配置（不跟随 DB_POOL_*，主从池容量互相独立）
fn pg_pool_config() -> SqlxPoolConfig {
    SqlxPoolConfig {
        max_connections: PG_POOL_MAX,
        min_connections: PG_POOL_MIN_IDLE,
        acquire_timeout: Duration::from_secs(30),
        idle_timeout: Duration::from_secs(600),
        max_lifetime: Duration::from_secs(1800),
    }
}

fn env_u32(key: &str) -> Option<u32> {
    std::env::var(key).ok().and_then(|v| v.parse().ok())
}

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

    let pool_cfg = mysql_pool_config_from_env();

    // SQLx 池容量与 sz-orm Pool 对齐（max_connections / min_connections 同源）。
    // 历史缺陷：SQLx 默认 10 < sz-orm max_size 20，并发 acquire 第 11 个起超时。
    // 本次补齐 SQLx 层 min_connections / idle_timeout / max_lifetime，
    // 与 sz-orm 层 idle_timeout(600s) / max_lifetime(1800s) 双保险。
    let sqlx_pool = sqlx::pool::PoolOptions::<sqlx::MySql>::new()
        .max_connections(pool_cfg.max_connections)
        .min_connections(pool_cfg.min_connections)
        .acquire_timeout(pool_cfg.acquire_timeout)
        .idle_timeout(pool_cfg.idle_timeout)
        .max_lifetime(pool_cfg.max_lifetime)
        .connect(&conn_str)
        .await?;
    let factory = SqlxMySqlConnectionFactory::new(Arc::new(MySqlPoolHandle::from_pool(sqlx_pool)));

    // 预热接线：sz-orm 原生 prewarm（facade PoolWarmer 的 connect_fn 不返回连接，
    // 与 sz-orm Pool 无公开"建连入池"API，故使用 sz-orm 自身预热路径）
    let base = pool_cfg.to_orm_pool_config();
    let mut orm_cfg = PoolConfigBuilder::new()
        .max_size(base.max_size)
        .min_idle(base.min_idle)
        .acquire_timeout(base.acquire_timeout.as_secs())
        .idle_timeout(base.idle_timeout.as_secs())
        .max_lifetime(base.max_lifetime.as_secs())
        .prewarm(true)
        .build()?;
    orm_cfg.connection_timeout = Duration::from_secs(10);

    let pool = Pool::new(orm_cfg, Arc::new(factory))?;
    Ok(pool)
}

/// 初始化 PostgreSQL 连接池
pub async fn init_pg_pool(config: &crate::config::PgDatabaseConfig) -> anyhow::Result<Pool> {
    let conn_str = format!(
        "postgres://{}:{}@{}:{}/{}",
        config.username, config.password, config.host, config.port, config.database,
    );

    let pool_cfg = pg_pool_config();

    // P2-11: SQLx 池 max_connections=10，与 sz-orm Pool max_size(10) 对齐
    // 修复 PostgreSQL 池使用默认配置（max_connections=10, acquire_timeout=30s）的不一致问题
    let sqlx_pool = sqlx::pool::PoolOptions::<sqlx::Postgres>::new()
        .max_connections(pool_cfg.max_connections)
        .min_connections(pool_cfg.min_connections)
        .acquire_timeout(pool_cfg.acquire_timeout)
        .idle_timeout(pool_cfg.idle_timeout)
        .max_lifetime(pool_cfg.max_lifetime)
        .connect(&conn_str)
        .await?;
    let factory = SqlxPgConnectionFactory::new(Arc::new(PgPoolHandle::from_pool(sqlx_pool)));

    // P3-7：min_idle 提升至 max_size 的 50%，避免突发流量下冷连接建立延迟
    let mut pool_cfg = PoolConfigBuilder::new()
        .max_size(pool_cfg.max_connections)
        .min_idle(pool_cfg.min_connections)
        .prewarm(true)
        .build()?;
    pool_cfg.connection_timeout = Duration::from_secs(10);

    let pool = Pool::new(pool_cfg, Arc::new(factory))?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `std::env` 是进程级全局状态，测试并行执行会互相污染，
    /// 所有读写 `DB_POOL_*` 的测试必须持有此锁串行执行。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_mysql_pool_config_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = mysql_pool_config_from_env();
        assert_eq!(cfg.max_connections, 20);
        assert_eq!(cfg.min_connections, 10);
        assert_eq!(cfg.acquire_timeout, Duration::from_secs(30));
        assert_eq!(cfg.idle_timeout, Duration::from_secs(600));
        assert_eq!(cfg.max_lifetime, Duration::from_secs(1800));
    }

    #[test]
    fn test_mysql_pool_config_env_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("DB_POOL_MAX", "50");
        std::env::set_var("DB_POOL_MIN", "15");
        std::env::set_var("DB_POOL_ACQUIRE_TIMEOUT", "5");
        let cfg = mysql_pool_config_from_env();
        assert_eq!(cfg.max_connections, 50);
        assert_eq!(cfg.min_connections, 15);
        assert_eq!(cfg.acquire_timeout, Duration::from_secs(5));
        // 未覆盖项保持默认
        assert_eq!(cfg.idle_timeout, Duration::from_secs(600));
        std::env::remove_var("DB_POOL_MAX");
        std::env::remove_var("DB_POOL_MIN");
        std::env::remove_var("DB_POOL_ACQUIRE_TIMEOUT");
    }

    #[test]
    fn test_mysql_pool_config_invalid_env_falls_back() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("DB_POOL_MAX", "not-a-number");
        let cfg = mysql_pool_config_from_env();
        assert_eq!(cfg.max_connections, 20);
        std::env::remove_var("DB_POOL_MAX");
    }

    #[test]
    fn test_pg_pool_config_fixed() {
        let cfg = pg_pool_config();
        assert_eq!(cfg.max_connections, 10);
        assert_eq!(cfg.min_connections, 5);
    }

    /// 覆盖 init_pool 错误路径 — 使用不可达的 DB 地址，连接应失败
    #[tokio::test]
    async fn init_pool_returns_err_when_db_unreachable() {
        let config = crate::config::AppConfig {
            server: crate::config::ServerConfig {
                port: 8300,
                host: "0.0.0.0".to_string(),
            },
            database: crate::config::DatabaseConfig {
                host: "127.0.0.1".to_string(),
                port: 1, // 不可达端口
                database: "fake".to_string(),
                username: "fake".to_string(),
                password: "fake".to_string(),
            },
        };
        let result = init_pool(&config).await;
        assert!(result.is_err(), "不可达 DB 应返回 Err");
    }

    /// 覆盖 init_pg_pool 错误路径 — 使用不可达的 DB 地址，连接应失败
    #[tokio::test]
    async fn init_pg_pool_returns_err_when_db_unreachable() {
        let config = crate::config::PgDatabaseConfig {
            host: "127.0.0.1".to_string(),
            port: 1, // 不可达端口
            database: "fake".to_string(),
            username: "fake".to_string(),
            password: "fake".to_string(),
        };
        let result = init_pg_pool(&config).await;
        assert!(result.is_err(), "不可达 PG 应返回 Err");
    }
}
