//! Phase 3.29 单元测试 — 外键运行时校验。
//!
//! 覆盖类别：
//! - Catalog FK 注册（4）：列级 FK、表级 FK、PK 引用省略列名、add_foreign_key API
//! - INSERT 子表校验（5）：合法插入、非法插入拒绝、NULL 跳过、复合 FK 部分NULL、无 catalog 不校验
//! - UPDATE 子表校验（3）：合法 FK 更新、非法 FK 更新拒绝、非 FK 列更新不校验
//! - DELETE 父表 RESTRICT（3）：RESTRICT 拒绝、NO ACTION 拒绝、无引用成功
//! - DELETE CASCADE（3）：CASCADE 删子行、SET NULL 置 NULL、SET DEFAULT 回退 SET NULL
//! - UPDATE 父表 CASCADE（3）：CASCADE 更新子 FK、SET NULL 置 NULL、RESTRICT 拒绝
//! - 复合 FK（2）：多列 FK INSERT 校验、多列 FK DELETE CASCADE
//!
//! 共 23 个测试用例。

use crate::ast::*;
use crate::executor::{ExecutionError, Executor, InMemoryTable, MutableTable, TableStorage};
use crate::foreign_key::{CascadeOp, ForeignKeyValidator};
use crate::parser::parse_one;
use crate::plan::{Catalog, InMemoryCatalog, LogicalPlan, Planner, TableSchema};
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

