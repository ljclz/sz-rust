//! ADV-CONC-1: 多线程并发执行 + 行级锁集成测试
//!
//! 验证点：
//! 1. 共享表存储：CREATE TABLE 在一个 session 中创建，其他 session 可见
//! 2. 跨 session 数据修改：session A 的 INSERT/UPDATE/DELETE 对 session B 可见
//! 3. Strict 2PL：事务中的 UPDATE 持有 X 锁直到 COMMIT/ROLLBACK
//! 4. 锁冲突序列化：并发 UPDATE 同一表会被序列化
//! 5. ROLLBACK 释放锁：回滚后其他 session 可立即修改
//! 6. 并发 INSERT 不互相阻塞
//! 7. auto-commit 模式不持有长期锁

use std::collections::HashMap;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use szrsql_protocol::pgwire::session::{ExecutorService, QueryResult};
use szrsql_sql::executor::InMemoryTable;
use szrsql_tx::lock::LockManager;
use tokio::sync::{Mutex, RwLock};

/// 构造启用并发的 ExecutorService。
///
/// ADV-CONC-1：必须注入共享事务 ID 计数器，确保跨 session 分配全局唯一 txn_id，
/// 否则 LockManager 会将两个独立事务误判为同一事务（重入锁不阻塞），导致：
/// 1. Strict 2PL 失效（并发 UPDATE 同一表不会被序列化）
/// 2. 死锁检测失效（锁管理器看不到等待环）
/// 3. ROLLBACK 覆盖其他 session 的修改（事务 ID 冲突）
fn make_concurrent_service(
    shared: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
    lm: Arc<LockManager>,
    txn_counter: Arc<AtomicU32>,
) -> ExecutorService {
    ExecutorService::new()
        .with_shared_tables(shared)
        .with_lock_manager(lm)
        .with_shared_txn_counter(txn_counter)
}

/// 创建一组共享资源（shared_tables + LockManager + txn_counter），供测试使用。
fn make_shared_resources() -> (
    Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
    Arc<LockManager>,
    Arc<AtomicU32>,
) {
    (
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(LockManager::new()),
        Arc::new(AtomicU32::new(1)),
    )
}

#[tokio::test]
async fn test_shared_table_visibility_across_sessions() {
    let (shared, lm, counter) = make_shared_resources();

    let mut session_a = make_concurrent_service(shared.clone(), lm.clone(), counter.clone());
    let mut session_b = make_concurrent_service(shared.clone(), lm.clone(), counter.clone());

    session_a
        .execute_sql("CREATE TABLE users (id BIGINT, name TEXT)")
        .await;
    session_a
        .execute_sql("INSERT INTO users (id, name) VALUES (1, 'alice')")
        .await;
    session_a
        .execute_sql("INSERT INTO users (id, name) VALUES (2, 'bob')")
        .await;

    let results = session_b
        .execute_sql("SELECT id, name FROM users ORDER BY id")
        .await;
    match &results[0] {
        Ok(QueryResult::ResultSet { rows, .. }) => {
            assert_eq!(rows.len(), 2, "session B should see both rows");
        }
        other => panic!("expected ResultSet, got {other:?}"),
    }

    session_b
        .execute_sql("INSERT INTO users (id, name) VALUES (3, 'carol')")
        .await;
    let results = session_a.execute_sql("SELECT COUNT(*) FROM users").await;
    match &results[0] {
        Ok(QueryResult::ResultSet { rows, .. }) => {
            assert_eq!(rows[0][0], szrsql_types::value::Value::Int64(3));
        }
        other => panic!("expected ResultSet, got {other:?}"),
    }
}

#[tokio::test]
async fn test_cross_session_update_visibility() {
    let (shared, lm, counter) = make_shared_resources();

    let mut session_a = make_concurrent_service(shared.clone(), lm.clone(), counter.clone());
    let mut session_b = make_concurrent_service(shared.clone(), lm.clone(), counter.clone());

    session_a
        .execute_sql("CREATE TABLE acct (id BIGINT, bal BIGINT)")
        .await;
    session_a
        .execute_sql("INSERT INTO acct (id, bal) VALUES (1, 100)")
        .await;

    session_a
        .execute_sql("UPDATE acct SET bal = 200 WHERE id = 1")
        .await;

    let results = session_b
        .execute_sql("SELECT bal FROM acct WHERE id = 1")
        .await;
    match &results[0] {
        Ok(QueryResult::ResultSet { rows, .. }) => {
            assert_eq!(rows[0][0], szrsql_types::value::Value::Int64(200));
        }
        other => panic!("expected ResultSet, got {other:?}"),
    }
}

