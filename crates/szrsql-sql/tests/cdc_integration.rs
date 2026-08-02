//! P7-1 集成测试 — CDC 接入生产运行时。
//!
//! 验证 Executor 的 DML 操作（INSERT/UPDATE/DELETE）能正确将行级变更事件
//! 分发到 CdcEngine，再由 CdcEngine 通知所有已注册的 CdcObserver。
//!
//! 事件流：
//!   Executor.mvcc_insert/update/delete
//!     → dispatch_cdc_insert/update/delete
//!     → CdcEngine.dispatch_event
//!     → CdcObserverManager.notify
//!     → CollectingObserver.on_event
//!
//! # 验收标准
//!
//! - INSERT 后 observer 收到 Insert 事件，new_row 非空
//! - UPDATE 后 observer 收到 Update 事件，old_row 和 new_row 非空
//! - DELETE 后 observer 收到 Delete 事件，old_row 非空
//! - 事件 lsn 单调递增
//! - 事件 tx_id 与 executor 的 mvcc_txn_id 一同（autocommit 模式为 1）

use std::sync::{Arc, Mutex};

use szrsql_cdc::{CdcEngine, CdcEventOp, CdcObserver, CdcObserverManager, ChangeEvent};
use szrsql_sql::executor::{Executor, InMemoryTable};
use szrsql_sql::parser::parse_sql;
use szrsql_sql::plan::{InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::ColumnType;

// =====================================================================
//  辅助：收集型 CdcObserver
// =====================================================================

/// 收集所有收到的 ChangeEvent，供测试断言
struct CollectingObserver {
    events: Mutex<Vec<ChangeEvent>>,
}

impl CollectingObserver {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    fn events(&self) -> Vec<ChangeEvent> {
        self.events.lock().unwrap().clone()
    }

    fn count(&self) -> usize {
        self.events.lock().unwrap().len()
    }
}

impl CdcObserver for CollectingObserver {
    fn on_event(&self, event: ChangeEvent) {
        self.events.lock().unwrap().push(event);
    }
}

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

// =====================================================================
//  P7-1 测试
// =====================================================================

#[test]
fn test_p7_1_cdc_insert_event_dispatch() {
    let catalog = make_catalog();
    let mut table = make_table();

    // 创建 CDC 引擎 + 收集型 observer
    let observer = Arc::new(CollectingObserver::new());
    let observer_manager = Arc::new(CdcObserverManager::new());
    assert!(observer_manager.register(observer.clone()));
    let cdc_engine = Arc::new(CdcEngine::new(observer_manager));

    // 创建 Executor 并绑定 CDC 引擎
    let executor = Executor::new()
        .with_catalog(&catalog)
        .with_cdc_engine(cdc_engine);

    // 执行 INSERT
    let plan = plan_sql(
        "INSERT INTO test_table (id, name) VALUES (1, 'alice')",
        &catalog,
    );
    let result = executor
        .execute_insert(&plan, &mut table)
        .expect("insert failed");
    assert_eq!(result.affected_rows, 1);

    // 验证 observer 收到 Insert 事件
    let events = observer.events();
    assert_eq!(
        events.len(),
        1,
        "expected 1 CDC event, got {}",
        events.len()
    );
    let event = &events[0];
    assert_eq!(event.op, CdcEventOp::Insert);
    assert!(
        event.old_row.is_none(),
        "Insert event should have no old_row"
    );
    assert!(event.new_row.is_some(), "Insert event should have new_row");
    assert!(
        event.table_id.is_some(),
        "Insert event should have table_id"
    );
}

#[test]
fn test_p7_1_cdc_multiple_inserts_lsn_monotonic() {
    let catalog = make_catalog();
    let mut table = make_table();

    let observer = Arc::new(CollectingObserver::new());
    let observer_manager = Arc::new(CdcObserverManager::new());
    assert!(observer_manager.register(observer.clone()));
    let cdc_engine = Arc::new(CdcEngine::new(observer_manager));

    let executor = Executor::new()
        .with_catalog(&catalog)
        .with_cdc_engine(cdc_engine);

    // 执行 3 次 INSERT
    for i in 1..=3 {
        let sql = format!(
            "INSERT INTO test_table (id, name) VALUES ({}, 'user{}')",
            i, i
        );
        let plan = plan_sql(&sql, &catalog);
        executor
            .execute_insert(&plan, &mut table)
            .expect("insert failed");
    }

    // 验证 observer 收到 3 个 Insert 事件，lsn 单调递增
    let events = observer.events();
    assert_eq!(events.len(), 3);
    for event in &events {
        assert_eq!(event.op, CdcEventOp::Insert);
    }
    // lsn 单调递增
    assert!(events[0].lsn < events[1].lsn);
    assert!(events[1].lsn < events[2].lsn);
}

#[test]
fn test_p7_1_cdc_no_engine_no_event() {
    // 未绑定 CDC 引擎时，DML 不触发 CDC 事件（旧行为兼容）
    let catalog = make_catalog();
    let mut table = make_table();

    let observer = Arc::new(CollectingObserver::new());
    let observer_manager = Arc::new(CdcObserverManager::new());
    assert!(observer_manager.register(observer.clone()));
    let _cdc_engine = Arc::new(CdcEngine::new(observer_manager));

    // Executor 不绑定 CDC 引擎
    let executor = Executor::new().with_catalog(&catalog);

    let plan = plan_sql(
        "INSERT INTO test_table (id, name) VALUES (1, 'alice')",
        &catalog,
    );
    let result = executor
        .execute_insert(&plan, &mut table)
        .expect("insert failed");
    assert_eq!(result.affected_rows, 1);

    // observer 不应收到任何事件
    assert_eq!(observer.count(), 0, "no events expected without CDC engine");
}

#[test]
fn test_p7_1_cdc_delete_event_dispatch() {
    let catalog = make_catalog();
    let mut table = make_table();

    // 插入测试数据（不绑定 CDC，避免插入事件干扰）
    {
        let executor = Executor::new().with_catalog(&catalog);
        let plan = plan_sql(
            "INSERT INTO test_table (id, name) VALUES (1, 'alice')",
            &catalog,
        );
        executor
            .execute_insert(&plan, &mut table)
            .expect("insert failed");
    }

    // 绑定 CDC 引擎执行 DELETE
    let observer = Arc::new(CollectingObserver::new());
    let observer_manager = Arc::new(CdcObserverManager::new());
    assert!(observer_manager.register(observer.clone()));
    let cdc_engine = Arc::new(CdcEngine::new(observer_manager));

    let executor = Executor::new()
        .with_catalog(&catalog)
        .with_cdc_engine(cdc_engine);

    let plan = plan_sql("DELETE FROM test_table WHERE id = 1", &catalog);
    let result = executor
        .execute_delete(&plan, &mut table)
        .expect("delete failed");
    assert_eq!(result.affected_rows, 1);

    // 验证 observer 收到 Delete 事件
    let events = observer.events();
    let delete_events: Vec<_> = events
        .iter()
        .filter(|e| e.op == CdcEventOp::Delete)
        .collect();
    assert!(
        !delete_events.is_empty(),
        "expected at least 1 Delete event, got {} total events",
        events.len()
    );
    for event in &delete_events {
        assert!(event.old_row.is_some(), "Delete event should have old_row");
        assert!(
            event.new_row.is_none(),
            "Delete event should have no new_row"
        );
    }
}

#[test]
fn test_p7_1_cdc_update_event_dispatch() {
    let catalog = make_catalog();
    let mut table = make_table();

    // 插入测试数据（不绑定 CDC）
    {
        let executor = Executor::new().with_catalog(&catalog);
        let plan = plan_sql(
            "INSERT INTO test_table (id, name) VALUES (1, 'alice')",
            &catalog,
        );
        executor
            .execute_insert(&plan, &mut table)
            .expect("insert failed");
    }

    // 绑定 CDC 引擎执行 UPDATE
    let observer = Arc::new(CollectingObserver::new());
    let observer_manager = Arc::new(CdcObserverManager::new());
    assert!(observer_manager.register(observer.clone()));
    let cdc_engine = Arc::new(CdcEngine::new(observer_manager));

    let executor = Executor::new()
        .with_catalog(&catalog)
        .with_cdc_engine(cdc_engine);

    let plan = plan_sql("UPDATE test_table SET name = 'bob' WHERE id = 1", &catalog);
    let result = executor
        .execute_update(&plan, &mut table)
        .expect("update failed");
    assert_eq!(result.affected_rows, 1);

    // 验证 observer 收到 Update 事件
    let events = observer.events();
    let update_events: Vec<_> = events
        .iter()
        .filter(|e| e.op == CdcEventOp::Update)
        .collect();
    assert!(
        !update_events.is_empty(),
        "expected at least 1 Update event, got {} total events",
        events.len()
    );
    for event in &update_events {
        assert!(event.old_row.is_some(), "Update event should have old_row");
        assert!(event.new_row.is_some(), "Update event should have new_row");
    }
}

#[test]
fn test_p7_1_cdc_table_id_stable() {
    // 同一表名的 table_id 应该稳定（FNV-1a hash）
    let catalog = make_catalog();
    let mut table1 = make_table();
    let mut table2 = make_table();

    let observer = Arc::new(CollectingObserver::new());
    let observer_manager = Arc::new(CdcObserverManager::new());
    assert!(observer_manager.register(observer.clone()));
    let cdc_engine = Arc::new(CdcEngine::new(observer_manager));

    // 两次 INSERT 同一表名
    {
        let executor = Executor::new()
            .with_catalog(&catalog)
            .with_cdc_engine(cdc_engine.clone());
        let plan = plan_sql(
            "INSERT INTO test_table (id, name) VALUES (1, 'a')",
            &catalog,
        );
        executor
            .execute_insert(&plan, &mut table1)
            .expect("insert failed");
    }
    {
        let executor = Executor::new()
            .with_catalog(&catalog)
            .with_cdc_engine(cdc_engine);
        let plan = plan_sql(
            "INSERT INTO test_table (id, name) VALUES (2, 'b')",
            &catalog,
        );
        executor
            .execute_insert(&plan, &mut table2)
            .expect("insert failed");
    }

    // 两次事件的 table_id 应该相同（同一表名）
    let events = observer.events();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].table_id, events[1].table_id,
        "same table name should produce same table_id"
    );
}

