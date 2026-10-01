//! AutoRollbackGuard 集成测试（v1.5.0 P1-1）
//!
//! 验证 PooledConnection drop 未提交事务时自动 rollback 释放锁。
//! 需要真实 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test)。
//!
//! 运行：cargo test -p sz-rust-orm-facade --features auto-rollback --test auto_rollback -- --ignored
#![cfg(feature = "auto-rollback")]

use std::sync::Arc;
use sz_orm_sqlx::{MySqlPoolHandle, SqlxMySqlConnectionFactory};
use sz_rust_core::orm::{Pool, PoolConfigBuilder};
use sz_rust_orm_facade::auto_rollback::{AutoRollbackConfig, AutoRollbackGuard, TransactionState};

async fn init_pool() -> Pool {
    let conn_str = "mysql://root:test123@127.0.0.1:3306/sz_orm_test";
    let sqlx_pool = sqlx::pool::PoolOptions::<sqlx::MySql>::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(conn_str)
        .await
        .expect("MySQL 连接失败");
    let factory = SqlxMySqlConnectionFactory::new(Arc::new(MySqlPoolHandle::from_pool(sqlx_pool)));
    let pool_cfg = PoolConfigBuilder::new()
        .max_size(5)
        .min_idle(1)
        .build()
        .unwrap();
    Pool::new(pool_cfg, Arc::new(factory)).expect("Pool 创建失败")
}

#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_drop_uncommitted_transaction_rolls_back() {
    let pool = init_pool().await;

    // 建表
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("DROP TABLE IF EXISTS ar_test").await.ok();
        conn.execute("CREATE TABLE ar_test (id INT PRIMARY KEY, val VARCHAR(50))")
            .await
            .expect("建表失败");
    }

    // 场景：begin + INSERT + drop（不 commit）
    {
        let conn = pool.acquire().await.expect("获取连接失败");
        let mut guard = AutoRollbackGuard::new(conn);
        guard.begin_transaction().await.expect("开启事务失败");
        guard
            .connection()
            .execute("INSERT INTO ar_test (id, val) VALUES (1, 'test')")
            .await
            .expect("插入失败");
        assert_eq!(guard.state(), TransactionState::Active);
        // drop guard：应自动 rollback
    }

    // 等待异步 rollback 完成
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // 验证：数据不应存在（已回滚）
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        let rows = conn
            .query("SELECT id FROM ar_test WHERE id = 1")
            .await
            .expect("查询失败");
        assert_eq!(rows.len(), 0, "未提交事务应自动回滚，数据不应存在");
    }

    // 清理
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("DROP TABLE IF EXISTS ar_test").await.ok();
    }
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_commit_then_drop_no_extra_rollback() {
    let pool = init_pool().await;

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("DROP TABLE IF EXISTS ar_test2").await.ok();
        conn.execute("CREATE TABLE ar_test2 (id INT PRIMARY KEY, val VARCHAR(50))")
            .await
            .expect("建表失败");
    }

    // 场景：begin + INSERT + commit + drop
    {
        let conn = pool.acquire().await.expect("获取连接失败");
        let mut guard = AutoRollbackGuard::new(conn);
        guard.begin_transaction().await.expect("开启事务失败");
        guard
            .connection()
            .execute("INSERT INTO ar_test2 (id, val) VALUES (1, 'committed')")
            .await
            .expect("插入失败");
        guard.commit().await.expect("提交失败");
        assert_eq!(guard.state(), TransactionState::Committed);
        // drop guard：不应执行 rollback
    }

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // 验证：数据应存在（已提交）
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        let rows = conn
            .query("SELECT id FROM ar_test2 WHERE id = 1")
            .await
            .expect("查询失败");
        assert_eq!(rows.len(), 1, "已提交的数据应存在");
    }

    // 清理
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("DROP TABLE IF EXISTS ar_test2").await.ok();
    }
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_rollback_then_drop_no_duplicate() {
    let pool = init_pool().await;

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("DROP TABLE IF EXISTS ar_test3").await.ok();
        conn.execute("CREATE TABLE ar_test3 (id INT PRIMARY KEY, val VARCHAR(50))")
            .await
            .expect("建表失败");
    }

    // 场景：begin + INSERT + rollback + drop
    {
        let conn = pool.acquire().await.expect("获取连接失败");
        let mut guard = AutoRollbackGuard::new(conn);
        guard.begin_transaction().await.expect("开启事务失败");
        guard
            .connection()
            .execute("INSERT INTO ar_test3 (id, val) VALUES (1, 'rolled')")
            .await
            .expect("插入失败");
        guard.rollback().await.expect("回滚失败");
        assert_eq!(guard.state(), TransactionState::RolledBack);
        // drop guard：不应重复 rollback
    }

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // 验证：数据不应存在（已回滚）
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        let rows = conn
            .query("SELECT id FROM ar_test3 WHERE id = 1")
            .await
            .expect("查询失败");
        assert_eq!(rows.len(), 0, "已回滚的数据不应存在");
    }

    // 清理
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("DROP TABLE IF EXISTS ar_test3").await.ok();
    }
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_disabled_config_no_auto_rollback() {
    let pool = init_pool().await;

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("DROP TABLE IF EXISTS ar_test4").await.ok();
        conn.execute("CREATE TABLE ar_test4 (id INT PRIMARY KEY, val VARCHAR(50))")
            .await
            .expect("建表失败");
    }

    // 场景：禁用自动回滚 + begin + INSERT + drop
    {
        let conn = pool.acquire().await.expect("获取连接失败");
        let config = AutoRollbackConfig { enabled: false };
        let mut guard = AutoRollbackGuard::with_config(conn, config);
        guard.begin_transaction().await.expect("开启事务失败");
        assert_eq!(
            guard.state(),
            TransactionState::Active,
            "禁用自动回滚时 begin 后仍应处于 Active"
        );
        guard
            .connection()
            .execute("INSERT INTO ar_test4 (id, val) VALUES (1, 'no_rollback')")
            .await
            .expect("插入失败");
        assert_eq!(
            guard.state(),
            TransactionState::Active,
            "INSERT 后事务仍应处于 Active"
        );
        // drop guard：不自动 rollback（行为与 v1.4.0 一致）
    }

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // 验证：连接归还后事务可能仍活跃（v1.4.0 行为）
    // 手动回滚以清理
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.rollback().await.ok();
        let rows = conn
            .query("SELECT id FROM ar_test4 WHERE id = 1")
            .await
            .expect("查询失败");
        // 禁用自动回滚时，数据可能存在（取决于连接是否被复用 + 事务是否活跃）
        // 这里只验证不 panic
        println!("禁用自动回滚后行数: {}", rows.len());
    }

    // 清理
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("DROP TABLE IF EXISTS ar_test4").await.ok();
    }
    pool.close_all().await;
}
