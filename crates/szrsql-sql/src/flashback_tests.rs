//! Phase 3.35 单元测试 — 闪回事务（FLASHBACK TRANSACTION / FLASHBACK TABLE）。
//!
//! 覆盖类别：
//! - Parser（8）：FLASHBACK TRANSACTION 单事务、txn_id 边界、FLASHBACK TABLE 多种时间戳格式、
//!   schema 限定表名、错误语法
//! - Plan（3）：FlashbackTransaction、FlashbackTable、混合语句顺序保持
//! - TransactionHistory（6）：new/empty、record_commit 自增 ID、take 成功、
//!   事务不存在、重复闪回、get_snapshot_as_of 历史查询
//! - TableSnapshot（3）：empty / from_rows / active_rows + active_row_count
//! - 时间戳解析（6）：Unix 毫秒、ISO 8601 日期、空格分隔、T 分隔、Z 后缀、非法格式
//! - Executor FLASHBACK TRANSACTION（5）：单表闪回、多表闪回、计划类型错误、
//!   事务不存在、事务已闪回
//! - Executor FLASHBACK TABLE（3）：查询历史快照、计划类型错误、无快照
//! - 端到端验收（2）：进度表场景一（INSERT 3 行 + FLASHBACK TRANSACTION 撤销）、
//!   场景二（FLASHBACK TABLE TO TIMESTAMP 查询历史）
//!
//! 共 36 个测试用例。

use crate::ast::{ColumnDefinition, Statement, TableName};
use crate::executor::{
    current_unix_millis, CommittedTransaction, Executor, FlashbackError, InMemoryTable,
    MutableTable, TableSnapshot, TableStorage, TransactionHistory,
};
use crate::parser::{parse_one, parse_sql};
use crate::plan::{InMemoryCatalog, LogicalPlan, Planner, TableSchema};
use std::collections::HashMap;
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 解析 SQL 并断言成功
fn must_parse(sql: &str) -> Statement {
    match parse_one(sql) {
        Ok(stmt) => stmt,
        Err(e) => panic!("parse failed for SQL: {sql}\nerror: {e:?}"),
    }
}

/// 解析 + 规划，返回 LogicalPlan
fn plan_sql(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
    let stmt = must_parse(sql);
    let planner = Planner::new(catalog);
    planner
        .plan_statement(stmt)
        .unwrap_or_else(|e| panic!("plan failed for SQL: {sql}\nerror: {e:?}"))
}

/// 创建两列 (INT, TEXT) 表 schema
fn make_int_text_schema(name: &str) -> TableSchema {
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
    InMemoryTable::new(make_int_text_schema(name))
}

/// 创建一个有 3 行数据的表
fn make_filled_table(name: &str) -> InMemoryTable {
    let mut t = make_empty_table(name);
    t.insert_row(vec![Value::Int64(1), Value::Text("alice".into())]);
    t.insert_row(vec![Value::Int64(2), Value::Text("bob".into())]);
    t.insert_row(vec![Value::Int64(3), Value::Text("carol".into())]);
    t
}

/// 收集单表快照为 HashMap
fn snap_single(name: &str, table: &InMemoryTable) -> HashMap<String, TableSnapshot> {
    let mut m = HashMap::new();
    m.insert(name.to_string(), table.snapshot());
    m
}

/// 将单表的事务前快照包装为 HashMap（用于已拍摄的快照）
fn snap_from(name: &str, snapshot: TableSnapshot) -> HashMap<String, TableSnapshot> {
    let mut m = HashMap::new();
    m.insert(name.to_string(), snapshot);
    m
}

/// 应用快照列表到对应表（按表名匹配）
fn apply_snapshots(
    tables: &mut [(&str, &mut InMemoryTable)],
    snapshots: Vec<(String, TableSnapshot)>,
) {
    for (name, snap) in snapshots {
        for (table_name, table) in tables.iter_mut() {
            if name == *table_name {
                table.restore(snap);
                break;
            }
        }
    }
}

/// 收集表当前所有行
fn collect_rows(table: &InMemoryTable) -> Vec<Vec<Value>> {
    table.scan_iter().collect()
}

