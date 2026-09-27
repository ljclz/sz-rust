//! TransactionManager 集成测试（v1.5.0 P1-2）
//!
//! 验证事务超时治理：超时触发回滚 + 连接释放 + 事件通知。
//! 需要真实 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test)。
//!
//! 运行：cargo test -p sz-rust-orm-facade --features tx-timeout --test tx_timeout -- --ignored
#![cfg(feature = "tx-timeout")]

use std::sync::Arc;
use std::time::Duration;

use sz_orm_sqlx::{MySqlPoolHandle, SqlxMySqlConnectionFactory};
use sz_rust_core::orm::{Pool, PoolConfigBuilder};
use sz_rust_orm_facade::auto_rollback::TransactionState;
use sz_rust_orm_facade::transaction_manager::{TransactionError, TransactionManager};

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

async fn setup_table(pool: &Pool, table: &str) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute(&format!("DROP TABLE IF EXISTS {}", table))
        .await
        .ok();
    conn.execute(&format!(
        "CREATE TABLE {} (id INT PRIMARY KEY, val VARCHAR(50))",
        table
    ))
    .await
    .expect("建表失败");
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

/// 超时触发回滚：配置 1s + SQL 耗时 2s → commit 超时 → 自动回滚（spec 5.2.1 规则2）
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_timeout_triggers_rollback() {
    let pool = init_pool().await;
    setup_table(&pool, "tx_to1").await;

    let mgr = TransactionManager::new(Arc::new(pool.clone()), Duration::from_secs(30));
    let mut tx = mgr
        .begin_with_timeout(Duration::from_secs(1))
        .await
        .expect("开启事务失败");

    let tx_id = tx.tx_id();
    let mut rx = mgr.subscribe();

    {
        let conn = tx.connection();
        conn.execute("INSERT INTO tx_to1 (id, val) VALUES (1, 'test')")
            .await
            .expect("插入失败");
    }

    tokio::time::sleep(Duration::from_secs(2)).await;

    let err = tx.commit().await.unwrap_err();
    assert!(
        matches!(err, TransactionError::Timeout { tx_id: t, .. } if t == tx_id),
        "应返回 Timeout 错误，实际: {:?}",
        err
    );

    let event = rx.try_recv().expect("应收到超时事件");
    assert_eq!(event.tx_id, tx_id);
    assert_eq!(event.timeout, Duration::from_secs(1));

    tokio::time::sleep(Duration::from_millis(200)).await;
    let count = count_rows(&pool, "tx_to1", 1).await;
    assert_eq!(count, 0, "超时事务应已回滚，数据不应存在");

    cleanup_table(&pool, "tx_to1").await;
    pool.close_all().await;
}