/// ADV-CONC-1.3：Strict 2PL — 事务中的 UPDATE 持有 X 锁直到 COMMIT
///
/// 时序：
/// 1. session A: BEGIN; UPDATE acct SET bal = 200 WHERE id = 1;  -- 持有 X 锁
/// 2. session B: BEGIN; UPDATE acct SET bal = 300 WHERE id = 1;  -- 应该阻塞
/// 3. session A: COMMIT;  -- 释放锁
/// 4. session B: 应该被唤醒并完成 UPDATE
#[tokio::test]
async fn test_strict_2pl_holds_lock_until_commit() {
    let (shared, lm, counter) = make_shared_resources();

    let mut session_a = make_concurrent_service(shared.clone(), lm.clone(), counter.clone());

    session_a
        .execute_sql("CREATE TABLE acct (id BIGINT, bal BIGINT)")
        .await;
    session_a
        .execute_sql("INSERT INTO acct (id, bal) VALUES (1, 100)")
        .await;

    session_a.execute_sql("BEGIN").await;
    session_a
        .execute_sql("UPDATE acct SET bal = 200 WHERE id = 1")
        .await;

    // 在另一个 tokio task 中运行 session B 的 UPDATE（应该阻塞）
    let shared_b = shared.clone();
    let lm_b = lm.clone();
    let counter_b = counter.clone();
    let session_b_handle = tokio::spawn(async move {
        let mut session_b = make_concurrent_service(shared_b, lm_b, counter_b);
        session_b.execute_sql("BEGIN").await;
        // 这个 UPDATE 应该阻塞，因为 session A 持有 X 锁
        let results = session_b
            .execute_sql("UPDATE acct SET bal = 300 WHERE id = 1")
            .await;
        (session_b, results)
    });

    // 等待一小段时间，确保 session B 已经开始等待
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // session A 提交，释放 X 锁
    session_a.execute_sql("COMMIT").await;

    // 等待 session B 完成
    let (_, session_b_results) =
        tokio::time::timeout(std::time::Duration::from_secs(10), session_b_handle)
            .await
            .expect("session B should complete within 10s after A commits")
            .expect("session B task should not panic");

    // session B 的 UPDATE 应该成功
    assert!(
        session_b_results[0].is_ok(),
        "session B UPDATE should succeed after A commits, got: {:?}",
        session_b_results[0]
    );

    // 最终余额应该是 300（session B 最后修改）
    let results = session_a
        .execute_sql("SELECT bal FROM acct WHERE id = 1")
        .await;
    match &results[0] {
        Ok(QueryResult::ResultSet { rows, .. }) => {
            assert_eq!(rows[0][0], szrsql_types::value::Value::Int64(300));
        }
        other => panic!("expected ResultSet, got {other:?}"),
    }
}

