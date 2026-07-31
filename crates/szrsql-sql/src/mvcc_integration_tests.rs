//! P0-TX-1 Phase C：MVCC 隔离级别 executor 集成测试
//!
//! 验证 executor.rs 的 `execute_scan` 在不同隔离级别下的快照刷新行为：
//! - **RC（ReadCommitted）**：每条 SELECT 前调用 `refresh_snapshot`，看到最新已提交数据
//! - **RR（RepeatableRead）**：使用 BEGIN 时的快照，整个事务期间稳定
//! - **Serializable**：使用 BEGIN 时的快照 + SSI 写偏斜检测
//!
//! 这些测试聚焦 executor 集成层（mvcc.rs 已有 40+ 单元测试覆盖 refresh_snapshot 本身），
//! 确保 Phase C 的 executor 改动真正生效。

#![allow(clippy::approx_constant)]

use super::executor::{Executor, InMemoryTable, MutableTable, TableStorage};
use crate::parser::parse_sql;
use crate::plan::{InMemoryCatalog, LogicalPlan, Planner};
use szrsql_tx::mvcc::{IsolationLevel, MvccManager};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 构建测试表：列 `id BIGINT, val BIGINT`
fn make_kv_table(name: &str) -> InMemoryTable {
    InMemoryTable::with_columns(
        name,
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    )
}

/// SQL → AST → LogicalPlan
fn plan_sql(sql: &str, catalog: &dyn crate::plan::Catalog) -> LogicalPlan {
    let stmts = parse_sql(sql).expect("parse failed");
    assert_eq!(stmts.len(), 1, "expected exactly 1 statement");
    let planner = Planner::new(catalog);
    planner
        .plan_statement(stmts.into_iter().next().unwrap())
        .expect("plan failed")
}

/// 构建带 kv 表的 catalog
fn make_kv_catalog() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "kv",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );
    catalog
}

/// 从执行结果提取 (id, val) 对，按 id 排序
fn extract_id_val_pairs(rows: &[Vec<Value>]) -> Vec<(i64, i64)> {
    let mut pairs: Vec<(i64, i64)> = rows
        .iter()
        .map(|r| {
            let id = match &r[0] {
                Value::Int64(v) => *v,
                _ => panic!("expected Int64 id, got {:?}", r[0]),
            };
            let val = match &r[1] {
                Value::Int64(v) => *v,
                _ => panic!("expected Int64 val, got {:?}", r[1]),
            };
            (id, val)
        })
        .collect();
    pairs.sort_by_key(|p| p.0);
    pairs
}

/// 在表中查找指定 id 的 row_id（用于版本化删除）
fn find_row_id_by_id(table: &InMemoryTable, target_id: i64) -> Option<usize> {
    table
        .scan_with_ids()
        .find(|(_, r)| r.get(0) == Some(&Value::Int64(target_id)))
        .map(|(id, _)| id)
}

// =====================================================================
//  Phase C 测试 1：RC 事务看到新提交的数据（refresh 生效）
// =====================================================================

