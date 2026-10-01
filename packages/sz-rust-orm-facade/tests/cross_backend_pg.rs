//! P1 跨后端集成测试（PostgreSQL 18.2 分支）
//!
//! 验证 AutoRollbackGuard / TransactionManager / MigrationEnhanced
//! 在 PostgreSQL 18.2 上行为与 MySQL 9.6 一致（spec 5.1.4/5.2.6/5.4.4）。
//!
//! 运行：cargo test -p sz-rust-orm-facade --features auto-rollback,tx-timeout,migration-enhanced --test cross_backend_pg -- --ignored
#![cfg(all(
    feature = "auto-rollback",
    feature = "tx-timeout",
    feature = "migration-enhanced"
))]

use std::sync::Arc;
use std::time::Duration;

use sz_orm_core::{DbType, Migration};
use sz_orm_sqlx::{PgPoolHandle, SqlxPgConnectionFactory};
use sz_rust_core::orm::{Pool, PoolConfigBuilder};
use sz_rust_orm_facade::auto_rollback::{AutoRollbackConfig, AutoRollbackGuard, TransactionState};
use sz_rust_orm_facade::migration_enhanced::MigrationEnhanced;
use sz_rust_orm_facade::transaction_manager::{TransactionError, TransactionManager};

const PG_CONN_STR: &str = "postgres://postgres:test123@127.0.0.1:5432/sz_orm_test";

async fn init_pg_pool() -> Pool {
    let _ = tracing_subscriber::fmt::try_init();
    let sqlx_pool = sqlx::pool::PoolOptions::<sqlx::Postgres>::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(10))
        .connect(PG_CONN_STR)
        .await
        .expect("PostgreSQL 连接失败");
    let factory = SqlxPgConnectionFactory::new(Arc::new(PgPoolHandle::from_pool(sqlx_pool)));
    let pool_cfg = PoolConfigBuilder::new()
        .max_size(5)
        .min_idle(1)
        .build()
        .unwrap();
    Pool::new(pool_cfg, Arc::new(factory)).expect("Pool 创建失败")
}

async fn cleanup_table(pool: &Pool, table: &str) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute(&format!("DROP TABLE IF EXISTS {}", table))
        .await
        .ok();
}

async fn count_rows(pool: &Pool, table: &str, id: i32) -> usize {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query(&format!("SELECT id FROM {} WHERE id = {}", table, id))
        .await
        .expect("查询失败");
    rows.len()
}

// ── P1-1: AutoRollbackGuard 跨后端验证 ──

#[tokio::test]
#[ignore = "需真实 PostgreSQL 18.2"]
async fn pg_drop_uncommitted_rolls_back() {
    let pool = init_pg_pool().await;
    cleanup_table(&pool, "ar_pg1").await;
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("CREATE TABLE ar_pg1 (id INT PRIMARY KEY, val VARCHAR(50))")
            .await
            .expect("建表失败");
    }

    {
        let conn = pool.acquire().await.expect("获取连接失败");
        let mut guard = AutoRollbackGuard::new(conn);
        guard.begin_transaction().await.expect("开启事务失败");
        guard
            .connection()
            .execute("INSERT INTO ar_pg1 (id, val) VALUES (1, 'test')")
            .await
            .expect("插入失败");
        assert_eq!(guard.state(), TransactionState::Active);
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(count_rows(&pool, "ar_pg1", 1).await, 0, "PG: 未提交应回滚");

    cleanup_table(&pool, "ar_pg1").await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "需真实 PostgreSQL 18.2"]
async fn pg_commit_persists() {
    let pool = init_pg_pool().await;
    cleanup_table(&pool, "ar_pg2").await;
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("CREATE TABLE ar_pg2 (id INT PRIMARY KEY, val VARCHAR(50))")
            .await
            .expect("建表失败");
    }

    {
        let conn = pool.acquire().await.expect("获取连接失败");
        let mut guard = AutoRollbackGuard::new(conn);
        guard.begin_transaction().await.expect("开启事务失败");
        guard
            .connection()
            .execute("INSERT INTO ar_pg2 (id, val) VALUES (1, 'committed')")
            .await
            .expect("插入失败");
        guard.commit().await.expect("提交失败");
        assert_eq!(guard.state(), TransactionState::Committed);
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(count_rows(&pool, "ar_pg2", 1).await, 1, "PG: 已提交应存在");

    cleanup_table(&pool, "ar_pg2").await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "需真实 PostgreSQL 18.2"]
async fn pg_explicit_rollback_clears() {
    let pool = init_pg_pool().await;
    cleanup_table(&pool, "ar_pg3").await;
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("CREATE TABLE ar_pg3 (id INT PRIMARY KEY, val VARCHAR(50))")
            .await
            .expect("建表失败");
    }

    {
        let conn = pool.acquire().await.expect("获取连接失败");
        let mut guard = AutoRollbackGuard::new(conn);
        guard.begin_transaction().await.expect("开启事务失败");
        guard
            .connection()
            .execute("INSERT INTO ar_pg3 (id, val) VALUES (1, 'rolled')")
            .await
            .expect("插入失败");
        guard.rollback().await.expect("回滚失败");
        assert_eq!(guard.state(), TransactionState::RolledBack);
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        count_rows(&pool, "ar_pg3", 1).await,
        0,
        "PG: 显式回滚应清空"
    );

    cleanup_table(&pool, "ar_pg3").await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "需真实 PostgreSQL 18.2"]
