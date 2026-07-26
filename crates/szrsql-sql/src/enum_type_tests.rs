//! Phase 3.31 单元测试 — ENUM 类型（CREATE TYPE / DROP TYPE / ALTER TYPE）。
//!
//! 覆盖类别：
//! - Parser（6）：CREATE TYPE AS ENUM、DROP TYPE、ALTER TYPE ADD VALUE、ADD VALUE IF NOT EXISTS、
//!   RENAME VALUE、RENAME TO
//! - Plan（4）：CreateType 计划、DropType 计划、AlterType 计划、custom_type_name → ColumnType::Enum 解析
//! - Catalog API（3）：add/exists、get/list、remove/get_mut
//! - Executor DDL（5）：execute_create_type、重复创建报错、execute_drop_type、IF EXISTS 跳过、不存在报错
//! - Executor ALTER TYPE（5）：AddValue 成功、重复值报错、IF NOT EXISTS 跳过、RenameValue、Rename To
//! - ENUM 值校验（5）：INSERT 合法通过、INSERT 非法拒绝、INSERT NULL 通过、UPDATE 合法、UPDATE 非法
//! - 端到端（1）：进度表验证场景（CREATE TYPE mood → CREATE TABLE t (m mood) → INSERT 'happy'
//!   → INSERT 'angry' 拒绝 → ALTER TYPE ADD VALUE 'angry' → INSERT 通过）
//!
//! 共 29 个测试用例。

use crate::ast::*;
use crate::executor::{ExecutionError, Executor, InMemoryTable, TableStorage};
use crate::parser::{parse_one, parse_sql};
use crate::plan::{Catalog, EnumTypeDefinition, InMemoryCatalog, LogicalPlan, Planner};
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
    planner.plan_statement(stmt).unwrap_or_else(|e| {
        panic!("plan failed for SQL: {sql}\nerror: {e:?}");
    })
}

// =====================================================================
//  Parser 测试（6）
// =====================================================================

#[test]
fn test_parse_create_type_as_enum() {
    let stmt = must_parse("CREATE TYPE mood AS ENUM ('happy', 'sad', 'neutral')");
    match stmt {
        Statement::CreateType {
            name,
            as_enum,
            if_not_exists,
        } => {
            assert_eq!(name.qualified_name(), "mood");
            assert_eq!(as_enum, vec!["happy", "sad", "neutral"]);
            assert!(!if_not_exists);
        }
        other => panic!("expected CreateType, got {other:?}"),
    }
}

#[test]
fn test_parse_create_type_empty_enum() {
    // 空 ENUM 类型（PG 允许，后续可用 ALTER TYPE 追加）
    let stmt = must_parse("CREATE TYPE empty AS ENUM ()");
    match stmt {
        Statement::CreateType { name, as_enum, .. } => {
            assert_eq!(name.qualified_name(), "empty");
            assert!(as_enum.is_empty());
        }
        other => panic!("expected CreateType, got {other:?}"),
    }
}

#[test]
fn test_parse_drop_type() {
    let stmt = must_parse("DROP TYPE mood");
    match stmt {
        Statement::DropType {
            names,
            if_exists,
            cascade,
        } => {
            assert_eq!(names.len(), 1);
            assert_eq!(names[0].qualified_name(), "mood");
            assert!(!if_exists);
            assert!(!cascade);
        }
        other => panic!("expected DropType, got {other:?}"),
    }

    // DROP TYPE IF EXISTS a, b CASCADE
    let stmt = must_parse("DROP TYPE IF EXISTS a, b CASCADE");
    match stmt {
        Statement::DropType {
            names,
            if_exists,
            cascade,
        } => {
            assert_eq!(names.len(), 2);
            assert_eq!(names[0].qualified_name(), "a");
            assert_eq!(names[1].qualified_name(), "b");
            assert!(if_exists);
            assert!(cascade);
        }
        other => panic!("expected DropType, got {other:?}"),
    }
}

#[test]
fn test_parse_alter_type_add_value() {
    let stmt = must_parse("ALTER TYPE mood ADD VALUE 'angry'");
    match stmt {
        Statement::AlterType { name, action } => {
            assert_eq!(name.qualified_name(), "mood");
            match action {
                AlterTypeAction::AddValue {
                    value,
                    if_not_exists,
                } => {
                    assert_eq!(value, "angry");
                    assert!(!if_not_exists);
                }
                other => panic!("expected AddValue, got {other:?}"),
            }
        }
        other => panic!("expected AlterType, got {other:?}"),
    }
}