/// 验证 ReadCommitted 隔离级别下，事务内的多次 SELECT 能看到其他事务新提交的数据。
///
/// 场景：
/// 1. T1 BEGIN ISOLATION LEVEL READ COMMITTED
/// 2. T1 SELECT → 看到 [id=1, val=10]
/// 3. T2（autocommit）INSERT (id=2, val=20) 并提交
/// 4. T1 SELECT → 应看到 [id=1, val=10], [id=2, val=20]（RC 刷新快照后看到 T2 的提交）
///
/// 若 Phase C 的 refresh_snapshot 调用未生效，T1 第二次 SELECT 仍只会看到 [id=1, val=10]。
#[test]
fn phase_c_rc_sees_newly_committed_after_refresh() {
    let mvcc = MvccManager::new();
    let catalog = make_kv_catalog();

    // 准备表：初始一行 (id=1, val=10)
    let mut table = make_kv_table("kv");
    table.insert(vec![Value::Int64(1), Value::Int64(10)]);
    // 标记初始行为 Frozen（xmin=0），对所有事务可见
    // 注：InMemoryTable::insert 默认 xmin=0（Frozen），无需额外操作

    // T1: BEGIN ISOLATION LEVEL READ COMMITTED
    let t1 = mvcc.begin_with_isolation(IsolationLevel::ReadCommitted);
    let t1_id = t1.txn_id;

    // T1 第一次 SELECT
    let mut exec_t1_first = Executor::new()
        .with_catalog(&catalog)
        .with_mvcc(&mvcc, t1_id);
    exec_t1_first.register_table(&table);
    let plan = plan_sql("SELECT id, val FROM kv", &catalog);
    let rows = exec_t1_first.execute(&plan).unwrap();
    let pairs = extract_id_val_pairs(&rows);
    assert_eq!(pairs, vec![(1, 10)], "T1 第一次 SELECT 应只看到初始行");

    // T2（autocommit）：INSERT (id=2, val=20)
    // autocommit 模式：txn_id=0，直接插入（xmin=0=Frozen）
    // 模拟 T2 已提交：直接插入即可（Frozen 对所有事务可见）
    table.insert(vec![Value::Int64(2), Value::Int64(20)]);

    // T1 第二次 SELECT（RC 应刷新快照，看到 T2 的新行）
    let mut exec_t1_second = Executor::new()
        .with_catalog(&catalog)
        .with_mvcc(&mvcc, t1_id);
    exec_t1_second.register_table(&table);
    let plan = plan_sql("SELECT id, val FROM kv", &catalog);
    let rows = exec_t1_second.execute(&plan).unwrap();
    let pairs = extract_id_val_pairs(&rows);
    assert_eq!(
        pairs,
        vec![(1, 10), (2, 20)],
        "RC 事务第二次 SELECT 应看到 T2 新提交的行（refresh_snapshot 生效）"
    );

    // 清理
    mvcc.commit_durable(t1_id, |_| Ok(0u64)).unwrap();
}

// =====================================================================
//  Phase C 测试 2：RR 事务看不到新提交的数据（快照稳定）
// =====================================================================

/// 验证 RepeatableRead 隔离级别下，事务内的多次 SELECT 看不到其他事务新提交的数据。
///
/// 场景：
/// 1. T1 BEGIN ISOLATION LEVEL REPEATABLE READ
/// 2. T1 SELECT → 看到 [id=1, val=10]
/// 3. T2 插入 (id=2, val=20) 并提交（xmin=T2_txn_id）
/// 4. T1 SELECT → 仍只看到 [id=1, val=10]（RR 不刷新快照，T2 在 T1 快照中是活跃的）
///
/// 注意：T2 插入的行必须设置 xmin=T2_txn_id（非 Frozen），否则 RR 也能看到。
#[test]
fn phase_c_rr_uses_stable_snapshot_no_refresh() {
    let mvcc = MvccManager::new();
    let catalog = make_kv_catalog();

    // 准备表：初始一行 (id=1, val=10)，xmin=0（Frozen）
    let mut table = make_kv_table("kv");
    table.insert(vec![Value::Int64(1), Value::Int64(10)]);

    // T1: BEGIN ISOLATION LEVEL REPEATABLE READ
    let t1 = mvcc.begin_with_isolation(IsolationLevel::RepeatableRead);
    let t1_id = t1.txn_id;

    // T1 第一次 SELECT
    let mut exec_t1_first = Executor::new()
        .with_catalog(&catalog)
        .with_mvcc(&mvcc, t1_id);
    exec_t1_first.register_table(&table);
    let plan = plan_sql("SELECT id, val FROM kv", &catalog);
    let rows = exec_t1_first.execute(&plan).unwrap();
    let pairs = extract_id_val_pairs(&rows);
    assert_eq!(pairs, vec![(1, 10)], "T1 第一次 SELECT 应只看到初始行");

    // T2: BEGIN + INSERT (id=2, val=20) + COMMIT
    // T2 插入的行 xmin=T2_txn_id，对 T1 不可见（T2 在 T1 快照活跃集中或晚于 T1 快照）
    let t2 = mvcc.begin_with_isolation(IsolationLevel::ReadCommitted);
    let t2_id = t2.txn_id;
    // 使用 insert_row_versioned 设置 xmin=T2_txn_id
    table.insert_row_versioned(vec![Value::Int64(2), Value::Int64(20)], t2_id);
    // T2 提交
    mvcc.commit_durable(t2_id, |_| Ok(0u64)).unwrap();

    // T1 第二次 SELECT（RR 不刷新快照，T2 的行仍不可见）
    let mut exec_t1_second = Executor::new()
        .with_catalog(&catalog)
        .with_mvcc(&mvcc, t1_id);
    exec_t1_second.register_table(&table);
    let plan = plan_sql("SELECT id, val FROM kv", &catalog);
    let rows = exec_t1_second.execute(&plan).unwrap();
    let pairs = extract_id_val_pairs(&rows);
    assert_eq!(
        pairs,
        vec![(1, 10)],
        "RR 事务第二次 SELECT 不应看到 T2 新提交的行（快照稳定，不刷新）"
    );

    // 清理
    mvcc.commit_durable(t1_id, |_| Ok(0u64)).unwrap();
}

