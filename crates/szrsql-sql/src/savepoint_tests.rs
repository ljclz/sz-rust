//! Phase 3.23 单元测试 — Savepoint 保存点。
//!
//! 覆盖类别：
//! - SavepointStack 基础（6）：new/is_active/depth/begin/savepoint/commit
//! - ROLLBACK TO（5）：单 savepoint 回滚、多 savepoint 部分回滚、回滚到事务起始、不存在报错、无活动事务报错
//! - RELEASE（4）：release 中间 savepoint、release 后 ROLLBACK TO 前一个、release 不存在报错、release 事务起始报错
//! - ROLLBACK 无参数（3）：恢复到事务起始、无活动事务返回 None、清空栈
//! - 端到端 SQL 流程（5）：BEGIN+INSERT+ROLLBACK、BEGIN+SAVEPOINT+INSERT+ROLLBACK TO+COMMIT、
//!   BEGIN+SAVEPOINT sp1+INSERT+SAVEPOINT sp2+INSERT+ROLLBACK TO sp2+COMMIT、
//!   BEGIN+INSERT+RELEASE+COMMIT、嵌套 SAVEPOINT 多层回滚
//! - 多表事务（3）：两表同时回滚、单表 ROLLBACK TO 不影响其他表、COMMIT 保留所有修改
//! - PG 兼容性边界（4）：BEGIN 嵌套静默忽略、COMMIT 无事务静默忽略、
//!   SAVEPOINT 无事务报错、ROLLBACK TO 不存在报错
//! - NamedSavepoint + 便捷函数（4）：is_transaction_start、get_snapshots、collect_snapshots、apply_snapshots
//!
//! 共 34 个测试用例。

use crate::ast::{ColumnDefinition, TableName};
use crate::executor::{InMemoryTable, MutableTable, TableStorage};
use crate::plan::TableSchema;
use crate::savepoint::{
    apply_snapshots, collect_snapshots, NamedSavepoint, SavepointError, SavepointStack,
};
use std::collections::HashMap;
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 创建单列 INT 表 schema
fn make_int_table_schema(name: &str) -> TableSchema {
    TableSchema {
        name: TableName::new(name),
        columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
    }
}

/// 创建两列 (INT, TEXT) 表 schema
fn make_int_text_table_schema(name: &str) -> TableSchema {
    TableSchema {
        name: TableName::new(name),
        columns: vec![
            ColumnDefinition::new("id", ColumnType::Int64),
            ColumnDefinition::new("name", ColumnType::Text),
        ],
    }
}

/// 创建一个空表（两列：INT, TEXT）
fn make_empty_table(name: &str) -> InMemoryTable {
    InMemoryTable::new(make_int_text_table_schema(name))
}

/// 创建一个有 3 行数据的表
fn make_filled_table(name: &str) -> InMemoryTable {
    let mut t = make_empty_table(name);
    t.insert_row(vec![Value::Int64(1), Value::Text("alice".into())]);
    t.insert_row(vec![Value::Int64(2), Value::Text("bob".into())]);
    t.insert_row(vec![Value::Int64(3), Value::Text("carol".into())]);
    t
}

/// 收集单表快照
fn snap_single(
    name: &str,
    table: &InMemoryTable,
) -> HashMap<String, crate::executor::TableSnapshot> {
    let mut m = HashMap::new();
    m.insert(name.to_string(), table.snapshot());
    m
}

/// 收集多表快照
fn snap_pair(
    n1: &str,
    t1: &InMemoryTable,
    n2: &str,
    t2: &InMemoryTable,
) -> HashMap<String, crate::executor::TableSnapshot> {
    let mut m = HashMap::new();
    m.insert(n1.to_string(), t1.snapshot());
    m.insert(n2.to_string(), t2.snapshot());
    m
}

/// 恢复单表
fn restore_single(
    table: &mut InMemoryTable,
    name: &str,
    snaps: HashMap<String, crate::executor::TableSnapshot>,
) {
    let mut snaps = snaps;
    if let Some(s) = snaps.remove(name) {
        table.restore(s);
    }
}

// =====================================================================
//  SavepointStack 基础测试
// =====================================================================

#[test]
fn test_sp_01_new_stack_is_empty() {
    let stack = SavepointStack::new();
    assert!(!stack.is_active());
    assert_eq!(stack.depth(), 0);
    assert!(stack.list_names().is_empty());
}

