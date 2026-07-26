//! Phase 3.24 单元测试 — MERGE 语句（SQL:2003 标准）。
//!
//! 覆盖类别：
//! - Parser（5）：WHEN MATCHED UPDATE / WHEN MATCHED DELETE / WHEN NOT MATCHED INSERT /
//!   多子句 + predicate / WHEN NOT MATCHED BY SOURCE
//! - Planner（3）：基本计划生成 / 目标表不存在错误 / 源表不存在错误
//! - Executor WHEN MATCHED UPDATE（2）：基本匹配更新、多行匹配更新
//! - Executor WHEN NOT MATCHED INSERT（2）：基本不匹配插入、显式列插入
//! - Executor WHEN MATCHED DELETE（1）：匹配删除
//! - Executor WHEN NOT MATCHED BY SOURCE（2）：DELETE 未匹配目标行、UPDATE 未匹配目标行
//! - Executor 三向分支（1）：完整 PG 示例（MATCHED UPDATE + NOT MATCHED INSERT + NOT MATCHED BY SOURCE DELETE）
//! - Executor predicate（1）：WHEN MATCHED AND predicate THEN ...
//! - Executor 快照语义（1）：INSERT 的新行不会被后续源行匹配（PG 兼容）
//! - 错误处理（3）：WHEN MATCHED THEN INSERT 错误 / WHEN NOT MATCHED THEN UPDATE 错误 / 错误计划类型
//!
//! 共 21 个测试用例。

use super::executor::{ExecutionError, Executor, InMemoryTable, MutableTable, TableStorage};
use crate::ast::*;
use crate::parser::{parse_one, ParseError};
use crate::plan::{InMemoryCatalog, LogicalPlan, PlanError, Planner, TableSchema};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 创建 (id INT, x INT) 两列表 schema
fn make_id_x_schema(name: &str) -> TableSchema {
    TableSchema {
        name: TableName::new(name),
        columns: vec![
            ColumnDefinition::new("id", ColumnType::Int64),
            ColumnDefinition::new("x", ColumnType::Int64),
        ],
    }
}

/// 创建 catalog：注册两个表 `t` 和 `s`，均为 (id INT, x INT)
fn make_catalog_with_two_tables() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_table(make_id_x_schema("t"));
    catalog.add_table(make_id_x_schema("s"));
    catalog
}

/// 创建空的目标表 t（id, x）
fn make_empty_target() -> InMemoryTable {
    InMemoryTable::new(make_id_x_schema("t"))
}

/// 创建带数据的目标表 t：[(1, 10), (2, 20), (3, 30)]
fn make_filled_target() -> InMemoryTable {
    let mut t = make_empty_target();
    t.insert_row(vec![Value::Int64(1), Value::Int64(10)]);
    t.insert_row(vec![Value::Int64(2), Value::Int64(20)]);
    t.insert_row(vec![Value::Int64(3), Value::Int64(30)]);
    t
}

/// 创建源表 s：[(1, 100), (4, 400)]
/// - id=1 与目标匹配 → 走 WHEN MATCHED
/// - id=4 与目标不匹配 → 走 WHEN NOT MATCHED
fn make_source_basic() -> InMemoryTable {
    let mut s = InMemoryTable::new(make_id_x_schema("s"));
    s.insert_row(vec![Value::Int64(1), Value::Int64(100)]);
    s.insert_row(vec![Value::Int64(4), Value::Int64(400)]);
    s
}

/// SQL → AST → LogicalPlan（断言成功）
fn plan_sql(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
    let stmt = parse_one(sql).expect("parse failed");
    let planner = Planner::new(catalog);
    planner.plan_statement(stmt).expect("plan failed")
}

/// SQL → AST → LogicalPlan（断言失败，返回错误）
fn plan_sql_err(sql: &str, catalog: &InMemoryCatalog) -> PlanError {
    let stmt = parse_one(sql).expect("parse failed");
    let planner = Planner::new(catalog);
    planner
        .plan_statement(stmt)
        .expect_err("expected plan error")
}

