//! 事务隔离测试夹具（v1.5.0 P1-4）
//!
//! 每个测试在独立事务中执行，结束后自动回滚，确保测试间无状态共享（spec 5.4.5）。
//!
//! ## 使用示例
//!
//! ```ignore
//! use sz_rust_testkit::tx_isolation_fixture::TxIsolationFixture;
//! use std::sync::Arc;
//!
//! let fixture = TxIsolationFixture::new(Arc::new(pool));
//! let mut guard = fixture.begin().await?;
//! guard.connection().execute("INSERT INTO users (id) VALUES (1)").await?;
//! fixture.end(guard).await?; // 自动回滚
//! ```
#![forbid(unsafe_code)]

use std::sync::Arc;

use sz_rust_orm_facade::auto_rollback::{AutoRollbackGuard, TransactionState};
use sz_rust_orm_facade::Pool;

use crate::test_error::TestError;

/// 事务隔离测试夹具
///
/// 为每个测试提供独立事务，测试结束后自动回滚，防止测试间状态污染。
pub struct TxIsolationFixture {
    pool: Arc<Pool>,
}

impl TxIsolationFixture {
    /// 创建事务隔离夹具
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }

    /// 开启独立事务，返回 AutoRollbackGuard（spec 5.4.5）
    pub async fn begin(&self) -> Result<AutoRollbackGuard, TestError> {
        let conn = self.pool.acquire().await?;
        let mut guard = AutoRollbackGuard::new(conn);
        guard.begin_transaction().await?;
        Ok(guard)
    }

    /// 结束事务并自动回滚（spec 5.4.5）
    ///
    /// 若事务仍为 Active 状态则回滚；已 commit/rollback 则无操作。
    pub async fn end(&self, mut guard: AutoRollbackGuard) -> Result<(), TestError> {
        if guard.state() == TransactionState::Active {
            guard.rollback().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tx_isolation_fixture_is_send_sync() {
        // 夹具会被移动到异步测试任务中跨 .await 使用（见 tests/p1_4_testkit.rs），
        // Send + Sync 是编译期硬约束：若未来字段破坏该约束，本测试将无法编译。
        fn is_send_sync<T: Send + Sync>() -> bool {
            let _ = std::marker::PhantomData::<T>;
            true
        }
        assert!(
            is_send_sync::<TxIsolationFixture>(),
            "TxIsolationFixture 必须满足 Send + Sync（异步测试夹具的编译期要求）"
        );
    }
}