#[test]
fn test_sp_02_begin_activates_transaction() {
    let mut stack = SavepointStack::new();
    let table = make_empty_table("t");
    stack.begin(snap_single("t", &table));
    assert!(stack.is_active());
    assert_eq!(stack.depth(), 1);
    assert_eq!(stack.list_names(), vec![""]); // 事务起始 name 为空
}

#[test]
fn test_sp_03_savepoint_pushes_named() {
    let mut stack = SavepointStack::new();
    let table = make_empty_table("t");
    stack.begin(snap_single("t", &table));
    stack.savepoint("sp1", snap_single("t", &table)).unwrap();
    assert_eq!(stack.depth(), 2);
    assert_eq!(stack.list_names(), vec!["", "sp1"]);
}

#[test]
fn test_sp_04_commit_clears_stack() {
    let mut stack = SavepointStack::new();
    let table = make_empty_table("t");
    stack.begin(snap_single("t", &table));
    stack.savepoint("sp1", snap_single("t", &table)).unwrap();
    stack.commit();
    assert!(!stack.is_active());
    assert_eq!(stack.depth(), 0);
}

#[test]
fn test_sp_05_savepoint_duplicate_name_errors() {
    let mut stack = SavepointStack::new();
    let table = make_empty_table("t");
    stack.begin(snap_single("t", &table));
    stack.savepoint("sp1", snap_single("t", &table)).unwrap();
    let err = stack
        .savepoint("sp1", snap_single("t", &table))
        .unwrap_err();
    assert_eq!(err, SavepointError::DuplicateName("sp1".into()));
}

#[test]
fn test_sp_06_savepoint_without_transaction_errors() {
    let mut stack = SavepointStack::new();
    let table = make_empty_table("t");
    let err = stack
        .savepoint("sp1", snap_single("t", &table))
        .unwrap_err();
    assert_eq!(err, SavepointError::NoActiveTransaction);
}

// =====================================================================
//  ROLLBACK TO 测试
// =====================================================================

#[test]
fn test_sp_10_rollback_to_single_savepoint() {
    let mut stack = SavepointStack::new();
    let mut table = make_filled_table("t");
    stack.begin(snap_single("t", &table));

    // SAVEPOINT sp1
    stack.savepoint("sp1", snap_single("t", &table)).unwrap();
    assert_eq!(table.row_count(), 3);

    // INSERT 一行
    table.insert_row(vec![Value::Int64(4), Value::Text("dave".into())]);
    assert_eq!(table.row_count(), 4);

    // ROLLBACK TO sp1 → 应恢复到 3 行
    let snaps = stack.rollback_to("sp1").unwrap();
    restore_single(&mut table, "t", snaps);
    assert_eq!(table.row_count(), 3);
    assert!(stack.is_active());
    assert_eq!(stack.depth(), 2); // 事务起始 + sp1 保留
}

#[test]
fn test_sp_11_rollback_to_multi_savepoints_partial() {
    let mut stack = SavepointStack::new();
    let mut table = make_empty_table("t");
    stack.begin(snap_single("t", &table));

    // SAVEPOINT sp1 → INSERT 1
    stack.savepoint("sp1", snap_single("t", &table)).unwrap();
    table.insert_row(vec![Value::Int64(1), Value::Text("first".into())]);

    // SAVEPOINT sp2 → INSERT 2
    stack.savepoint("sp2", snap_single("t", &table)).unwrap();
    table.insert_row(vec![Value::Int64(2), Value::Text("second".into())]);
    assert_eq!(table.row_count(), 2);

    // ROLLBACK TO sp1 → 应恢复到 0 行（sp2 之后的所有保存点被丢弃）
    let snaps = stack.rollback_to("sp1").unwrap();
    restore_single(&mut table, "t", snaps);
    assert_eq!(table.row_count(), 0);
    assert_eq!(stack.depth(), 2); // 事务起始 + sp1
}

#[test]
fn test_sp_12_rollback_to_transaction_start() {
    let mut stack = SavepointStack::new();
    let mut table = make_filled_table("t");
    stack.begin(snap_single("t", &table));

    // SAVEPOINT sp1 → INSERT
    stack.savepoint("sp1", snap_single("t", &table)).unwrap();
    table.insert_row(vec![Value::Int64(99), Value::Text("new".into())]);
    assert_eq!(table.row_count(), 4);

    // ROLLBACK TO ""（事务起始）→ 恢复到 BEGIN 时状态
    let snaps = stack.rollback_to("").unwrap();
    restore_single(&mut table, "t", snaps);
    assert_eq!(table.row_count(), 3);
    assert_eq!(stack.depth(), 1); // 仅事务起始保留
}