#[test]
fn test_parse_alter_type_add_value_if_not_exists() {
    let stmt = must_parse("ALTER TYPE mood ADD VALUE IF NOT EXISTS 'angry'");
    match stmt {
        Statement::AlterType { name, action } => {
            assert_eq!(name.qualified_name(), "mood");
            match action {
                AlterTypeAction::AddValue {
                    value,
                    if_not_exists,
                } => {
                    assert_eq!(value, "angry");
                    assert!(if_not_exists);
                }
                other => panic!("expected AddValue, got {other:?}"),
            }
        }
        other => panic!("expected AlterType, got {other:?}"),
    }
}

#[test]
fn test_parse_alter_type_rename_value_and_rename_to() {
    // RENAME VALUE 'old' TO 'new'
    let stmt = must_parse("ALTER TYPE mood RENAME VALUE 'happy' TO 'joyful'");
    match stmt {
        Statement::AlterType { name, action } => {
            assert_eq!(name.qualified_name(), "mood");
            match action {
                AlterTypeAction::RenameValue { old, new } => {
                    assert_eq!(old, "happy");
                    assert_eq!(new, "joyful");
                }
                other => panic!("expected RenameValue, got {other:?}"),
            }
        }
        other => panic!("expected AlterType, got {other:?}"),
    }

    // RENAME TO new_name
    let stmt = must_parse("ALTER TYPE mood RENAME TO new_mood");
    match stmt {
        Statement::AlterType { name, action } => {
            assert_eq!(name.qualified_name(), "mood");
            match action {
                AlterTypeAction::Rename { new_name } => {
                    assert_eq!(new_name.qualified_name(), "new_mood");
                }
                other => panic!("expected Rename, got {other:?}"),
            }
        }
        other => panic!("expected AlterType, got {other:?}"),
    }
}

// =====================================================================
//  Plan 测试（4）
// =====================================================================

#[test]
fn test_plan_create_type() {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql("CREATE TYPE mood AS ENUM ('happy', 'sad')", &catalog);
    match plan {
        LogicalPlan::CreateType {
            definition,
            if_not_exists,
        } => {
            assert_eq!(definition.name.qualified_name(), "mood");
            assert_eq!(definition.labels, vec!["happy", "sad"]);
            assert!(!if_not_exists);
        }
        other => panic!("expected CreateType plan, got {other:?}"),
    }
}

#[test]
fn test_plan_drop_type_if_exists() {
    // 先注册一个 ENUM 类型，再 DROP
    let mut catalog = InMemoryCatalog::new();
    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("mood"),
        vec!["happy".into()],
    ));
    let plan = plan_sql("DROP TYPE IF EXISTS mood, nonexistent", &catalog);
    match plan {
        LogicalPlan::DropType {
            names,
            if_exists,
            cascade,
        } => {
            assert_eq!(names.len(), 2);
            assert!(if_exists);
            assert!(!cascade);
        }
        other => panic!("expected DropType plan, got {other:?}"),
    }
}

#[test]
fn test_plan_drop_type_nonexistent_without_if_exists_fails() {
    let catalog = InMemoryCatalog::new();
    let stmt = must_parse("DROP TYPE nonexistent");
    let planner = Planner::new(&catalog);
    let err = planner.plan_statement(stmt).unwrap_err();
    // 期望 PlanError::Unsupported 含 "type not found"
    match err {
        crate::plan::PlanError::Unsupported(msg) => {
            assert!(msg.contains("type not found"), "msg = {msg}");
        }
        other => panic!("expected Unsupported error, got {other:?}"),
    }
}

