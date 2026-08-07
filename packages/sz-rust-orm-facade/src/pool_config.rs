//! 连接池配置调优（L2 方案：sqlx 配置调优，不修改 sz-orm 上游）
//!
//! 提供 `SqlxPoolConfig` 便捷配置结构体，支持：
//! - `Default`：与 sqlx 默认一致
//! - `from_env()`：从环境变量读取
//! - `for_high_concurrency()`：高并发预设
//! - `for_low_latency()`：低延迟预设
//! - `to_orm_pool_config()`：转换为 sz_orm_core::PoolConfig

use std::time::Duration;

use sz_orm_core::PoolConfig;

/// sqlx 连接池配置（L2 调优层）
///
/// 不修改 sz-orm 上游，在 sz-rust-orm-facade 层提供便捷配置预设。
#[derive(Debug, Clone)]
pub struct SqlxPoolConfig {
    /// 最大连接数
    pub max_connections: u32,
    /// 最小空闲连接数
    pub min_connections: u32,
    /// 获取连接超时
    pub acquire_timeout: Duration,
    /// 空闲连接超时
    pub idle_timeout: Duration,
    /// 连接最大生命周期
    pub max_lifetime: Duration,
}

impl Default for SqlxPoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 10,
            min_connections: 0,
            acquire_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(600),
            max_lifetime: Duration::from_secs(1800),
        }
    }
}

impl SqlxPoolConfig {
    /// 从环境变量读取配置
    ///
    /// 环境变量：
    /// - `DB_POOL_MAX`：最大连接数（默认 10）
    /// - `DB_POOL_MIN`：最小空闲连接数（默认 0）
    /// - `DB_POOL_ACQUIRE_TIMEOUT`：获取超时秒数（默认 30）
    /// - `DB_POOL_IDLE_TIMEOUT`：空闲超时秒数（默认 600）
    /// - `DB_POOL_MAX_LIFETIME`：最大生命周期秒数（默认 1800）
    pub fn from_env() -> Self {
        fn parse_env_u32(key: &str, default: u32) -> u32 {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        }
        fn parse_env_secs(key: &str, default: u64) -> Duration {
            Duration::from_secs(parse_env_u32(key, default as u32) as u64)
        }

        Self {
            max_connections: parse_env_u32("DB_POOL_MAX", 10),
            min_connections: parse_env_u32("DB_POOL_MIN", 0),
            acquire_timeout: parse_env_secs("DB_POOL_ACQUIRE_TIMEOUT", 30),
            idle_timeout: parse_env_secs("DB_POOL_IDLE_TIMEOUT", 600),
            max_lifetime: parse_env_secs("DB_POOL_MAX_LIFETIME", 1800),
        }
    }

    /// 高并发预设：max=50, min=5, acquire=10s, idle=300s, max_lifetime=1800s
    pub fn for_high_concurrency() -> Self {
        Self {
            max_connections: 50,
            min_connections: 5,
            acquire_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(300),
            max_lifetime: Duration::from_secs(1800),
        }
    }

    /// 低延迟预设：max=20, min=10, acquire=5s, idle=120s, max_lifetime=600s
    pub fn for_low_latency() -> Self {
        Self {
            max_connections: 20,
            min_connections: 10,
            acquire_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(120),
            max_lifetime: Duration::from_secs(600),
        }
    }

    /// 转换为 sz_orm_core::PoolConfig
    pub fn to_orm_pool_config(&self) -> PoolConfig {
        PoolConfig {
            max_size: self.max_connections,
            min_idle: self.min_connections,
            acquire_timeout: self.acquire_timeout,
            idle_timeout: self.idle_timeout,
            max_lifetime: self.max_lifetime,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_config_default() {
        let config = SqlxPoolConfig::default();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 0);
        assert_eq!(config.acquire_timeout, Duration::from_secs(30));
        assert_eq!(config.idle_timeout, Duration::from_secs(600));
        assert_eq!(config.max_lifetime, Duration::from_secs(1800));
    }

    #[test]
    fn test_pool_config_from_env() {
        std::env::set_var("DB_POOL_MAX", "42");
        std::env::set_var("DB_POOL_MIN", "7");
        std::env::set_var("DB_POOL_ACQUIRE_TIMEOUT", "15");
        std::env::set_var("DB_POOL_IDLE_TIMEOUT", "200");
        std::env::set_var("DB_POOL_MAX_LIFETIME", "900");

        let config = SqlxPoolConfig::from_env();
        assert_eq!(config.max_connections, 42);
        assert_eq!(config.min_connections, 7);
        assert_eq!(config.acquire_timeout, Duration::from_secs(15));
        assert_eq!(config.idle_timeout, Duration::from_secs(200));
        assert_eq!(config.max_lifetime, Duration::from_secs(900));

        std::env::remove_var("DB_POOL_MAX");
        std::env::remove_var("DB_POOL_MIN");
        std::env::remove_var("DB_POOL_ACQUIRE_TIMEOUT");
        std::env::remove_var("DB_POOL_IDLE_TIMEOUT");
        std::env::remove_var("DB_POOL_MAX_LIFETIME");
    }

    #[test]
    fn test_pool_config_high_concurrency() {
        let config = SqlxPoolConfig::for_high_concurrency();
        assert_eq!(config.max_connections, 50);
        assert_eq!(config.min_connections, 5);
        assert_eq!(config.acquire_timeout, Duration::from_secs(10));
        assert_eq!(config.idle_timeout, Duration::from_secs(300));
        assert_eq!(config.max_lifetime, Duration::from_secs(1800));
    }

    #[test]
    fn test_pool_config_low_latency() {
        let config = SqlxPoolConfig::for_low_latency();
        assert_eq!(config.max_connections, 20);
        assert_eq!(config.min_connections, 10);
        assert_eq!(config.acquire_timeout, Duration::from_secs(5));
        assert_eq!(config.idle_timeout, Duration::from_secs(120));
        assert_eq!(config.max_lifetime, Duration::from_secs(600));
    }

    #[test]
    fn test_pool_config_to_orm() {
        let config = SqlxPoolConfig::for_high_concurrency();
        let orm_config = config.to_orm_pool_config();
        assert_eq!(orm_config.max_size, 50);
        assert_eq!(orm_config.min_idle, 5);
        assert_eq!(orm_config.acquire_timeout, Duration::from_secs(10));
    }
}