#[test]
fn test_sp_13_rollback_to_nonexistent_errors() {
    let mut stack = SavepointStack::new();
    let table = make_empty_table("t");
    stack.begin(snap_single("t", &table));

    let err = stack.rollback_to("nonexistent").unwrap_err();
    assert_eq!(err, SavepointError::NotFound("nonexistent".into()));
}

#[test]
fn test_sp_14_rollback_to_without_transaction_errors() {
    let mut stack = SavepointStack::new();
    let err = stack.rollback_to("sp1").unwrap_err();
    assert_eq!(err, SavepointError::NoActiveTransaction);
}

// =====================================================================
//  RELEASE 测试
// =====================================================================

#[test]
fn test_sp_20_release_middle_savepoint() {
    let mut stack = SavepointStack::new();
    let table = make_empty_table("t");
    stack.begin(snap_single("t", &table));
    stack.savepoint("sp1", snap_single("t", &table)).unwrap();
    stack.savepoint("sp2", snap_single("t", &table)).unwrap();
    assert_eq!(stack.depth(), 3);

    stack.release("sp2").unwrap();
    assert_eq!(stack.depth(), 2);
    assert_eq!(stack.list_names(), vec!["", "sp1"]);
}

#[test]
fn test_sp_21_release_then_rollback_to_previous() {
    let mut stack = SavepointStack::new();
    let mut table = make_empty_table("t");
    stack.begin(snap_single("t", &table));

    stack.savepoint("sp1", snap_single("t", &table)).unwrap();
    table.insert_row(vec![Value::Int64(1), Value::Text("a".into())]);

    stack.savepoint("sp2", snap_single("t", &table)).unwrap();
    table.insert_row(vec![Value::Int64(2), Value::Text("b".into())]);

    // RELEASE sp2 → 仅删除 sp2，表状态不变
    stack.release("sp2").unwrap();
    assert_eq!(table.row_count(), 2);

    // ROLLBACK TO sp1 → 应恢复到 0 行（sp1 时的快照）
    let snaps = stack.rollback_to("sp1").unwrap();
    restore_single(&mut table, "t", snaps);
    assert_eq!(table.row_count(), 0);
}

#[test]
fn test_sp_22_release_nonexistent_errors() {
    let mut stack = SavepointStack::new();
    let table = make_empty_table("t");
    stack.begin(snap_single("t", &table));

    let err = stack.release("nonexistent").unwrap_err();
    assert_eq!(err, SavepointError::NotFound("nonexistent".into()));
}

#[test]
fn test_sp_23_release_transaction_start_errors() {
    let mut stack = SavepointStack::new();
    let table = make_empty_table("t");
    stack.begin(snap_single("t", &table));

    // RELEASE "" → 不能释放事务起始
    let err = stack.release("").unwrap_err();
    assert_eq!(err, SavepointError::CannotReleaseTransaction("".into()));
}

// =====================================================================
//  ROLLBACK 无参数 测试
// =====================================================================

#[test]
fn test_sp_30_rollback_all_restores_to_begin() {
    let mut stack = SavepointStack::new();
    let mut table = make_filled_table("t");
    stack.begin(snap_single("t", &table));

    // INSERT 一些行
    table.insert_row(vec![Value::Int64(4), Value::Text("dave".into())]);
    table.insert_row(vec![Value::Int64(5), Value::Text("eve".into())]);
    assert_eq!(table.row_count(), 5);

    // ROLLBACK（无参数）→ 应返回 BEGIN 时的快照
    let snaps = stack.rollback_all().expect("应有事务起始快照");
    restore_single(&mut table, "t", snaps);
    assert_eq!(table.row_count(), 3);
    assert!(!stack.is_active()); // 栈已清空
}

#[test]
fn test_sp_31_rollback_all_without_transaction_returns_none() {
    let mut stack = SavepointStack::new();
    assert!(stack.rollback_all().is_none());
}

#[test]
fn test_sp_32_rollback_all_clears_savepoints() {
    let mut stack = SavepointStack::new();
    let table = make_empty_table("t");
    stack.begin(snap_single("t", &table));
    stack.savepoint("sp1", snap_single("t", &table)).unwrap();
    stack.savepoint("sp2", snap_single("t", &table)).unwrap();
    assert_eq!(stack.depth(), 3);

    let _ = stack.rollback_all();
    assert!(!stack.is_active());
    assert_eq!(stack.depth(), 0);
}

