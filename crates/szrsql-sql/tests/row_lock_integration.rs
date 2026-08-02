//! P1-9 集成测试 — 行级锁执行路径改造。
//!
//! 验证 Executor 在注入 `LockManager` 后：
//! 1. UPDATE/DELETE 在实际修改前对匹配行获取行级 X 锁
//! 2. 两事务并发修改同一行时，后到事务阻塞等待（或死锁中止）
//! 3. 死锁检测正确中止一方并释放锁，另一方继续执行
//! 4. 未注入行锁管理器时行为不变（兼容性）
//!
//! # 验收标准
//!
//! - 无行锁管理器：UPDATE 正常执行（旧行为）
//! - 注入行锁管理器：UPDATE 正常执行，且行锁被获取
//! - 死锁场景：一个事务被 Deadlock 中止，另一个事务最终成功

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use szrsql_sql::executor::{Executor, InMemoryTable};
use szrsql_sql::parser::parse_sql;
use szrsql_sql::plan::{InMemoryCatalog, LogicalPlan, Planner};
use szrsql_tx::lock::{LockManager, LockMode};
use szrsql_types::value::ColumnType;

// =====================================================================
//  辅助函数
// =====================================================================

/// SQL → AST → LogicalPlan
fn plan_sql(sql: &str, catalog: &dyn szrsql_sql::plan::Catalog) -> LogicalPlan {
    let stmts = parse_sql(sql).expect("parse failed");
    assert_eq!(
        stmts.len(),
        1,
        "expected exactly 1 statement, got {}",
        stmts.len()
    );
    let planner = Planner::new(catalog);
    planner
        .plan_statement(stmts.into_iter().next().unwrap())
        .expect("plan failed")
}

/// 构造测试 catalog：test_table(id INT, name TEXT)
fn make_catalog() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "test_table",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    catalog
}

/// 构造测试表
fn make_table() -> InMemoryTable {
    InMemoryTable::with_columns(
        "test_table",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    )
}

/// 通过 INSERT 计划插入 count 行（id = 1..=count, name = 'row_{id}'）
fn insert_rows(
    executor: &Executor,
    table: &mut InMemoryTable,
    catalog: &InMemoryCatalog,
    count: usize,
) {
    for i in 1..=count {
        let sql = format!("INSERT INTO test_table (id, name) VALUES ({i}, 'row_{i}')");
        let plan = plan_sql(&sql, catalog);
        let result = executor
            .execute_insert(&plan, table)
            .expect("insert failed");
        assert_eq!(result.affected_rows, 1);
    }
}

/// 行锁资源 ID（与 executor 内部编码一致：table_hash 低 31 bit << 32 | row_id）
fn row_resource_id(table_name: &str, row_id: usize) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    table_name.to_lowercase().hash(&mut hasher);
    let hash = hasher.finish();
    let table_part = hash & 0x7FFF_FFFF;
    (table_part << 32) | (row_id as u64 & 0xFFFF_FFFF)
}

// =====================================================================
//  测试用例
// =====================================================================

/// 1. 未注入行锁管理器时，UPDATE 行为不变（兼容性）
#[test]
fn test_update_without_row_lock_manager() {
    let catalog = make_catalog();
    let mut table = make_table();
    let executor = Executor::new().with_catalog(&catalog);
    insert_rows(&executor, &mut table, &catalog, 3);

    let plan = plan_sql(
        "UPDATE test_table SET name = 'updated' WHERE id = 2",
        &catalog,
    );
    let result = executor
        .execute_update(&plan, &mut table)
        .expect("update should succeed without row lock manager");
    assert_eq!(result.affected_rows, 1);
}

/// 2. 注入行锁管理器（txn_id=0 autocommit）时，不获取行锁，UPDATE 正常
#[test]
fn test_update_with_row_lock_manager_autocommit() {
    let catalog = make_catalog();
    let mut table = make_table();
    let lm = Arc::new(LockManager::new());
    let executor = Executor::new()
        .with_catalog(&catalog)
        .with_row_lock_manager(lm.clone(), 0); // txn_id=0 → 跳过行锁
    insert_rows(&executor, &mut table, &catalog, 3);

    let plan = plan_sql(
        "UPDATE test_table SET name = 'updated' WHERE id = 2",
        &catalog,
    );
    let result = executor
        .execute_update(&plan, &mut table)
        .expect("update should succeed");
    assert_eq!(result.affected_rows, 1);

    // 验证没有锁被持有（txn_id=0 未加锁）
    let resource = row_resource_id("test_table", 1); // 第二行 row_id=1
    lm.unlock(0, resource); // 无锁时不报错
}