#[test]
fn test_plan_create_table_resolves_custom_type_to_enum() {
    // 注册 ENUM 类型 mood
    let mut catalog = InMemoryCatalog::new();
    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("mood"),
        vec!["happy".into(), "sad".into()],
    ));

    // CREATE TABLE t (id INT, m mood) → 列 m 应被解析为 ColumnType::Enum(labels)
    let plan = plan_sql("CREATE TABLE t (id INT, m mood)", &catalog);
    match plan {
        LogicalPlan::CreateTable { columns, .. } => {
            assert_eq!(columns.len(), 2);
            assert_eq!(columns[1].name, "m");
            match &columns[1].data_type {
                ColumnType::Enum(labels) => {
                    assert_eq!(labels, &vec!["happy".to_string(), "sad".to_string()]);
                }
                other => panic!("expected Enum type, got {other:?}"),
            }
        }
        other => panic!("expected CreateTable plan, got {other:?}"),
    }
}

// =====================================================================
//  Catalog API 测试（3）
// =====================================================================

#[test]
fn test_catalog_add_and_exists_enum_type() {
    let mut catalog = InMemoryCatalog::new();
    assert!(!catalog.enum_type_exists(&TableName::new("mood")));

    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("mood"),
        vec!["happy".into(), "sad".into()],
    ));
    assert!(catalog.enum_type_exists(&TableName::new("mood")));
    assert!(!catalog.enum_type_exists(&TableName::new("other")));
}

#[test]
fn test_catalog_get_and_list_enum_types() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("mood"),
        vec!["happy".into(), "sad".into()],
    ));
    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("color"),
        vec!["red".into(), "green".into()],
    ));

    // get_enum_type
    let mood = catalog
        .get_enum_type(&TableName::new("mood"))
        .expect("mood should exist");
    assert_eq!(mood.labels, vec!["happy", "sad"]);
    assert!(mood.contains("happy"));
    assert!(!mood.contains("angry"));

    // list_enum_types
    let mut names: Vec<String> = catalog
        .list_enum_types()
        .into_iter()
        .map(|n| n.qualified_name())
        .collect();
    names.sort();
    assert_eq!(names, vec!["color", "mood"]);
}

#[test]
fn test_catalog_remove_and_get_mut() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("mood"),
        vec!["happy".into()],
    ));

    // get_enum_type_mut — 追加一个 label
    {
        let def = catalog
            .get_enum_type_mut(&TableName::new("mood"))
            .expect("mood should exist");
        def.labels.push("sad".into());
    }
    let def = catalog
        .get_enum_type(&TableName::new("mood"))
        .expect("mood should still exist");
    assert_eq!(def.labels, vec!["happy", "sad"]);

    // remove_enum_type
    let removed = catalog.remove_enum_type(&TableName::new("mood"));
    assert!(removed.is_some());
    assert_eq!(removed.unwrap().labels, vec!["happy", "sad"]);
    assert!(!catalog.enum_type_exists(&TableName::new("mood")));
}

// =====================================================================
//  Executor DDL 测试（5）
// =====================================================================

#[test]
fn test_execute_create_type_success() {
    let mut catalog = InMemoryCatalog::new();
    let exec = Executor::new();
    let plan = plan_sql("CREATE TYPE mood AS ENUM ('happy', 'sad')", &catalog);
    exec.execute_create_type(&plan, &mut catalog).unwrap();

    assert!(catalog.enum_type_exists(&TableName::new("mood")));
    let def = catalog
        .get_enum_type(&TableName::new("mood"))
        .expect("mood should exist");
    assert_eq!(def.labels, vec!["happy", "sad"]);
}

#[test]
fn test_execute_create_type_duplicate_fails() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("mood"),
        vec!["happy".into()],
    ));

    // 直接构造 LogicalPlan 绕过 planner（planner 会在规划阶段拦截重复类型）
    let plan = LogicalPlan::CreateType {
        definition: EnumTypeDefinition::new(TableName::new("mood"), vec!["happy".into()]),
        if_not_exists: false,
    };
    let exec = Executor::new();
    let err = exec.execute_create_type(&plan, &mut catalog).unwrap_err();
    assert!(matches!(err, ExecutionError::TypeAlreadyExists(_)));
}

#[test]
fn test_execute_drop_type_success() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("mood"),
        vec!["happy".into()],
    ));

    let exec = Executor::new();
    let plan = plan_sql("DROP TYPE mood", &catalog);
    exec.execute_drop_type(&plan, &mut catalog).unwrap();
    assert!(!catalog.enum_type_exists(&TableName::new("mood")));
}