// =====================================================================
//  端到端 SQL 流程测试（模拟 PG 行为）
// =====================================================================

#[test]
fn test_sp_40_e2e_begin_insert_rollback() {
    // 模拟：BEGIN → INSERT → ROLLBACK → 表应回到 BEGIN 前
    let mut table = make_filled_table("t");
    let mut stack = SavepointStack::new();
    let initial_count = table.row_count();

    // BEGIN
    stack.begin(snap_single("t", &table));

    // INSERT
    table.insert_row(vec![Value::Int64(100), Value::Text("new".into())]);
    assert_eq!(table.row_count(), initial_count + 1);

    // ROLLBACK
    let snaps = stack.rollback_all().expect("应返回事务起始快照");
    restore_single(&mut table, "t", snaps);
    assert_eq!(table.row_count(), initial_count);
    assert!(!stack.is_active());
}

#[test]
fn test_sp_41_e2e_begin_savepoint_insert_rollback_to_commit() {
    // 模拟：BEGIN → SAVEPOINT sp1 → INSERT → ROLLBACK TO sp1 → COMMIT
    // 验证：INSERT 被回滚，事务正常提交
    let mut table = make_empty_table("t");
    let mut stack = SavepointStack::new();

    // BEGIN
    stack.begin(snap_single("t", &table));

    // SAVEPOINT sp1
    stack.savepoint("sp1", snap_single("t", &table)).unwrap();

    // INSERT
    table.insert_row(vec![Value::Int64(1), Value::Text("first".into())]);
    assert_eq!(table.row_count(), 1);

    // ROLLBACK TO sp1
    let snaps = stack.rollback_to("sp1").unwrap();
    restore_single(&mut table, "t", snaps);
    assert_eq!(table.row_count(), 0);

    // COMMIT
    stack.commit();
    assert!(!stack.is_active());
    assert_eq!(table.row_count(), 0); // INSERT 被回滚
}

#[test]
fn test_sp_42_e2e_two_savepoints_partial_rollback() {
    // 验收标准示例：
    // BEGIN → SAVEPOINT sp1 → INSERT → SAVEPOINT sp2 → INSERT → ROLLBACK TO sp2 → COMMIT
    // 期望：第一次 INSERT 存在，第二次不存在
    let mut table = make_empty_table("t");
    let mut stack = SavepointStack::new();

    // BEGIN
    stack.begin(snap_single("t", &table));

    // SAVEPOINT sp1 → INSERT first
    stack.savepoint("sp1", snap_single("t", &table)).unwrap();
    table.insert_row(vec![Value::Int64(1), Value::Text("first".into())]);

    // SAVEPOINT sp2 → INSERT second
    stack.savepoint("sp2", snap_single("t", &table)).unwrap();
    table.insert_row(vec![Value::Int64(2), Value::Text("second".into())]);
    assert_eq!(table.row_count(), 2);

    // ROLLBACK TO sp2 → 仅回滚 sp2 之后的（second INSERT）
    let snaps = stack.rollback_to("sp2").unwrap();
    restore_single(&mut table, "t", snaps);
    assert_eq!(table.row_count(), 1);
    // 验证 first 存在
    let rows: Vec<_> = table.scan_iter().collect();
    assert_eq!(rows[0][1], Value::Text("first".into()));

    // COMMIT
    stack.commit();
    assert_eq!(table.row_count(), 1); // first 保留
}

#[test]
fn test_sp_43_e2e_release_savepoint_normal_flow() {
    // BEGIN → INSERT → SAVEPOINT sp1 → INSERT → RELEASE sp1 → COMMIT
    // 期望：两个 INSERT 都保留（RELEASE 仅删除保存点，不影响数据）
    let mut table = make_empty_table("t");
    let mut stack = SavepointStack::new();

    stack.begin(snap_single("t", &table));
    table.insert_row(vec![Value::Int64(1), Value::Text("first".into())]);

    stack.savepoint("sp1", snap_single("t", &table)).unwrap();
    table.insert_row(vec![Value::Int64(2), Value::Text("second".into())]);

    stack.release("sp1").unwrap();
    assert_eq!(table.row_count(), 2); // RELEASE 不影响数据

    stack.commit();
    assert_eq!(table.row_count(), 2);
}

