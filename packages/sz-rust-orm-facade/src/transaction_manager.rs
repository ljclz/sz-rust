//! 事务超时治理（v1.5.0 P1-2）
//!
//! 为数据库事务提供可配置超时，超时后自动回滚并释放连接，防止长事务阻塞。
//!
//! ## 使用示例
//!
//! ```ignore
//! use sz_rust_orm_facade::transaction_manager::TransactionManager;
//! use std::time::Duration;
//!
//! let mgr = TransactionManager::new(pool, Duration::from_secs(30));
//! let mut tx = mgr.begin().await?;
//! let conn = tx.connection();
//! conn.execute("UPDATE accounts SET balance = balance - 100 WHERE id = 1").await?;
//! tx.commit().await?; // 超时则自动回滚 + 返回 TransactionError::Timeout
//! ```
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use sz_orm_core::{DbError, Pool, PoolError, PooledConnection};
use tokio::sync::broadcast;
use tokio::time::Instant;

use crate::auto_rollback::{AutoRollbackGuard, TransactionState};

/// 事务超时错误
#[derive(Debug, thiserror::Error)]
pub enum TransactionError {
    /// 事务超时（spec 5.2.2）
    #[error("transaction {tx_id} timeout after {timeout:?}")]
    Timeout {
        /// 事务 ID
        tx_id: u64,
        /// 超时时长
        timeout: Duration,
    },

    /// 无效配置（spec 5.2.3 异常2：timeout=0）
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    /// 数据库错误
    #[error(transparent)]
    Db(#[from] DbError),

    /// 连接池错误
    #[error(transparent)]
    Pool(#[from] PoolError),
}

/// 事务超时事件（spec 5.2.1 规则3：超时事件通知）
#[derive(Debug, Clone)]
pub struct TransactionTimeoutEvent {
    /// 事务 ID
    pub tx_id: u64,
    /// 超时时长
    pub timeout: Duration,
}

/// 事务管理器
///
/// 封装连接池 + 事务超时治理，支持全局默认超时 + 单事务覆盖两级配置（spec 5.2.1 规则4）。
/// 运行时可通过 `set_default_timeout` 动态调整全局超时（ArcSwap，无重启，spec 5.2.1 规则5）。
pub struct TransactionManager {
    pool: Arc<Pool>,
    default_timeout: ArcSwap<Duration>,
    timeout_tx: broadcast::Sender<TransactionTimeoutEvent>,
}

/// 事务句柄
///
/// 持有 `AutoRollbackGuard` + 超时截止时间。
/// `commit` 时用 `tokio::time::timeout_at` 包装，超时则 rollback + 发出事件 + 返回 `TransactionError::Timeout`。
/// Drop 时由 `AutoRollbackGuard` 兜底回滚未提交事务。
pub struct TxHandle {
    guard: AutoRollbackGuard,
    deadline: Instant,
    timeout: Duration,
    timeout_tx: broadcast::Sender<TransactionTimeoutEvent>,
}

impl TransactionManager {
    /// 创建事务管理器
    ///
    /// `default_timeout` 应大于零；若为零，后续 `begin` 返回 `TransactionError::InvalidConfig`。
    pub fn new(pool: Arc<Pool>, default_timeout: Duration) -> Self {
        let (timeout_tx, _) = broadcast::channel(64);
        Self {
            pool,
            default_timeout: ArcSwap::from_pointee(default_timeout),
            timeout_tx,
        }
    }

    /// 使用全局默认超时开启事务
    pub async fn begin(&self) -> Result<TxHandle, TransactionError> {
        let timeout = **self.default_timeout.load();
        self.begin_with_timeout(timeout).await
    }

    /// 使用指定超时开启事务（覆盖全局默认，spec 5.2.1 规则4）
    pub async fn begin_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<TxHandle, TransactionError> {
        if timeout.is_zero() {
            return Err(TransactionError::InvalidConfig(
                "transaction timeout must be positive".to_string(),
            ));
        }

        let conn = self.pool.acquire().await?;
        let mut guard = AutoRollbackGuard::new(conn);
        guard.begin_transaction().await?;

        let deadline = Instant::now() + timeout;
        Ok(TxHandle {
            guard,
            deadline,
            timeout,
            timeout_tx: self.timeout_tx.clone(),
        })
    }

    /// 运行时动态调整全局默认超时（无重启，spec 5.2.1 规则5）
    pub fn set_default_timeout(&self, timeout: Duration) {
        self.default_timeout.store(Arc::new(timeout));
    }

    /// 获取当前全局默认超时
    pub fn default_timeout(&self) -> Duration {
        **self.default_timeout.load()
    }

    /// 订阅超时事件（spec 5.2.1 规则3）
    pub fn subscribe(&self) -> broadcast::Receiver<TransactionTimeoutEvent> {
        self.timeout_tx.subscribe()
    }
}

impl TxHandle {
    /// 提交事务，超时则自动回滚并发出事件（spec 5.2.1 规则2）
    pub async fn commit(&mut self) -> Result<(), TransactionError> {
        if self.is_timed_out() {
            self.fire_timeout_event();
            let tx_id = self.guard.tx_id();
            if let Err(e) = self.guard.rollback().await {
                tracing::error!(tx_id, "超时回滚失败: {:?}", e);
            }
            return Err(TransactionError::Timeout {
                tx_id,
                timeout: self.timeout,
            });
        }

        let deadline = self.deadline;
        let result = tokio::time::timeout_at(deadline, self.guard.commit()).await;
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e.into()),
            Err(_) => {
                self.fire_timeout_event();
                let tx_id = self.guard.tx_id();
                if let Err(e) = self.guard.rollback().await {
                    tracing::error!(tx_id, "超时回滚失败: {:?}", e);
                }
                Err(TransactionError::Timeout {
                    tx_id,
                    timeout: self.timeout,
                })
            }
        }
    }

