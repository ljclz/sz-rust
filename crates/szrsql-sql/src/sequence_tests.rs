//! Phase 3.22 单元测试 — Sequence 序列 + SERIAL 类型。
//!
//! 覆盖类别：
//! - Parser（8）：CREATE SEQUENCE 基础/START/INCREMENT/MINVALUE/MAXVALUE/CYCLE/IF NOT EXISTS、DROP SEQUENCE/IF EXISTS
//! - SERIAL 类型（3）：SERIAL/BIGSERIAL/SMALLSERIAL 自动生成 DEFAULT nextval + NOT NULL
//! - SequenceStore 基础（6）：create/drop/next_value 1,2,3/current_value 未调用报错/已存在报错/drop 不存在
//! - CYCLE/NO CYCLE（4）：CYCLE 升序回环、CYCLE 降序回环、NO CYCLE 越界报错、负 increment
//! - Executor DDL（4）：execute_create_sequence、execute_drop_sequence、IF NOT EXISTS 跳过、DROP IF EXISTS
//! - Executor nextval/currval（5）：SELECT nextval→1/2/3、SELECT currval、currval 未调用报错、表达式中的 nextval、CAST(nextval)
//! - SERIAL 端到端（4）：CREATE TABLE SERIAL→INSERT→id 自动递增、多行 INSERT、显式列 INSERT、BIGSERIAL
//! - 端到端完整流程（3）：CREATE→nextval 1,2,3、DROP 后 nextval 报错、currval 会话语义
//!
//! 共 37 个测试用例。

use crate::ast::*;
use crate::executor::{
    ExecutionError, Executor, InMemorySequenceStore, SequenceStore, TableStorage,
};
use crate::parser::parse_one;
use crate::plan::{InMemoryCatalog, LogicalPlan, Planner, SequenceDefinition, TableSchema};
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
fn plan_sql(sql: &str) -> LogicalPlan {
    let stmt = must_parse(sql);
    let catalog = InMemoryCatalog::new();
    let planner = Planner::new(&catalog);
    planner.plan_statement(stmt).unwrap_or_else(|e| {
        panic!("plan failed for SQL: {sql}\nerror: {e:?}");
    })
}

/// 解析 + 规划（使用指定 catalog），返回 LogicalPlan
fn plan_sql_with_catalog(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
    let stmt = must_parse(sql);
    let planner = Planner::new(catalog);
    planner.plan_statement(stmt).unwrap_or_else(|e| {
        panic!("plan failed for SQL: {sql}\nerror: {e:?}");
    })
}

/// 从 CREATE TABLE 计划中提取 TableSchema，并注册到 catalog
fn register_table_from_create_plan(create_plan: &LogicalPlan, catalog: &mut InMemoryCatalog) {
    if let LogicalPlan::CreateTable { name, columns, .. } = create_plan {
        catalog.add_table(TableSchema {
            name: name.clone(),
            columns: columns.clone(),
        });
    }
}

// =====================================================================
//  Parser 测试（8 条）
// =====================================================================

#[test]
fn test_parse_create_sequence_minimal() {
    let stmt = must_parse("CREATE SEQUENCE seq");
    match stmt {
        Statement::CreateSequence {
            name,
            if_not_exists,
            start,
            increment,
            min_value,
            max_value,
            cycle,
        } => {
            assert_eq!(name.qualified_name(), "seq");
            assert!(!if_not_exists);
            assert_eq!(start, 1);
            assert_eq!(increment, 1);
            assert_eq!(min_value, None);
            assert_eq!(max_value, None);
            assert!(!cycle);
        }
        other => panic!("expected CreateSequence, got {other:?}"),
    }
}

#[test]
fn test_parse_create_sequence_start_with() {
    let stmt = must_parse("CREATE SEQUENCE seq START WITH 100");
    match stmt {
        Statement::CreateSequence { start, .. } => assert_eq!(start, 100),
        other => panic!("expected CreateSequence, got {other:?}"),
    }
}

#[test]
fn test_parse_create_sequence_increment_by() {
    let stmt = must_parse("CREATE SEQUENCE seq INCREMENT BY 5 START WITH 10");
    match stmt {
        Statement::CreateSequence {
            start, increment, ..
        } => {
            assert_eq!(start, 10);
            assert_eq!(increment, 5);
        }
        other => panic!("expected CreateSequence, got {other:?}"),
    }
}