#[test]
fn test_sp_44_e2e_nested_savepoints_multi_level_rollback() {
    // 嵌套多层 savepoint：BEGIN → sp1 → sp2 → sp3 → ROLLBACK TO sp2 → 验证 sp3 之后数据被回滚
    let mut table = make_empty_table("t");
    let mut stack = SavepointStack::new();

    stack.begin(snap_single("t", &table));

    stack.savepoint("sp1", snap_single("t", &table)).unwrap();
    table.insert_row(vec![Value::Int64(1), Value::Text("a".into())]);

    stack.savepoint("sp2", snap_single("t", &table)).unwrap();
    table.insert_row(vec![Value::Int64(2), Value::Text("b".into())]);

    stack.savepoint("sp3", snap_single("t", &table)).unwrap();
    table.insert_row(vec![Value::Int64(3), Value::Text("c".into())]);
    assert_eq!(table.row_count(), 3);

    // ROLLBACK TO sp2 → 应恢复到 sp2 时（1 行 'a'）
    let snaps = stack.rollback_to("sp2").unwrap();
    restore_single(&mut table, "t", snaps);
    assert_eq!(table.row_count(), 1);
    let rows: Vec<_> = table.scan_iter().collect();
    assert_eq!(rows[0][1], Value::Text("a".into()));

    // 栈应保留：[事务起始, sp1, sp2]（sp3 被丢弃）
    assert_eq!(stack.list_names(), vec!["", "sp1", "sp2"]);

    stack.commit();
}

// =====================================================================
//  多表事务测试
// =====================================================================

#[test]
fn test_sp_50_multi_table_both_rollback() {
    // 两表事务：BEGIN → INSERT t1 → INSERT t2 → ROLLBACK → 两表都回到 BEGIN 前
    let mut t1 = make_filled_table("t1");
    let mut t2 = make_filled_table("t2");
    let mut stack = SavepointStack::new();
    let c1 = t1.row_count();
    let c2 = t2.row_count();

    // BEGIN（快照两表）
    stack.begin(snap_pair("t1", &t1, "t2", &t2));

    // INSERT 两表
    t1.insert_row(vec![Value::Int64(99), Value::Text("t1-new".into())]);
    t2.insert_row(vec![Value::Int64(99), Value::Text("t2-new".into())]);
    assert_eq!(t1.row_count(), c1 + 1);
    assert_eq!(t2.row_count(), c2 + 1);

    // ROLLBACK
    let snaps = stack.rollback_all().unwrap();
    apply_snapshots(
        [
            ("t1", &mut t1 as &mut dyn MutableTable),
            ("t2", &mut t2 as &mut dyn MutableTable),
        ],
        snaps,
    );
    assert_eq!(t1.row_count(), c1);
    assert_eq!(t2.row_count(), c2);
}

#[test]
fn test_sp_51_multi_table_partial_rollback_only_affects_target() {
    // BEGIN → SAVEPOINT sp1 → INSERT t1 → SAVEPOINT sp2 → INSERT t2 → ROLLBACK TO sp2
    // 期望：t1 保留 INSERT，t2 回滚到 sp2 时（即 sp2 时 t2 的状态）
    let mut t1 = make_empty_table("t1");
    let mut t2 = make_empty_table("t2");
    let mut stack = SavepointStack::new();

    stack.begin(snap_pair("t1", &t1, "t2", &t2));

    // SAVEPOINT sp1 → INSERT t1
    stack
        .savepoint("sp1", snap_pair("t1", &t1, "t2", &t2))
        .unwrap();
    t1.insert_row(vec![Value::Int64(1), Value::Text("t1-a".into())]);
    assert_eq!(t1.row_count(), 1);

    // SAVEPOINT sp2 → INSERT t2
    stack
        .savepoint("sp2", snap_pair("t1", &t1, "t2", &t2))
        .unwrap();
    t2.insert_row(vec![Value::Int64(1), Value::Text("t2-a".into())]);
    assert_eq!(t2.row_count(), 1);

    // ROLLBACK TO sp2 → t2 应回到 sp2 时（0 行），t1 不变（1 行）
    let snaps = stack.rollback_to("sp2").unwrap();
    apply_snapshots(
        [
            ("t1", &mut t1 as &mut dyn MutableTable),
            ("t2", &mut t2 as &mut dyn MutableTable),
        ],
        snaps,
    );
    assert_eq!(t1.row_count(), 1); // t1 保留
    assert_eq!(t2.row_count(), 0); // t2 回滚
}