/// 创建父子表 + catalog，用于 FK 测试
///
/// 父表 `parent(id INT PRIMARY KEY, name TEXT)`
/// 子表 `child(cid INT PRIMARY KEY, pid INT REFERENCES parent(id))`
fn make_parent_child_setup() -> (InMemoryCatalog, InMemoryTable, InMemoryTable) {
    let mut catalog = InMemoryCatalog::new();

    // 注册父表
    let parent_plan = plan_sql(
        "CREATE TABLE parent (id INT PRIMARY KEY, name TEXT)",
        &catalog,
    );
    catalog.register_from_create_plan(&parent_plan).unwrap();

    // 注册子表（列级 FK）
    let child_plan = plan_sql(
        "CREATE TABLE child (cid INT PRIMARY KEY, pid INT REFERENCES parent(id))",
        &catalog,
    );
    catalog.register_from_create_plan(&child_plan).unwrap();

    let parent_table = InMemoryTable::with_columns(
        "parent",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let child_table = InMemoryTable::with_columns(
        "child",
        vec![("cid", ColumnType::Int64), ("pid", ColumnType::Int64)],
    );

    (catalog, parent_table, child_table)
}

/// 创建父子表 + catalog，使用指定 ON DELETE 动作
fn make_parent_child_with_on_delete(
    action: &str,
) -> (InMemoryCatalog, InMemoryTable, InMemoryTable) {
    let mut catalog = InMemoryCatalog::new();

    let parent_plan = plan_sql(
        "CREATE TABLE parent (id INT PRIMARY KEY, name TEXT)",
        &catalog,
    );
    catalog.register_from_create_plan(&parent_plan).unwrap();

    let child_sql = format!(
        "CREATE TABLE child (cid INT PRIMARY KEY, pid INT REFERENCES parent(id) ON DELETE {action})"
    );
    let child_plan = plan_sql(&child_sql, &catalog);
    catalog.register_from_create_plan(&child_plan).unwrap();

    let parent_table = InMemoryTable::with_columns(
        "parent",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let child_table = InMemoryTable::with_columns(
        "child",
        vec![("cid", ColumnType::Int64), ("pid", ColumnType::Int64)],
    );

    (catalog, parent_table, child_table)
}

/// 创建父子表 + catalog，使用指定 ON UPDATE 动作
fn make_parent_child_with_on_update(
    action: &str,
) -> (InMemoryCatalog, InMemoryTable, InMemoryTable) {
    let mut catalog = InMemoryCatalog::new();

    let parent_plan = plan_sql(
        "CREATE TABLE parent (id INT PRIMARY KEY, name TEXT)",
        &catalog,
    );
    catalog.register_from_create_plan(&parent_plan).unwrap();

    let child_sql = format!(
        "CREATE TABLE child (cid INT PRIMARY KEY, pid INT REFERENCES parent(id) ON UPDATE {action})"
    );
    let child_plan = plan_sql(&child_sql, &catalog);
    catalog.register_from_create_plan(&child_plan).unwrap();

    let parent_table = InMemoryTable::with_columns(
        "parent",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let child_table = InMemoryTable::with_columns(
        "child",
        vec![("cid", ColumnType::Int64), ("pid", ColumnType::Int64)],
    );

    (catalog, parent_table, child_table)
}

// =====================================================================
//  Catalog FK 注册测试（4）
// =====================================================================

#[test]
fn test_fk_register_column_level() {
    // 列级 FK：`pid INT REFERENCES parent(id)`
    let (catalog, _, _) = make_parent_child_setup();

    // 验证子表有 1 个 outgoing FK
    let child_fks = catalog.get_foreign_keys(&TableName::new("child"));
    assert_eq!(child_fks.len(), 1);
    assert_eq!(child_fks[0].columns, vec!["pid"]);
    assert_eq!(child_fks[0].reference.table.name, "parent");
    assert_eq!(child_fks[0].reference.columns, Some(vec!["id".to_string()]));

    // 验证父表有 1 个 incoming 引用
    let parent_refs = catalog.get_referencing_keys(&TableName::new("parent"));
    assert_eq!(parent_refs.len(), 1);
    assert_eq!(parent_refs[0].child_table.name, "child");
    assert_eq!(parent_refs[0].child_columns, vec!["pid"]);
    assert_eq!(parent_refs[0].parent_columns, vec!["id"]);
}

#[test]
fn test_fk_register_table_level() {
    // 表级 FK：`FOREIGN KEY (pid) REFERENCES parent(id)`
    let mut catalog = InMemoryCatalog::new();

    let parent_plan = plan_sql(
        "CREATE TABLE parent (id INT PRIMARY KEY, name TEXT)",
        &catalog,
    );
    catalog.register_from_create_plan(&parent_plan).unwrap();

    let child_plan = plan_sql(
        "CREATE TABLE child (
            cid INT PRIMARY KEY,
            pid INT,
            FOREIGN KEY (pid) REFERENCES parent(id)
        )",
        &catalog,
    );
    catalog.register_from_create_plan(&child_plan).unwrap();

    let child_fks = catalog.get_foreign_keys(&TableName::new("child"));
    assert_eq!(child_fks.len(), 1);
    assert_eq!(child_fks[0].columns, vec!["pid"]);
    assert_eq!(child_fks[0].reference.table.name, "parent");

    let parent_refs = catalog.get_referencing_keys(&TableName::new("parent"));
    assert_eq!(parent_refs.len(), 1);
}

#[test]
fn test_fk_register_pk_omission() {
    // FK 引用省略列名 → 应引用父表 PK
    let mut catalog = InMemoryCatalog::new();

    let parent_plan = plan_sql(
        "CREATE TABLE parent (id INT PRIMARY KEY, name TEXT)",
        &catalog,
    );
    catalog.register_from_create_plan(&parent_plan).unwrap();

    // REFERENCES parent（省略列名）
    let child_plan = plan_sql(
        "CREATE TABLE child (cid INT PRIMARY KEY, pid INT REFERENCES parent)",
        &catalog,
    );
    catalog.register_from_create_plan(&child_plan).unwrap();

    // 验证 parent_columns 被解析为 PK 列 "id"
    let parent_refs = catalog.get_referencing_keys(&TableName::new("parent"));
    assert_eq!(parent_refs.len(), 1);
    assert_eq!(parent_refs[0].parent_columns, vec!["id"]);
}

#[test]
fn test_fk_add_foreign_key_api() {
    // 直接使用 add_foreign_key API
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "parent",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    // 手动设置 PK
    {
        let schema = catalog.get_table(&TableName::new("parent")).unwrap();
        let mut new_cols = schema.columns.clone();
        new_cols[0].primary_key = true;
        catalog.add_table(TableSchema {
            name: TableName::new("parent"),
            columns: new_cols,
        });
    }
    catalog.add_simple_table(
        "child",
        vec![("cid", ColumnType::Int64), ("pid", ColumnType::Int64)],
    );

    // 添加 FK
    catalog
        .add_foreign_key(
            &TableName::new("child"),
            crate::plan::ForeignKeyConstraint {
                name: Some("fk_child_pid".to_string()),
                columns: vec!["pid".to_string()],
                reference: ForeignKeyReference {
                    table: TableName::new("parent"),
                    columns: Some(vec!["id".to_string()]),
                    on_delete: Some(ReferenceAction::Cascade),
                    on_update: Some(ReferenceAction::NoAction),
                },
            },
        )
        .unwrap();

    // 验证
    let child_fks = catalog.get_foreign_keys(&TableName::new("child"));
    assert_eq!(child_fks.len(), 1);
    assert_eq!(child_fks[0].name, Some("fk_child_pid".to_string()));

    let parent_refs = catalog.get_referencing_keys(&TableName::new("parent"));
    assert_eq!(parent_refs.len(), 1);
    assert_eq!(parent_refs[0].on_delete, ReferenceAction::Cascade);
    assert_eq!(parent_refs[0].on_update, ReferenceAction::NoAction);
}

// =====================================================================
//  INSERT 子表校验测试（5）
// =====================================================================

#[test]
fn test_fk_insert_valid_child() {
    // 父表有 id=1，子表插入 pid=1 → 成功
    let (catalog, mut parent, mut child) = make_parent_child_setup();
    parent.insert(vec![Value::Int64(1), Value::Text("alice".into())]);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&parent);
    let plan = plan_sql("INSERT INTO child VALUES (10, 1)", &catalog);
    let result = exec.execute_insert(&plan, &mut child).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(child.row_count(), 1);
}

#[test]
fn test_fk_insert_invalid_child_rejected() {
    // 父表有 id=1，子表插入 pid=999 → 拒绝（ForeignKeyViolation）
    let (catalog, mut parent, mut child) = make_parent_child_setup();
    parent.insert(vec![Value::Int64(1), Value::Text("alice".into())]);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&parent);
    let plan = plan_sql("INSERT INTO child VALUES (10, 999)", &catalog);
    let err = exec.execute_insert(&plan, &mut child).unwrap_err();
    assert!(matches!(err, ExecutionError::ForeignKeyViolation(_)));
    assert_eq!(child.row_count(), 0); // 行未插入
}