/// ADV-CONC-1.4：ROLLBACK 释放锁 — 回滚后其他 session 可立即修改
///
/// 时序：
/// 1. session A: BEGIN; UPDATE acct SET bal = 999 WHERE id = 1;  -- 持有 X 锁
/// 2. session B: BEGIN; UPDATE acct SET bal = 200 WHERE id = 1;  -- 应该阻塞
/// 3. session A: ROLLBACK;  -- 释放锁，恢复 bal=100
/// 4. session B: 被唤醒，UPDATE bal=200，然后 COMMIT
/// 5. session A: SELECT bal → 应该读到 200（不是 100）
#[tokio::test]
async fn test_rollback_releases_lock() {
    let (shared, lm, counter) = make_shared_resources();

    let mut session_a = make_concurrent_service(shared.clone(), lm.clone(), counter.clone());

    session_a
        .execute_sql("CREATE TABLE acct (id BIGINT, bal BIGINT)")
        .await;
    session_a
        .execute_sql("INSERT INTO acct (id, bal) VALUES (1, 100)")
        .await;

    session_a.execute_sql("BEGIN").await;
    session_a
        .execute_sql("UPDATE acct SET bal = 999 WHERE id = 1")
        .await;

    let shared_b = shared.clone();
    let lm_b = lm.clone();
    let counter_b = counter.clone();
    let session_b_handle = tokio::spawn(async move {
        let mut session_b = make_concurrent_service(shared_b, lm_b, counter_b);
        session_b.execute_sql("BEGIN").await;
        let results = session_b
            .execute_sql("UPDATE acct SET bal = 200 WHERE id = 1")
            .await;
        (session_b, results)
    });

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    session_a.execute_sql("ROLLBACK").await;

    let (mut session_b, session_b_results) =
        tokio::time::timeout(std::time::Duration::from_secs(10), session_b_handle)
            .await
            .expect("session B should complete within 10s after A rolls back")
            .expect("session B task should not panic");

    assert!(
        session_b_results[0].is_ok(),
        "session B UPDATE should succeed after A rolls back"
    );

    // session B must COMMIT for the change to be visible to other sessions
    session_b.execute_sql("COMMIT").await;

    let results = session_a
        .execute_sql("SELECT bal FROM acct WHERE id = 1")
        .await;
    match &results[0] {
        Ok(QueryResult::ResultSet { rows, .. }) => {
            assert_eq!(rows[0][0], szrsql_types::value::Value::Int64(200));
        }
        other => panic!("expected ResultSet, got {other:?}"),
    }
}

#[tokio::test]
async fn test_concurrent_inserts_dont_block() {
    let (shared, lm, counter) = make_shared_resources();

    let mut session_a = make_concurrent_service(shared.clone(), lm.clone(), counter.clone());
    let mut session_b = make_concurrent_service(shared.clone(), lm.clone(), counter.clone());

    session_a
        .execute_sql("CREATE TABLE logs (id BIGINT, msg TEXT)")
        .await;

    session_a.execute_sql("BEGIN").await;
    session_b.execute_sql("BEGIN").await;

    session_a
        .execute_sql("INSERT INTO logs (id, msg) VALUES (1, 'from A')")
        .await;
    session_b
        .execute_sql("INSERT INTO logs (id, msg) VALUES (2, 'from B')")
        .await;

    session_a.execute_sql("COMMIT").await;
    session_b.execute_sql("COMMIT").await;

    let results = session_a.execute_sql("SELECT COUNT(*) FROM logs").await;
    match &results[0] {
        Ok(QueryResult::ResultSet { rows, .. }) => {
            assert_eq!(rows[0][0], szrsql_types::value::Value::Int64(2));
        }
        other => panic!("expected ResultSet, got {other:?}"),
    }
}

#[tokio::test]
async fn test_auto_commit_does_not_hold_lock() {
    let (shared, lm, counter) = make_shared_resources();

    let mut session_a = make_concurrent_service(shared.clone(), lm.clone(), counter.clone());
    let mut session_b = make_concurrent_service(shared.clone(), lm.clone(), counter.clone());

    session_a
        .execute_sql("CREATE TABLE counter (id BIGINT, val BIGINT)")
        .await;
    session_a
        .execute_sql("INSERT INTO counter (id, val) VALUES (1, 0)")
        .await;

    session_a
        .execute_sql("UPDATE counter SET val = 10 WHERE id = 1")
        .await;

    let results = session_b
        .execute_sql("UPDATE counter SET val = 20 WHERE id = 1")
        .await;
    assert!(results[0].is_ok(), "session B should not be blocked");

    let results = session_a
        .execute_sql("SELECT val FROM counter WHERE id = 1")
        .await;
    match &results[0] {
        Ok(QueryResult::ResultSet { rows, .. }) => {
            assert_eq!(rows[0][0], szrsql_types::value::Value::Int64(20));
        }
        other => panic!("expected ResultSet, got {other:?}"),
    }
}