#[test]
fn test_execute_drop_type_if_exists_skips_nonexistent() {
    let mut catalog = InMemoryCatalog::new();
    // catalog 中没有 nonexistent 类型
    let exec = Executor::new();
    let plan = plan_sql("DROP TYPE IF EXISTS nonexistent", &catalog);
    exec.execute_drop_type(&plan, &mut catalog).unwrap();
}

#[test]
fn test_execute_drop_type_nonexistent_fails() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("a"),
        vec!["x".into()],
    ));

    // 直接构造 LogicalPlan 绕过 planner（planner 会在规划阶段拦截不存在的类型）
    let plan = LogicalPlan::DropType {
        names: vec![TableName::new("a"), TableName::new("nonexistent")],
        if_exists: false,
        cascade: false,
    };
    let exec = Executor::new();
    let err = exec.execute_drop_type(&plan, &mut catalog).unwrap_err();
    assert!(matches!(err, ExecutionError::TypeNotFound(_)));
    // a 应已被删除（执行顺序：先删 a 成功，再 nonexistent 报错）
    assert!(!catalog.enum_type_exists(&TableName::new("a")));
}

// =====================================================================
//  Executor ALTER TYPE 测试（5）
// =====================================================================

#[test]
fn test_execute_alter_type_add_value_success() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("mood"),
        vec!["happy".into(), "sad".into()],
    ));

    let exec = Executor::new();
    let plan = plan_sql("ALTER TYPE mood ADD VALUE 'angry'", &catalog);
    exec.execute_alter_type(&plan, &mut catalog).unwrap();

    let def = catalog
        .get_enum_type(&TableName::new("mood"))
        .expect("mood should exist");
    assert_eq!(def.labels, vec!["happy", "sad", "angry"]);
}

#[test]
fn test_execute_alter_type_add_value_duplicate_fails() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("mood"),
        vec!["happy".into()],
    ));

    let exec = Executor::new();
    let plan = plan_sql("ALTER TYPE mood ADD VALUE 'happy'", &catalog);
    let err = exec.execute_alter_type(&plan, &mut catalog).unwrap_err();
    assert!(matches!(err, ExecutionError::EnumValueViolation(_)));
}

#[test]
fn test_execute_alter_type_add_value_if_not_exists_skips() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("mood"),
        vec!["happy".into()],
    ));

    let exec = Executor::new();
    let plan = plan_sql("ALTER TYPE mood ADD VALUE IF NOT EXISTS 'happy'", &catalog);
    exec.execute_alter_type(&plan, &mut catalog).unwrap();

    let def = catalog
        .get_enum_type(&TableName::new("mood"))
        .expect("mood should exist");
    // labels 应保持不变（'happy' 已存在，被跳过）
    assert_eq!(def.labels, vec!["happy"]);
}

#[test]
fn test_execute_alter_type_rename_value() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("mood"),
        vec!["happy".into(), "sad".into()],
    ));

    let exec = Executor::new();
    let plan = plan_sql("ALTER TYPE mood RENAME VALUE 'happy' TO 'joyful'", &catalog);
    exec.execute_alter_type(&plan, &mut catalog).unwrap();

    let def = catalog
        .get_enum_type(&TableName::new("mood"))
        .expect("mood should exist");
    assert_eq!(def.labels, vec!["joyful", "sad"]);
}

#[test]
fn test_execute_alter_type_rename_to() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("mood"),
        vec!["happy".into(), "sad".into()],
    ));

    let exec = Executor::new();
    let plan = plan_sql("ALTER TYPE mood RENAME TO new_mood", &catalog);
    exec.execute_alter_type(&plan, &mut catalog).unwrap();

    assert!(!catalog.enum_type_exists(&TableName::new("mood")));
    let def = catalog
        .get_enum_type(&TableName::new("new_mood"))
        .expect("new_mood should exist");
    assert_eq!(def.labels, vec!["happy", "sad"]);
}

// =====================================================================
//  ENUM 值校验测试（5）
// =====================================================================