#[test]
fn test_fk_insert_null_skipped() {
    // FK 列为 NULL → 跳过校验（MATCH SIMPLE 语义）
    let (catalog, _parent, mut child) = make_parent_child_setup();

    let exec = Executor::new().with_catalog(&catalog);
    // pid 为 NULL → 即使父表为空也应成功
    let plan = plan_sql("INSERT INTO child (cid) VALUES (10)", &catalog);
    let result = exec.execute_insert(&plan, &mut child).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(child.get_row(0).unwrap()[1], Value::Null);
}

#[test]
fn test_fk_insert_composite_partial_null_skipped() {
    // 复合 FK：任一列为 NULL → 跳过校验
    let mut catalog = InMemoryCatalog::new();

    // 父表 PK 为 (a, b)
    let parent_plan = plan_sql(
        "CREATE TABLE parent (a INT, b INT, name TEXT, PRIMARY KEY (a, b))",
        &catalog,
    );
    catalog.register_from_create_plan(&parent_plan).unwrap();

    // 子表复合 FK
    let child_plan = plan_sql(
        "CREATE TABLE child (cid INT PRIMARY KEY, ca INT, cb INT, FOREIGN KEY (ca, cb) REFERENCES parent(a, b))",
        &catalog,
    );
    catalog.register_from_create_plan(&child_plan).unwrap();

    let mut child = InMemoryTable::with_columns(
        "child",
        vec![
            ("cid", ColumnType::Int64),
            ("ca", ColumnType::Int64),
            ("cb", ColumnType::Int64),
        ],
    );

    let exec = Executor::new().with_catalog(&catalog);
    // ca=NULL, cb=2 → 跳过校验
    let plan = plan_sql("INSERT INTO child (cid, cb) VALUES (10, 2)", &catalog);
    let result = exec.execute_insert(&plan, &mut child).unwrap();
    assert_eq!(result.affected_rows, 1);
}