#[test]
fn test_sp_52_multi_table_commit_preserves_all() {
    // BEGIN → INSERT 两表 → COMMIT → 两表数据都保留
    let mut t1 = make_empty_table("t1");
    let mut t2 = make_empty_table("t2");
    let mut stack = SavepointStack::new();

    stack.begin(snap_pair("t1", &t1, "t2", &t2));
    t1.insert_row(vec![Value::Int64(1), Value::Text("t1-a".into())]);
    t2.insert_row(vec![Value::Int64(1), Value::Text("t2-a".into())]);

    stack.commit();
    assert_eq!(t1.row_count(), 1);
    assert_eq!(t2.row_count(), 1);
}

// =====================================================================
//  PG 兼容性边界测试
// =====================================================================

#[test]
fn test_sp_60_pg_nested_begin_silently_ignored() {
    // PG 行为：嵌套 BEGIN 仅警告，不创建新事务
    let mut stack = SavepointStack::new();
    let table = make_empty_table("t");
    stack.begin(snap_single("t", &table));
    assert_eq!(stack.depth(), 1);

    // 嵌套 BEGIN → 应静默忽略
    stack.begin(snap_single("t", &table));
    assert_eq!(stack.depth(), 1); // 深度不变
}

#[test]
fn test_sp_61_pg_commit_without_transaction_silent() {
    // PG 行为：无活动事务时 COMMIT 仅警告，不报错
    let mut stack = SavepointStack::new();
    stack.commit(); // 应静默
    assert!(!stack.is_active());
}

#[test]
fn test_sp_62_pg_savepoint_without_transaction_errors() {
    // PG 行为：无活动事务时 SAVEPOINT 报错
    let mut stack = SavepointStack::new();
    let table = make_empty_table("t");
    let err = stack
        .savepoint("sp1", snap_single("t", &table))
        .unwrap_err();
    assert_eq!(err, SavepointError::NoActiveTransaction);
}

#[test]
fn test_sp_63_pg_rollback_to_nonexistent_errors() {
    // PG 行为：ROLLBACK TO 不存在的 savepoint 报错
    let mut stack = SavepointStack::new();
    let table = make_empty_table("t");
    stack.begin(snap_single("t", &table));
    let err = stack.rollback_to("nonexistent").unwrap_err();
    assert_eq!(err, SavepointError::NotFound("nonexistent".into()));
}

// =====================================================================
//  NamedSavepoint 单元测试
// =====================================================================

#[test]
fn test_sp_70_named_savepoint_is_transaction_start() {
    let sp = NamedSavepoint::new("", HashMap::new());
    assert!(sp.is_transaction_start());
    let sp2 = NamedSavepoint::new("sp1", HashMap::new());
    assert!(!sp2.is_transaction_start());
}

#[test]
fn test_sp_71_get_snapshots_readonly() {
    let mut stack = SavepointStack::new();
    let table = make_filled_table("t");
    stack.begin(snap_single("t", &table));
    stack.savepoint("sp1", snap_single("t", &table)).unwrap();

    let snaps = stack.get_snapshots("sp1").expect("应存在 sp1");
    assert!(snaps.contains_key("t"));
    assert!(stack.get_snapshots("nonexistent").is_none());
}

// =====================================================================
//  便捷函数测试
// =====================================================================

#[test]
fn test_sp_80_collect_snapshots_helper() {
    let t1 = make_filled_table("t1");
    let snaps = collect_snapshots([("t1", &t1 as &dyn crate::executor::MutableTable)]);
    assert_eq!(snaps.len(), 1);
    assert!(snaps.contains_key("t1"));
}

#[test]
fn test_sp_81_apply_snapshots_helper() {
    let mut t1 = make_filled_table("t1");
    let original_count = t1.row_count();

    // 收集快照
    let snaps = collect_snapshots([("t1", &t1 as &dyn crate::executor::MutableTable)]);

    // 修改表
    t1.insert_row(vec![Value::Int64(99), Value::Text("new".into())]);
    assert_eq!(t1.row_count(), original_count + 1);

    // 应用快照 → 应恢复
    apply_snapshots(
        [("t1", &mut t1 as &mut dyn crate::executor::MutableTable)],
        snaps,
    );
    assert_eq!(t1.row_count(), original_count);
}
