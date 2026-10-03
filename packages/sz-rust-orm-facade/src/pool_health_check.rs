// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 连接池健康检查驱逐（spec §5.6 规则 2/3）
//!
//! 定期 ping 空闲连接，驱逐不健康连接，回收空闲超时连接，补充至最小连接数。

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;

/// 健康检查驱逐配置（spec §6.6）
#[derive(Debug, Clone)]
pub struct HealthCheckConfig {
    /// 健康检查周期（默认 60s，spec §6.6 规则 3）
    pub check_interval: Duration,
    /// 空闲超时（默认 300s，spec §6.6 规则 2）
    pub idle_timeout: Duration,
    /// 最小连接数（驱逐后补充至此，spec §5.6 规则 3）
    pub min_connections: usize,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(300),
            min_connections: 5,
        }
    }
}

/// 连接健康状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionHealth {
    /// 健康
    Healthy,
    /// 不健康（含原因）
    Unhealthy(String),
}

/// 连接信息
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    /// 连接 ID
    pub id: u64,
    /// 是否空闲
    pub is_idle: bool,
    /// 最后使用时间
    pub last_used: Instant,
}

/// 健康检查一轮结果
#[derive(Debug, Clone, Default)]
pub struct HealthCheckResult {
    /// 检查的连接数
    pub checked: u64,
    /// 驱逐的不健康连接数
    pub evicted: u64,
    /// 回收的空闲超时连接数
    pub idle_reclaimed: u64,
    /// 补充的新连接数
    pub replenished: u64,
}

/// 健康检查错误
#[derive(Debug, Clone, thiserror::Error)]
pub enum PoolHealthError {
    /// 池操作失败
    #[error("池操作失败: {0}")]
    PoolOp(String),
    /// 补充连接失败
    #[error("补充连接失败: {0}")]
    ReplenishFailed(String),
}

/// 可健康检查的连接池 trait
#[async_trait]
pub trait HealthCheckablePool: Send + Sync {
    /// 列出所有连接信息
    async fn list_connections(&self) -> Result<Vec<ConnectionInfo>, PoolHealthError>;
    /// ping 连接，返回健康状态
    async fn ping(&self, conn_id: u64) -> Result<ConnectionHealth, PoolHealthError>;
    /// 驱逐指定连接
    async fn evict(&self, conn_id: u64) -> Result<(), PoolHealthError>;
    /// 补充连接至目标数，返回实际补充数
    async fn replenish(&self, target: usize) -> Result<u64, PoolHealthError>;
    /// 当前连接数
    async fn connection_count(&self) -> Result<usize, PoolHealthError>;
}

/// 健康检查驱逐器
pub struct HealthCheckEvictor {
    config: HealthCheckConfig,
}

impl HealthCheckEvictor {
    /// 创建健康检查驱逐器
    pub fn new(config: HealthCheckConfig) -> Self {
        Self { config }
    }

    /// 获取配置引用
    pub fn config(&self) -> &HealthCheckConfig {
        &self.config
    }

    /// 执行一轮健康检查驱逐（spec §5.6 规则 2/3）
    ///
    /// 1. 列出所有连接
    /// 2. 空闲超时连接回收
    /// 3. 空闲连接 ping 健康检查，不健康驱逐
    /// 4. 补充至 min_connections
    pub async fn run_once(
        &self,
        pool: &dyn HealthCheckablePool,
    ) -> Result<HealthCheckResult, PoolHealthError> {
        let connections = pool.list_connections().await?;
        let now = Instant::now();
        let mut result = HealthCheckResult::default();

        for conn in &connections {
            if !conn.is_idle {
                continue;
            }
            // 空闲超时回收（spec §5.6 规则 3）
            if now.duration_since(conn.last_used) > self.config.idle_timeout {
                pool.evict(conn.id).await?;
                result.idle_reclaimed += 1;
                continue;
            }
            // 健康检查（spec §5.6 规则 2）
            let health = pool.ping(conn.id).await?;
            result.checked += 1;
            if !matches!(health, ConnectionHealth::Healthy) {
                pool.evict(conn.id).await?;
                result.evicted += 1;
            }
        }

        // 补充至 min_connections（spec §5.6 规则 3）
        let current = pool.connection_count().await?;
        if current < self.config.min_connections {
            result.replenished = pool.replenish(self.config.min_connections).await?;
        }

        Ok(result)
    }