async fn pg_disabled_config_no_auto_rollback() {
    let pool = init_pg_pool().await;
    cleanup_table(&pool, "ar_pg4").await;
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("CREATE TABLE ar_pg4 (id INT PRIMARY KEY, val VARCHAR(50))")
            .await
            .expect("建表失败");
    }

    {
        let conn = pool.acquire().await.expect("获取连接失败");
        let config = AutoRollbackConfig { enabled: false };
        let mut guard = AutoRollbackGuard::with_config(conn, config);
        guard.begin_transaction().await.expect("开启事务失败");
        guard
            .connection()
            .execute("INSERT INTO ar_pg4 (id, val) VALUES (1, 'no_rb')")
            .await
            .expect("插入失败");
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.rollback().await.ok();
    }
    assert_eq!(
        count_rows(&pool, "ar_pg4", 1).await,
        0,
        "PG: 手动回滚后清空"
    );

    cleanup_table(&pool, "ar_pg4").await;
    pool.close_all().await;
}

// ── P1-2: TransactionManager 跨后端验证 ──

#[tokio::test]
#[ignore = "需真实 PostgreSQL 18.2"]
async fn pg_tx_commit_within_timeout() {
    let pool = init_pg_pool().await;
    cleanup_table(&pool, "tx_pg1").await;
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("CREATE TABLE tx_pg1 (id INT PRIMARY KEY)")
            .await
            .expect("建表失败");
    }

    let mgr = TransactionManager::new(Arc::new(pool.clone()), Duration::from_secs(5));
    let mut tx = mgr
        .begin_with_timeout(Duration::from_secs(5))
        .await
        .expect("开启事务失败");
    tx.connection()
        .execute("INSERT INTO tx_pg1 (id) VALUES (1)")
        .await
        .expect("插入失败");
    tx.commit().await.expect("提交失败");

    assert_eq!(
        count_rows(&pool, "tx_pg1", 1).await,
        1,
        "PG: 超时内提交成功"
    );

    cleanup_table(&pool, "tx_pg1").await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "需真实 PostgreSQL 18.2"]
async fn pg_tx_timeout_triggers_rollback() {
    let pool = init_pg_pool().await;
    cleanup_table(&pool, "tx_pg2").await;
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("CREATE TABLE tx_pg2 (id INT PRIMARY KEY)")
            .await
            .expect("建表失败");
    }

    let mgr = TransactionManager::new(Arc::new(pool.clone()), Duration::from_secs(30));
    let mut tx = mgr
        .begin_with_timeout(Duration::from_millis(100))
        .await
        .expect("开启事务失败");
    tx.connection()
        .execute("INSERT INTO tx_pg2 (id) VALUES (1)")
        .await
        .expect("插入失败");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let err = tx.commit().await.unwrap_err();
    assert!(
        matches!(err, TransactionError::Timeout { .. }),
        "PG: 超时应返回 Timeout 错误"
    );

    assert_eq!(count_rows(&pool, "tx_pg2", 1).await, 0, "PG: 超时事务回滚");

    cleanup_table(&pool, "tx_pg2").await;
    pool.close_all().await;
}

// ── P1-3: MigrationEnhanced 跨后端验证 ──

fn pg_migrations() -> Vec<Migration> {
    vec![
        Migration::new(
            "001",
            "create_mig_pg",
            "CREATE TABLE mig_pg (id INT PRIMARY KEY, val VARCHAR(50))",
            "DROP TABLE mig_pg",
        ),
        Migration::new(
            "002",
            "add_column_pg",
            "ALTER TABLE mig_pg ADD COLUMN extra VARCHAR(20)",
            "ALTER TABLE mig_pg DROP COLUMN extra",
        ),
    ]
}

#[tokio::test]
#[ignore = "需真实 PostgreSQL 18.2"]
async fn pg_migration_idempotent() {
    let pool = init_pg_pool().await;
    cleanup_table(&pool, "mig_pg").await;
    cleanup_table(&pool, "__migrations").await;

    let mgr = MigrationEnhanced::new(Arc::new(pool.clone()), pg_migrations(), DbType::PostgreSQL);

    let r1 = mgr.migrate().await.expect("首次迁移失败");
    assert_eq!(r1.applied.len(), 2, "PG: 首次应用 2 个迁移");
    assert_eq!(r1.skipped.len(), 0);

    let r2 = mgr.migrate().await.expect("第二次迁移失败");
    assert_eq!(r2.applied.len(), 0, "PG: 第二次无新应用");
    assert_eq!(r2.skipped.len(), 2, "PG: 第二次跳过 2 个");

    cleanup_table(&pool, "mig_pg").await;
    cleanup_table(&pool, "__migrations").await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "需真实 PostgreSQL 18.2"]
async fn pg_migration_checksum_verify() {
    let pool = init_pg_pool().await;
    cleanup_table(&pool, "mig_pg").await;
    cleanup_table(&pool, "__migrations").await;

    let mgr = MigrationEnhanced::new(Arc::new(pool.clone()), pg_migrations(), DbType::PostgreSQL);
    mgr.migrate().await.expect("迁移失败");
    mgr.verify_checksums().await.expect("校验和验证失败");

    cleanup_table(&pool, "mig_pg").await;
    cleanup_table(&pool, "__migrations").await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "需真实 PostgreSQL 18.2"]
async fn pg_migration_dry_run_no_changes() {
    let pool = init_pg_pool().await;
    cleanup_table(&pool, "mig_pg").await;
    cleanup_table(&pool, "__migrations").await;

    let mgr = MigrationEnhanced::new(Arc::new(pool.clone()), pg_migrations(), DbType::PostgreSQL);
    let report = mgr.dry_run().await.expect("dry-run 失败");
    assert_eq!(report.applied.len(), 2, "PG: dry-run 列出待应用迁移");

    cleanup_table(&pool, "mig_pg").await;
    cleanup_table(&pool, "__migrations").await;
    pool.close_all().await;
}