// =====================================================================
//  Parser 测试（8）
// =====================================================================

#[test]
fn test_parse_flashback_transaction_basic() {
    let stmt = must_parse("FLASHBACK TRANSACTION 1");
    match stmt {
        Statement::FlashbackTransaction { txn_id } => assert_eq!(txn_id, 1),
        other => panic!("expected FlashbackTransaction, got {other:?}"),
    }
}

#[test]
fn test_parse_flashback_transaction_zero() {
    let stmt = must_parse("FLASHBACK TRANSACTION 0");
    match stmt {
        Statement::FlashbackTransaction { txn_id } => assert_eq!(txn_id, 0),
        other => panic!("expected FlashbackTransaction, got {other:?}"),
    }
}

#[test]
fn test_parse_flashback_transaction_large_id() {
    let stmt = must_parse("FLASHBACK TRANSACTION 18446744073709551615");
    match stmt {
        Statement::FlashbackTransaction { txn_id } => {
            assert_eq!(txn_id, u64::MAX);
        }
        other => panic!("expected FlashbackTransaction, got {other:?}"),
    }
}

#[test]
fn test_parse_flashback_table_iso_date() {
    let stmt = must_parse("FLASHBACK TABLE users TO TIMESTAMP '2026-07-20'");
    match stmt {
        Statement::FlashbackTable { table, timestamp } => {
            assert_eq!(table.name, "users");
            assert_eq!(timestamp, "2026-07-20");
        }
        other => panic!("expected FlashbackTable, got {other:?}"),
    }
}

#[test]
fn test_parse_flashback_table_iso_datetime_space() {
    let stmt = must_parse("FLASHBACK TABLE users TO TIMESTAMP '2026-07-20 10:30:00'");
    match stmt {
        Statement::FlashbackTable { table, timestamp } => {
            assert_eq!(table.name, "users");
            assert_eq!(timestamp, "2026-07-20 10:30:00");
        }
        other => panic!("expected FlashbackTable, got {other:?}"),
    }
}

#[test]
fn test_parse_flashback_table_iso_datetime_t() {
    let stmt = must_parse("FLASHBACK TABLE users TO TIMESTAMP '2026-07-20T10:30:00Z'");
    match stmt {
        Statement::FlashbackTable { table, timestamp } => {
            assert_eq!(table.name, "users");
            assert_eq!(timestamp, "2026-07-20T10:30:00Z");
        }
        other => panic!("expected FlashbackTable, got {other:?}"),
    }
}

#[test]
fn test_parse_flashback_table_qualified_name() {
    let stmt = must_parse("FLASHBACK TABLE public.users TO TIMESTAMP '1700000000000'");
    match stmt {
        Statement::FlashbackTable { table, timestamp } => {
            assert_eq!(table.schema.as_deref(), Some("public"));
            assert_eq!(table.name, "users");
            assert_eq!(timestamp, "1700000000000");
        }
        other => panic!("expected FlashbackTable, got {other:?}"),
    }
}

#[test]
fn test_parse_flashback_invalid_syntax() {
    // 缺少子关键字
    assert!(parse_one("FLASHBACK").is_err());
    // 不支持的子关键字
    assert!(parse_one("FLASHBACK INDEX 1").is_err());
    // TRANSACTION 缺少 txn_id
    assert!(parse_one("FLASHBACK TRANSACTION").is_err());
    // TABLE 缺少 TO TIMESTAMP
    assert!(parse_one("FLASHBACK TABLE users").is_err());
    // txn_id 非数字
    assert!(parse_one("FLASHBACK TRANSACTION abc").is_err());
}

// =====================================================================
//  Plan 测试（3）
// =====================================================================

#[test]
fn test_plan_flashback_transaction() {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql("FLASHBACK TRANSACTION 42", &catalog);
    match plan {
        LogicalPlan::FlashbackTransaction { txn_id } => assert_eq!(txn_id, 42),
        other => panic!("expected FlashbackTransaction plan, got {other:?}"),
    }
}