/// 收集表所有行，按 id 排序后返回（便于断言）
fn collect_sorted_by_id(table: &InMemoryTable) -> Vec<(i64, i64)> {
    let mut rows: Vec<(i64, i64)> = table
        .scan_iter()
        .map(|r| match (&r[0], &r[1]) {
            (Value::Int64(a), Value::Int64(b)) => (*a, *b),
            _ => panic!("expected Int64, got {:?}", r),
        })
        .collect();
    rows.sort_by_key(|(a, _)| *a);
    rows
}

// =====================================================================
//  Parser 测试（5）
// =====================================================================

#[test]
fn test_merge_parser_01_when_matched_update() {
    let sql = "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.x = s.x";
    let stmt = parse_one(sql).unwrap();
    match stmt {
        Statement::Merge {
            target,
            target_alias,
            source,
            clauses,
            ..
        } => {
            assert_eq!(target, TableName::new("t"));
            assert_eq!(target_alias, None);
            // source 是 TableFactor::Table
            assert!(matches!(source, TableFactor::Table { .. }));
            assert_eq!(clauses.len(), 1);
            match &clauses[0] {
                MergeClause {
                    kind: MergeClauseKind::Matched,
                    predicate: None,
                    action: MergeAction::Update { assignments },
                } => {
                    assert_eq!(assignments.len(), 1);
                    assert_eq!(assignments[0].column, "x");
                }
                other => panic!("expected Matched Update, got {other:?}"),
            }
        }
        other => panic!("expected Merge, got {other:?}"),
    }
}

#[test]
fn test_merge_parser_02_when_matched_delete() {
    let sql = "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN DELETE";
    let stmt = parse_one(sql).unwrap();
    match stmt {
        Statement::Merge { clauses, .. } => {
            assert_eq!(clauses.len(), 1);
            assert!(matches!(
                &clauses[0],
                MergeClause {
                    kind: MergeClauseKind::Matched,
                    predicate: None,
                    action: MergeAction::Delete,
                }
            ));
        }
        other => panic!("expected Merge, got {other:?}"),
    }
}

#[test]
fn test_merge_parser_03_when_not_matched_insert() {
    let sql = "MERGE INTO t USING s ON t.id = s.id WHEN NOT MATCHED THEN INSERT (id, x) VALUES (s.id, s.x)";
    let stmt = parse_one(sql).unwrap();
    match stmt {
        Statement::Merge { clauses, .. } => {
            assert_eq!(clauses.len(), 1);
            match &clauses[0] {
                MergeClause {
                    kind: MergeClauseKind::NotMatched,
                    predicate: None,
                    action: MergeAction::Insert { columns, values },
                } => {
                    assert_eq!(columns, &vec!["id".to_string(), "x".to_string()]);
                    assert_eq!(values.len(), 2);
                }
                other => panic!("expected NotMatched Insert, got {other:?}"),
            }
        }
        other => panic!("expected Merge, got {other:?}"),
    }
}

#[test]
fn test_merge_parser_04_multiple_clauses_with_predicate() {
    let sql = "MERGE INTO t USING s ON t.id = s.id \
               WHEN MATCHED AND t.x > 5 THEN UPDATE SET t.x = s.x \
               WHEN NOT MATCHED THEN INSERT (id, x) VALUES (s.id, s.x)";
    let stmt = parse_one(sql).unwrap();
    match stmt {
        Statement::Merge { clauses, .. } => {
            assert_eq!(clauses.len(), 2);
            // 第一个子句：WHEN MATCHED AND predicate THEN UPDATE
            assert!(matches!(
                &clauses[0],
                MergeClause {
                    kind: MergeClauseKind::Matched,
                    predicate: Some(_),
                    action: MergeAction::Update { .. },
                }
            ));
            // 第二个子句：WHEN NOT MATCHED THEN INSERT
            assert!(matches!(
                &clauses[1],
                MergeClause {
                    kind: MergeClauseKind::NotMatched,
                    predicate: None,
                    action: MergeAction::Insert { .. },
                }
            ));
        }
        other => panic!("expected Merge, got {other:?}"),
    }
}

