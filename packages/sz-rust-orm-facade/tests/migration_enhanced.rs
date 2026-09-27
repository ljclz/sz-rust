//! MigrationEnhanced 集成测试（v1.5.0 P1-3）
//!
//! 验证迁移幂等性、校验和验证、回滚、dry-run、并发锁。
//! 需要真实 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test)。
//!
//! 运行：cargo test -p sz-rust-orm-facade --features migration-enhanced --test migration_enhanced -- --ignored
#![cfg(feature = "migration-enhanced")]

use std::sync::Arc;

use sz_orm_core::{DbType, Migration};
use sz_orm_sqlx::{MySqlPoolHandle, SqlxMySqlConnectionFactory};
use sz_rust_core::orm::{Pool, PoolConfigBuilder};
use sz_rust_orm_facade::migration_enhanced::{MigrationEnhanced, MigrationError};

async fn init_pool() -> Pool {
    let _ = tracing_subscriber::fmt::try_init();
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

async fn cleanup_table(pool: &Pool, table: &str) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute(&format!("DROP TABLE IF EXISTS {}", table))
        .await
        .ok();
}

fn test_migrations() -> Vec<Migration> {
    vec![
        Migration::new(
            "001",
            "create_mig_test",
            "CREATE TABLE mig_test (id INT PRIMARY KEY, val VARCHAR(50))",
            "DROP TABLE mig_test",
        ),
        Migration::new(
            "002",
            "add_column",
            "ALTER TABLE mig_test ADD COLUMN extra VARCHAR(20)",
            "ALTER TABLE mig_test DROP COLUMN extra",
        ),
    ]
}

/// 重复执行同一迁移 → 跳过已应用版本（spec 5.3.1 规则1）
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_migrate_idempotent_skip_applied() {
    let pool = init_pool().await;
    cleanup_table(&pool, "mig_test").await;
    cleanup_table(&pool, "__migrations").await;

    let mgr = MigrationEnhanced::new(Arc::new(pool.clone()), test_migrations(), DbType::MySQL);

    let report1 = mgr.migrate().await.expect("第一次迁移失败");
    assert_eq!(report1.applied.len(), 2, "首次应应用 2 个迁移");
    assert_eq!(report1.skipped.len(), 0);

    let report2 = mgr.migrate().await.expect("第二次迁移失败");
    assert_eq!(report2.applied.len(), 0, "第二次应无新应用");
    assert_eq!(report2.skipped.len(), 2, "第二次应跳过 2 个已应用迁移");

    cleanup_table(&pool, "mig_test").await;
    cleanup_table(&pool, "__migrations").await;
    pool.close_all().await;
}

/// 篡改已应用迁移脚本 → 校验失败 + 报告不匹配版本（spec 5.3.1 规则2）
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_checksum_mismatch_detected() {
    let pool = init_pool().await;
    cleanup_table(&pool, "mig_test").await;
    cleanup_table(&pool, "__migrations").await;

    let migrations = test_migrations();
    let mgr = MigrationEnhanced::new(Arc::new(pool.clone()), migrations, DbType::MySQL);
    mgr.migrate().await.expect("首次迁移失败");

    let tampered = vec![
        Migration::new(
            "001",
            "create_mig_test",
            "CREATE TABLE mig_test (id INT PRIMARY KEY, val VARCHAR(100))",
            "DROP TABLE mig_test",
        ),
        Migration::new(
            "002",
            "add_column",
            "ALTER TABLE mig_test ADD COLUMN extra VARCHAR(20)",
            "ALTER TABLE mig_test DROP COLUMN extra",
        ),
    ];
    let mgr2 = MigrationEnhanced::new(Arc::new(pool.clone()), tampered, DbType::MySQL);
    let err = mgr2.migrate().await.unwrap_err();
    assert!(
        matches!(err, MigrationError::ChecksumMismatch(ref v) if v == "001"),
        "应检测到版本 001 校验和不匹配，实际: {:?}",
        err
    );

    cleanup_table(&pool, "mig_test").await;
    cleanup_table(&pool, "__migrations").await;
    pool.close_all().await;
}

/// 回滚到版本 N → 版本 > N 全部回滚（spec 5.3.1 规则3）
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_rollback_to_version() {
    let pool = init_pool().await;
    cleanup_table(&pool, "mig_test").await;
    cleanup_table(&pool, "__migrations").await;

    let mgr = MigrationEnhanced::new(Arc::new(pool.clone()), test_migrations(), DbType::MySQL);
    mgr.migrate().await.expect("迁移失败");

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        let rows = conn
            .query("SHOW COLUMNS FROM mig_test LIKE 'extra'")
            .await
            .expect("查询失败");
        assert_eq!(rows.len(), 1, "extra 列应存在");
    }

    let report = mgr.rollback_to("001").await.expect("回滚失败");
    assert_eq!(report.applied.len(), 1, "应回滚 1 个迁移（版本 002）");

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        let rows = conn
            .query("SHOW COLUMNS FROM mig_test LIKE 'extra'")
            .await
            .expect("查询失败");
        assert_eq!(rows.len(), 0, "extra 列应已回滚删除");
    }

    cleanup_table(&pool, "mig_test").await;
    cleanup_table(&pool, "__migrations").await;
    pool.close_all().await;
}