/// 单事务超时覆盖全局默认：全局 30s + 单事务 1s → 使用 1s（spec 5.2.1 规则4）
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_per_transaction_timeout_overrides_global() {
    let pool = init_pool().await;
    setup_table(&pool, "tx_to2").await;

    let mgr = TransactionManager::new(Arc::new(pool.clone()), Duration::from_secs(30));

    assert_eq!(mgr.default_timeout(), Duration::from_secs(30));

    let mut tx = mgr
        .begin_with_timeout(Duration::from_secs(1))
        .await
        .expect("开启事务失败");
    assert_eq!(tx.timeout(), Duration::from_secs(1));

    {
        let conn = tx.connection();
        conn.execute("INSERT INTO tx_to2 (id, val) VALUES (1, 'short')")
            .await
            .expect("插入失败");
    }

    tokio::time::sleep(Duration::from_secs(2)).await;

    let err = tx.commit().await.unwrap_err();
    assert!(
        matches!(err, TransactionError::Timeout { .. }),
        "单事务 1s 超时应生效"
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    let count = count_rows(&pool, "tx_to2", 1).await;
    assert_eq!(count, 0, "超时事务应已回滚");

    cleanup_table(&pool, "tx_to2").await;
    pool.close_all().await;
}

/// 运行时动态调整全局超时：修改后新事务使用新值（spec 5.2.1 规则5）
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_runtime_timeout_adjustment() {
    let pool = init_pool().await;
    setup_table(&pool, "tx_to3").await;

    let mgr = TransactionManager::new(Arc::new(pool.clone()), Duration::from_secs(30));
    assert_eq!(mgr.default_timeout(), Duration::from_secs(30));

    mgr.set_default_timeout(Duration::from_secs(60));
    assert_eq!(mgr.default_timeout(), Duration::from_secs(60));

    let mut tx = mgr.begin().await.expect("开启事务失败");
    assert_eq!(
        tx.timeout(),
        Duration::from_secs(60),
        "新事务应使用更新后的全局超时"
    );
    tx.rollback().await.expect("回滚失败");

    mgr.set_default_timeout(Duration::from_secs(2));
    let mut tx2 = mgr.begin().await.expect("开启事务失败");
    assert_eq!(tx2.timeout(), Duration::from_secs(2));

    {
        let conn = tx2.connection();
        conn.execute("INSERT INTO tx_to3 (id, val) VALUES (1, 'dynamic')")
            .await
            .expect("插入失败");
    }
    tokio::time::sleep(Duration::from_secs(3)).await;

    let err = tx2.commit().await.unwrap_err();
    assert!(
        matches!(err, TransactionError::Timeout { .. }),
        "动态调整后 2s 超时应生效"
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    let count = count_rows(&pool, "tx_to3", 1).await;
    assert_eq!(count, 0, "超时事务应已回滚");

    cleanup_table(&pool, "tx_to3").await;
    pool.close_all().await;
}

/// timeout=0 → 返回 InvalidConfig（spec 5.2.3 异常2）
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_zero_timeout_rejected() {
    let pool = init_pool().await;

    let mgr = TransactionManager::new(Arc::new(pool.clone()), Duration::from_secs(30));

    let err = match mgr.begin_with_timeout(Duration::ZERO).await {
        Ok(_) => panic!("timeout=0 应返回错误"),
        Err(e) => e,
    };
    assert!(
        matches!(err, TransactionError::InvalidConfig(_)),
        "timeout=0 应返回 InvalidConfig，实际: {:?}",
        err
    );

    pool.close_all().await;
}

/// 全局默认 timeout=0 → begin 返回 InvalidConfig
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_zero_default_timeout_rejected() {
    let pool = init_pool().await;

    let mgr = TransactionManager::new(Arc::new(pool.clone()), Duration::ZERO);

    let err = match mgr.begin().await {
        Ok(_) => panic!("全局 timeout=0 应返回错误"),
        Err(e) => e,
    };
    assert!(
        matches!(err, TransactionError::InvalidConfig(_)),
        "全局 timeout=0 应返回 InvalidConfig，实际: {:?}",
        err
    );

    pool.close_all().await;
}

/// 在超时内 commit → 成功（spec 5.2.1 规则1）
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_commit_within_timeout_succeeds() {
    let pool = init_pool().await;
    setup_table(&pool, "tx_to4").await;

    let mgr = TransactionManager::new(Arc::new(pool.clone()), Duration::from_secs(30));
    let mut tx = mgr
        .begin_with_timeout(Duration::from_secs(5))
        .await
        .expect("开启事务失败");

    {
        let conn = tx.connection();
        conn.execute("INSERT INTO tx_to4 (id, val) VALUES (1, 'ok')")
            .await
            .expect("插入失败");
    }

    tx.commit().await.expect("在超时内 commit 应成功");
    assert_eq!(tx.state(), TransactionState::Committed);

    let count = count_rows(&pool, "tx_to4", 1).await;
    assert_eq!(count, 1, "已提交的数据应存在");

    cleanup_table(&pool, "tx_to4").await;
    pool.close_all().await;
}

/// check_timeout：超时后检查 → 返回 Timeout 错误 + 回滚
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_check_timeout_after_elapsed() {
    let pool = init_pool().await;
    setup_table(&pool, "tx_to5").await;

    let mgr = TransactionManager::new(Arc::new(pool.clone()), Duration::from_secs(30));
    let mut tx = mgr
        .begin_with_timeout(Duration::from_secs(1))
        .await
        .expect("开启事务失败");

    {
        let conn = tx.connection();
        conn.execute("INSERT INTO tx_to5 (id, val) VALUES (1, 'check')")
            .await
            .expect("插入失败");
    }

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(tx.is_timed_out(), "应已超时");

    let err = tx.check_timeout().await.unwrap_err();
    assert!(
        matches!(err, TransactionError::Timeout { .. }),
        "check_timeout 应返回 Timeout"
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    let count = count_rows(&pool, "tx_to5", 1).await;
    assert_eq!(count, 0, "超时事务应已回滚");

    cleanup_table(&pool, "tx_to5").await;
    pool.close_all().await;
}

/// 超时事件包含事务标识与时长（spec 5.2.1 规则3）
#[tokio::test]
#[ignore = "需真实 MySQL 9.6"]
async fn test_timeout_event_contains_tx_id_and_duration() {
    let pool = init_pool().await;
    setup_table(&pool, "tx_to6").await;

    let mgr = TransactionManager::new(Arc::new(pool.clone()), Duration::from_secs(30));
    let mut rx = mgr.subscribe();

    let mut tx = mgr
        .begin_with_timeout(Duration::from_millis(500))
        .await
        .expect("开启事务失败");
    let expected_tx_id = tx.tx_id();
    let expected_timeout = Duration::from_millis(500);

    tokio::time::sleep(Duration::from_secs(1)).await;

    let err = tx.commit().await.unwrap_err();
    assert!(matches!(err, TransactionError::Timeout { .. }));

    let event = rx.try_recv().expect("应收到超时事件");
    assert_eq!(event.tx_id, expected_tx_id, "事件应包含正确的事务标识");
    assert_eq!(event.timeout, expected_timeout, "事件应包含正确的超时时长");

    cleanup_table(&pool, "tx_to6").await;
    pool.close_all().await;
}