#[test]
fn test_merge_parser_05_when_not_matched_by_source() {
    let sql = "MERGE INTO t USING s ON t.id = s.id \
               WHEN MATCHED THEN UPDATE SET t.x = s.x \
               WHEN NOT MATCHED BY SOURCE THEN DELETE";
    let stmt = parse_one(sql).unwrap();
    match stmt {
        Statement::Merge { clauses, .. } => {
            assert_eq!(clauses.len(), 2);
            // 第二个子句是 WHEN NOT MATCHED BY SOURCE THEN DELETE
            assert!(matches!(
                &clauses[1],
                MergeClause {
                    kind: MergeClauseKind::NotMatchedBySource,
                    predicate: None,
                    action: MergeAction::Delete,
                }
            ));
        }
        other => panic!("expected Merge, got {other:?}"),
    }
}

// =====================================================================
//  Planner 测试（3）
// =====================================================================

#[test]
fn test_merge_planner_01_basic_plan() {
    let catalog = make_catalog_with_two_tables();
    let sql = "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.x = s.x";
    let plan = plan_sql(sql, &catalog);
    match plan {
        LogicalPlan::Merge {
            target,
            target_schema,
            source,
            source_schema,
            clauses,
            ..
        } => {
            assert_eq!(target, TableName::new("t"));
            assert_eq!(target_schema.columns.len(), 2);
            assert!(matches!(source, TableFactor::Table { .. }));
            assert!(source_schema.is_some());
            assert_eq!(clauses.len(), 1);
        }
        other => panic!("expected Merge, got {other:?}"),
    }
}

#[test]
fn test_merge_planner_02_target_not_found() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_table(make_id_x_schema("s")); // 只有源表，没有目标表 t

    let sql = "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.x = s.x";
    let err = plan_sql_err(sql, &catalog);
    assert!(matches!(err, PlanError::TableNotFound(_)));
}

#[test]
fn test_merge_planner_03_source_not_found() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_table(make_id_x_schema("t")); // 只有目标表，没有源表 s

    let sql = "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.x = s.x";
    let err = plan_sql_err(sql, &catalog);
    assert!(matches!(err, PlanError::TableNotFound(_)));
}

// =====================================================================
//  Executor WHEN MATCHED UPDATE 测试（2）
// =====================================================================

#[test]
fn test_merge_exec_01_when_matched_update_basic() {
    let catalog = make_catalog_with_two_tables();
    let mut target = make_filled_target(); // t: [(1,10), (2,20), (3,30)]
    let source = make_source_basic(); // s: [(1,100), (4,400)]

    let plan = plan_sql(
        "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.x = s.x",
        &catalog,
    );

    let mut exec = Executor::new();
    exec.register_table(&source);
    let result = exec.execute_merge(&plan, &mut target).unwrap();

    // 仅 id=1 匹配 → 1 行被更新
    assert_eq!(result.affected_rows, 1);
    let rows = collect_sorted_by_id(&target);
    assert_eq!(rows, vec![(1, 100), (2, 20), (3, 30)]);
}

#[test]
fn test_merge_exec_02_when_matched_update_multiple_rows() {
    // 源表含 3 行匹配目标的多行
    let catalog = make_catalog_with_two_tables();
    let mut target = make_filled_target(); // t: [(1,10), (2,20), (3,30)]
    let mut source = InMemoryTable::new(make_id_x_schema("s"));
    source.insert_row(vec![Value::Int64(1), Value::Int64(100)]);
    source.insert_row(vec![Value::Int64(2), Value::Int64(200)]);
    source.insert_row(vec![Value::Int64(3), Value::Int64(300)]);

    let plan = plan_sql(
        "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.x = s.x",
        &catalog,
    );

    let mut exec = Executor::new();
    exec.register_table(&source);
    let result = exec.execute_merge(&plan, &mut target).unwrap();

    assert_eq!(result.affected_rows, 3);
    let rows = collect_sorted_by_id(&target);
    assert_eq!(rows, vec![(1, 100), (2, 200), (3, 300)]);
}

// =====================================================================
//  Executor WHEN NOT MATCHED INSERT 测试（2）
// =====================================================================