#[test]
fn test_fk_insert_without_catalog_no_validation() {
    // 未绑定 catalog → 不做 FK 校验（向后兼容）
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "child",
        vec![("cid", ColumnType::Int64), ("pid", ColumnType::Int64)],
    );
    let mut child = InMemoryTable::with_columns(
        "child",
        vec![("cid", ColumnType::Int64), ("pid", ColumnType::Int64)],
    );

    // Executor 未设置 catalog（即使 catalog 中有表但无 FK 元数据也不校验）
    let exec = Executor::new();
    let plan = plan_sql("INSERT INTO child VALUES (10, 999)", &catalog);
    // 应该成功（无 FK 校验）
    let result = exec.execute_insert(&plan, &mut child).unwrap();
    assert_eq!(result.affected_rows, 1);
}

// =====================================================================
//  UPDATE 子表校验测试（3）
// =====================================================================

#[test]
fn test_fk_update_valid_fk() {
    // 父表有 id=1, id=2；子表 cid=10, pid=1；UPDATE pid=2 → 成功
    let (catalog, mut parent, mut child) = make_parent_child_setup();
    parent.insert(vec![Value::Int64(1), Value::Text("a".into())]);
    parent.insert(vec![Value::Int64(2), Value::Text("b".into())]);
    child.insert(vec![Value::Int64(10), Value::Int64(1)]);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&parent);
    let plan = plan_sql("UPDATE child SET pid = 2 WHERE cid = 10", &catalog);
    let result = exec.execute_update(&plan, &mut child).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(child.get_row(0).unwrap()[1], Value::Int64(2));
}

#[test]
fn test_fk_update_invalid_fk_rejected() {
    // 父表有 id=1；子表 cid=10, pid=1；UPDATE pid=999 → 拒绝
    let (catalog, mut parent, mut child) = make_parent_child_setup();
    parent.insert(vec![Value::Int64(1), Value::Text("a".into())]);
    child.insert(vec![Value::Int64(10), Value::Int64(1)]);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&parent);
    let plan = plan_sql("UPDATE child SET pid = 999 WHERE cid = 10", &catalog);
    let err = exec.execute_update(&plan, &mut child).unwrap_err();
    assert!(matches!(err, ExecutionError::ForeignKeyViolation(_)));
    // 原值应保持不变
    assert_eq!(child.get_row(0).unwrap()[1], Value::Int64(1));
}

#[test]
fn test_fk_update_non_fk_column_no_validation() {
    // 更新非 FK 列 → 不触发 FK 校验
    let (catalog, mut parent, mut child) = make_parent_child_setup();
    parent.insert(vec![Value::Int64(1), Value::Text("a".into())]);
    child.insert(vec![Value::Int64(10), Value::Int64(1)]);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&parent);
    // 更新 cid（PK，非 FK）→ 不应触发 FK 校验
    let plan = plan_sql("UPDATE child SET cid = 20 WHERE cid = 10", &catalog);
    let result = exec.execute_update(&plan, &mut child).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(child.get_row(0).unwrap()[0], Value::Int64(20));
}