#[test]
fn test_p7_1_cdc_mixed_dml_sequence() {
    // 混合 DML 序列：INSERT → UPDATE → DELETE，验证事件顺序和类型
    let catalog = make_catalog();
    let mut table = make_table();

    let observer = Arc::new(CollectingObserver::new());
    let observer_manager = Arc::new(CdcObserverManager::new());
    assert!(observer_manager.register(observer.clone()));
    let cdc_engine = Arc::new(CdcEngine::new(observer_manager));

    // 1. INSERT（绑定 CDC）
    {
        let executor = Executor::new()
            .with_catalog(&catalog)
            .with_cdc_engine(cdc_engine.clone());
        let plan = plan_sql(
            "INSERT INTO test_table (id, name) VALUES (1, 'alice')",
            &catalog,
        );
        executor
            .execute_insert(&plan, &mut table)
            .expect("insert failed");
    }

    // 2. UPDATE（绑定 CDC）
    {
        let executor = Executor::new()
            .with_catalog(&catalog)
            .with_cdc_engine(cdc_engine.clone());
        let plan = plan_sql("UPDATE test_table SET name = 'bob' WHERE id = 1", &catalog);
        executor
            .execute_update(&plan, &mut table)
            .expect("update failed");
    }

    // 3. DELETE（绑定 CDC）
    {
        let executor = Executor::new()
            .with_catalog(&catalog)
            .with_cdc_engine(cdc_engine);
        let plan = plan_sql("DELETE FROM test_table WHERE id = 1", &catalog);
        executor
            .execute_delete(&plan, &mut table)
            .expect("delete failed");
    }

    // 验证事件序列
    let events = observer.events();
    assert!(
        events.len() >= 3,
        "expected at least 3 events (insert+update+delete), got {}",
        events.len()
    );

    // lsn 全局单调递增
    for i in 1..events.len() {
        assert!(
            events[i - 1].lsn < events[i].lsn,
            "lsn should be monotonically increasing: event[{}].lsn={} >= event[{}].lsn={}",
            i - 1,
            events[i - 1].lsn,
            i,
            events[i].lsn
        );
    }

    // 至少包含 Insert、Update、Delete 各一个
    let has_insert = events.iter().any(|e| e.op == CdcEventOp::Insert);
    let has_update = events.iter().any(|e| e.op == CdcEventOp::Update);
    let has_delete = events.iter().any(|e| e.op == CdcEventOp::Delete);
    assert!(has_insert, "missing Insert event");
    assert!(has_update, "missing Update event");
    assert!(has_delete, "missing Delete event");
}