#[test]
fn test_plan_flashback_table() {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "FLASHBACK TABLE users TO TIMESTAMP '2026-07-20 10:30:00'",
        &catalog,
    );
    match plan {
        LogicalPlan::FlashbackTable { table, timestamp } => {
            assert_eq!(table.name, "users");
            assert_eq!(timestamp, "2026-07-20 10:30:00");
        }
        other => panic!("expected FlashbackTable plan, got {other:?}"),
    }
}

#[test]
fn test_plan_flashback_mixed_statements_preserve_order() {
    // 混合 FLASHBACK 与普通 SQL：验证顺序保持
    let sql = "FLASHBACK TRANSACTION 1; SELECT * FROM users; FLASHBACK TABLE users TO TIMESTAMP '2026-07-20'";
    let stmts = parse_sql(sql).expect("parse_sql should succeed");
    assert_eq!(stmts.len(), 3);
    assert!(matches!(
        stmts[0],
        Statement::FlashbackTransaction { txn_id: 1 }
    ));
    assert!(matches!(stmts[1], Statement::Select(_)));
    assert!(matches!(stmts[2], Statement::FlashbackTable { .. }));
}

// =====================================================================
//  TransactionHistory 测试（6）
// =====================================================================

#[test]
fn test_transaction_history_new_empty() {
    let history = TransactionHistory::new();
    assert!(history.is_empty());
    assert_eq!(history.len(), 0);
}

#[test]
fn test_transaction_history_record_commit_assigns_incrementing_ids() {
    let mut history = TransactionHistory::new();
    let snap = TableSnapshot::empty();
    let id1 = history.record_commit(HashMap::from([("users".into(), snap.clone())]));
    let id2 = history.record_commit(HashMap::from([("orders".into(), snap)]));
    let id3 = history.record_commit(HashMap::new());
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
    assert_eq!(id3, 3);
    assert_eq!(history.len(), 3);
}

#[test]
fn test_transaction_history_take_flashback_snapshots_success() {
    let mut history = TransactionHistory::new();
    let table = make_filled_table("users");
    let snap = table.snapshot();
    let txn_id = history.record_commit(HashMap::from([("users".into(), snap)]));

    let taken = history
        .take_flashback_snapshots(txn_id)
        .expect("take should succeed");
    assert_eq!(taken.len(), 1);
    assert!(taken.contains_key("users"));
    // 验证快照内容
    let users_snap = &taken["users"];
    assert_eq!(users_snap.active_row_count(), 3);

    // 验证事务被标记为已闪回
    let txn = history
        .get_transaction(txn_id)
        .expect("transaction should exist");
    assert!(txn.flashed_back);
}

#[test]
fn test_transaction_history_take_nonexistent_transaction() {
    let mut history = TransactionHistory::new();
    let result = history.take_flashback_snapshots(999);
    assert!(matches!(
        result,
        Err(FlashbackError::TransactionNotFound(999))
    ));
}

#[test]
fn test_transaction_history_take_already_flashed_back() {
    let mut history = TransactionHistory::new();
    let txn_id = history.record_commit(HashMap::from([("users".into(), TableSnapshot::empty())]));

    // 第一次闪回成功
    history
        .take_flashback_snapshots(txn_id)
        .expect("first take should succeed");

    // 第二次闪回应失败
    let result = history.take_flashback_snapshots(txn_id);
    assert!(matches!(result, Err(FlashbackError::AlreadyFlashedBack(_))));
}

#[test]
fn test_transaction_history_get_snapshot_as_of() {
    let mut history = TransactionHistory::new();
    let table_v1 = make_filled_table("users"); // 3 行
    let snap_v1 = table_v1.snapshot();

    let txn1 = history.record_commit(HashMap::from([("users".into(), snap_v1)]));
    // 注意：record_commit 自动用当前时间作为 commit_ts

    // 查询当前时间点之后的快照（应找到 txn1 的事务前快照）
    let now = current_unix_millis();
    let found = history.get_snapshot_as_of("users", now);
    assert!(
        found.is_some(),
        "should find snapshot for users at time <= now"
    );
    let snap = found.unwrap();
    assert_eq!(
        snap.active_row_count(),
        3,
        "snapshot should have 3 active rows"
    );

    // 查询不存在的事务时间
    let old_ts: u64 = 0;
    let none_result = history.get_snapshot_as_of("users", old_ts);
    assert!(
        none_result.is_none(),
        "should not find snapshot at timestamp 0"
    );

    // 查询不存在的表
    let missing_table = history.get_snapshot_as_of("nonexistent", now);
    assert!(
        missing_table.is_none(),
        "should not find snapshot for nonexistent table"
    );

    // 已闪回的事务不应参与查询
    let _ = history.take_flashback_snapshots(txn1).unwrap();
    let after_flashback = history.get_snapshot_as_of("users", now);
    assert!(
        after_flashback.is_none(),
        "flashed-back transactions should not be visible"
    );
}