// =====================================================================
//  DELETE 父表 RESTRICT 测试（3）
// =====================================================================

#[test]
fn test_fk_delete_restrict_rejected() {
    // ON DELETE RESTRICT + 有子表引用 → 拒绝
    let (catalog, mut parent, mut child) = make_parent_child_with_on_delete("RESTRICT");
    parent.insert(vec![Value::Int64(1), Value::Text("a".into())]);
    child.insert(vec![Value::Int64(10), Value::Int64(1)]);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&child);
    let plan = plan_sql("DELETE FROM parent WHERE id = 1", &catalog);
    let err = exec.execute_delete(&plan, &mut parent).unwrap_err();
    assert!(matches!(err, ExecutionError::ForeignKeyViolation(_)));
    // 父行应保留
    assert_eq!(parent.row_count(), 1);
}

#[test]
fn test_fk_delete_no_action_rejected() {
    // ON DELETE NO ACTION + 有子表引用 → 拒绝
    let (catalog, mut parent, mut child) = make_parent_child_with_on_delete("NO ACTION");
    parent.insert(vec![Value::Int64(1), Value::Text("a".into())]);
    child.insert(vec![Value::Int64(10), Value::Int64(1)]);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&child);
    let plan = plan_sql("DELETE FROM parent WHERE id = 1", &catalog);
    let err = exec.execute_delete(&plan, &mut parent).unwrap_err();
    assert!(matches!(err, ExecutionError::ForeignKeyViolation(_)));
    assert_eq!(parent.row_count(), 1);
}

#[test]
fn test_fk_delete_no_reference_success() {
    // 无子表引用 → 删除成功
    let (catalog, mut parent, mut child) = make_parent_child_with_on_delete("RESTRICT");
    parent.insert(vec![Value::Int64(1), Value::Text("a".into())]);
    parent.insert(vec![Value::Int64(2), Value::Text("b".into())]);
    child.insert(vec![Value::Int64(10), Value::Int64(1)]); // 引用 id=1

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&child);
    // 删除 id=2（无引用）→ 成功
    let plan = plan_sql("DELETE FROM parent WHERE id = 2", &catalog);
    let result = exec.execute_delete(&plan, &mut parent).unwrap();
    assert_eq!(result.affected_rows, 1);
}

// =====================================================================
//  DELETE CASCADE 测试（3）
// =====================================================================

#[test]
fn test_fk_delete_cascade() {
    // ON DELETE CASCADE → 删除父行 + 级联删除子行
    let (catalog, mut parent, mut child) = make_parent_child_with_on_delete("CASCADE");
    parent.insert(vec![Value::Int64(1), Value::Text("a".into())]);
    child.insert(vec![Value::Int64(10), Value::Int64(1)]);
    child.insert(vec![Value::Int64(11), Value::Int64(1)]); // 第二个子行引用 id=1

    // 使用 scoped Executor：注册 child 用于 FK 查找，作用域结束后释放借用
    let (result, cascades) = {
        let mut exec = Executor::new().with_catalog(&catalog);
        exec.register_table(&child);
        let plan = plan_sql("DELETE FROM parent WHERE id = 1", &catalog);
        exec.execute_delete_with_cascades(&plan, &mut parent)
            .unwrap()
    };
    assert_eq!(result.affected_rows, 1);

    // 应有 2 个 DeleteChildRow 级联操作
    assert_eq!(cascades.len(), 2);
    assert!(cascades
        .iter()
        .all(|op| matches!(op, CascadeOp::DeleteChildRow { .. })));

    // 应用级联（child 借用已释放）
    let mut tables: [(&str, &mut dyn MutableTable); 1] = [("child", &mut child)];
    let applied = Executor::apply_cascade_ops(cascades, &mut tables).unwrap();
    assert_eq!(applied, 2);

    // 子表应清空（活跃行）
    let active: Vec<_> = child.scan_iter().collect();
    assert_eq!(active.len(), 0);
}