    /// 启动健康检查驱逐循环
    ///
    /// 定期执行 `run_once`，间隔 `check_interval`。
    pub async fn run_loop(
        &self,
        pool: Arc<dyn HealthCheckablePool>,
    ) -> Result<(), PoolHealthError> {
        loop {
            self.run_once(pool.as_ref()).await?;
            tokio::time::sleep(self.config.check_interval).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct MockPool {
        connections: parking_lot::RwLock<Vec<ConnectionInfo>>,
        evict_count: AtomicU64,
        replenish_count: AtomicU64,
        ping_results: parking_lot::RwLock<std::collections::HashMap<u64, ConnectionHealth>>,
    }

    impl MockPool {
        fn new(connections: Vec<ConnectionInfo>) -> Self {
            Self {
                connections: parking_lot::RwLock::new(connections),
                evict_count: AtomicU64::new(0),
                replenish_count: AtomicU64::new(0),
                ping_results: parking_lot::RwLock::new(std::collections::HashMap::new()),
            }
        }

        fn set_ping_result(&self, conn_id: u64, health: ConnectionHealth) {
            self.ping_results.write().insert(conn_id, health);
        }
    }

    #[async_trait]
    impl HealthCheckablePool for MockPool {
        async fn list_connections(&self) -> Result<Vec<ConnectionInfo>, PoolHealthError> {
            Ok(self.connections.read().clone())
        }

        async fn ping(&self, conn_id: u64) -> Result<ConnectionHealth, PoolHealthError> {
            Ok(self
                .ping_results
                .read()
                .get(&conn_id)
                .cloned()
                .unwrap_or(ConnectionHealth::Healthy))
        }

        async fn evict(&self, conn_id: u64) -> Result<(), PoolHealthError> {
            self.evict_count.fetch_add(1, Ordering::SeqCst);
            self.connections.write().retain(|c| c.id != conn_id);
            Ok(())
        }

        async fn replenish(&self, target: usize) -> Result<u64, PoolHealthError> {
            let current = self.connections.read().len();
            if current >= target {
                return Ok(0);
            }
            let needed = (target - current) as u64;
            self.replenish_count.fetch_add(needed, Ordering::SeqCst);
            let mut conns = self.connections.write();
            for i in 0..needed {
                conns.push(ConnectionInfo {
                    id: 10000 + i,
                    is_idle: true,
                    last_used: Instant::now(),
                });
            }
            Ok(needed)
        }

        async fn connection_count(&self) -> Result<usize, PoolHealthError> {
            Ok(self.connections.read().len())
        }
    }

    fn make_conn(id: u64, idle: bool, age: Duration) -> ConnectionInfo {
        ConnectionInfo {
            id,
            is_idle: idle,
            last_used: Instant::now() - age,
        }
    }

    #[tokio::test]
    async fn test_evict_unhealthy() {
        let pool = MockPool::new(vec![
            make_conn(1, true, Duration::from_secs(10)),
            make_conn(2, true, Duration::from_secs(10)),
        ]);
        pool.set_ping_result(2, ConnectionHealth::Unhealthy("timeout".into()));
        let evictor = HealthCheckEvictor::new(HealthCheckConfig {
            check_interval: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(300),
            min_connections: 1,
        });
        let result = evictor.run_once(&pool).await.unwrap();
        assert_eq!(result.evicted, 1);
        assert_eq!(result.checked, 2);
        assert_eq!(result.idle_reclaimed, 0);
    }

    #[tokio::test]
    async fn test_reclaim_idle_timeout() {
        let pool = MockPool::new(vec![
            make_conn(1, true, Duration::from_secs(400)),
            make_conn(2, true, Duration::from_secs(10)),
        ]);
        let evictor = HealthCheckEvictor::new(HealthCheckConfig {
            check_interval: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(300),
            min_connections: 1,
        });
        let result = evictor.run_once(&pool).await.unwrap();
        assert_eq!(result.idle_reclaimed, 1);
        assert_eq!(result.checked, 1);
        assert_eq!(result.evicted, 0);
    }

    #[tokio::test]
    async fn test_replenish_to_min() {
        let pool = MockPool::new(vec![]);
        let evictor = HealthCheckEvictor::new(HealthCheckConfig {
            check_interval: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(300),
            min_connections: 5,
        });
        let result = evictor.run_once(&pool).await.unwrap();
        assert_eq!(result.replenished, 5);
    }

    #[tokio::test]
    async fn test_no_replenish_when_sufficient() {
        let pool = MockPool::new(vec![
            make_conn(1, false, Duration::from_secs(0)),
            make_conn(2, false, Duration::from_secs(0)),
            make_conn(3, false, Duration::from_secs(0)),
        ]);
        let evictor = HealthCheckEvictor::new(HealthCheckConfig {
            check_interval: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(300),
            min_connections: 2,
        });
        let result = evictor.run_once(&pool).await.unwrap();
        assert_eq!(result.replenished, 0);
    }

    #[tokio::test]
    async fn test_skip_active_connections() {
        let pool = MockPool::new(vec![
            make_conn(1, false, Duration::from_secs(400)),
            make_conn(2, true, Duration::from_secs(10)),
        ]);
        let evictor = HealthCheckEvictor::new(HealthCheckConfig {
            check_interval: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(300),
            min_connections: 1,
        });
        let result = evictor.run_once(&pool).await.unwrap();
        assert_eq!(result.checked, 1);
        assert_eq!(result.idle_reclaimed, 0);
    }

    #[tokio::test]
    async fn test_all_healthy_no_eviction() {
        let pool = MockPool::new(vec![
            make_conn(1, true, Duration::from_secs(10)),
            make_conn(2, true, Duration::from_secs(20)),
        ]);
        let evictor = HealthCheckEvictor::new(HealthCheckConfig {
            check_interval: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(300),
            min_connections: 1,
        });
        let result = evictor.run_once(&pool).await.unwrap();
        assert_eq!(result.evicted, 0);
        assert_eq!(result.checked, 2);
    }

    #[test]
    fn test_default_config() {
        let config = HealthCheckConfig::default();
        assert_eq!(config.check_interval, Duration::from_secs(60));
        assert_eq!(config.idle_timeout, Duration::from_secs(300));
        assert_eq!(config.min_connections, 5);
    }
}
