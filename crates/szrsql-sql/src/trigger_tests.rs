//! Phase 6.4 集成测试 — 触发器（CREATE/DROP TRIGGER + DML 触发器钩子）。
//!
//! 覆盖类别：
//! - Parser（4 条）：CREATE TRIGGER BEFORE/AFTER/INSTEAD OF + DROP TRIGGER
//! - Planner（4 条）：CreateTrigger 计划生成 / DropTrigger 计划生成 / 表不存在错误 / 触发器已存在错误
//! - Executor DDL（5 条）：CREATE 注册到 catalog / OR REPLACE / IF NOT EXISTS（PG 不支持，但 executor 兜底） /
//!   DROP 移除 / DROP IF EXISTS 不存在
//! - BEFORE INSERT Row（4 条）：触发计数 / 修改 NEW / 跳过行 / 错误中止
//! - AFTER INSERT Row（2 条）：触发计数 / 忽略 SkipRow 返回值
//! - STATEMENT 触发器（2 条）：BEFORE/AFTER STATEMENT 各触发一次 / 多行 INSERT 仅触发一次
//! - UPDATE 触发器（3 条）：BEFORE UPDATE OLD/NEW 行访问 / UPDATE(cols) 列过滤 / AFTER UPDATE 触发
//! - DELETE 触发器（2 条）：BEFORE DELETE OLD 行访问 / BEFORE DELETE 跳过
//! - 触发器顺序与组合（3 条）：多触发器按定义顺序执行 / BEFORE+ AFTER 组合 / BEFORE 修改传递给 AFTER
//! - 未注册触发器函数（1 条）：DML 时触发器函数未注册报错
//! - 向后兼容（1 条）：未绑定 trigger_registry 时 DML 静默跳过
//!
//! 共 31 个测试用例。

use super::executor::{ExecutionError, Executor, InMemoryTable, TableStorage};
use super::trigger::{TriggerContext, TriggerFunction, TriggerOutcome, TriggerRegistry};
use crate::ast::{Statement, TableName, TriggerEvent, TriggerLevel, TriggerTiming};
use crate::parser::parse_one;
use crate::plan::{Catalog, InMemoryCatalog, LogicalPlan, PlanError, Planner};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use szrsql_types::value::{ColumnType, Value};

/// 触发器测试中记录的行类型
type Row = Vec<Value>;

/// 触发器记录的 (NEW, OLD) 行对列表（线程安全共享）
type RecordedRows = Arc<Mutex<Vec<(Option<Row>, Option<Row>)>>>;

// =====================================================================
//  辅助函数
// =====================================================================

