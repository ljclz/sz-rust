// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use std::sync::Arc;
use sz_orm_sqlx::{
    MySqlPoolHandle, PgPoolHandle, SqlxMySqlConnectionFactory, SqlxPgConnectionFactory,
};
use sz_rust_core::orm::{Pool, PoolConfigBuilder, PoolError, PooledConnection};

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
    /// 空闲连接回收时间（秒），0 表示不回收
    pub idle_timeout_secs: u64,
    /// 连接最大存活时间（秒），0 表示不限制
    pub max_lifetime_secs: u64,
    /// 借出前探活（`SELECT 1`），防止把已断开的连接交给业务
    ///
    /// 2026-10-29 性能评测整改：默认关闭（`false`）。开启时每次借出连接都要多一次
    /// 网络往返（业务热路径每请求 3 次 SQL ⇒ 多 3 次 RTT），而空闲连接已由
    /// `idle_timeout`（600s）/ `max_lifetime`（1800s）回收，残存死连接的概率很低。
    /// 需要严格兜底时用 `SZ300_POOL_TEST_BEFORE_ACQUIRE=1` 显式打开。
    pub test_before_acquire: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 20,
            min_idle: 10,
            acquire_timeout_secs: 5,
            connection_timeout_secs: 10,
            idle_timeout_secs: 600,
            max_lifetime_secs: 1800,
            test_before_acquire: false,
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
    /// - `SZ300_POOL_IDLE_TIMEOUT`（默认 600 秒，0=禁用）
    /// - `SZ300_POOL_MAX_LIFETIME`（默认 1800 秒，0=禁用）
    /// - `SZ300_POOL_TEST_BEFORE_ACQUIRE`（默认 false；置 1/true/yes 开启借出前 `SELECT 1`）
    pub fn from_env() -> Self {
        let default = Self::default();
        Self {
            max_connections: std::env::var("SZ300_POOL_MAX")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(default.max_connections),
            min_idle: std::env::var("SZ300_POOL_MIN_IDLE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(default.min_idle),
            acquire_timeout_secs: std::env::var("SZ300_POOL_ACQUIRE_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(default.acquire_timeout_secs),
            connection_timeout_secs: default.connection_timeout_secs,
            idle_timeout_secs: std::env::var("SZ300_POOL_IDLE_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(default.idle_timeout_secs),
            max_lifetime_secs: std::env::var("SZ300_POOL_MAX_LIFETIME")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(default.max_lifetime_secs),
            test_before_acquire: std::env::var("SZ300_POOL_TEST_BEFORE_ACQUIRE")
                .ok()
                .map(|s| !matches!(s.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no"))
                .unwrap_or(default.test_before_acquire),
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

/// 取消安全的连接获取（规避上游 `Pool::acquire` 的槽位泄漏）
///
/// 上游 `Pool::acquire` 先 CAS 递增 `total_count` 再 `await` 建连，future 在该
/// await 点被取消时不会执行回滚 `fetch_sub`，槽位永久丢失；累计 `max_size` 次
/// 取消后池子彻底卡死（`total_count == max_size` 但无可用连接）。
/// 客户端断连、`tokio::time::timeout` 包裹、请求 future 被 drop 都会触发该路径。
///
/// 因此把 `acquire` 放进独立任务：调用方被取消时任务继续跑完，`send` 失败后
/// 连接随 `Result` 一起被 drop，正常归还池中。
///
/// 同构实现见 `sz_rust_orm_facade::acquire_cancel_safe`（供 addons 等业务包使用）；
/// sz300 不直接依赖该 facade，故保留本地副本，两处逻辑必须保持一致。
pub async fn acquire_cancel_safe(pool: &Pool) -> Result<PooledConnection, PoolError> {
    let pool = pool.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        // 接收端已 drop 时 send 返回 Err(连接)，随作用域结束归还
        let _ = tx.send(pool.acquire().await);
    });
    rx.await.unwrap_or(Err(PoolError::Closed))
}

/// MySQL 传输加密模式（映射到 sqlx `MySqlSslMode`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbTlsMode {
    /// 不加密（仅限本地/内网非生产）
    Disabled,
    /// 服务端支持则加密，否则明文（兼容存量实例）
    Preferred,
    /// 必须加密，但不校验证书
    Required,
    /// 必须加密且校验 CA
    VerifyCa,
    /// 必须加密、校验 CA 且校验主机名
    VerifyIdentity,
}

impl DbTlsMode {
    /// 解析配置字符串（大小写不敏感，`-`/`_` 等价）
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "disabled" | "off" | "none" => Some(Self::Disabled),
            "preferred" | "prefer" => Some(Self::Preferred),
            "required" | "require" => Some(Self::Required),
            "verify-ca" => Some(Self::VerifyCa),
            "verify-identity" | "verify-full" => Some(Self::VerifyIdentity),
            _ => None,
        }
    }
}

/// 解析 TLS 模式：生产默认 `required` 且禁止 `disabled`，非生产默认 `preferred`
///
/// 环境变量：`SZ300_DB_TLS`（disabled/preferred/required/verify-ca/verify-identity）
pub fn resolve_tls_mode(raw: Option<&str>, production: bool) -> anyhow::Result<DbTlsMode> {
    let mode = match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(value) => DbTlsMode::parse(value).ok_or_else(|| {
            anyhow::anyhow!(
                "SZ300_DB_TLS 取值非法: {value}（可选 disabled/preferred/required/verify-ca/verify-identity）"
            )
        })?,
        None if production => DbTlsMode::Required,
        None => DbTlsMode::Preferred,
    };
    if production && mode == DbTlsMode::Disabled {
        return Err(anyhow::anyhow!(
            "生产环境禁止关闭数据库传输加密（SZ300_DB_TLS=disabled 仅允许本地一次性实例）"
        ));
    }
    Ok(mode)
}

/// v1.9.0 使用自定义 PoolConfig 初始化 MySQL 连接池（spec §5.8.1 规则 2）
///
/// 使用 `MySqlConnectOptions` 结构化构造连接参数，避免 DSN 字符串拼接在
/// 密码含 `@`/`:`/`/` 等字符时被错误解析（也避免凭据出现在错误信息中）。
///
/// 传输加密（2026-10-28 数据层修复）：
/// - `SZ300_DB_TLS`：disabled / preferred / required / verify-ca / verify-identity
///   （生产默认 required，且禁止 disabled；非生产默认 preferred）
/// - `SZ300_DB_TLS_CA`：verify-ca / verify-identity 必填的 CA 证书路径
pub async fn init_pool_with_config(
    config: &crate::config::AppConfig,
    pool_config: PoolConfig,
) -> anyhow::Result<Pool> {
    let tls_mode = resolve_tls_mode(
        std::env::var("SZ300_DB_TLS").ok().as_deref(),
        crate::config::is_production(),
    )?;

    let mut connect_options = sqlx::mysql::MySqlConnectOptions::new()
        .host(&config.database.host)
        .port(config.database.port)
        .username(&config.database.username)
        .password(&config.database.password)
        .database(&config.database.database);

    connect_options = match tls_mode {
        DbTlsMode::Disabled => connect_options.ssl_mode(sqlx::mysql::MySqlSslMode::Disabled),
        DbTlsMode::Preferred => connect_options.ssl_mode(sqlx::mysql::MySqlSslMode::Preferred),
        DbTlsMode::Required => connect_options.ssl_mode(sqlx::mysql::MySqlSslMode::Required),
        DbTlsMode::VerifyCa | DbTlsMode::VerifyIdentity => {
            // 路径有效性交给 sqlx 读取；此处只保证配置存在，避免静默降级为不校验证书
            let ca = std::env::var("SZ300_DB_TLS_CA")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "SZ300_DB_TLS={tls_mode:?} 需要设置 SZ300_DB_TLS_CA 指向 CA 证书"
                    )
                })?;
            let mode = if tls_mode == DbTlsMode::VerifyCa {
                sqlx::mysql::MySqlSslMode::VerifyCa
            } else {
                sqlx::mysql::MySqlSslMode::VerifyIdentity
            };
            connect_options.ssl_mode(mode).ssl_ca(&ca)
        }
    };

    let mut options = sqlx::pool::PoolOptions::<sqlx::MySql>::new()
        .max_connections(pool_config.max_connections)
        .min_connections(pool_config.min_idle.min(pool_config.max_connections))
        .acquire_timeout(std::time::Duration::from_secs(
            pool_config.acquire_timeout_secs,
        ))
        .test_before_acquire(pool_config.test_before_acquire);
    if pool_config.idle_timeout_secs > 0 {
        options = options.idle_timeout(Some(std::time::Duration::from_secs(
            pool_config.idle_timeout_secs,
        )));
    }
    if pool_config.max_lifetime_secs > 0 {
        options = options.max_lifetime(Some(std::time::Duration::from_secs(
            pool_config.max_lifetime_secs,
        )));
    }
    let sqlx_pool = options.connect_with(connect_options).await?;
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
///
/// 池参数可通过环境变量覆盖：
/// - `SZ_PG_POOL_MAX`（默认 10）
/// - `SZ_PG_POOL_MIN_IDLE`（默认 5）
/// - `SZ_PG_POOL_ACQUIRE_TIMEOUT`（默认 5 秒）
/// - `SZ_PG_POOL_IDLE_TIMEOUT`（默认 600 秒，0=禁用）
/// - `SZ_PG_POOL_MAX_LIFETIME`（默认 1800 秒，0=禁用）
/// - `SZ_PG_POOL_TEST_BEFORE_ACQUIRE`（默认 false；置 1/true/yes 开启借出前探活）
pub async fn init_pg_pool(config: &crate::config::PgDatabaseConfig) -> anyhow::Result<Pool> {
    fn env_u64(key: &str, default: u64) -> u64 {
        std::env::var(key)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
    }

    let max_connections = env_u64("SZ_PG_POOL_MAX", 10).max(1) as u32;
    let min_idle = (env_u64("SZ_PG_POOL_MIN_IDLE", 5) as u32).min(max_connections);
    let acquire_timeout_secs = env_u64("SZ_PG_POOL_ACQUIRE_TIMEOUT", 5);
    let idle_timeout_secs = env_u64("SZ_PG_POOL_IDLE_TIMEOUT", 600);
    let max_lifetime_secs = env_u64("SZ_PG_POOL_MAX_LIFETIME", 1800);
    let test_before_acquire = std::env::var("SZ_PG_POOL_TEST_BEFORE_ACQUIRE")
        .map(|s| !matches!(s.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no"))
        .unwrap_or(false);

    let connect_options = sqlx::postgres::PgConnectOptions::new()
        .host(&config.host)
        .port(config.port)
        .username(&config.username)
        .password(&config.password)
        .database(&config.database);

    let mut options = sqlx::pool::PoolOptions::<sqlx::Postgres>::new()
        .max_connections(max_connections)
        .min_connections(min_idle)
        .acquire_timeout(std::time::Duration::from_secs(acquire_timeout_secs))
        .test_before_acquire(test_before_acquire);
    if idle_timeout_secs > 0 {
        options = options.idle_timeout(Some(std::time::Duration::from_secs(idle_timeout_secs)));
    }
    if max_lifetime_secs > 0 {
        options = options.max_lifetime(Some(std::time::Duration::from_secs(max_lifetime_secs)));
    }
    let sqlx_pool = options.connect_with(connect_options).await?;
    let factory = SqlxPgConnectionFactory::new(Arc::new(PgPoolHandle::from_pool(sqlx_pool)));

    let mut pool_cfg = PoolConfigBuilder::new()
        .max_size(max_connections)
        .min_idle(min_idle)
        .build()?;
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
        // 2026-10-29 整改：默认不做借出前探活（省一次 SQL 往返）
        assert!(!cfg.test_before_acquire);
    }

    #[test]
    fn test_pool_config_test_before_acquire_env_override() {
        std::env::set_var("SZ300_POOL_TEST_BEFORE_ACQUIRE", "1");
        assert!(PoolConfig::from_env().test_before_acquire);
        std::env::set_var("SZ300_POOL_TEST_BEFORE_ACQUIRE", "false");
        assert!(!PoolConfig::from_env().test_before_acquire);
        std::env::remove_var("SZ300_POOL_TEST_BEFORE_ACQUIRE");
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

    #[test]
    fn test_tls_mode_parse_variants() {
        assert_eq!(DbTlsMode::parse("disabled"), Some(DbTlsMode::Disabled));
        assert_eq!(DbTlsMode::parse("PREFERRED"), Some(DbTlsMode::Preferred));
        assert_eq!(DbTlsMode::parse("verify_ca"), Some(DbTlsMode::VerifyCa));
        assert_eq!(
            DbTlsMode::parse("verify-identity"),
            Some(DbTlsMode::VerifyIdentity)
        );
        assert_eq!(DbTlsMode::parse("bogus"), None);
    }

    #[test]
    fn test_resolve_tls_mode_defaults_follow_environment() {
        assert_eq!(
            resolve_tls_mode(None, false).unwrap(),
            DbTlsMode::Preferred,
            "非生产默认 preferred（兼容无 TLS 的本地实例）"
        );
        assert_eq!(
            resolve_tls_mode(None, true).unwrap(),
            DbTlsMode::Required,
            "生产默认强制 TLS"
        );
    }

    #[test]
    fn test_resolve_tls_mode_rejects_disabled_in_production() {
        let err = resolve_tls_mode(Some("disabled"), true).unwrap_err();
        assert!(err.to_string().contains("禁止关闭数据库传输加密"));
        assert_eq!(
            resolve_tls_mode(Some("disabled"), false).unwrap(),
            DbTlsMode::Disabled
        );
    }

    #[test]
    fn test_resolve_tls_mode_rejects_unknown_value() {
        assert!(resolve_tls_mode(Some("ssl3"), true).is_err());
    }
}