/// dry-run → 数据库无变更（spec 5.3.1 规则6）
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_dry_run_no_changes() {
    let pool = init_pool().await;
    cleanup_table(&pool, "mig_test").await;
    cleanup_table(&pool, "__migrations").await;

    let mgr = MigrationEnhanced::new(Arc::new(pool.clone()), test_migrations(), DbType::MySQL);

    let report = mgr.dry_run().await.expect("dry-run 失败");
    assert_eq!(report.applied.len(), 2, "dry-run 应报告 2 个待应用迁移");

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        let rows = conn
            .query("SHOW TABLES LIKE 'mig_test'")
            .await
            .expect("查询失败");
        assert_eq!(rows.len(), 0, "dry-run 不应实际建表");
    }

    cleanup_table(&pool, "__migrations").await;
    pool.close_all().await;
}

/// verify_checksums：校验通过返回 Ok，篡改返回 Err
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_verify_checksums() {
    let pool = init_pool().await;
    cleanup_table(&pool, "mig_test").await;
    cleanup_table(&pool, "__migrations").await;

    let migrations = test_migrations();
    let mgr = MigrationEnhanced::new(Arc::new(pool.clone()), migrations.clone(), DbType::MySQL);
    mgr.migrate().await.expect("迁移失败");

    mgr.verify_checksums().await.expect("校验和应匹配");

    let tampered = vec![
        Migration::new(
            "001",
            "create_mig_test",
            "CREATE TABLE mig_test (id INT PRIMARY KEY, val VARCHAR(200))",
            "DROP TABLE mig_test",
        ),
        migrations[1].clone(),
    ];
    let mgr2 = MigrationEnhanced::new(Arc::new(pool.clone()), tampered, DbType::MySQL);
    let err = mgr2.verify_checksums().await.unwrap_err();
    assert!(matches!(err, MigrationError::ChecksumMismatch(_)));

    cleanup_table(&pool, "mig_test").await;
    cleanup_table(&pool, "__migrations").await;
    pool.close_all().await;
}

/// 并发启动两个迁移 → 其中一个被锁阻塞（spec 5.3.1 规则8）
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_concurrent_migration_lock_conflict() {
    let pool = init_pool().await;
    cleanup_table(&pool, "mig_concurrent").await;
    cleanup_table(&pool, "__migrations").await;

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("SELECT GET_LOCK('sz_migration_lock', 0)")
            .await
            .expect("获取锁失败");
    }

    let migrations = vec![Migration::new(
        "001",
        "create_concurrent",
        "CREATE TABLE mig_concurrent (id INT PRIMARY KEY)",
        "DROP TABLE mig_concurrent",
    )];
    let mgr = MigrationEnhanced::new(Arc::new(pool.clone()), migrations, DbType::MySQL);
    let err = mgr.migrate().await.unwrap_err();
    assert!(
        matches!(err, MigrationError::LockConflict),
        "应返回锁冲突错误，实际: {:?}",
        err
    );

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("SELECT RELEASE_LOCK('sz_migration_lock')")
            .await
            .expect("释放锁失败");
    }

    cleanup_table(&pool, "mig_concurrent").await;
    cleanup_table(&pool, "__migrations").await;
    pool.close_all().await;
}

/// 迁移报告结构正确：applied + skipped + failed
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_migration_report_structure() {
    let pool = init_pool().await;
    cleanup_table(&pool, "mig_report").await;
    cleanup_table(&pool, "__migrations").await;

    let migrations = vec![
        Migration::new(
            "001",
            "create_report_test",
            "CREATE TABLE mig_report (id INT PRIMARY KEY)",
            "DROP TABLE mig_report",
        ),
        Migration::new("002", "bad_migration", "INVALID SQL SYNTAX HERE !!!", ""),
    ];
    let mgr = MigrationEnhanced::new(Arc::new(pool.clone()), migrations, DbType::MySQL);
    let report = mgr.migrate().await.expect("迁移不应 panic");

    assert_eq!(report.applied.len(), 1, "应成功应用 1 个迁移");
    assert_eq!(report.failed.len(), 1, "应失败 1 个迁移");
    assert_eq!(report.failed[0].version, "002", "失败版本应为 002");

    cleanup_table(&pool, "mig_report").await;
    cleanup_table(&pool, "__migrations").await;
    pool.close_all().await;
}