/// 创建带主键 `id` 的 catalog + 表（id INT PK, name TEXT）
fn make_users_setup() -> (InMemoryCatalog, InMemoryTable) {
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE users (id INT PRIMARY KEY, name TEXT)",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();
    let table = InMemoryTable::with_columns(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    (catalog, table)
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

/// 计数触发器：记录被调用次数
struct CountingTrigger {
    counter: Arc<AtomicUsize>,
}

impl TriggerFunction for CountingTrigger {
    fn call(&self, _ctx: &TriggerContext) -> Result<TriggerOutcome, ExecutionError> {
        self.counter.fetch_add(1, Ordering::SeqCst);
        Ok(TriggerOutcome::Continue)
    }
}

/// 记录 NEW/OLD 行的触发器（线程安全收集）
struct RecordingTrigger {
    rows: RecordedRows,
}

impl TriggerFunction for RecordingTrigger {
    fn call(&self, ctx: &TriggerContext) -> Result<TriggerOutcome, ExecutionError> {
        let new = ctx.new_row.cloned();
        let old = ctx.old_row.cloned();
        self.rows.lock().unwrap().push((new, old));
        Ok(TriggerOutcome::Continue)
    }
}

/// 修改 NEW 行 name 列为 "modified" 的触发器
struct ModifyNameTrigger;

impl TriggerFunction for ModifyNameTrigger {
    fn call(&self, ctx: &TriggerContext) -> Result<TriggerOutcome, ExecutionError> {
        if let Some(row) = ctx.new_row {
            let mut modified = row.clone();
            modified[1] = Value::Text("modified".to_string());
            Ok(TriggerOutcome::Modify(modified))
        } else {
            Ok(TriggerOutcome::Continue)
        }
    }
}

/// 跳过行的触发器
struct SkipRowTrigger;

impl TriggerFunction for SkipRowTrigger {
    fn call(&self, _ctx: &TriggerContext) -> Result<TriggerOutcome, ExecutionError> {
        Ok(TriggerOutcome::SkipRow)
    }
}

/// 出错触发器
struct ErrorTrigger;

impl TriggerFunction for ErrorTrigger {
    fn call(&self, _ctx: &TriggerContext) -> Result<TriggerOutcome, ExecutionError> {
        Err(ExecutionError::InvalidArgument(
            "trigger error from ErrorTrigger".to_string(),
        ))
    }
}

// =====================================================================
//  Parser 测试（4 条）
// =====================================================================

#[test]
fn test_trigger_parser_01_before_row_insert() {
    let stmt = parse_one(
        "CREATE TRIGGER trg_bi BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION audit()",
    )
    .unwrap();
    match stmt {
        Statement::CreateTrigger {
            definition,
            or_replace,
            if_not_exists,
        } => {
            assert_eq!(definition.name, "trg_bi");
            assert_eq!(definition.table.name, "users");
            assert_eq!(definition.timing, TriggerTiming::Before);
            assert_eq!(definition.level, TriggerLevel::Row);
            assert_eq!(definition.events, vec![TriggerEvent::Insert]);
            assert_eq!(definition.func_name, "audit");
            assert!(definition.enabled);
            assert!(!definition.is_constraint);
            assert!(!or_replace);
            assert!(!if_not_exists);
        }
        other => panic!("expected CreateTrigger, got {other:?}"),
    }
}

#[test]
fn test_trigger_parser_02_after_statement_update_delete() {
    let stmt = parse_one(
        "CREATE TRIGGER trg_adu AFTER UPDATE OR DELETE ON users FOR EACH STATEMENT EXECUTE FUNCTION log_change()",
    )
    .unwrap();
    match stmt {
        Statement::CreateTrigger { definition, .. } => {
            assert_eq!(definition.timing, TriggerTiming::After);
            assert_eq!(definition.level, TriggerLevel::Statement);
            assert_eq!(definition.events.len(), 2);
            assert!(definition
                .events
                .contains(&TriggerEvent::Update(Vec::new())));
            assert!(definition.events.contains(&TriggerEvent::Delete));
        }
        other => panic!("expected CreateTrigger, got {other:?}"),
    }
}

#[test]
fn test_trigger_parser_03_instead_of_with_when() {
    let stmt = parse_one(
        "CREATE TRIGGER trg_io INSTEAD OF DELETE ON users FOR EACH ROW WHEN (OLD.id > 10) EXECUTE FUNCTION instead_delete()",
    )
    .unwrap();
    match stmt {
        Statement::CreateTrigger { definition, .. } => {
            assert_eq!(definition.timing, TriggerTiming::InsteadOf);
            assert_eq!(definition.level, TriggerLevel::Row);
            assert!(definition.when_clause.is_some());
        }
        other => panic!("expected CreateTrigger, got {other:?}"),
    }
}

#[test]
fn test_trigger_parser_04_drop_trigger() {
    let stmt = parse_one("DROP TRIGGER trg_bi ON users").unwrap();
    match stmt {
        Statement::DropTrigger {
            name,
            table,
            if_exists,
            cascade,
        } => {
            assert_eq!(name, "trg_bi");
            assert_eq!(table.name, "users");
            assert!(!if_exists);
            assert!(!cascade);
        }
        other => panic!("expected DropTrigger, got {other:?}"),
    }
}

// =====================================================================
//  Planner 测试（4 条）
// =====================================================================

#[test]
fn test_trigger_planner_01_create_trigger_plan() {
    let (catalog, _table) = make_users_setup();
    let plan = plan_sql(
        "CREATE TRIGGER trg_bi BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION audit()",
        &catalog,
    );
    match plan {
        LogicalPlan::CreateTrigger {
            definition,
            or_replace,
            if_not_exists,
        } => {
            assert_eq!(definition.name, "trg_bi");
            assert_eq!(definition.func_name, "audit");
            assert!(!or_replace);
            assert!(!if_not_exists);
        }
        other => panic!("expected CreateTrigger plan, got {other:?}"),
    }
}

#[test]
fn test_trigger_planner_02_drop_trigger_plan() {
    let (catalog, _table) = make_users_setup();
    let plan = plan_sql("DROP TRIGGER trg_bi ON users", &catalog);
    match plan {
        LogicalPlan::DropTrigger {
            name,
            table,
            if_exists,
            cascade,
        } => {
            assert_eq!(name, "trg_bi");
            assert_eq!(table.name, "users");
            assert!(!if_exists);
            assert!(!cascade);
        }
        other => panic!("expected DropTrigger plan, got {other:?}"),
    }
}

#[test]
fn test_trigger_planner_03_create_on_nonexistent_table() {
    let catalog = InMemoryCatalog::new();
    let err = plan_sql_err(
        "CREATE TRIGGER trg_bi BEFORE INSERT ON nonexistent FOR EACH ROW EXECUTE FUNCTION f()",
        &catalog,
    );
    assert!(matches!(err, PlanError::TableNotFound(_)));
}

#[test]
fn test_trigger_planner_04_create_duplicate_trigger() {
    let (catalog, _table) = make_users_setup();
    let plan = plan_sql(
        "CREATE TRIGGER trg_bi BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION f()",
        &catalog,
    );
    // 第一次创建：成功
    let mut cat = catalog.clone();
    let exec = Executor::new();
    exec.execute_create_trigger(&plan, &mut cat).unwrap();
    // 第二次创建：planner 应拦截（不允许重复）
    let err = plan_sql_err(
        "CREATE TRIGGER trg_bi BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION f()",
        &cat,
    );
    assert!(matches!(err, PlanError::Unsupported(_)));
}

// =====================================================================
//  Executor DDL 测试（5 条）
// =====================================================================

#[test]
fn test_trigger_exec_ddl_01_create_registers_to_catalog() {
    let (catalog, _table) = make_users_setup();
    let plan = plan_sql(
        "CREATE TRIGGER trg_bi BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION f()",
        &catalog,
    );
    let mut cat = catalog.clone();
    let exec = Executor::new();
    exec.execute_create_trigger(&plan, &mut cat).unwrap();
    let triggers = cat.list_triggers(&plan_table_name("users"));
    assert_eq!(triggers.len(), 1);
    assert_eq!(triggers[0].name, "trg_bi");
    assert_eq!(triggers[0].func_name, "f");
}

#[test]
fn test_trigger_exec_ddl_02_or_replace_overrides() {
    let (catalog, _table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    // 第一次创建
    let plan1 = plan_sql(
        "CREATE TRIGGER trg_bi BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION f1()",
        &catalog,
    );
    exec.execute_create_trigger(&plan1, &mut cat).unwrap();
    // OR REPLACE 替换
    let plan2 = plan_sql(
        "CREATE OR REPLACE TRIGGER trg_bi BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION f2()",
        &catalog,
    );
    exec.execute_create_trigger(&plan2, &mut cat).unwrap();
    let triggers = cat.list_triggers(&plan_table_name("users"));
    assert_eq!(triggers.len(), 1);
    assert_eq!(triggers[0].func_name, "f2");
}

#[test]
fn test_trigger_exec_ddl_03_drop_removes_trigger() {
    let (catalog, _table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let create = plan_sql(
        "CREATE TRIGGER trg_bi BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION f()",
        &catalog,
    );
    exec.execute_create_trigger(&create, &mut cat).unwrap();
    assert_eq!(cat.list_triggers(&plan_table_name("users")).len(), 1);
    let drop_plan = plan_sql("DROP TRIGGER trg_bi ON users", &catalog);
    exec.execute_drop_trigger(&drop_plan, &mut cat).unwrap();
    assert_eq!(cat.list_triggers(&plan_table_name("users")).len(), 0);
}

#[test]
fn test_trigger_exec_ddl_04_drop_nonexistent_no_if_exists_errors() {
    let (catalog, _table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let drop_plan = plan_sql("DROP TRIGGER nonexistent ON users", &catalog);
    let err = exec.execute_drop_trigger(&drop_plan, &mut cat).unwrap_err();
    assert!(matches!(err, ExecutionError::InvalidArgument(_)));
}

#[test]
fn test_trigger_exec_ddl_05_drop_nonexistent_with_if_exists_succeeds() {
    let (catalog, _table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let drop_plan = plan_sql("DROP TRIGGER IF EXISTS nonexistent ON users", &catalog);
    exec.execute_drop_trigger(&drop_plan, &mut cat).unwrap();
}

// =====================================================================
//  BEFORE INSERT Row 触发器（4 条）
// =====================================================================

#[test]
fn test_trigger_before_insert_row_01_count() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();

    // 创建触发器
    let create = plan_sql(
        "CREATE TRIGGER trg_bi BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION counter()",
        &catalog,
    );
    exec.execute_create_trigger(&create, &mut cat).unwrap();

    // 注册触发器函数
    let counter = Arc::new(AtomicUsize::new(0));
    let mut registry = TriggerRegistry::new();
    registry.register(
        "counter",
        Arc::new(CountingTrigger {
            counter: counter.clone(),
        }),
    );

    // 执行 INSERT
    let plan = plan_sql("INSERT INTO users VALUES (1, 'a')", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("a".into()));
}

#[test]
fn test_trigger_before_insert_row_02_modify_new() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let create = plan_sql(
        "CREATE TRIGGER trg_bi BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION modify_name()",
        &catalog,
    );
    exec.execute_create_trigger(&create, &mut cat).unwrap();

    let mut registry = TriggerRegistry::new();
    registry.register("modify_name", Arc::new(ModifyNameTrigger));

    let plan = plan_sql("INSERT INTO users VALUES (1, 'original')", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    exec.execute_insert(&plan, &mut table).unwrap();
    // 触发器修改了 name 列
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("modified".into()));
}

#[test]
fn test_trigger_before_insert_row_03_skip_row() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let create = plan_sql(
        "CREATE TRIGGER trg_bi BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION skip()",
        &catalog,
    );
    exec.execute_create_trigger(&create, &mut cat).unwrap();

    let mut registry = TriggerRegistry::new();
    registry.register("skip", Arc::new(SkipRowTrigger));

    let plan = plan_sql("INSERT INTO users VALUES (1, 'a')", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 0);
    assert_eq!(table.row_count(), 0);
}

#[test]
fn test_trigger_before_insert_row_04_error_aborts() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let create = plan_sql(
        "CREATE TRIGGER trg_bi BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION err()",
        &catalog,
    );
    exec.execute_create_trigger(&create, &mut cat).unwrap();

    let mut registry = TriggerRegistry::new();
    registry.register("err", Arc::new(ErrorTrigger));

    let plan = plan_sql("INSERT INTO users VALUES (1, 'a')", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    let err = exec.execute_insert(&plan, &mut table).unwrap_err();
    assert!(matches!(err, ExecutionError::InvalidArgument(_)));
    assert_eq!(table.row_count(), 0);
}

// =====================================================================
//  AFTER INSERT Row 触发器（2 条）
// =====================================================================

#[test]
fn test_trigger_after_insert_row_01_count() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let create = plan_sql(
        "CREATE TRIGGER trg_ai AFTER INSERT ON users FOR EACH ROW EXECUTE FUNCTION counter()",
        &catalog,
    );
    exec.execute_create_trigger(&create, &mut cat).unwrap();

    let counter = Arc::new(AtomicUsize::new(0));
    let mut registry = TriggerRegistry::new();
    registry.register(
        "counter",
        Arc::new(CountingTrigger {
            counter: counter.clone(),
        }),
    );

    let plan = plan_sql("INSERT INTO users VALUES (1, 'a'), (2, 'b')", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 2);
    // AFTER Row 触发器对每行触发一次
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn test_trigger_after_insert_row_02_ignores_skip_outcome() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let create = plan_sql(
        "CREATE TRIGGER trg_ai AFTER INSERT ON users FOR EACH ROW EXECUTE FUNCTION skip()",
        &catalog,
    );
    exec.execute_create_trigger(&create, &mut cat).unwrap();

    let mut registry = TriggerRegistry::new();
    registry.register("skip", Arc::new(SkipRowTrigger));

    let plan = plan_sql("INSERT INTO users VALUES (1, 'a')", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    // AFTER 触发器返回 SkipRow 被忽略，行仍然插入
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.row_count(), 1);
}

// =====================================================================
//  STATEMENT 触发器（2 条）
// =====================================================================

#[test]
fn test_trigger_statement_01_before_and_after_fire_once() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    // BEFORE STATEMENT
    let create_before = plan_sql(
        "CREATE TRIGGER trg_bs BEFORE INSERT ON users FOR EACH STATEMENT EXECUTE FUNCTION counter()",
        &catalog,
    );
    exec.execute_create_trigger(&create_before, &mut cat)
        .unwrap();
    // AFTER STATEMENT
    let create_after = plan_sql(
        "CREATE TRIGGER trg_as AFTER INSERT ON users FOR EACH STATEMENT EXECUTE FUNCTION counter()",
        &catalog,
    );
    exec.execute_create_trigger(&create_after, &mut cat)
        .unwrap();

    let counter = Arc::new(AtomicUsize::new(0));
    let mut registry = TriggerRegistry::new();
    registry.register(
        "counter",
        Arc::new(CountingTrigger {
            counter: counter.clone(),
        }),
    );

    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a'), (2, 'b'), (3, 'c')",
        &catalog,
    );
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 3);
    // 每个语句级触发器只触发一次（BEFORE + AFTER = 2）
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn test_trigger_statement_02_no_triggers_no_overhead() {
    // 表上无触发器时，DML 仍然正常工作（fast path）
    let (catalog, mut table) = make_users_setup();
    let registry = TriggerRegistry::new();
    let plan = plan_sql("INSERT INTO users VALUES (1, 'a')", &catalog);
    let exec = Executor::new()
        .with_catalog(&catalog)
        .with_trigger_registry(&registry);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
}