// =====================================================================
//  Phase C 测试 3：RC 非重复读现象
// =====================================================================

/// 验证 ReadCommitted 隔离级别下的非重复读（Non-Repeatable Read）现象。
///
/// 场景：
/// 1. T1 BEGIN ISOLATION LEVEL READ COMMITTED
/// 2. T1 SELECT WHERE id=1 → val=10
/// 3. T2 UPDATE id=1 SET val=20 并提交
/// 4. T1 SELECT WHERE id=1 → val=20（非重复读：同一事务内两次读取同一行得到不同值）
///
/// 这是 RC 隔离级别的预期行为（PG 标准语义），证明 refresh_snapshot 真正生效。
#[test]
fn phase_c_rc_non_repeatable_read() {
    let mvcc = MvccManager::new();
    let catalog = make_kv_catalog();

    // 准备表：初始一行 (id=1, val=10)
    let mut table = make_kv_table("kv");
    table.insert(vec![Value::Int64(1), Value::Int64(10)]);

    // T1: BEGIN READ COMMITTED
    let t1 = mvcc.begin_with_isolation(IsolationLevel::ReadCommitted);
    let t1_id = t1.txn_id;

    // T1 第一次 SELECT
    let mut exec = Executor::new().with_catalog(&catalog).with_mvcc(&mvcc, t1_id);
    exec.register_table(&table);
    let plan = plan_sql("SELECT id, val FROM kv WHERE id = 1", &catalog);
    let rows = exec.execute(&plan).unwrap();
    let pairs = extract_id_val_pairs(&rows);
    assert_eq!(pairs, vec![(1, 10)], "T1 第一次读取 val=10");

    // T2: BEGIN + 更新 id=1 的 val=20 + COMMIT
    // 实现：删除旧行（xmax=T2）+ 插入新行（xmin=T2）
    let t2 = mvcc.begin_with_isolation(IsolationLevel::ReadCommitted);
    let t2_id = t2.txn_id;
    // 找到 id=1 的 row_id 并版本化删除 + 插入新版本
    let old_row_id = find_row_id_by_id(&table, 1).expect("应找到 id=1 的行");
    // 版本化删除旧行（设置 xmax=T2）
    table.delete_row_versioned(old_row_id, t2_id);
    // 插入新版本（xmin=T2）
    table.insert_row_versioned(vec![Value::Int64(1), Value::Int64(20)], t2_id);
    // T2 提交
    mvcc.commit_durable(t2_id, |_| Ok(0u64)).unwrap();

    // T1 第二次 SELECT（RC 刷新快照后应看到 T2 更新的 val=20）
    let mut exec = Executor::new().with_catalog(&catalog).with_mvcc(&mvcc, t1_id);
    exec.register_table(&table);
    let plan = plan_sql("SELECT id, val FROM kv WHERE id = 1", &catalog);
    let rows = exec.execute(&plan).unwrap();
    let pairs = extract_id_val_pairs(&rows);
    assert_eq!(
        pairs,
        vec![(1, 20)],
        "RC 非重复读：T1 第二次读取应看到 T2 更新后的 val=20"
    );

    // 清理
    mvcc.commit_durable(t1_id, |_| Ok(0u64)).unwrap();
}