#[test]
fn test_merge_exec_03_when_not_matched_insert_explicit_cols() {
    let catalog = make_catalog_with_two_tables();
    let mut target = make_filled_target(); // t: [(1,10), (2,20), (3,30)]
    let source = make_source_basic(); // s: [(1,100), (4,400)]

    let plan = plan_sql(
        "MERGE INTO t USING s ON t.id = s.id \
         WHEN NOT MATCHED THEN INSERT (id, x) VALUES (s.id, s.x)",
        &catalog,
    );

    let mut exec = Executor::new();
    exec.register_table(&source);
    let result = exec.execute_merge(&plan, &mut target).unwrap();

    // 仅 id=4 不匹配 → 插入 1 行
    assert_eq!(result.affected_rows, 1);
    let rows = collect_sorted_by_id(&target);
    assert_eq!(rows, vec![(1, 10), (2, 20), (3, 30), (4, 400)]);
}

#[test]
fn test_merge_exec_04_when_not_matched_insert_no_cols() {
    // 不指定列，按表顺序插入
    let catalog = make_catalog_with_two_tables();
    let mut target = make_filled_target();
    let source = make_source_basic();

    let plan = plan_sql(
        "MERGE INTO t USING s ON t.id = s.id \
         WHEN NOT MATCHED THEN INSERT VALUES (s.id, s.x)",
        &catalog,
    );

    let mut exec = Executor::new();
    exec.register_table(&source);
    let result = exec.execute_merge(&plan, &mut target).unwrap();

    assert_eq!(result.affected_rows, 1);
    let rows = collect_sorted_by_id(&target);
    assert_eq!(rows, vec![(1, 10), (2, 20), (3, 30), (4, 400)]);
}

// =====================================================================
//  Executor WHEN MATCHED DELETE 测试（1）
// =====================================================================

#[test]
fn test_merge_exec_05_when_matched_delete() {
    let catalog = make_catalog_with_two_tables();
    let mut target = make_filled_target(); // t: [(1,10), (2,20), (3,30)]
    let source = make_source_basic(); // s: [(1,100), (4,400)]

    let plan = plan_sql(
        "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN DELETE",
        &catalog,
    );

    let mut exec = Executor::new();
    exec.register_table(&source);
    let result = exec.execute_merge(&plan, &mut target).unwrap();

    // id=1 匹配 → 删除 1 行
    assert_eq!(result.affected_rows, 1);
    let rows = collect_sorted_by_id(&target);
    assert_eq!(rows, vec![(2, 20), (3, 30)]);
}

// =====================================================================
//  Executor WHEN NOT MATCHED BY SOURCE 测试（2）
// =====================================================================

#[test]
fn test_merge_exec_06_when_not_matched_by_source_delete() {
    let catalog = make_catalog_with_two_tables();
    let mut target = make_filled_target(); // t: [(1,10), (2,20), (3,30)]
    let source = make_source_basic(); // s: [(1,100), (4,400)] — 仅 id=1 匹配

    let plan = plan_sql(
        "MERGE INTO t USING s ON t.id = s.id \
         WHEN NOT MATCHED BY SOURCE THEN DELETE",
        &catalog,
    );

    let mut exec = Executor::new();
    exec.register_table(&source);
    let result = exec.execute_merge(&plan, &mut target).unwrap();

    // 目标表 id=2、id=3 未被任何源行匹配 → 删除 2 行
    assert_eq!(result.affected_rows, 2);
    let rows = collect_sorted_by_id(&target);
    assert_eq!(rows, vec![(1, 10)]);
}

#[test]
fn test_merge_exec_07_when_not_matched_by_source_update() {
    let catalog = make_catalog_with_two_tables();
    let mut target = make_filled_target(); // t: [(1,10), (2,20), (3,30)]
    let source = make_source_basic(); // s: [(1,100), (4,400)] — 仅 id=1 匹配

    let plan = plan_sql(
        "MERGE INTO t USING s ON t.id = s.id \
         WHEN NOT MATCHED BY SOURCE THEN UPDATE SET t.x = 0",
        &catalog,
    );

    let mut exec = Executor::new();
    exec.register_table(&source);
    let result = exec.execute_merge(&plan, &mut target).unwrap();

    // id=2、id=3 未匹配 → 更新 2 行
    assert_eq!(result.affected_rows, 2);
    let rows = collect_sorted_by_id(&target);
    assert_eq!(rows, vec![(1, 10), (2, 0), (3, 0)]);
}