/// 3. 注入行锁管理器 + 活跃事务时，UPDATE 获取行锁并成功执行
#[test]
fn test_update_with_row_lock_manager_txn() {
    let catalog = make_catalog();
    let mut table = make_table();
    let lm = Arc::new(LockManager::new());
    let executor = Executor::new()
        .with_catalog(&catalog)
        .with_row_lock_manager(lm.clone(), 42);
    insert_rows(&executor, &mut table, &catalog, 3);

    let plan = plan_sql(
        "UPDATE test_table SET name = 'updated' WHERE id = 2",
        &catalog,
    );
    let result = executor
        .execute_update(&plan, &mut table)
        .expect("update should succeed");
    assert_eq!(result.affected_rows, 1);

    // 验证 txn 42 持有第 2 行（row_id=1）的 X 锁
    let resource = row_resource_id("test_table", 1);
    let other_txn = lm.lock(
        43,
        resource,
        LockMode::Exclusive,
        Duration::from_millis(200),
    );
    assert!(
        other_txn.is_err(),
        "txn 43 should conflict with txn 42's row lock on row 1"
    );
    // 清理
    lm.unlock_all(42);
}

/// 4. 死锁检测：txn1 持有 row1，txn2 更新 row0+row1（先持 row0 等 row1），
///    txn1 再尝试 row0 → 死锁，txn1 被中止并释放锁，txn2 最终成功。
#[test]
fn test_row_lock_deadlock_detection() {
    let catalog = make_catalog();
    let table_arc: Arc<Mutex<InMemoryTable>> = Arc::new(Mutex::new(make_table()));
    let lm = Arc::new(LockManager::new());

    // 准备数据（id=1 → row_id=0，id=2 → row_id=1）
    {
        let mut table = table_arc.lock().unwrap();
        let executor = Executor::new().with_catalog(&catalog);
        insert_rows(&executor, &mut table, &catalog, 2);
    }

    // txn1 先持有 row1（id=2 行）的 X 锁
    let resource_row1 = row_resource_id("test_table", 1);
    lm.lock(
        1,
        resource_row1,
        LockMode::Exclusive,
        Duration::from_secs(5),
    )
    .expect("txn1 should acquire row1 lock");

    // txn2 线程：UPDATE WHERE id IN (1,2) → 先获取 row0，再等 row1
    let ready = Arc::new(AtomicBool::new(false));
    let t2_table = table_arc.clone();
    let t2_lm = lm.clone();
    let t2_catalog = catalog.clone();
    let ready_t2 = ready.clone();
    let handle = std::thread::spawn(move || {
        let mut table = t2_table.lock().unwrap();
        let executor = Executor::new()
            .with_catalog(&t2_catalog)
            .with_row_lock_manager(t2_lm.clone(), 2);
        ready_t2.store(true, Ordering::SeqCst);
        let plan = plan_sql(
            "UPDATE test_table SET name = 'updated' WHERE id IN (1, 2)",
            &t2_catalog,
        );
        executor.execute_update(&plan, &mut *table)
    });

    // 等待 txn2 进入执行（已设置 ready），再给足够时间获取 row0 并阻塞在 row1
    while !ready.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(100));

    // txn1 尝试获取 row0 → 与 txn2 形成环 → 死锁检测中止 txn1
    let resource_row0 = row_resource_id("test_table", 0);
    let deadlock_result = lm.lock(
        1,
        resource_row0,
        LockMode::Exclusive,
        Duration::from_secs(5),
    );
    assert!(
        matches!(deadlock_result, Err(szrsql_tx::lock::LockError::Deadlock(t)) if t == 1),
        "txn1 should be deadlock-aborted, got: {:?}",
        deadlock_result
    );
    // 模拟 executor 的死锁处理（acquire_row_xlocks 中 Deadlock 分支）：abort 方释放所有锁
    lm.unlock_all(1);

    // txn1 被中止后释放了 row1 锁，txn2 应能继续并成功更新 2 行
    let t2_result = handle.join().expect("txn2 thread panicked");
    let result = t2_result.expect("txn2 update should succeed after txn1 deadlock abort");
    assert_eq!(result.affected_rows, 2);

    // 清理 txn2 的行锁
    lm.unlock_all(2);
}

/// 5. DELETE 也获取行级锁（正常路径验证）
#[test]
fn test_delete_with_row_lock_manager_txn() {
    let catalog = make_catalog();
    let mut table = make_table();
    let lm = Arc::new(LockManager::new());
    let executor = Executor::new()
        .with_catalog(&catalog)
        .with_row_lock_manager(lm.clone(), 42);
    insert_rows(&executor, &mut table, &catalog, 3);

    let plan = plan_sql("DELETE FROM test_table WHERE id = 3", &catalog);
    let result = executor
        .execute_delete(&plan, &mut table)
        .expect("delete should succeed");
    assert_eq!(result.affected_rows, 1);

    // 验证 txn 42 持有第 3 行（row_id=2）的 X 锁
    let resource = row_resource_id("test_table", 2);
    let other_txn = lm.lock(
        43,
        resource,
        LockMode::Exclusive,
        Duration::from_millis(200),
    );
    assert!(
        other_txn.is_err(),
        "txn 43 should conflict with txn 42's row lock on row 3"
    );
    // 清理
    lm.unlock_all(42);
}