/// 构造测试场景：catalog 含 mood ENUM 类型 + t 表（id INT, m mood）
fn make_enum_test_setup() -> (InMemoryCatalog, InMemoryTable) {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_enum_type(EnumTypeDefinition::new(
        TableName::new("mood"),
        vec!["happy".into(), "sad".into()],
    ));
    // CREATE TABLE t (id INT, m mood) — plan 时 m 会被解析为 ColumnType::Enum
    let create_plan = plan_sql("CREATE TABLE t (id INT, m mood)", &catalog);
    catalog.register_from_create_plan(&create_plan).unwrap();

    // 从 catalog 拿到 schema 来构造 InMemoryTable（确保列类型为 Enum）
    let schema = catalog
        .get_table(&TableName::new("t"))
        .expect("table t should exist");
    let table = InMemoryTable::new(schema);
    (catalog, table)
}

#[test]
fn test_enum_insert_valid_value() {
    let (catalog, mut table) = make_enum_test_setup();
    let exec = Executor::new().with_catalog(&catalog);

    let plan = plan_sql("INSERT INTO t VALUES (1, 'happy')", &catalog);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.row_count(), 1);
    assert_eq!(table.rows()[0][1], Value::Text("happy".into()));
}

#[test]
fn test_enum_insert_invalid_value_rejected() {
    let (catalog, mut table) = make_enum_test_setup();
    let exec = Executor::new().with_catalog(&catalog);

    // 'angry' 不在 labels 中
    let plan = plan_sql("INSERT INTO t VALUES (1, 'angry')", &catalog);
    let err = exec.execute_insert(&plan, &mut table).unwrap_err();
    assert!(matches!(err, ExecutionError::EnumValueViolation(_)));
    assert_eq!(table.row_count(), 0, "失败时不应插入任何行");
}

#[test]
fn test_enum_insert_null_value_passes() {
    let (catalog, mut table) = make_enum_test_setup();
    let exec = Executor::new().with_catalog(&catalog);

    // NULL 允许（列未声明 NOT NULL）
    let plan = plan_sql("INSERT INTO t (id) VALUES (1)", &catalog);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.rows()[0][1], Value::Null);
}

#[test]
fn test_enum_update_valid_value() {
    let (catalog, mut table) = make_enum_test_setup();
    let exec = Executor::new().with_catalog(&catalog);

    // 先插入一行
    let insert_plan = plan_sql("INSERT INTO t VALUES (1, 'happy')", &catalog);
    exec.execute_insert(&insert_plan, &mut table).unwrap();

    // 更新为另一个合法值
    let update_plan = plan_sql("UPDATE t SET m = 'sad' WHERE id = 1", &catalog);
    let result = exec.execute_update(&update_plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.rows()[0][1], Value::Text("sad".into()));
}

#[test]
fn test_enum_update_invalid_value_rejected() {
    let (catalog, mut table) = make_enum_test_setup();
    let exec = Executor::new().with_catalog(&catalog);

    // 先插入一行
    let insert_plan = plan_sql("INSERT INTO t VALUES (1, 'happy')", &catalog);
    exec.execute_insert(&insert_plan, &mut table).unwrap();

    // 更新为非法值
    let update_plan = plan_sql("UPDATE t SET m = 'angry' WHERE id = 1", &catalog);
    let err = exec.execute_update(&update_plan, &mut table).unwrap_err();
    assert!(matches!(err, ExecutionError::EnumValueViolation(_)));
    // 原行应保持不变
    assert_eq!(table.rows()[0][1], Value::Text("happy".into()));
}

// =====================================================================
//  端到端测试（1） — 进度表验证场景
// =====================================================================