#[test]
fn test_parse_create_sequence_min_max() {
    let stmt = must_parse("CREATE SEQUENCE seq MINVALUE 1 MAXVALUE 1000");
    match stmt {
        Statement::CreateSequence {
            min_value,
            max_value,
            ..
        } => {
            assert_eq!(min_value, Some(1));
            assert_eq!(max_value, Some(1000));
        }
        other => panic!("expected CreateSequence, got {other:?}"),
    }
}

#[test]
fn test_parse_create_sequence_cycle() {
    // 注意：sqlparser 0.53.0 要求选项顺序：INCREMENT, MINVALUE, MAXVALUE, START, CACHE, CYCLE
    let stmt = must_parse("CREATE SEQUENCE seq MAXVALUE 3 START 1 CYCLE");
    match stmt {
        Statement::CreateSequence {
            cycle, max_value, ..
        } => {
            assert!(cycle);
            assert_eq!(max_value, Some(3));
        }
        other => panic!("expected CreateSequence, got {other:?}"),
    }
}

#[test]
fn test_parse_create_sequence_no_cycle() {
    let stmt = must_parse("CREATE SEQUENCE seq NO CYCLE");
    match stmt {
        Statement::CreateSequence { cycle, .. } => assert!(!cycle),
        other => panic!("expected CreateSequence, got {other:?}"),
    }
}

#[test]
fn test_parse_create_sequence_if_not_exists() {
    let stmt = must_parse("CREATE SEQUENCE IF NOT EXISTS seq");
    match stmt {
        Statement::CreateSequence {
            if_not_exists,
            name,
            ..
        } => {
            assert!(if_not_exists);
            assert_eq!(name.qualified_name(), "seq");
        }
        other => panic!("expected CreateSequence, got {other:?}"),
    }
}

#[test]
fn test_parse_drop_sequence() {
    let stmt = must_parse("DROP SEQUENCE seq");
    match stmt {
        Statement::DropSequence {
            names,
            if_exists,
            cascade,
        } => {
            assert_eq!(names.len(), 1);
            assert_eq!(names[0].qualified_name(), "seq");
            assert!(!if_exists);
            assert!(!cascade);
        }
        other => panic!("expected DropSequence, got {other:?}"),
    }

    let stmt = must_parse("DROP SEQUENCE IF EXISTS seq1, seq2 CASCADE");
    match stmt {
        Statement::DropSequence {
            names,
            if_exists,
            cascade,
        } => {
            assert_eq!(names.len(), 2);
            assert_eq!(names[0].qualified_name(), "seq1");
            assert_eq!(names[1].qualified_name(), "seq2");
            assert!(if_exists);
            assert!(cascade);
        }
        other => panic!("expected DropSequence, got {other:?}"),
    }
}

// =====================================================================
//  SERIAL 类型测试（3 条）
// =====================================================================

