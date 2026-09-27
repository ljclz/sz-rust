//! 连接池自动回滚守卫（v1.5.0 P1-1）
//!
//! 修复 v1.4.0 [high] 缺陷：PooledConnection drop 未提交事务时自动 rollback 释放锁。
//!
//! ## 使用示例
//!
//! ```ignore
//! use sz_rust_orm_facade::auto_rollback::AutoRollbackGuard;
//!
//! let conn = pool.acquire().await?;
//! let mut guard = AutoRollbackGuard::new(conn);
//! guard.begin_transaction().await?;
//! // SQL 操作...
//! guard.commit().await?; // 显式提交
//! // 或：不调用 commit，drop 时自动 rollback
//! ```
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};

use sz_orm_core::PooledConnection;
use tracing::{error, warn};

static TX_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 事务状态机
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    /// 未开启事务
    Idle,
    /// 事务进行中
    Active,
    /// 已提交
    Committed,
    /// 已回滚
    RolledBack,
}

/// 自动回滚配置
#[derive(Debug, Clone)]
pub struct AutoRollbackConfig {
    /// 是否启用自动回滚（默认 true）
    pub enabled: bool,
}

impl Default for AutoRollbackConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// 连接池自动回滚守卫
///
/// 包装 `PooledConnection`，在 Drop 时检测未提交事务并自动 rollback。
/// 确保未提交事务的连接不会被归还到池中复用，避免锁残留。
pub struct AutoRollbackGuard {
    conn: Option<PooledConnection>,
    state: TransactionState,
    config: AutoRollbackConfig,
    tx_id: u64,
}

impl AutoRollbackGuard {
    /// 创建新的自动回滚守卫
    pub fn new(conn: PooledConnection) -> Self {
        Self::with_config(conn, AutoRollbackConfig::default())
    }

    /// 使用指定配置创建守卫
    pub fn with_config(conn: PooledConnection, config: AutoRollbackConfig) -> Self {
        let tx_id = TX_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self {
            conn: Some(conn),
            state: TransactionState::Idle,
            config,
            tx_id,
        }
    }

    /// 开启事务
    pub async fn begin_transaction(&mut self) -> Result<(), sz_orm_core::DbError> {
        let conn = self.conn.as_mut().expect("connection already taken");
        conn.begin_transaction().await?;
        self.state = TransactionState::Active;
        Ok(())
    }

    /// 提交事务
    pub async fn commit(&mut self) -> Result<(), sz_orm_core::DbError> {
        let conn = self.conn.as_mut().expect("connection already taken");
        conn.commit().await?;
        self.state = TransactionState::Committed;
        Ok(())
    }

    /// 回滚事务
    pub async fn rollback(&mut self) -> Result<(), sz_orm_core::DbError> {
        let conn = self.conn.as_mut().expect("connection already taken");
        conn.rollback().await?;
        self.state = TransactionState::RolledBack;
        Ok(())
    }

    /// 获取当前事务状态
    pub fn state(&self) -> TransactionState {
        self.state
    }

    /// 获取事务 ID
    pub fn tx_id(&self) -> u64 {
        self.tx_id
    }

    /// 获取连接的可变引用（用于执行 SQL）
    pub fn connection(&mut self) -> &mut PooledConnection {
        self.conn.as_mut().expect("connection already taken")
    }

    /// 消费守卫，返回内部连接（仅允许在 Committed/RolledBack/Idle 状态下调用）
    pub fn into_inner(mut self) -> PooledConnection {
        self.conn.take().expect("connection already taken")
    }
}

impl Drop for AutoRollbackGuard {
    fn drop(&mut self) {
        if let Some(mut conn) = self.conn.take() {
            if self.config.enabled && self.state == TransactionState::Active {
                warn!(tx_id = self.tx_id, "事务未提交，自动回滚中");
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        if let Err(e) = conn.rollback().await {
                            error!("自动回滚失败: {:?}", e);
                        }
                        drop(conn);
                    });
                } else {
                    error!(
                        tx_id = self.tx_id,
                        "不在 tokio runtime 中，无法自动回滚，连接将被销毁"
                    );
                    drop(conn);
                }
            } else {
                drop(conn);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_state_equality() {
        assert_eq!(TransactionState::Idle, TransactionState::Idle);
        assert_ne!(TransactionState::Active, TransactionState::Committed);
        assert_ne!(TransactionState::Committed, TransactionState::RolledBack);
    }

    #[test]
    fn test_auto_rollback_config_default() {
        let config = AutoRollbackConfig::default();
        assert!(config.enabled);
    }

    #[test]
    fn test_tx_id_monotonic() {
        let id1 = TX_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id2 = TX_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        assert!(id2 > id1, "tx_id should be monotonically increasing");
    }
}
