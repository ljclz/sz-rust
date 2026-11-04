// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//!
//! 回归测试：`db::acquire_cancel_safe` 在调用方被取消时不泄漏连接池槽位。
//!
//! 背景：上游 `Pool::acquire` 非取消安全（先 CAS 递增 `total_count`，再 `await` 建连，
//! 在 await 点被取消时回滚分支不执行）。未包装时累计 `max_size` 次取消即让连接池
//! 永久卡死：`total_count == max_size` 但没有任何可用连接，只能重启进程恢复。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sz_rust_core::orm::{Connection, ConnectionFactory, DbError, Pool, PoolConfigBuilder, Value};
use sz_rust_sz300::db::acquire_cancel_safe;

/// 建连耗时可控的假连接（模拟慢库建连）
struct SlowFactory {
    delay: Duration,
}

/// 无状态桩连接：仅满足 trait，不做任何 IO
struct StubConn;

impl Connection for StubConn {
    fn execute<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<u64, DbError>> + Send + 'a>> {
        Box::pin(async { Ok(0) })
    }

    fn query<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<HashMap<String, Value>>, DbError>> + Send + 'a>>
    {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn begin_transaction<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn commit<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn rollback<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    fn is_connected(&self) -> bool {
        true
    }

    fn ping<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async { true })
    }

    fn close<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[async_trait]
impl ConnectionFactory for SlowFactory {
    async fn create(&self) -> Result<Box<dyn Connection>, DbError> {
        tokio::time::sleep(self.delay).await;
        Ok(Box::new(StubConn))
    }
}

fn build_pool(max_size: u32, create_delay: Duration) -> Pool {
    let cfg = PoolConfigBuilder::new()
        .max_size(max_size)
        .min_idle(0)
        .build()
        .expect("构造 PoolConfig 失败");
    Pool::new(
        cfg,
        Arc::new(SlowFactory {
            delay: create_delay,
        }),
    )
    .expect("构造 Pool 失败")
}

/// 连续取消 `max_size` 次建连中的 acquire 后，池必须仍可服务
#[tokio::test]
async fn cancelled_acquire_does_not_wedge_pool() {
    const MAX: u32 = 2;
    let pool = build_pool(MAX, Duration::from_millis(200));

    // 两个并发 acquire 在「建连中」被取消：恰好占满 max_size 个槽位
    let a = tokio::time::timeout(Duration::from_millis(20), acquire_cancel_safe(&pool)).await;
    let b = tokio::time::timeout(Duration::from_millis(20), acquire_cancel_safe(&pool)).await;
    assert!(
        a.is_err() && b.is_err(),
        "建连期间不应完成（否则用例未覆盖泄漏路径）"
    );

    // 等后台任务建连完成并归还被丢弃的连接
    tokio::time::sleep(Duration::from_millis(300)).await;

    let conn = tokio::time::timeout(Duration::from_secs(1), acquire_cancel_safe(&pool))
        .await
        .expect("取消 2 次后连接池已卡死——槽位泄漏回归");
    assert!(
        conn.is_ok(),
        "取消后 acquire 应成功，实际错误: {:?}",
        conn.err()
    );
}

/// 未取消的获取路径不受包装影响：连接可正常取得并归还
#[tokio::test]
async fn acquire_returns_connection_and_reuses_slot() {
    let pool = build_pool(1, Duration::from_millis(10));

    for _ in 0..3 {
        let conn = tokio::time::timeout(Duration::from_secs(2), acquire_cancel_safe(&pool))
            .await
            .expect("acquire 超时")
            .expect("acquire 失败");
        drop(conn);
    }
}