#[test]
fn test_parse_serial_column() {
    let stmt = must_parse("CREATE TABLE t (id SERIAL PRIMARY KEY, name TEXT)");
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert_eq!(columns.len(), 2);
            // id 列：SERIAL → Int64 + DEFAULT nextval('t_id_seq') + NOT NULL + PRIMARY KEY
            let id_col = &columns[0];
            assert_eq!(id_col.name, "id");
            assert_eq!(id_col.data_type, ColumnType::Int64);
            assert!(id_col.not_null);
            assert!(id_col.primary_key);
            let default = id_col.default.as_ref().expect("id should have DEFAULT");
            match default {
                Expr::Function { name, args, .. } => {
                    assert_eq!(name, "nextval");
                    assert_eq!(args.len(), 1);
                    match &args[0] {
                        Expr::Literal(Value::Text(s)) => {
                            assert_eq!(s, "t_id_seq");
                        }
                        other => panic!("expected Text arg, got {other:?}"),
                    }
                }
                other => panic!("expected Function, got {other:?}"),
            }
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_parse_bigserial_smallserial() {
    let stmt = must_parse("CREATE TABLE t (a BIGSERIAL, b SMALLSERIAL)");
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert_eq!(columns.len(), 2);
            // BIGSERIAL 与 SMALLSERIAL 都映射为 Int64（PG 中 BIGSERIAL→BIGINT, SMALLSERIAL→SMALLINT）
            assert_eq!(columns[0].data_type, ColumnType::Int64);
            assert_eq!(columns[1].data_type, ColumnType::Int64);
            assert!(columns[0].not_null);
            assert!(columns[1].not_null);
            assert!(columns[0].default.is_some());
            assert!(columns[1].default.is_some());
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_parse_serial_with_explicit_not_null() {
    // SERIAL 隐含 NOT NULL；显式 NOT NULL 不冲突
    let stmt = must_parse("CREATE TABLE t (id SERIAL NOT NULL)");
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert!(columns[0].not_null);
            assert!(columns[0].default.is_some());
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

// =====================================================================
//  SequenceStore 基础测试（6 条）
// =====================================================================

#[test]
fn test_sequence_store_create_and_exists() {
    let mut store = InMemorySequenceStore::new();
    let def = SequenceDefinition::new(TableName::new("seq"));
    store.create_sequence(def).unwrap();
    assert!(store.sequence_exists(&TableName::new("seq")));
    assert!(!store.sequence_exists(&TableName::new("other")));
}

#[test]
fn test_sequence_store_create_duplicate_fails() {
    let mut store = InMemorySequenceStore::new();
    let def = SequenceDefinition::new(TableName::new("seq"));
    store.create_sequence(def).unwrap();
    let def2 = SequenceDefinition::new(TableName::new("seq"));
    let err = store.create_sequence(def2).unwrap_err();
    assert!(matches!(err, ExecutionError::SequenceAlreadyExists(_)));
}

#[test]
fn test_sequence_store_drop() {
    let mut store = InMemorySequenceStore::new();
    store
        .create_sequence(SequenceDefinition::new(TableName::new("seq")))
        .unwrap();
    store.drop_sequence(&TableName::new("seq"), false).unwrap();
    assert!(!store.sequence_exists(&TableName::new("seq")));
}

#[test]
fn test_sequence_store_drop_nonexistent_with_if_exists() {
    let mut store = InMemorySequenceStore::new();
    // IF EXISTS=true：不存在不报错
    store.drop_sequence(&TableName::new("seq"), true).unwrap();
    // IF EXISTS=false：不存在报错
    let err = store
        .drop_sequence(&TableName::new("seq"), false)
        .unwrap_err();
    assert!(matches!(err, ExecutionError::SequenceNotFound(_)));
}

#[test]
fn test_sequence_store_nextval_123() {
    let mut store = InMemorySequenceStore::new();
    store
        .create_sequence(SequenceDefinition::new(TableName::new("seq")))
        .unwrap();
    // 默认 start=1, increment=1 → 1, 2, 3
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 1);
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 2);
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 3);
}

#[test]
fn test_sequence_store_currval_not_defined() {
    let mut store = InMemorySequenceStore::new();
    store
        .create_sequence(SequenceDefinition::new(TableName::new("seq")))
        .unwrap();
    // 未调用 nextval → currval 报错
    let err = store.current_value(&TableName::new("seq")).unwrap_err();
    assert!(matches!(err, ExecutionError::SequenceCurrvalNotDefined(_)));
    // 调用 nextval 后 currval 可用
    store.next_value(&TableName::new("seq")).unwrap();
    assert_eq!(store.current_value(&TableName::new("seq")).unwrap(), 1);
}

// =====================================================================
//  CYCLE / NO CYCLE 测试（4 条）
// =====================================================================

#[test]
fn test_sequence_cycle_ascending() {
    let mut store = InMemorySequenceStore::new();
    let def = SequenceDefinition {
        name: TableName::new("seq"),
        start: 1,
        increment: 1,
        min_value: Some(1),
        max_value: Some(3),
        cycle: true,
    };
    store.create_sequence(def).unwrap();
    // 升序 CYCLE：1, 2, 3, 1, 2, 3, ...
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 1);
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 2);
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 3);
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 1);
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 2);
}

#[test]
fn test_sequence_cycle_descending() {
    let mut store = InMemorySequenceStore::new();
    let def = SequenceDefinition {
        name: TableName::new("seq"),
        start: 3,
        increment: -1,
        min_value: Some(1),
        max_value: Some(3),
        cycle: true,
    };
    store.create_sequence(def).unwrap();
    // 降序 CYCLE：3, 2, 1, 3, 2, 1, ...
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 3);
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 2);
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 1);
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 3);
}