#[test]
fn test_fk_delete_set_null() {
    // ON DELETE SET NULL → 子表 FK 列置为 NULL
    let (catalog, mut parent, mut child) = make_parent_child_with_on_delete("SET NULL");
    parent.insert(vec![Value::Int64(1), Value::Text("a".into())]);
    child.insert(vec![Value::Int64(10), Value::Int64(1)]);

    let (result, cascades) = {
        let mut exec = Executor::new().with_catalog(&catalog);
        exec.register_table(&child);
        let plan = plan_sql("DELETE FROM parent WHERE id = 1", &catalog);
        exec.execute_delete_with_cascades(&plan, &mut parent)
            .unwrap()
    };
    assert_eq!(result.affected_rows, 1);

    // 应有 1 个 UpdateChildRow 级联操作
    assert_eq!(cascades.len(), 1);
    match &cascades[0] {
        CascadeOp::UpdateChildRow { table, updates, .. } => {
            assert_eq!(table, "child");
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].0, 1); // pid 列索引
            assert_eq!(updates[0].1, Value::Null);
        }
        other => panic!("expected UpdateChildRow, got {other:?}"),
    }

    // 应用级联
    let mut tables: [(&str, &mut dyn MutableTable); 1] = [("child", &mut child)];
    let applied = Executor::apply_cascade_ops(cascades, &mut tables).unwrap();
    assert_eq!(applied, 1);

    // 子表 pid 列应为 NULL
    let active: Vec<_> = child.scan_iter().collect();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0][1], Value::Null);
}

#[test]
fn test_fk_delete_set_default_fallback() {
    // ON DELETE SET DEFAULT → 当前回退为 SET NULL
    let (catalog, mut parent, mut child) = make_parent_child_with_on_delete("SET DEFAULT");
    parent.insert(vec![Value::Int64(1), Value::Text("a".into())]);
    child.insert(vec![Value::Int64(10), Value::Int64(1)]);

    let (result, cascades) = {
        let mut exec = Executor::new().with_catalog(&catalog);
        exec.register_table(&child);
        let plan = plan_sql("DELETE FROM parent WHERE id = 1", &catalog);
        exec.execute_delete_with_cascades(&plan, &mut parent)
            .unwrap()
    };
    assert_eq!(result.affected_rows, 1);

    // 应有 1 个 UpdateChildRow（回退为 SET NULL）
    assert_eq!(cascades.len(), 1);
    assert!(matches!(cascades[0], CascadeOp::UpdateChildRow { .. }));
}

// =====================================================================
//  UPDATE 父表 CASCADE 测试（3）
// =====================================================================

#[test]
fn test_fk_update_cascade() {
    // ON UPDATE CASCADE → 父表 PK 改变，子表 FK 跟着改
    let (catalog, mut parent, mut child) = make_parent_child_with_on_update("CASCADE");
    parent.insert(vec![Value::Int64(1), Value::Text("a".into())]);
    child.insert(vec![Value::Int64(10), Value::Int64(1)]);

    let (result, cascades) = {
        let mut exec = Executor::new().with_catalog(&catalog);
        exec.register_table(&child);
        let plan = plan_sql("UPDATE parent SET id = 100 WHERE id = 1", &catalog);
        exec.execute_update_with_cascades(&plan, &mut parent)
            .unwrap()
    };
    assert_eq!(result.affected_rows, 1);

    // 应有 1 个 UpdateChildRow 级联
    assert_eq!(cascades.len(), 1);
    match &cascades[0] {
        CascadeOp::UpdateChildRow { table, updates, .. } => {
            assert_eq!(table, "child");
            assert_eq!(updates[0].1, Value::Int64(100)); // 新值
        }
        other => panic!("expected UpdateChildRow, got {other:?}"),
    }

    // 应用级联
    let mut tables: [(&str, &mut dyn MutableTable); 1] = [("child", &mut child)];
    let applied = Executor::apply_cascade_ops(cascades, &mut tables).unwrap();
    assert_eq!(applied, 1);

    // 子表 pid 应为 100
    let active: Vec<_> = child.scan_iter().collect();
    assert_eq!(active[0][1], Value::Int64(100));
}