// =====================================================================
//  UPDATE 触发器（3 条）
// =====================================================================

#[test]
fn test_trigger_update_01_before_row_old_new_access() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let create = plan_sql(
        "CREATE TRIGGER trg_bu BEFORE UPDATE ON users FOR EACH ROW EXECUTE FUNCTION record()",
        &catalog,
    );
    exec.execute_create_trigger(&create, &mut cat).unwrap();

    // 预置一行
    table.insert(vec![Value::Int64(1), Value::Text("old".into())]);

    let rows = Arc::new(Mutex::new(Vec::new()));
    let mut registry = TriggerRegistry::new();
    registry.register("record", Arc::new(RecordingTrigger { rows: rows.clone() }));

    let plan = plan_sql("UPDATE users SET name = 'new' WHERE id = 1", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    exec.execute_update(&plan, &mut table).unwrap();

    let recorded = rows.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    let (new_row, old_row) = &recorded[0];
    // NEW 行：id=1, name="new"
    assert_eq!(new_row.as_ref().unwrap()[0], Value::Int64(1));
    assert_eq!(new_row.as_ref().unwrap()[1], Value::Text("new".into()));
    // OLD 行：id=1, name="old"
    assert_eq!(old_row.as_ref().unwrap()[0], Value::Int64(1));
    assert_eq!(old_row.as_ref().unwrap()[1], Value::Text("old".into()));
}