#[test]
fn test_sequence_no_cycle_overflow() {
    let mut store = InMemorySequenceStore::new();
    let def = SequenceDefinition {
        name: TableName::new("seq"),
        start: 1,
        increment: 1,
        min_value: Some(1),
        max_value: Some(3),
        cycle: false,
    };
    store.create_sequence(def).unwrap();
    // NO CYCLE：1, 2, 3，第 4 次越界报错
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 1);
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 2);
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 3);
    let err = store.next_value(&TableName::new("seq")).unwrap_err();
    assert!(matches!(err, ExecutionError::SequenceOutOfRange(_)));
}

#[test]
fn test_sequence_negative_increment() {
    let mut store = InMemorySequenceStore::new();
    let def = SequenceDefinition {
        name: TableName::new("seq"),
        start: 10,
        increment: -2,
        min_value: None,
        max_value: None,
        cycle: false,
    };
    store.create_sequence(def).unwrap();
    // 10, 8, 6, 4
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 10);
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 8);
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 6);
    assert_eq!(store.next_value(&TableName::new("seq")).unwrap(), 4);
}

// =====================================================================
//  Executor DDL 测试（4 条）
// =====================================================================

#[test]
fn test_executor_create_sequence() {
    let plan = plan_sql("CREATE SEQUENCE seq");
    let executor = Executor::new();
    let mut store = InMemorySequenceStore::new();
    executor.execute_create_sequence(&plan, &mut store).unwrap();
    assert!(store.sequence_exists(&TableName::new("seq")));
}

#[test]
fn test_executor_drop_sequence() {
    let plan_create = plan_sql("CREATE SEQUENCE seq");
    let plan_drop = plan_sql("DROP SEQUENCE seq");
    let executor = Executor::new();
    let mut store = InMemorySequenceStore::new();
    executor
        .execute_create_sequence(&plan_create, &mut store)
        .unwrap();
    executor
        .execute_drop_sequence(&plan_drop, &mut store)
        .unwrap();
    assert!(!store.sequence_exists(&TableName::new("seq")));
}

#[test]
fn test_executor_create_sequence_if_not_exists() {
    let plan = plan_sql("CREATE SEQUENCE IF NOT EXISTS seq");
    let executor = Executor::new();
    let mut store = InMemorySequenceStore::new();
    // 第一次创建
    executor.execute_create_sequence(&plan, &mut store).unwrap();
    // 第二次：IF NOT EXISTS，应跳过
    executor.execute_create_sequence(&plan, &mut store).unwrap();
    assert!(store.sequence_exists(&TableName::new("seq")));
}

#[test]
fn test_executor_drop_sequence_if_exists() {
    let plan = plan_sql("DROP SEQUENCE IF EXISTS nonexistent");
    let executor = Executor::new();
    let mut store = InMemorySequenceStore::new();
    // 不存在 + IF EXISTS → 不报错
    executor.execute_drop_sequence(&plan, &mut store).unwrap();
}

// =====================================================================
//  Executor nextval/currval 测试（5 条）
// =====================================================================

#[test]
fn test_executor_select_nextval_123() {
    // 验证用例：CREATE SEQUENCE seq START 1 → SELECT nextval('seq') → 1,2,3
    let create_plan = plan_sql("CREATE SEQUENCE seq START 1");
    let select_plan = plan_sql("SELECT nextval('seq')");
    let executor = Executor::new();
    let mut store = InMemorySequenceStore::new();
    executor
        .execute_create_sequence(&create_plan, &mut store)
        .unwrap();
    // 第一次 SELECT nextval → 1
    let rows = executor
        .execute_with_sequences(&select_plan, &mut store)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int64(1));
    // 第二次 → 2
    let rows = executor
        .execute_with_sequences(&select_plan, &mut store)
        .unwrap();
    assert_eq!(rows[0][0], Value::Int64(2));
    // 第三次 → 3
    let rows = executor
        .execute_with_sequences(&select_plan, &mut store)
        .unwrap();
    assert_eq!(rows[0][0], Value::Int64(3));
}