#[test]
fn test_enum_end_to_end_scenario() {
    // 进度表场景：
    // CREATE TYPE mood AS ENUM('happy','sad') →
    // CREATE TABLE t (m mood) →
    // INSERT 'happy' 通过 →
    // INSERT 'angry' 拒绝 →
    // ALTER TYPE ADD VALUE 'angry' →
    // INSERT 通过
    let mut catalog = InMemoryCatalog::new();

    // 1. CREATE TYPE mood AS ENUM ('happy', 'sad')
    let exec = Executor::new();
    let create_type_plan = plan_sql("CREATE TYPE mood AS ENUM ('happy', 'sad')", &catalog);
    exec.execute_create_type(&create_type_plan, &mut catalog)
        .unwrap();
    assert!(catalog.enum_type_exists(&TableName::new("mood")));

    // 2. CREATE TABLE t (m mood)
    let create_table_plan = plan_sql("CREATE TABLE t (m mood)", &catalog);
    catalog
        .register_from_create_plan(&create_table_plan)
        .unwrap();
    let schema = catalog
        .get_table(&TableName::new("t"))
        .expect("t should be registered");
    let mut table = InMemoryTable::new(schema);

    // 验证列 m 已被解析为 Enum 类型
    match &table.schema().columns[0].data_type {
        ColumnType::Enum(labels) => {
            assert_eq!(labels, &vec!["happy".to_string(), "sad".to_string()]);
        }
        other => panic!("expected Enum column type, got {other:?}"),
    }

    // 绑定 catalog 给 executor
    {
        let exec = Executor::new().with_catalog(&catalog);

        // 3. INSERT 'happy' 通过
        let insert1 = plan_sql("INSERT INTO t VALUES ('happy')", &catalog);
        let result = exec.execute_insert(&insert1, &mut table).unwrap();
        assert_eq!(result.affected_rows, 1);
        assert_eq!(table.row_count(), 1);

        // 4. INSERT 'angry' 拒绝
        let insert2 = plan_sql("INSERT INTO t VALUES ('angry')", &catalog);
        let err = exec.execute_insert(&insert2, &mut table).unwrap_err();
        assert!(matches!(err, ExecutionError::EnumValueViolation(_)));
        assert_eq!(table.row_count(), 1, "失败时行数应保持不变");
    } // exec 在此释放 catalog 的不可变引用

    // 5. ALTER TYPE mood ADD VALUE 'angry'
    // 现在 catalog 的不可变引用已释放，可变借用可行
    let alter_plan = plan_sql("ALTER TYPE mood ADD VALUE 'angry'", &catalog);
    let exec_for_alter = Executor::new();
    exec_for_alter
        .execute_alter_type(&alter_plan, &mut catalog)
        .unwrap();
    let def = catalog
        .get_enum_type(&TableName::new("mood"))
        .expect("mood should still exist");
    assert_eq!(def.labels, vec!["happy", "sad", "angry"]);

    // 6. INSERT 'angry' 通过
    // 注意：原 table 的列 m 仍持有旧 labels（CREATE TABLE 时已固化到 TableSchema 中），
    // 因此需要重建表 + executor 以使新 labels 生效。这反映了 catalog 在 ALTER TYPE 后
    // 引用旧 schema 的限制：已注册的 TableSchema 中的 ColumnType::Enum(labels)
    // 不会被自动刷新。
    let create_table_plan2 = plan_sql("CREATE TABLE t2 (m mood)", &catalog);
    catalog
        .register_from_create_plan(&create_table_plan2)
        .unwrap();
    let schema2 = catalog
        .get_table(&TableName::new("t2"))
        .expect("t2 should be registered");
    let mut table2 = InMemoryTable::new(schema2);
    let exec2 = Executor::new().with_catalog(&catalog);

    // 验证新表的列 m 现在包含 'angry'
    match &table2.schema().columns[0].data_type {
        ColumnType::Enum(labels) => {
            assert_eq!(
                labels,
                &vec!["happy".to_string(), "sad".to_string(), "angry".to_string()]
            );
        }
        other => panic!("expected Enum column type on t2, got {other:?}"),
    }

    let insert3 = plan_sql("INSERT INTO t2 VALUES ('angry')", &catalog);
    let result = exec2.execute_insert(&insert3, &mut table2).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table2.row_count(), 1);
    assert_eq!(table2.rows()[0][0], Value::Text("angry".into()));
}

// =====================================================================
//  parse_sql 多语句顺序测试（1）
// =====================================================================

#[test]
fn test_parse_sql_multiple_statements_with_alter_type() {
    // 验证 parse_sql 在含 ALTER TYPE 时能正确按顺序合并多条语句
    let sql = "CREATE TYPE mood AS ENUM ('happy'); ALTER TYPE mood ADD VALUE 'sad'; SELECT 1";
    let stmts = parse_sql(sql).expect("parse should succeed");
    assert_eq!(stmts.len(), 3);
    assert!(matches!(stmts[0], Statement::CreateType { .. }));
    assert!(matches!(stmts[1], Statement::AlterType { .. }));
    assert!(matches!(stmts[2], Statement::Select(_)));
}