#[test]
fn test_fk_update_set_null() {
    // ON UPDATE SET NULL → 父表 PK 改变，子表 FK 置 NULL
    let (catalog, mut parent, mut child) = make_parent_child_with_on_update("SET NULL");
    parent.insert(vec![Value::Int64(1), Value::Text("a".into())]);
    child.insert(vec![Value::Int64(10), Value::Int64(1)]);

    let (result, cascades) = {
        let mut exec = Executor::new().with_catalog(&catalog);
        exec.register_table(&child);
        let plan = plan_sql("UPDATE parent SET id = 100 WHERE id = 1", &catalog);
        exec.execute_update_with_cascades(&plan, &mut parent)
            .unwrap()
    };
    assert_eq!(result.affected_rows, 1);

    assert_eq!(cascades.len(), 1);
    match &cascades[0] {
        CascadeOp::UpdateChildRow { updates, .. } => {
            assert_eq!(updates[0].1, Value::Null);
        }
        other => panic!("expected UpdateChildRow, got {other:?}"),
    }

    // 应用级联
    let mut tables: [(&str, &mut dyn MutableTable); 1] = [("child", &mut child)];
    let _ = Executor::apply_cascade_ops(cascades, &mut tables).unwrap();

    let active: Vec<_> = child.scan_iter().collect();
    assert_eq!(active[0][1], Value::Null);
}

#[test]
fn test_fk_update_restrict_rejected() {
    // ON UPDATE RESTRICT + 有子表引用 → 拒绝
    let (catalog, mut parent, mut child) = make_parent_child_with_on_update("RESTRICT");
    parent.insert(vec![Value::Int64(1), Value::Text("a".into())]);
    child.insert(vec![Value::Int64(10), Value::Int64(1)]);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&child);
    let plan = plan_sql("UPDATE parent SET id = 100 WHERE id = 1", &catalog);
    let err = exec
        .execute_update_with_cascades(&plan, &mut parent)
        .unwrap_err();
    assert!(matches!(err, ExecutionError::ForeignKeyViolation(_)));
    // 父行应保留原值
    assert_eq!(parent.get_row(0).unwrap()[0], Value::Int64(1));
}

// =====================================================================
//  复合 FK 测试（2）
// =====================================================================

#[test]
fn test_fk_composite_insert_validation() {
    // 复合 FK：子表 (ca, cb) 引用父表 (a, b)
    let mut catalog = InMemoryCatalog::new();

    let parent_plan = plan_sql(
        "CREATE TABLE parent (a INT, b INT, name TEXT, PRIMARY KEY (a, b))",
        &catalog,
    );
    catalog.register_from_create_plan(&parent_plan).unwrap();

    let child_plan = plan_sql(
        "CREATE TABLE child (cid INT PRIMARY KEY, ca INT, cb INT, FOREIGN KEY (ca, cb) REFERENCES parent(a, b))",
        &catalog,
    );
    catalog.register_from_create_plan(&child_plan).unwrap();

    let mut parent = InMemoryTable::with_columns(
        "parent",
        vec![
            ("a", ColumnType::Int64),
            ("b", ColumnType::Int64),
            ("name", ColumnType::Text),
        ],
    );
    parent.insert(vec![
        Value::Int64(1),
        Value::Int64(2),
        Value::Text("a".into()),
    ]);

    let mut child = InMemoryTable::with_columns(
        "child",
        vec![
            ("cid", ColumnType::Int64),
            ("ca", ColumnType::Int64),
            ("cb", ColumnType::Int64),
        ],
    );

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&parent);

    // 合法插入 (1, 2) → 成功
    let plan = plan_sql("INSERT INTO child VALUES (10, 1, 2)", &catalog);
    let result = exec.execute_insert(&plan, &mut child).unwrap();
    assert_eq!(result.affected_rows, 1);

    // 非法插入 (1, 999) → 拒绝
    let plan = plan_sql("INSERT INTO child VALUES (11, 1, 999)", &catalog);
    let err = exec.execute_insert(&plan, &mut child).unwrap_err();
    assert!(matches!(err, ExecutionError::ForeignKeyViolation(_)));
}