#[test]
fn test_executor_select_currval() {
    let create_plan = plan_sql("CREATE SEQUENCE seq");
    let nextval_plan = plan_sql("SELECT nextval('seq')");
    let currval_plan = plan_sql("SELECT currval('seq')");
    let executor = Executor::new();
    let mut store = InMemorySequenceStore::new();
    executor
        .execute_create_sequence(&create_plan, &mut store)
        .unwrap();
    // 未调用 nextval → currval 报错
    let err = executor
        .execute_with_sequences(&currval_plan, &mut store)
        .unwrap_err();
    assert!(matches!(err, ExecutionError::SequenceCurrvalNotDefined(_)));
    // 调用 nextval 后 currval 返回最近值
    let _ = executor
        .execute_with_sequences(&nextval_plan, &mut store)
        .unwrap();
    let _ = executor
        .execute_with_sequences(&nextval_plan, &mut store)
        .unwrap();
    let rows = executor
        .execute_with_sequences(&currval_plan, &mut store)
        .unwrap();
    assert_eq!(rows[0][0], Value::Int64(2));
}

#[test]
fn test_executor_nextval_in_expression() {
    let create_plan = plan_sql("CREATE SEQUENCE seq");
    // nextval('seq') + 100 → 101
    let select_plan = plan_sql("SELECT nextval('seq') + 100 AS result");
    let executor = Executor::new();
    let mut store = InMemorySequenceStore::new();
    executor
        .execute_create_sequence(&create_plan, &mut store)
        .unwrap();
    let rows = executor
        .execute_with_sequences(&select_plan, &mut store)
        .unwrap();
    assert_eq!(rows[0][0], Value::Int64(101));
}

#[test]
fn test_executor_nextval_with_cast() {
    let create_plan = plan_sql("CREATE SEQUENCE seq");
    let select_plan = plan_sql("SELECT CAST(nextval('seq') AS BIGINT)");
    let executor = Executor::new();
    let mut store = InMemorySequenceStore::new();
    executor
        .execute_create_sequence(&create_plan, &mut store)
        .unwrap();
    let rows = executor
        .execute_with_sequences(&select_plan, &mut store)
        .unwrap();
    assert_eq!(rows[0][0], Value::Int64(1));
}

#[test]
fn test_executor_nextval_nonexistent_fails() {
    let select_plan = plan_sql("SELECT nextval('nonexistent')");
    let executor = Executor::new();
    let mut store = InMemorySequenceStore::new();
    let err = executor
        .execute_with_sequences(&select_plan, &mut store)
        .unwrap_err();
    assert!(matches!(err, ExecutionError::SequenceNotFound(_)));
}

// =====================================================================
//  SERIAL 端到端测试（4 条）
// =====================================================================

#[test]
fn test_serial_end_to_end_auto_increment() {
    // 验证用例：CREATE TABLE t (id SERIAL PRIMARY KEY, name TEXT) → INSERT → id 自动递增
    use crate::executor::InMemoryTable;

    // 1. CREATE TABLE（包含 SERIAL 列）
    let create_table_plan = plan_sql("CREATE TABLE t (id SERIAL PRIMARY KEY, name TEXT)");
    // 从计划中提取 schema
    let schema = match &create_table_plan {
        LogicalPlan::CreateTable { name, columns, .. } => {
            assert_eq!(name.qualified_name(), "t");
            assert_eq!(columns.len(), 2);
            TableSchema {
                name: name.clone(),
                columns: columns.clone(),
            }
        }
        other => panic!("expected CreateTable, got {other:?}"),
    };

    // 2. 创建内存表 + catalog（用于 plan INSERT）
    let mut table = InMemoryTable::new(schema);
    let mut catalog = InMemoryCatalog::new();
    register_table_from_create_plan(&create_table_plan, &mut catalog);

    // 3. 创建对应的序列（CREATE TABLE 不会自动创建序列，需手动创建）
    let mut store = InMemorySequenceStore::new();
    store
        .create_sequence(SequenceDefinition::new(TableName::new("t_id_seq")))
        .unwrap();

    // 4. INSERT (name) VALUES ('alice') — id 应自动为 1
    let insert_plan = plan_sql_with_catalog("INSERT INTO t (name) VALUES ('alice')", &catalog);
    let executor = Executor::new();
    let result = executor
        .execute_insert_with_sequences(&insert_plan, &mut table, &mut store)
        .unwrap();
    assert_eq!(result.affected_rows, 1);

    // 5. 验证表中有 1 行，id=1, name='alice'
    let rows: Vec<_> = table.scan_iter().collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int64(1));
    assert_eq!(rows[0][1], Value::Text("alice".to_string()));

    // 6. 第二次 INSERT → id=2
    let insert_plan2 = plan_sql_with_catalog("INSERT INTO t (name) VALUES ('bob')", &catalog);
    let _ = executor
        .execute_insert_with_sequences(&insert_plan2, &mut table, &mut store)
        .unwrap();
    let rows: Vec<_> = table.scan_iter().collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1][0], Value::Int64(2));
    assert_eq!(rows[1][1], Value::Text("bob".to_string()));
}