// =====================================================================
//  Executor 三向分支测试（1）— 验收标准示例
// =====================================================================

#[test]
fn test_merge_exec_08_three_way_branch() {
    // 验收标准示例：完整 PG 风格 MERGE
    // - t: [(1,10), (2,20), (3,30)]
    // - s: [(1,100), (4,400)]
    //   → id=1 匹配 → UPDATE x=100
    //   → id=4 不匹配 → INSERT (4, 400)
    //   → id=2, id=3 无源匹配 → DELETE
    let catalog = make_catalog_with_two_tables();
    let mut target = make_filled_target();
    let source = make_source_basic();

    let plan = plan_sql(
        "MERGE INTO t USING s ON t.id = s.id \
         WHEN MATCHED THEN UPDATE SET t.x = s.x \
         WHEN NOT MATCHED THEN INSERT (id, x) VALUES (s.id, s.x) \
         WHEN NOT MATCHED BY SOURCE THEN DELETE",
        &catalog,
    );

    let mut exec = Executor::new();
    exec.register_table(&source);
    let result = exec.execute_merge(&plan, &mut target).unwrap();

    // 影响 4 行：1 UPDATE + 1 INSERT + 2 DELETE
    assert_eq!(result.affected_rows, 4);
    let rows = collect_sorted_by_id(&target);
    // 剩余：id=1（被更新为 100）+ id=4（新插入）
    assert_eq!(rows, vec![(1, 100), (4, 400)]);
}

// =====================================================================
//  Executor predicate 测试（1）
// =====================================================================

#[test]
fn test_merge_exec_09_when_matched_with_predicate() {
    // WHEN MATCHED AND t.x > 15 THEN UPDATE — 仅 x>15 的匹配行被更新
    let catalog = make_catalog_with_two_tables();
    let mut target = make_filled_target(); // t: [(1,10), (2,20), (3,30)]
    let mut source = InMemoryTable::new(make_id_x_schema("s"));
    source.insert_row(vec![Value::Int64(1), Value::Int64(100)]);
    source.insert_row(vec![Value::Int64(2), Value::Int64(200)]);
    source.insert_row(vec![Value::Int64(3), Value::Int64(300)]);

    let plan = plan_sql(
        "MERGE INTO t USING s ON t.id = s.id \
         WHEN MATCHED AND t.x > 15 THEN UPDATE SET t.x = s.x",
        &catalog,
    );

    let mut exec = Executor::new();
    exec.register_table(&source);
    let result = exec.execute_merge(&plan, &mut target).unwrap();

    // id=1 (x=10) 不满足 predicate，id=2 (x=20) 和 id=3 (x=30) 满足 → 2 行更新
    assert_eq!(result.affected_rows, 2);
    let rows = collect_sorted_by_id(&target);
    // id=1 保持 x=10，id=2 和 id=3 被更新
    assert_eq!(rows, vec![(1, 10), (2, 200), (3, 300)]);
}

// =====================================================================
//  Executor 快照语义测试（1）— INSERT 的新行不会被后续源行匹配
// =====================================================================

#[test]
fn test_merge_exec_10_initial_snapshot_semantics() {
    // PG 兼容：使用初始目标表快照，INSERT 的新行不会被后续源行匹配
    // - t: []  (空)
    // - s: [(1, 100), (1, 200)]  (两个源行都不匹配目标，都走 INSERT)
    // 期望：插入 2 行（不会被自己刚插入的行匹配）
    let catalog = make_catalog_with_two_tables();
    let mut target = make_empty_target();
    let mut source = InMemoryTable::new(make_id_x_schema("s"));
    source.insert_row(vec![Value::Int64(1), Value::Int64(100)]);
    source.insert_row(vec![Value::Int64(1), Value::Int64(200)]);

    let plan = plan_sql(
        "MERGE INTO t USING s ON t.id = s.id \
         WHEN NOT MATCHED THEN INSERT (id, x) VALUES (s.id, s.x)",
        &catalog,
    );

    let mut exec = Executor::new();
    exec.register_table(&source);
    let result = exec.execute_merge(&plan, &mut target).unwrap();

    // 两个源行都走 INSERT（目标初始快照为空）→ 2 行插入
    assert_eq!(result.affected_rows, 2);
    assert_eq!(target.row_count(), 2);
}