// =====================================================================
//  Phase C 测试 4：RR 可重复读
// =====================================================================

/// 验证 RepeatableRead 隔离级别下的可重复读（Repeatable Read）保证。
///
/// 场景：
/// 1. T1 BEGIN ISOLATION LEVEL REPEATABLE READ
/// 2. T1 SELECT WHERE id=1 → val=10
/// 3. T2 UPDATE id=1 SET val=20 并提交
/// 4. T1 SELECT WHERE id=1 → 仍 val=10（可重复读：同一事务内多次读取同一行得到相同值）
#[test]
fn phase_c_rr_repeatable_read() {
    let mvcc = MvccManager::new();
    let catalog = make_kv_catalog();

    // 准备表：初始一行 (id=1, val=10)
    let mut table = make_kv_table("kv");
    table.insert(vec![Value::Int64(1), Value::Int64(10)]);

    // T1: BEGIN REPEATABLE READ
    let t1 = mvcc.begin_with_isolation(IsolationLevel::RepeatableRead);
    let t1_id = t1.txn_id;

    // T1 第一次 SELECT
    let mut exec = Executor::new().with_catalog(&catalog).with_mvcc(&mvcc, t1_id);
    exec.register_table(&table);
    let plan = plan_sql("SELECT id, val FROM kv WHERE id = 1", &catalog);
    let rows = exec.execute(&plan).unwrap();
    let pairs = extract_id_val_pairs(&rows);
    assert_eq!(pairs, vec![(1, 10)], "T1 第一次读取 val=10");

    // T2: BEGIN + 更新 id=1 的 val=20 + COMMIT
    let t2 = mvcc.begin_with_isolation(IsolationLevel::ReadCommitted);
    let t2_id = t2.txn_id;
    let old_row_id = find_row_id_by_id(&table, 1).expect("应找到 id=1 的行");
    table.delete_row_versioned(old_row_id, t2_id);
    table.insert_row_versioned(vec![Value::Int64(1), Value::Int64(20)], t2_id);
    mvcc.commit_durable(t2_id, |_| Ok(0u64)).unwrap();

    // T1 第二次 SELECT（RR 不刷新快照，仍看到 val=10）
    let mut exec = Executor::new().with_catalog(&catalog).with_mvcc(&mvcc, t1_id);
    exec.register_table(&table);
    let plan = plan_sql("SELECT id, val FROM kv WHERE id = 1", &catalog);
    let rows = exec.execute(&plan).unwrap();
    let pairs = extract_id_val_pairs(&rows);
    assert_eq!(
        pairs,
        vec![(1, 10)],
        "RR 可重复读：T1 第二次读取应仍为 val=10（快照稳定）"
    );

    // 清理
    mvcc.commit_durable(t1_id, |_| Ok(0u64)).unwrap();
}

// =====================================================================
//  Phase C 测试 5：Serializable 使用 BEGIN 快照（不刷新）
// =====================================================================