    /// 回滚事务（不检查超时）
    pub async fn rollback(&mut self) -> Result<(), TransactionError> {
        self.guard.rollback().await?;
        Ok(())
    }

    /// 检查是否已超时，超时则回滚并返回错误
    pub async fn check_timeout(&mut self) -> Result<(), TransactionError> {
        if Instant::now() >= self.deadline {
            self.fire_timeout_event();
            let _ = self.guard.rollback().await;
            return Err(TransactionError::Timeout {
                tx_id: self.guard.tx_id(),
                timeout: self.timeout,
            });
        }
        Ok(())
    }

    /// 获取事务 ID
    pub fn tx_id(&self) -> u64 {
        self.guard.tx_id()
    }

    /// 获取超时时长
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// 获取超时截止时间（用户可用 `tokio::time::timeout_at` 自行包装 SQL 操作）
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// 获取事务状态
    pub fn state(&self) -> TransactionState {
        self.guard.state()
    }

    /// 获取连接的可变引用（用于执行 SQL）
    pub fn connection(&mut self) -> &mut PooledConnection {
        self.guard.connection()
    }

    /// 检查是否已超时
    pub fn is_timed_out(&self) -> bool {
        Instant::now() >= self.deadline
    }

    fn fire_timeout_event(&self) {
        let event = TransactionTimeoutEvent {
            tx_id: self.guard.tx_id(),
            timeout: self.timeout,
        };
        let _ = self.timeout_tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_error_display() {
        let err = TransactionError::Timeout {
            tx_id: 42,
            timeout: Duration::from_secs(5),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("42"));
        assert!(msg.contains("5s"));
    }

    #[test]
    fn test_invalid_config_error() {
        let err = TransactionError::InvalidConfig("timeout must be positive".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("invalid config"));
        assert!(msg.contains("timeout must be positive"));
    }

    #[test]
    fn test_timeout_event_fields() {
        let event = TransactionTimeoutEvent {
            tx_id: 100,
            timeout: Duration::from_millis(500),
        };
        assert_eq!(event.tx_id, 100);
        assert_eq!(event.timeout, Duration::from_millis(500));
    }

    #[test]
    fn test_transaction_error_db_from() {
        let db_err = DbError::Internal("test".to_string());
        let tx_err: TransactionError = db_err.into();
        assert!(matches!(tx_err, TransactionError::Db(_)));
    }

    #[test]
    fn test_transaction_error_pool_from() {
        let pool_err = PoolError::Closed;
        let tx_err: TransactionError = pool_err.into();
        assert!(matches!(tx_err, TransactionError::Pool(_)));
    }
}