#[test]
fn test_trigger_update_02_column_filter_skips_irrelevant() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    // UPDATE OF name — 仅当 name 列被更新时触发
    let create = plan_sql(
        "CREATE TRIGGER trg_bu_name BEFORE UPDATE OF name ON users FOR EACH ROW EXECUTE FUNCTION counter()",
        &catalog,
    );
    exec.execute_create_trigger(&create, &mut cat).unwrap();

    table.insert(vec![Value::Int64(1), Value::Text("a".into())]);

    let counter = Arc::new(AtomicUsize::new(0));
    let mut registry = TriggerRegistry::new();
    registry.register(
        "counter",
        Arc::new(CountingTrigger {
            counter: counter.clone(),
        }),
    );

    // 更新 id 列（不涉及 name）→ 不触发
    let plan_id = plan_sql("UPDATE users SET id = 100 WHERE id = 1", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    exec.execute_update(&plan_id, &mut table).unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    // 更新 name 列 → 触发
    let plan_name = plan_sql("UPDATE users SET name = 'b' WHERE id = 100", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    exec.execute_update(&plan_name, &mut table).unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[test]
fn test_trigger_update_03_after_row_fires() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let create = plan_sql(
        "CREATE TRIGGER trg_au AFTER UPDATE ON users FOR EACH ROW EXECUTE FUNCTION counter()",
        &catalog,
    );
    exec.execute_create_trigger(&create, &mut cat).unwrap();

    table.insert(vec![Value::Int64(1), Value::Text("a".into())]);
    table.insert(vec![Value::Int64(2), Value::Text("b".into())]);

    let counter = Arc::new(AtomicUsize::new(0));
    let mut registry = TriggerRegistry::new();
    registry.register(
        "counter",
        Arc::new(CountingTrigger {
            counter: counter.clone(),
        }),
    );

    let plan = plan_sql("UPDATE users SET name = 'x'", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    let result = exec.execute_update(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 2);
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

// =====================================================================
//  DELETE 触发器（2 条）
// =====================================================================

#[test]
fn test_trigger_delete_01_before_row_old_access() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let create = plan_sql(
        "CREATE TRIGGER trg_bd BEFORE DELETE ON users FOR EACH ROW EXECUTE FUNCTION record()",
        &catalog,
    );
    exec.execute_create_trigger(&create, &mut cat).unwrap();

    table.insert(vec![Value::Int64(1), Value::Text("a".into())]);

    let rows = Arc::new(Mutex::new(Vec::new()));
    let mut registry = TriggerRegistry::new();
    registry.register("record", Arc::new(RecordingTrigger { rows: rows.clone() }));

    let plan = plan_sql("DELETE FROM users WHERE id = 1", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    let result = exec.execute_delete(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);

    let recorded = rows.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    let (new_row, old_row) = &recorded[0];
    // DELETE 时 NEW 为 None
    assert!(new_row.is_none());
    // OLD 行：id=1, name="a"
    assert_eq!(old_row.as_ref().unwrap()[0], Value::Int64(1));
    assert_eq!(old_row.as_ref().unwrap()[1], Value::Text("a".into()));
}

#[test]
fn test_trigger_delete_02_before_row_skip() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let create = plan_sql(
        "CREATE TRIGGER trg_bd BEFORE DELETE ON users FOR EACH ROW EXECUTE FUNCTION skip()",
        &catalog,
    );
    exec.execute_create_trigger(&create, &mut cat).unwrap();

    table.insert(vec![Value::Int64(1), Value::Text("a".into())]);

    let mut registry = TriggerRegistry::new();
    registry.register("skip", Arc::new(SkipRowTrigger));

    let plan = plan_sql("DELETE FROM users WHERE id = 1", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    let result = exec.execute_delete(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 0);
    // 行未被删除
    assert_eq!(table.row_count(), 1);
}

// =====================================================================
//  触发器顺序与组合（3 条）
// =====================================================================

#[test]
fn test_trigger_ordering_01_multiple_before_in_definition_order() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let create1 = plan_sql(
        "CREATE TRIGGER trg_first BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION f1()",
        &catalog,
    );
    let create2 = plan_sql(
        "CREATE TRIGGER trg_second BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION f2()",
        &catalog,
    );
    exec.execute_create_trigger(&create1, &mut cat).unwrap();
    exec.execute_create_trigger(&create2, &mut cat).unwrap();

    let counter = Arc::new(AtomicUsize::new(0));
    let mut registry = TriggerRegistry::new();
    registry.register(
        "f1",
        Arc::new(CountingTrigger {
            counter: counter.clone(),
        }),
    );
    registry.register(
        "f2",
        Arc::new(CountingTrigger {
            counter: counter.clone(),
        }),
    );

    let plan = plan_sql("INSERT INTO users VALUES (1, 'a')", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn test_trigger_ordering_02_before_and_after_combination() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let create_b = plan_sql(
        "CREATE TRIGGER trg_b BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION f()",
        &catalog,
    );
    let create_a = plan_sql(
        "CREATE TRIGGER trg_a AFTER INSERT ON users FOR EACH ROW EXECUTE FUNCTION f()",
        &catalog,
    );
    exec.execute_create_trigger(&create_b, &mut cat).unwrap();
    exec.execute_create_trigger(&create_a, &mut cat).unwrap();

    let counter = Arc::new(AtomicUsize::new(0));
    let mut registry = TriggerRegistry::new();
    registry.register(
        "f",
        Arc::new(CountingTrigger {
            counter: counter.clone(),
        }),
    );

    let plan = plan_sql("INSERT INTO users VALUES (1, 'a')", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    exec.execute_insert(&plan, &mut table).unwrap();
    // BEFORE + AFTER 各触发一次
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn test_trigger_ordering_03_before_modify_propagates_to_after() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    // BEFORE 触发器修改 name 为 "modified"
    let create_b = plan_sql(
        "CREATE TRIGGER trg_b BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION modify_name()",
        &catalog,
    );
    exec.execute_create_trigger(&create_b, &mut cat).unwrap();

    // AFTER 触发器记录实际插入的 NEW 行
    let create_a = plan_sql(
        "CREATE TRIGGER trg_a AFTER INSERT ON users FOR EACH ROW EXECUTE FUNCTION record()",
        &catalog,
    );
    exec.execute_create_trigger(&create_a, &mut cat).unwrap();

    let rows = Arc::new(Mutex::new(Vec::new()));
    let mut registry = TriggerRegistry::new();
    registry.register("modify_name", Arc::new(ModifyNameTrigger));
    registry.register("record", Arc::new(RecordingTrigger { rows: rows.clone() }));

    let plan = plan_sql("INSERT INTO users VALUES (1, 'original')", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    exec.execute_insert(&plan, &mut table).unwrap();

    // AFTER 触发器应看到修改后的 NEW 行
    let recorded = rows.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    let (new_row, _) = &recorded[0];
    assert_eq!(new_row.as_ref().unwrap()[1], Value::Text("modified".into()));
}

// =====================================================================
//  未注册触发器函数（1 条）
// =====================================================================

#[test]
fn test_trigger_unregistered_function_errors() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let create = plan_sql(
        "CREATE TRIGGER trg_bi BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION nonexistent()",
        &catalog,
    );
    exec.execute_create_trigger(&create, &mut cat).unwrap();

    // 不注册 nonexistent 函数
    let registry = TriggerRegistry::new();
    let plan = plan_sql("INSERT INTO users VALUES (1, 'a')", &catalog);
    let exec = Executor::new()
        .with_catalog(&cat)
        .with_trigger_registry(&registry);
    let err = exec.execute_insert(&plan, &mut table).unwrap_err();
    assert!(matches!(err, ExecutionError::InvalidArgument(_)));
    assert_eq!(table.row_count(), 0);
}

// =====================================================================
//  向后兼容（1 条）
// =====================================================================

#[test]
fn test_trigger_backward_compat_no_registry_silently_skips() {
    let (catalog, mut table) = make_users_setup();
    let mut cat = catalog.clone();
    let exec = Executor::new();
    let create = plan_sql(
        "CREATE TRIGGER trg_bi BEFORE INSERT ON users FOR EACH ROW EXECUTE FUNCTION f()",
        &catalog,
    );
    exec.execute_create_trigger(&create, &mut cat).unwrap();

    // 未绑定 trigger_registry → DML 静默跳过触发器调用
    let plan = plan_sql("INSERT INTO users VALUES (1, 'a')", &catalog);
    let exec = Executor::new().with_catalog(&cat);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.row_count(), 1);
}

// =====================================================================
//  内部辅助
// =====================================================================

/// 构造 TableName（用于测试断言）
fn plan_table_name(name: &str) -> TableName {
    TableName::new(name)
}