// =====================================================================
//  TableSnapshot 测试（3）
// =====================================================================

#[test]
fn test_table_snapshot_empty() {
    let snap = TableSnapshot::empty();
    assert_eq!(snap.active_row_count(), 0);
    assert!(snap.active_rows().is_empty());
}

#[test]
fn test_table_snapshot_from_rows() {
    let rows = vec![
        vec![Value::Int64(1), Value::Text("a".into())],
        vec![Value::Int64(2), Value::Text("b".into())],
    ];
    let snap = TableSnapshot::from_rows(rows);
    assert_eq!(snap.active_row_count(), 2);
    let active = snap.active_rows();
    assert_eq!(active.len(), 2);
    assert_eq!(active[0], vec![Value::Int64(1), Value::Text("a".into())]);
}

#[test]
fn test_table_snapshot_active_rows_with_deletions() {
    // 通过 InMemoryTable 构造带删除的快照
    let mut table = make_filled_table("users");
    // 删除第二行（row_id = 1）
    assert!(table.delete_row(1));
    let snap = table.snapshot();
    assert_eq!(snap.active_row_count(), 2);
    let active = snap.active_rows();
    assert_eq!(active.len(), 2);
    // 应保留 row_id 0 和 2，跳过 1
    assert_eq!(active[0][0], Value::Int64(1)); // alice
    assert_eq!(active[1][0], Value::Int64(3)); // carol
}

// =====================================================================
//  时间戳解析测试（6）
// =====================================================================

#[test]
fn test_parse_timestamp_unix_millis() {
    let ts = "1700000000000";
    let ms = crate::executor::parse_timestamp_to_millis_pub(ts);
    assert_eq!(ms, Some(1700000000000));
}

#[test]
fn test_parse_timestamp_iso_date() {
    // 2026-07-20 → 当天 00:00:00 UTC
    // 1970-01-01 到 2026-07-20 共 20654 天（已校验：56 年 × 365 + 14 个闰年 + 200 天）
    let ms = crate::executor::parse_timestamp_to_millis_pub("2026-07-20");
    assert!(ms.is_some());
    let expected_days = 20654u64;
    assert_eq!(ms, Some(expected_days * 86400 * 1000));
}

#[test]
fn test_parse_timestamp_iso_datetime_space() {
    let ms = crate::executor::parse_timestamp_to_millis_pub("2026-07-20 10:30:00");
    assert!(ms.is_some());
    let expected_days = 20654u64;
    let expected_secs = expected_days * 86400 + 10 * 3600 + 30 * 60;
    assert_eq!(ms, Some(expected_secs * 1000));
}

#[test]
fn test_parse_timestamp_iso_datetime_t() {
    let ms = crate::executor::parse_timestamp_to_millis_pub("2026-07-20T10:30:00");
    assert!(ms.is_some());
    let expected_days = 20654u64;
    let expected_secs = expected_days * 86400 + 10 * 3600 + 30 * 60;
    assert_eq!(ms, Some(expected_secs * 1000));
}

#[test]
fn test_parse_timestamp_iso_datetime_with_z() {
    let ms = crate::executor::parse_timestamp_to_millis_pub("2026-07-20T10:30:00Z");
    assert!(ms.is_some());
    let expected_days = 20654u64;
    let expected_secs = expected_days * 86400 + 10 * 3600 + 30 * 60;
    assert_eq!(ms, Some(expected_secs * 1000));
}