#[test]
fn test_serial_explicit_value_overrides_default() {
    use crate::executor::InMemoryTable;

    let create_table_plan = plan_sql("CREATE TABLE t (id SERIAL, name TEXT)");
    let schema = match &create_table_plan {
        LogicalPlan::CreateTable { name, columns, .. } => TableSchema {
            name: name.clone(),
            columns: columns.clone(),
        },
        _ => unreachable!(),
    };
    let mut table = InMemoryTable::new(schema);
    let mut catalog = InMemoryCatalog::new();
    register_table_from_create_plan(&create_table_plan, &mut catalog);
    let mut store = InMemorySequenceStore::new();
    store
        .create_sequence(SequenceDefinition::new(TableName::new("t_id_seq")))
        .unwrap();

    // 显式指定 id=100，覆盖 DEFAULT
    let insert_plan =
        plan_sql_with_catalog("INSERT INTO t (id, name) VALUES (100, 'alice')", &catalog);
    let executor = Executor::new();
    let result = executor
        .execute_insert_with_sequences(&insert_plan, &mut table, &mut store)
        .unwrap();
    assert_eq!(result.affected_rows, 1);
    let rows: Vec<_> = table.scan_iter().collect();
    assert_eq!(rows[0][0], Value::Int64(100));
}

#[test]
fn test_serial_multiple_rows_insert() {
    use crate::executor::InMemoryTable;

    let create_table_plan = plan_sql("CREATE TABLE t (id SERIAL, name TEXT)");
    let schema = match &create_table_plan {
        LogicalPlan::CreateTable { name, columns, .. } => TableSchema {
            name: name.clone(),
            columns: columns.clone(),
        },
        _ => unreachable!(),
    };
    let mut table = InMemoryTable::new(schema);
    let mut catalog = InMemoryCatalog::new();
    register_table_from_create_plan(&create_table_plan, &mut catalog);
    let mut store = InMemorySequenceStore::new();
    store
        .create_sequence(SequenceDefinition::new(TableName::new("t_id_seq")))
        .unwrap();

    // 多行 INSERT，每行 id 自动递增
    let insert_plan =
        plan_sql_with_catalog("INSERT INTO t (name) VALUES ('a'), ('b'), ('c')", &catalog);
    let executor = Executor::new();
    let result = executor
        .execute_insert_with_sequences(&insert_plan, &mut table, &mut store)
        .unwrap();
    assert_eq!(result.affected_rows, 3);
    let rows: Vec<_> = table.scan_iter().collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][0], Value::Int64(1));
    assert_eq!(rows[1][0], Value::Int64(2));
    assert_eq!(rows[2][0], Value::Int64(3));
}

#[test]
fn test_bigserial_end_to_end() {
    use crate::executor::InMemoryTable;

    let create_table_plan = plan_sql("CREATE TABLE t (id BIGSERIAL PRIMARY KEY, name TEXT)");
    let schema = match &create_table_plan {
        LogicalPlan::CreateTable { name, columns, .. } => TableSchema {
            name: name.clone(),
            columns: columns.clone(),
        },
        _ => unreachable!(),
    };
    let mut table = InMemoryTable::new(schema);
    let mut catalog = InMemoryCatalog::new();
    register_table_from_create_plan(&create_table_plan, &mut catalog);
    let mut store = InMemorySequenceStore::new();
    store
        .create_sequence(SequenceDefinition::new(TableName::new("t_id_seq")))
        .unwrap();

    let insert_plan = plan_sql_with_catalog("INSERT INTO t (name) VALUES ('x')", &catalog);
    let executor = Executor::new();
    let _ = executor
        .execute_insert_with_sequences(&insert_plan, &mut table, &mut store)
        .unwrap();
    let rows: Vec<_> = table.scan_iter().collect();
    assert_eq!(rows[0][0], Value::Int64(1));
}