#[test]
fn test_fk_composite_delete_cascade() {
    // 复合 FK + ON DELETE CASCADE
    let mut catalog = InMemoryCatalog::new();

    let parent_plan = plan_sql(
        "CREATE TABLE parent (a INT, b INT, PRIMARY KEY (a, b))",
        &catalog,
    );
    catalog.register_from_create_plan(&parent_plan).unwrap();

    let child_plan = plan_sql(
        "CREATE TABLE child (cid INT PRIMARY KEY, ca INT, cb INT, FOREIGN KEY (ca, cb) REFERENCES parent(a, b) ON DELETE CASCADE)",
        &catalog,
    );
    catalog.register_from_create_plan(&child_plan).unwrap();

    let mut parent = InMemoryTable::with_columns(
        "parent",
        vec![("a", ColumnType::Int64), ("b", ColumnType::Int64)],
    );
    parent.insert(vec![Value::Int64(1), Value::Int64(2)]);

    let mut child = InMemoryTable::with_columns(
        "child",
        vec![
            ("cid", ColumnType::Int64),
            ("ca", ColumnType::Int64),
            ("cb", ColumnType::Int64),
        ],
    );
    child.insert(vec![Value::Int64(10), Value::Int64(1), Value::Int64(2)]);

    let (result, cascades) = {
        let mut exec = Executor::new().with_catalog(&catalog);
        exec.register_table(&child);
        let plan = plan_sql("DELETE FROM parent WHERE a = 1 AND b = 2", &catalog);
        exec.execute_delete_with_cascades(&plan, &mut parent)
            .unwrap()
    };
    assert_eq!(result.affected_rows, 1);

    // 应有 1 个 DeleteChildRow
    assert_eq!(cascades.len(), 1);
    assert!(matches!(cascades[0], CascadeOp::DeleteChildRow { .. }));

    // 应用级联
    let mut tables: [(&str, &mut dyn MutableTable); 1] = [("child", &mut child)];
    let applied = Executor::apply_cascade_ops(cascades, &mut tables).unwrap();
    assert_eq!(applied, 1);

    // 子表应清空
    let active: Vec<_> = child.scan_iter().collect();
    assert_eq!(active.len(), 0);
}

// =====================================================================
//  ForeignKeyValidator 单元测试
// =====================================================================

#[test]
fn test_validator_resolve_column_indices() {
    let schema = TableSchema {
        name: TableName::new("t"),
        columns: vec![
            ColumnDefinition::new("id", ColumnType::Int64),
            ColumnDefinition::new("name", ColumnType::Text),
        ],
    };

    // 通过公共方法间接测试列索引解析
    // 构造一个 FK，引用不存在的列 → 应返回 ColumnNotFound
    let fk = crate::plan::ForeignKeyConstraint {
        name: None,
        columns: vec!["nonexistent".to_string()],
        reference: ForeignKeyReference {
            table: TableName::new("parent"),
            columns: Some(vec!["id".to_string()]),
            on_delete: None,
            on_update: None,
        },
    };

    let row = vec![Value::Int64(1)];
    let lookup = |_: &str| None;
    let err = ForeignKeyValidator::validate_insert(&schema, &row, &[fk], &lookup).unwrap_err();
    assert!(matches!(err, ExecutionError::ColumnNotFound(_)));
}