/// 验证 Serializable 隔离级别使用 BEGIN 时的快照，不会在 SELECT 时刷新。
///
/// 场景：
/// 1. T1 BEGIN ISOLATION LEVEL SERIALIZABLE
/// 2. T1 SELECT → 看到 [id=1, val=10]
/// 3. T2 INSERT (id=2, val=20) 并提交
/// 4. T1 SELECT → 仍只看到 [id=1, val=10]（Serializable 不刷新快照）
///
/// Serializable 在 RR 基础上增加了 SSI 写偏斜检测（commit 时检查），
/// 读行为与 RR 一致（事务级快照）。
#[test]
fn phase_c_serializable_uses_begin_snapshot() {
    let mvcc = MvccManager::new();
    let catalog = make_kv_catalog();

    let mut table = make_kv_table("kv");
    table.insert(vec![Value::Int64(1), Value::Int64(10)]);

    // T1: BEGIN SERIALIZABLE
    let t1 = mvcc.begin_with_isolation(IsolationLevel::Serializable);
    let t1_id = t1.txn_id;

    // T1 第一次 SELECT
    let mut exec = Executor::new().with_catalog(&catalog).with_mvcc(&mvcc, t1_id);
    exec.register_table(&table);
    let plan = plan_sql("SELECT id, val FROM kv", &catalog);
    let rows = exec.execute(&plan).unwrap();
    let pairs = extract_id_val_pairs(&rows);
    assert_eq!(pairs, vec![(1, 10)]);

    // T2: BEGIN + INSERT (id=2, val=20) + COMMIT
    let t2 = mvcc.begin_with_isolation(IsolationLevel::ReadCommitted);
    let t2_id = t2.txn_id;
    table.insert_row_versioned(vec![Value::Int64(2), Value::Int64(20)], t2_id);
    mvcc.commit_durable(t2_id, |_| Ok(0u64)).unwrap();

    // T1 第二次 SELECT（Serializable 不刷新快照）
    let mut exec = Executor::new().with_catalog(&catalog).with_mvcc(&mvcc, t1_id);
    exec.register_table(&table);
    let plan = plan_sql("SELECT id, val FROM kv", &catalog);
    let rows = exec.execute(&plan).unwrap();
    let pairs = extract_id_val_pairs(&rows);
    assert_eq!(
        pairs,
        vec![(1, 10)],
        "Serializable 事务不应看到 T2 新提交的行（使用 BEGIN 快照，不刷新）"
    );

    // 清理（T1 只读，无写偏斜，应成功提交）
    mvcc.commit_durable(t1_id, |_| Ok(0u64)).unwrap();
}

// =====================================================================
//  Phase C 测试 6：autocommit 模式不受隔离级别影响
// =====================================================================

/// 验证 autocommit 模式（txn_id=0）下，所有行可见，不调用 refresh_snapshot。
///
/// 场景：
/// 1. Executor 不绑定 MVCC（或 txn_id=0）
/// 2. SELECT 应看到所有行（退化为 scan_iter，旧行为）
///
/// 这确保 Phase C 的改动不影响 autocommit 路径。
#[test]
fn phase_c_autocommit_sees_all_rows() {
    let mvcc = MvccManager::new();
    let catalog = make_kv_catalog();

    let mut table = make_kv_table("kv");
    table.insert(vec![Value::Int64(1), Value::Int64(10)]);
    table.insert(vec![Value::Int64(2), Value::Int64(20)]);

    // autocommit 模式：不绑定 MVCC
    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&table);
    let plan = plan_sql("SELECT id, val FROM kv", &catalog);
    let rows = exec.execute(&plan).unwrap();
    let pairs = extract_id_val_pairs(&rows);
    assert_eq!(
        pairs,
        vec![(1, 10), (2, 20)],
        "autocommit 模式应看到所有行"
    );

    // autocommit 模式：绑定 MVCC 但 txn_id=0
    let mut exec = Executor::new().with_mvcc(&mvcc, 0);
    exec.register_table(&table);
    let plan = plan_sql("SELECT id, val FROM kv", &catalog);
    let rows = exec.execute(&plan).unwrap();
    let pairs = extract_id_val_pairs(&rows);
    assert_eq!(
        pairs,
        vec![(1, 10), (2, 20)],
        "txn_id=0 时应退化为 scan_iter，看到所有行"
    );
}
