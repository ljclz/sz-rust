//! P1-4 testkit 增强集成测试
//!
//! 验证 TxIsolationFixture + TestTimeoutGuard + ResourceLeakScanner。
//! 需要真实 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test)。
//!
//! 运行：cargo test -p sz-rust-testkit --features test-utils --test p1_4_testkit -- --ignored
#![cfg(feature = "test-utils")]

use std::sync::Arc;
use std::time::Duration;

use sz_orm_sqlx::{MySqlPoolHandle, SqlxMySqlConnectionFactory};
use sz_rust_core::orm::{Pool, PoolConfigBuilder};
use sz_rust_testkit::{ResourceLeakScanner, TestError, TestTimeoutGuard, TxIsolationFixture};

async fn init_pool() -> Pool {
    let _ = tracing_subscriber::fmt::try_init();
    let conn_str = "mysql://root:test123@127.0.0.1:3306/sz_orm_test";
    let sqlx_pool = sqlx::pool::PoolOptions::<sqlx::MySql>::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(10))
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

/// TxIsolationFixture：测试结束后事务自动回滚（spec 5.4.5）
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_tx_isolation_auto_rollback() {
    let pool = init_pool().await;

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("DROP TABLE IF EXISTS iso_test").await.ok();
        conn.execute("CREATE TABLE iso_test (id INT PRIMARY KEY, val VARCHAR(50))")
            .await
            .expect("建表失败");
    }

    let fixture = TxIsolationFixture::new(Arc::new(pool.clone()));

    let mut guard = fixture.begin().await.expect("开启事务失败");
    guard
        .connection()
        .execute("INSERT INTO iso_test (id, val) VALUES (1, 'isolated')")
        .await
        .expect("插入失败");
    fixture.end(guard).await.expect("结束事务失败");

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        let rows = conn
            .query("SELECT id FROM iso_test WHERE id = 1")
            .await
            .expect("查询失败");
        assert_eq!(rows.len(), 0, "事务应已回滚，数据不应存在");
    }

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("DROP TABLE IF EXISTS iso_test").await.ok();
    }
    pool.close_all().await;
}

/// TestTimeoutGuard：超时强制失败（spec 5.4.3）
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_timeout_guard_enforces_timeout() {
    let result = TestTimeoutGuard::run(Duration::from_millis(50), || async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        Ok::<_, sz_rust_orm_facade::DbError>(42)
    })
    .await;

    assert!(
        matches!(result, Err(TestError::Timeout { timeout, .. }) if timeout == Duration::from_millis(50)),
        "应返回 Timeout 错误"
    );
}

/// TestTimeoutGuard：正常执行返回结果
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_timeout_guard_normal_execution() {
    let result = TestTimeoutGuard::run(Duration::from_secs(5), || async {
        Ok::<_, sz_rust_orm_facade::DbError>(100)
    })
    .await;

    assert_eq!(result.unwrap(), 100);
}

/// ResourceLeakScanner：无泄漏时 is_clean 返回 true（spec 5.4.2）
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_resource_leak_scanner_clean() {
    let pool = init_pool().await;

    let scanner = ResourceLeakScanner::new(Arc::new(pool.clone()));
    let report = scanner.scan().await;

    assert!(
        report.is_clean(),
        "无活跃连接时应无泄漏: {:?}",
        report.leaks
    );
    assert_eq!(report.active_connections, 0, "不应有活跃连接");

    pool.close_all().await;
}

/// ResourceLeakScanner：有活跃连接时检测到泄漏
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_resource_leak_scanner_detects_leak() {
    let pool = init_pool().await;

    let _conn = pool.acquire().await.expect("获取连接失败");

    let scanner = ResourceLeakScanner::new(Arc::new(pool.clone()));
    let report = scanner.scan().await;

    assert!(!report.is_clean(), "有活跃连接时应检测到泄漏");
    assert!(report.active_connections > 0, "应有活跃连接");

    drop(_conn);
    pool.close_all().await;
}

/// TxIsolationFixture + TestTimeoutGuard 组合使用
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_tx_isolation_with_timeout_guard() {
    let pool = init_pool().await;

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("DROP TABLE IF EXISTS combo_test").await.ok();
        conn.execute("CREATE TABLE combo_test (id INT PRIMARY KEY)")
            .await
            .expect("建表失败");
    }

    let fixture = TxIsolationFixture::new(Arc::new(pool.clone()));

    let result = TestTimeoutGuard::run_raw(Duration::from_secs(5), async {
        let mut guard = fixture.begin().await.expect("开启事务失败");
        guard
            .connection()
            .execute("INSERT INTO combo_test (id) VALUES (1)")
            .await
            .expect("插入失败");
        fixture.end(guard).await.expect("结束事务失败");
    })
    .await;

    assert!(result.is_ok(), "事务隔离 + 超时守护应正常工作");

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        let rows = conn
            .query("SELECT id FROM combo_test WHERE id = 1")
            .await
            .expect("查询失败");
        assert_eq!(rows.len(), 0, "事务应已回滚");
    }

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("DROP TABLE IF EXISTS combo_test").await.ok();
    }
    pool.close_all().await;
}