#[test]
fn test_parse_timestamp_invalid() {
    assert_eq!(crate::executor::parse_timestamp_to_millis_pub(""), None);
    assert_eq!(crate::executor::parse_timestamp_to_millis_pub("abc"), None);
    assert_eq!(
        crate::executor::parse_timestamp_to_millis_pub("2026/07/20"),
        None
    );
    assert_eq!(
        crate::executor::parse_timestamp_to_millis_pub("2026-13-45"),
        None
    );
}

// =====================================================================
//  Executor FLASHBACK TRANSACTION 测试（5）
// =====================================================================

#[test]
fn test_execute_flashback_transaction_single_table() {
    // 场景：表初始 0 行 → 事务插入 3 行 → COMMIT → FLASHBACK → 表回到 0 行
    let mut table = make_empty_table("users");
    let pre_snap = table.snapshot(); // 事务前快照（0 行）

    // 模拟事务：插入 3 行
    table.insert_row(vec![Value::Int64(1), Value::Text("a".into())]);
    table.insert_row(vec![Value::Int64(2), Value::Text("b".into())]);
    table.insert_row(vec![Value::Int64(3), Value::Text("c".into())]);
    assert_eq!(table.row_count(), 3);

    // 记录事务
    let mut history = TransactionHistory::new();
    let txn_id = history.record_commit(snap_from("users", pre_snap));

    // 执行 FLASHBACK TRANSACTION
    let exec = Executor::new();
    let plan = LogicalPlan::FlashbackTransaction { txn_id };
    let snapshots = exec
        .execute_flashback_transaction(&plan, &mut history)
        .expect("flashback should succeed");

    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].0, "users");

    // 应用快照恢复
    apply_snapshots(&mut [("users", &mut table)], snapshots);

    // 验证表已回到事务前状态（0 行）
    assert_eq!(table.row_count(), 0);
    assert!(collect_rows(&table).is_empty());
}

#[test]
fn test_execute_flashback_transaction_multi_table() {
    // 场景：两张表同时被一个事务修改 → FLASHBACK → 两张表都回到事务前状态
    let mut users = make_empty_table("users");
    let mut orders = make_empty_table("orders");

    // 事务前快照
    let mut pre_snaps = HashMap::new();
    pre_snaps.insert("users".to_string(), users.snapshot());
    pre_snaps.insert("orders".to_string(), orders.snapshot());

    // 模拟事务：两表各插入 1 行
    users.insert_row(vec![Value::Int64(1), Value::Text("alice".into())]);
    orders.insert_row(vec![Value::Int64(100), Value::Text("order1".into())]);
    assert_eq!(users.row_count(), 1);
    assert_eq!(orders.row_count(), 1);

    let mut history = TransactionHistory::new();
    let txn_id = history.record_commit(pre_snaps);

    let exec = Executor::new();
    let plan = LogicalPlan::FlashbackTransaction { txn_id };
    let snapshots = exec
        .execute_flashback_transaction(&plan, &mut history)
        .expect("flashback should succeed");

    assert_eq!(snapshots.len(), 2);

    apply_snapshots(
        &mut [("users", &mut users), ("orders", &mut orders)],
        snapshots,
    );

    assert_eq!(users.row_count(), 0);
    assert_eq!(orders.row_count(), 0);
}

#[test]
fn test_execute_flashback_transaction_wrong_plan_type() {
    let exec = Executor::new();
    let mut history = TransactionHistory::new();
    // 传入错误的计划类型
    let wrong_plan = LogicalPlan::Empty;
    let result = exec.execute_flashback_transaction(&wrong_plan, &mut history);
    assert!(result.is_err());
    match result {
        Err(crate::executor::ExecutionError::InvalidArgument(msg)) => {
            assert!(msg.contains("expected FlashbackTransaction plan"));
        }
        other => panic!("expected InvalidArgument error, got {other:?}"),
    }
}

#[test]
fn test_execute_flashback_transaction_not_found() {
    let exec = Executor::new();
    let mut history = TransactionHistory::new();
    let plan = LogicalPlan::FlashbackTransaction { txn_id: 999 };
    let result = exec.execute_flashback_transaction(&plan, &mut history);
    assert!(result.is_err());
    match result {
        Err(crate::executor::ExecutionError::InvalidArgument(msg)) => {
            assert!(msg.contains("transaction not found: 999"));
        }
        other => panic!("expected InvalidArgument error, got {other:?}"),
    }
}