// =====================================================================
//  错误处理测试（3）
// =====================================================================

#[test]
fn test_merge_error_01_when_matched_then_insert_disallowed() {
    let mut target = make_filled_target();
    let source = make_source_basic();

    // WHEN MATCHED THEN INSERT 在 SQL 语法层就被 sqlparser 拒绝（实际测试中 PG dialect 不允许）
    // 这里我们手工构造错误计划来验证执行器的运行时检查
    // 改为：构造 LogicalPlan::Merge，其 clauses 包含 WHEN MATCHED + Insert action
    let target_schema = make_id_x_schema("t");
    let source_schema = make_id_x_schema("s");
    let source_tf = TableFactor::Table {
        name: TableName::new("s"),
        alias: None,
    };
    // ON: t.id = s.id
    let on = Expr::BinaryOp {
        left: Box::new(Expr::Identifier(vec!["t".into(), "id".into()])),
        op: BinaryOp::Eq,
        right: Box::new(Expr::Identifier(vec!["s".into(), "id".into()])),
    };
    let bad_clause = MergeClause {
        kind: MergeClauseKind::Matched,
        predicate: None,
        action: MergeAction::Insert {
            columns: vec!["id".into(), "x".into()],
            values: vec![
                Expr::Identifier(vec!["s".into(), "id".into()]),
                Expr::Identifier(vec!["s".into(), "x".into()]),
            ],
        },
    };
    let plan = LogicalPlan::Merge {
        target: TableName::new("t"),
        target_alias: None,
        target_schema,
        source: source_tf,
        source_schema: Some(source_schema),
        on,
        clauses: vec![bad_clause],
    };

    let mut exec = Executor::new();
    exec.register_table(&source);
    let result = exec.execute_merge(&plan, &mut target);
    assert!(matches!(result, Err(ExecutionError::InvalidArgument(_))));
}

#[test]
fn test_merge_error_02_when_not_matched_then_update_disallowed() {
    let mut target = make_filled_target();
    let source = make_source_basic();

    // 手工构造 WHEN NOT MATCHED THEN UPDATE 的非法计划
    let target_schema = make_id_x_schema("t");
    let source_schema = make_id_x_schema("s");
    let source_tf = TableFactor::Table {
        name: TableName::new("s"),
        alias: None,
    };
    let on = Expr::BinaryOp {
        left: Box::new(Expr::Identifier(vec!["t".into(), "id".into()])),
        op: BinaryOp::Eq,
        right: Box::new(Expr::Identifier(vec!["s".into(), "id".into()])),
    };
    let bad_clause = MergeClause {
        kind: MergeClauseKind::NotMatched,
        predicate: None,
        action: MergeAction::Update {
            assignments: vec![Assignment {
                column: "x".into(),
                value: Expr::Literal(Value::Int64(0)),
            }],
        },
    };
    let plan = LogicalPlan::Merge {
        target: TableName::new("t"),
        target_alias: None,
        target_schema,
        source: source_tf,
        source_schema: Some(source_schema),
        on,
        clauses: vec![bad_clause],
    };

    let mut exec = Executor::new();
    exec.register_table(&source);
    let result = exec.execute_merge(&plan, &mut target);
    assert!(matches!(result, Err(ExecutionError::InvalidArgument(_))));
}

#[test]
fn test_merge_error_03_wrong_plan_type() {
    // 传入非 Merge 计划应返回 InvalidArgument 错误
    let mut target = make_filled_target();
    let wrong_plan = LogicalPlan::Empty;
    let exec = Executor::new();
    let result = exec.execute_merge(&wrong_plan, &mut target);
    assert!(matches!(result, Err(ExecutionError::InvalidArgument(_))));
}

// =====================================================================
//  兼容性：parse_one 错误测试（验证 sqlparser 对非法语法的拒绝）
// =====================================================================

#[test]
fn test_merge_parser_compat_01_invalid_syntax_rejected() {
    // 缺少 ON 子句应被 sqlparser 拒绝
    let result = parse_one("MERGE INTO t USING s WHEN MATCHED THEN DELETE");
    assert!(matches!(result, Err(ParseError::SqlParser(_))));
}