// =====================================================================
//  端到端完整流程测试（3 条）
// =====================================================================

#[test]
fn test_end_to_end_create_nextval_drop() {
    // 完整流程：CREATE SEQUENCE → SELECT nextval → 1,2,3 → DROP SEQUENCE → nextval 报错
    let create_plan = plan_sql("CREATE SEQUENCE seq START 1");
    let nextval_plan = plan_sql("SELECT nextval('seq')");
    let drop_plan = plan_sql("DROP SEQUENCE seq");

    let executor = Executor::new();
    let mut store = InMemorySequenceStore::new();

    // 1. CREATE
    executor
        .execute_create_sequence(&create_plan, &mut store)
        .unwrap();
    assert!(store.sequence_exists(&TableName::new("seq")));

    // 2. SELECT nextval → 1, 2, 3
    for expected in [1_i64, 2, 3] {
        let rows = executor
            .execute_with_sequences(&nextval_plan, &mut store)
            .unwrap();
        assert_eq!(rows[0][0], Value::Int64(expected));
    }

    // 3. DROP
    executor
        .execute_drop_sequence(&drop_plan, &mut store)
        .unwrap();
    assert!(!store.sequence_exists(&TableName::new("seq")));

    // 4. nextval 报错
    let err = executor
        .execute_with_sequences(&nextval_plan, &mut store)
        .unwrap_err();
    assert!(matches!(err, ExecutionError::SequenceNotFound(_)));
}

#[test]
fn test_end_to_end_currval_session_semantics() {
    // currval 会话语义：调用 nextval 后 currval 返回最近值；不同序列互不干扰
    let create_a = plan_sql("CREATE SEQUENCE seq_a");
    let create_b = plan_sql("CREATE SEQUENCE seq_b");

    let executor = Executor::new();
    let mut store = InMemorySequenceStore::new();
    executor
        .execute_create_sequence(&create_a, &mut store)
        .unwrap();
    executor
        .execute_create_sequence(&create_b, &mut store)
        .unwrap();

    // seq_a: nextval → 1
    let rows = executor
        .execute_with_sequences(&plan_sql("SELECT nextval('seq_a')"), &mut store)
        .unwrap();
    assert_eq!(rows[0][0], Value::Int64(1));

    // seq_b: nextval → 1（独立序列）
    let rows = executor
        .execute_with_sequences(&plan_sql("SELECT nextval('seq_b')"), &mut store)
        .unwrap();
    assert_eq!(rows[0][0], Value::Int64(1));

    // currval('seq_a') → 1（不受 seq_b 影响）
    let rows = executor
        .execute_with_sequences(&plan_sql("SELECT currval('seq_a')"), &mut store)
        .unwrap();
    assert_eq!(rows[0][0], Value::Int64(1));

    // seq_a: nextval → 2，currval → 2
    let _ = executor
        .execute_with_sequences(&plan_sql("SELECT nextval('seq_a')"), &mut store)
        .unwrap();
    let rows = executor
        .execute_with_sequences(&plan_sql("SELECT currval('seq_a')"), &mut store)
        .unwrap();
    assert_eq!(rows[0][0], Value::Int64(2));
}

#[test]
fn test_end_to_end_custom_increment_and_start() {
    // CREATE SEQUENCE seq INCREMENT 10 START 100 → 100, 110, 120
    // 注意：sqlparser 0.53.0 要求选项顺序：INCREMENT 在 START 之前
    let create_plan = plan_sql("CREATE SEQUENCE seq INCREMENT BY 10 START WITH 100");
    let nextval_plan = plan_sql("SELECT nextval('seq')");

    let executor = Executor::new();
    let mut store = InMemorySequenceStore::new();
    executor
        .execute_create_sequence(&create_plan, &mut store)
        .unwrap();

    for expected in [100_i64, 110, 120] {
        let rows = executor
            .execute_with_sequences(&nextval_plan, &mut store)
            .unwrap();
        assert_eq!(rows[0][0], Value::Int64(expected));
    }
}