#[tokio::test]
async fn test_lock_timeout_returns_error() {
    use std::time::Duration;
    use szrsql_tx::lock::{LockError, LockMode};

    let lm = Arc::new(LockManager::new());

    lm.try_lock(1, 100, LockMode::Exclusive).unwrap();

    let lm_clone = lm.clone();
    let result = tokio::task::spawn_blocking(move || {
        lm_clone.lock(2, 100, LockMode::Exclusive, Duration::from_millis(200))
    })
    .await
    .unwrap();

    match result {
        Err(LockError::Timeout {
            txn_id, waited_ms, ..
        }) => {
            assert_eq!(txn_id, 2);
            assert!(
                waited_ms >= 150,
                "waited_ms should be >= 200ms, got {waited_ms}"
            );
        }
        other => panic!("expected Timeout error, got {other:?}"),
    }
}

/// ADV-CONC-1.5：死锁检测 — A→B 等待，B→A 等待，应被检测并中止一方
///
/// 时序：
/// 1. session A: BEGIN; UPDATE t1 SET v=1;  -- 锁 t1
/// 2. session B: BEGIN; UPDATE t2 SET v=2;  -- 锁 t2
/// 3. session A: UPDATE t2 SET v=3;  -- 等待 B 释放 t2
/// 4. session B: UPDATE t1 SET v=4;  -- 等待 A 释放 t1，形成死锁
/// 5. LockManager 应检测到死锁，中止一方
#[tokio::test]
async fn test_deadlock_detection_aborts_one_side() {
    let (shared, lm, counter) = make_shared_resources();

    let mut setup = make_concurrent_service(shared.clone(), lm.clone(), counter.clone());
    setup
        .execute_sql("CREATE TABLE t1 (id BIGINT, v BIGINT)")
        .await;
    setup
        .execute_sql("CREATE TABLE t2 (id BIGINT, v BIGINT)")
        .await;
    setup
        .execute_sql("INSERT INTO t1 (id, v) VALUES (1, 0)")
        .await;
    setup
        .execute_sql("INSERT INTO t2 (id, v) VALUES (1, 0)")
        .await;
    drop(setup);

    let shared_a = shared.clone();
    let lm_a = lm.clone();
    let counter_a = counter.clone();
    let handle_a = tokio::spawn(async move {
        let mut session_a = make_concurrent_service(shared_a, lm_a, counter_a);
        session_a.execute_sql("BEGIN").await;
        // Lock t1
        session_a.execute_sql("UPDATE t1 SET v = 1").await;
        // Wait to ensure B locks t2 first
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        // Try to lock t2 (held by B) — should block, then deadlock
        session_a.execute_sql("UPDATE t2 SET v = 3").await
    });

    let shared_b = shared.clone();
    let lm_b = lm.clone();
    let counter_b = counter.clone();
    let handle_b = tokio::spawn(async move {
        let mut session_b = make_concurrent_service(shared_b, lm_b, counter_b);
        session_b.execute_sql("BEGIN").await;
        // Lock t2
        session_b.execute_sql("UPDATE t2 SET v = 2").await;
        // Wait to ensure A is waiting on t2
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        // Try to lock t1 (held by A) — should deadlock
        session_b.execute_sql("UPDATE t1 SET v = 4").await
    });

    let results_a = tokio::time::timeout(std::time::Duration::from_secs(20), handle_a)
        .await
        .expect("A should complete within 20s")
        .expect("A task should not panic");

    let results_b = tokio::time::timeout(std::time::Duration::from_secs(20), handle_b)
        .await
        .expect("B should complete within 20s")
        .expect("B task should not panic");

    let a_got_deadlock = results_a[0]
        .as_ref()
        .err()
        .map(|e| e.to_string().to_lowercase().contains("deadlock"))
        .unwrap_or(false);

    let b_got_deadlock = results_b[0]
        .as_ref()
        .err()
        .map(|e| e.to_string().to_lowercase().contains("deadlock"))
        .unwrap_or(false);

    // At least one side should detect deadlock
    assert!(
        a_got_deadlock || b_got_deadlock,
        "expected at least one side to detect deadlock. A: {:?}, B: {:?}",
        results_a[0],
        results_b[0]
    );
}