#[test]
fn test_execute_flashback_transaction_already_flashed_back() {
    let exec = Executor::new();
    let mut history = TransactionHistory::new();
    let txn_id = history.record_commit(HashMap::from([("users".into(), TableSnapshot::empty())]));

    let plan = LogicalPlan::FlashbackTransaction { txn_id };
    // 第一次成功
    let _ = exec
        .execute_flashback_transaction(&plan, &mut history)
        .expect("first flashback should succeed");
    // 第二次失败
    let result = exec.execute_flashback_transaction(&plan, &mut history);
    assert!(result.is_err());
    match result {
        Err(crate::executor::ExecutionError::InvalidArgument(msg)) => {
            assert!(msg.contains("already been flashed back"));
        }
        other => panic!("expected InvalidArgument error, got {other:?}"),
    }
}

// =====================================================================
//  Executor FLASHBACK TABLE 测试（3）
// =====================================================================

#[test]
fn test_execute_flashback_table_query_historical() {
    // 场景：表 v1 有 3 行 → 记录事务（事务前快照=v1）→ 表 v2 有 5 行
    // FLASHBACK TABLE TO TIMESTAMP now → 返回 v1 的 3 行
    let table_v1 = make_filled_table("users"); // 3 行
    let pre_snap = table_v1.snapshot();

    let mut history = TransactionHistory::new();
    let _txn_id = history.record_commit(HashMap::from([("users".into(), pre_snap)]));

    // 当前时间查询历史
    let now = current_unix_millis();
    let exec = Executor::new();
    let plan = LogicalPlan::FlashbackTable {
        table: TableName::new("users"),
        timestamp: now.to_string(),
    };
    let rows = exec
        .execute_flashback_table(&plan, &history)
        .expect("flashback table should succeed");
    assert_eq!(rows.len(), 3);
    // 验证返回的行内容
    assert_eq!(rows[0][0], Value::Int64(1));
    assert_eq!(rows[1][0], Value::Int64(2));
    assert_eq!(rows[2][0], Value::Int64(3));
}

#[test]
fn test_execute_flashback_table_wrong_plan_type() {
    let exec = Executor::new();
    let history = TransactionHistory::new();
    let wrong_plan = LogicalPlan::Empty;
    let result = exec.execute_flashback_table(&wrong_plan, &history);
    assert!(result.is_err());
    match result {
        Err(crate::executor::ExecutionError::InvalidArgument(msg)) => {
            assert!(msg.contains("expected FlashbackTable plan"));
        }
        other => panic!("expected InvalidArgument error, got {other:?}"),
    }
}

#[test]
fn test_execute_flashback_table_no_snapshot_found() {
    let exec = Executor::new();
    let history = TransactionHistory::new();
    let plan = LogicalPlan::FlashbackTable {
        table: TableName::new("users"),
        timestamp: current_unix_millis().to_string(),
    };
    let result = exec.execute_flashback_table(&plan, &history);
    assert!(result.is_err());
    match result {
        Err(crate::executor::ExecutionError::InvalidArgument(msg)) => {
            assert!(msg.contains("no snapshot found"));
        }
        other => panic!("expected InvalidArgument error, got {other:?}"),
    }
}

// =====================================================================
//  端到端验收测试（2）— 对应进度表验收场景
// =====================================================================

#[test]
fn test_e2e_begin_insert_commit_flashback_transaction() {
    // 进度表验收场景一：
    // BEGIN → INSERT 3 行 → COMMIT → FLASHBACK TRANSACTION <txn_id>
    // → 3 行被撤销 → 数据回到事务前状态

    let mut table = make_empty_table("users");

    // === BEGIN ===
    let pre_snap = table.snapshot(); // 事务前快照（0 行）

    // === INSERT 3 行（事务内）===
    table.insert_row(vec![Value::Int64(1), Value::Text("alice".into())]);
    table.insert_row(vec![Value::Int64(2), Value::Text("bob".into())]);
    table.insert_row(vec![Value::Int64(3), Value::Text("carol".into())]);
    assert_eq!(table.row_count(), 3);

    // === COMMIT ===
    let mut history = TransactionHistory::new();
    let txn_id = history.record_commit(snap_from("users", pre_snap));
    assert_eq!(txn_id, 1);

    // 验证 COMMIT 后表中有 3 行
    let rows_after_commit = collect_rows(&table);
    assert_eq!(rows_after_commit.len(), 3);

    // === FLASHBACK TRANSACTION 1 ===
    let exec = Executor::new();
    let plan = LogicalPlan::FlashbackTransaction { txn_id: 1 };
    let snapshots = exec
        .execute_flashback_transaction(&plan, &mut history)
        .expect("flashback should succeed");

    // 应用恢复
    apply_snapshots(&mut [("users", &mut table)], snapshots);

    // === 验证 3 行被撤销，数据回到事务前状态（0 行）===
    assert_eq!(table.row_count(), 0);
    assert!(collect_rows(&table).is_empty());

    // 验证事务已被标记为已闪回
    let txn = history
        .get_transaction(1)
        .expect("transaction should exist");
    assert!(txn.flashed_back);
}

#[test]
fn test_e2e_flashback_table_as_of_timestamp() {
    // 进度表验收场景二：FLASHBACK QUERY AS OF TIMESTAMP 查询历史快照
    //
    // 流程：
    // 1. 表初始有 2 行（v1）
    // 2. 事务前快照记录 v1（2 行）
    // 3. 事务内再插入 1 行（v2 = 3 行）
    // 4. COMMIT
    // 5. FLASHBACK TABLE users TO TIMESTAMP '<now>'
    //    → 返回 v1 的 2 行（事务前状态）

    let mut table = make_empty_table("users");
    table.insert_row(vec![Value::Int64(1), Value::Text("alice".into())]);
    table.insert_row(vec![Value::Int64(2), Value::Text("bob".into())]);
    assert_eq!(table.row_count(), 2);

    // === BEGIN ===
    let pre_snap = table.snapshot(); // 2 行

    // === 事务内再插入 1 行 ===
    table.insert_row(vec![Value::Int64(3), Value::Text("carol".into())]);
    assert_eq!(table.row_count(), 3);

    // === COMMIT ===
    let mut history = TransactionHistory::new();
    let _txn_id = history.record_commit(snap_from("users", pre_snap));

    // 当前表有 3 行
    assert_eq!(table.row_count(), 3);

    // === FLASHBACK TABLE users TO TIMESTAMP '<now>' ===
    let now = current_unix_millis();
    let exec = Executor::new();
    let plan = LogicalPlan::FlashbackTable {
        table: TableName::new("users"),
        timestamp: now.to_string(),
    };
    let historical_rows = exec
        .execute_flashback_table(&plan, &history)
        .expect("flashback table should succeed");

    // === 验证返回事务前的 2 行 ===
    assert_eq!(historical_rows.len(), 2);
    assert_eq!(historical_rows[0][0], Value::Int64(1));
    assert_eq!(historical_rows[0][1], Value::Text("alice".into()));
    assert_eq!(historical_rows[1][0], Value::Int64(2));
    assert_eq!(historical_rows[1][1], Value::Text("bob".into()));
}

// =====================================================================
//  CommittedTransaction 结构验证（1）
// =====================================================================

#[test]
fn test_committed_transaction_struct_fields() {
    // 验证 CommittedTransaction 结构体能正确构造并访问字段
    let snap = TableSnapshot::from_rows(vec![vec![Value::Int64(42)]]);
    let txn = CommittedTransaction {
        txn_id: 5,
        commit_ts_ms: 1700000000000,
        pre_snapshots: HashMap::from([("users".into(), snap)]),
        flashed_back: false,
    };
    assert_eq!(txn.txn_id, 5);
    assert_eq!(txn.commit_ts_ms, 1700000000000);
    assert!(!txn.flashed_back);
    assert_eq!(txn.pre_snapshots.len(), 1);
    assert_eq!(txn.pre_snapshots["users"].active_row_count(), 1);
}
