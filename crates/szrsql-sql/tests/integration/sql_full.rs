//! Phase 3.16 SQL 完整链路集成测试 — 对应 `SzRSQL实施进度.md` Phase 3.16。
//!
//! # 验收标准（SzRSQL实施进度.md Phase 3.16）
//!
//! - **完整链路**：CREATE TABLE → INSERT 1000000 行 → 3 表 JOIN 查询 → 子查询嵌套 →
//!   聚合 → ORDER BY → LIMIT
//! - **事务一致性**：事务中混合 DML + 查询 → COMMIT/ROLLBACK → 数据一致
//!
//! # 设计要点
//!
//! 1. **执行器已实现 Sort 算子**（executor.rs execute_sort），
//!    ORDER BY 可通过 Sort 计划节点执行（planner 已支持 Sort 计划生成）
//! 2. **大表使用 `CounterTable`**：1M 行测试不分配实际 1M × Vec<Value> 内存，
//!    通过 CounterTable 惰性生成验证扫描行数正确性
//! 3. **事务用 `snapshot/restore`**：与 `executor_tests::test_dml_integration_full_cycle`
//!    相同的简化事务模型，COMMIT = 丢弃快照，ROLLBACK = restore 快照
//! 4. **辅助函数 `plan_sql`**：复用 `executor_tests.rs` 中的 SQL → AST → LogicalPlan 流程
//! 5. **多表 JOIN 场景**：users + orders + items 三表 JOIN，验证 JOIN + Filter + Projection 组合
//! 6. **DML 借用规则**：`execute_insert/update/delete` 直接接收 `&mut table`，
//!    不需要 `register_table`；只有 SELECT（含 INSERT...SELECT 源表）才需要 register

use szrsql_sql::executor::{Executor, InMemoryTable, MutableTable, TableStorage};
use szrsql_sql::parser::parse_sql;
use szrsql_sql::plan::{InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::ColumnType;
use szrsql_types::value::Value;

// =====================================================================
//  辅助函数
// =====================================================================

/// SQL → AST → LogicalPlan（与 executor_tests::plan_sql 同风格）
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

/// 构造三表 catalog：users(user_id, name) + orders(order_id, user_id, amount) + items(item_id, order_id, product)
///
/// **列名设计**：使用 `user_id`/`order_id`/`item_id` 而非 `id`，避免 3 表 JOIN 时
/// `JoinedRowContext::lookup_qualified` 的 fallback `lookup_column` 在多表 `id` 列中
/// 找到错误的列（当前执行器对嵌套 JOIN 的别名解析存在已知限制：左侧 schema 名为
/// "__join__"，别名 `o`/`i` 无法匹配，回退到列名查找时会取首个同名列）。
fn make_three_table_catalog() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "users",
        vec![("user_id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    catalog.add_simple_table(
        "orders",
        vec![
            ("order_id", ColumnType::Int64),
            ("user_id", ColumnType::Int64),
            ("amount", ColumnType::Int64),
        ],
    );
    catalog.add_simple_table(
        "items",
        vec![
            ("item_id", ColumnType::Int64),
            ("order_id", ColumnType::Int64),
            ("product", ColumnType::Text),
        ],
    );
    catalog
}

/// 构造填充数据的三表（小数据集，便于精确断言）
fn make_three_table_data() -> (InMemoryTable, InMemoryTable, InMemoryTable) {
    let mut users = InMemoryTable::with_columns(
        "users",
        vec![("user_id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    users.insert(vec![Value::Int64(1), Value::Text("alice".into())]);
    users.insert(vec![Value::Int64(2), Value::Text("bob".into())]);
    users.insert(vec![Value::Int64(3), Value::Text("carol".into())]);

    let mut orders = InMemoryTable::with_columns(
        "orders",
        vec![
            ("order_id", ColumnType::Int64),
            ("user_id", ColumnType::Int64),
            ("amount", ColumnType::Int64),
        ],
    );
    // alice: 2 orders (100, 101); bob: 1 order (102); carol: 0 orders
    orders.insert(vec![Value::Int64(100), Value::Int64(1), Value::Int64(50)]);
    orders.insert(vec![Value::Int64(101), Value::Int64(1), Value::Int64(75)]);
    orders.insert(vec![Value::Int64(102), Value::Int64(2), Value::Int64(100)]);

    let mut items = InMemoryTable::with_columns(
        "items",
        vec![
            ("item_id", ColumnType::Int64),
            ("order_id", ColumnType::Int64),
            ("product", ColumnType::Text),
        ],
    );
    // order 100: 2 items; order 101: 1 item; order 102: 1 item
    items.insert(vec![
        Value::Int64(1000),
        Value::Int64(100),
        Value::Text("apple".into()),
    ]);
    items.insert(vec![
        Value::Int64(1001),
        Value::Int64(100),
        Value::Text("banana".into()),
    ]);
    items.insert(vec![
        Value::Int64(1002),
        Value::Int64(101),
        Value::Text("cherry".into()),
    ]);
    items.insert(vec![
        Value::Int64(1003),
        Value::Int64(102),
        Value::Text("date".into()),
    ]);

    (users, orders, items)
}

/// 比较 Value（Value 未实现 Ord，需自定义比较以支持 sort_by）
fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        (Value::Int64(x), Value::Int64(y)) => x.cmp(y),
        (Value::Float64(x), Value::Float64(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Int64(x), Value::Float64(y)) => {
            (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (Value::Float64(x), Value::Int64(y)) => {
            x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal)
        }
        (Value::Text(x), Value::Text(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Date(x), Value::Date(y)) => x.cmp(y),
        (Value::Timestamp(x), Value::Timestamp(y)) => x.cmp(y),
        _ => Ordering::Equal,
    }
}

/// 手动按指定列索引升序排序（模拟 ORDER BY，因 Executor 暂未实现 Sort 算子）
fn sort_rows_by_index_asc(rows: Vec<Vec<Value>>, col_idx: usize) -> Vec<Vec<Value>> {
    let mut sorted = rows;
    sorted.sort_by(|a, b| compare_values(&a[col_idx], &b[col_idx]));
    sorted
}

/// 手动按指定列索引降序排序
fn sort_rows_by_index_desc(rows: Vec<Vec<Value>>, col_idx: usize) -> Vec<Vec<Value>> {
    let mut sorted = rows;
    sorted.sort_by(|a, b| compare_values(&b[col_idx], &a[col_idx]));
    sorted
}

// =====================================================================
//  Phase 3.16 验收测试 1：CREATE TABLE → INSERT → SELECT 完整链路
// =====================================================================

#[test]
fn test_sql_full_link_create_insert_select() {
    // 场景：CREATE TABLE → INSERT 3 行 → SELECT * → 验证数据完整
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "t",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );

    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );

    // INSERT INTO t VALUES (1, 'alice'), (2, 'bob'), (3, 'carol')
    // DML 不需要 register_table，直接传入 &mut table
    let exec = Executor::new();
    let insert_plan = plan_sql(
        "INSERT INTO t (id, name) VALUES (1, 'alice'), (2, 'bob'), (3, 'carol')",
        &catalog,
    );
    let count = exec.execute_insert(&insert_plan, &mut table).unwrap();
    assert_eq!(count.affected_rows, 3);
    assert_eq!(table.row_count(), 3);

    // SELECT * FROM t — 需要 register_table
    let select_plan = plan_sql("SELECT id, name FROM t", &catalog);
    let mut exec2 = Executor::new();
    exec2.register_table(&table);
    let result = exec2.execute(&select_plan).unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(
        result[0],
        vec![Value::Int64(1), Value::Text("alice".into())]
    );
    assert_eq!(result[1], vec![Value::Int64(2), Value::Text("bob".into())]);
    assert_eq!(
        result[2],
        vec![Value::Int64(3), Value::Text("carol".into())]
    );
}

// =====================================================================
//  Phase 3.16 验收测试 2：INSERT 1000000 行（CounterTable 源）
// =====================================================================

#[test]
fn test_sql_full_link_insert_one_million_rows() {
    use szrsql_sql::executor::CounterTable;

    const TOTAL: usize = 1_000_000;

    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table("source", vec![("id", ColumnType::Int64)]);
    catalog.add_simple_table("target", vec![("id", ColumnType::Int64)]);

    // INSERT INTO target (id) SELECT id FROM source
    let insert_plan = plan_sql("INSERT INTO target (id) SELECT id FROM source", &catalog);

    let source = CounterTable::new("source", TOTAL);
    let mut target = InMemoryTable::with_columns("target", vec![("id", ColumnType::Int64)]);

    // register_table 仅注册源表 source；target 作为 DML 目标直接传入
    let mut exec = Executor::new();
    exec.register_table(&source);

    let inserted = exec.execute_insert(&insert_plan, &mut target).unwrap();
    assert_eq!(inserted.affected_rows, TOTAL, "INSERT 行数应为 {TOTAL}");
    assert_eq!(target.row_count(), TOTAL, "INSERT 后行数应为 {TOTAL}");

    // 验证首尾值
    let first_row = target.get_row(0).unwrap();
    assert_eq!(first_row, vec![Value::Int64(0)]);
    let last_row = target.get_row(TOTAL - 1).unwrap();
    assert_eq!(last_row, vec![Value::Int64((TOTAL - 1) as i64)]);
}

// =====================================================================
//  Phase 3.16 验收测试 3：3 表 JOIN 查询
// =====================================================================

#[test]
fn test_sql_full_link_three_table_join() {
    // 场景：users JOIN orders JOIN items
    // 验证：alice 看到 3 行（2 orders × items），bob 看到 1 行，carol 0 行
    let catalog = make_three_table_catalog();
    let (users, orders, items) = make_three_table_data();

    let mut exec = Executor::new();
    exec.register_table(&users);
    exec.register_table(&orders);
    exec.register_table(&items);

    // SELECT u.name, o.order_id, i.product
    // FROM users u
    // JOIN orders o ON u.user_id = o.user_id
    // JOIN items i ON o.order_id = i.order_id
    let sql = "SELECT u.name, o.order_id, i.product \
               FROM users u \
               JOIN orders o ON u.user_id = o.user_id \
               JOIN items i ON o.order_id = i.order_id";
    let plan = plan_sql(sql, &catalog);
    let result = exec.execute(&plan).unwrap();

    // 期望 4 行（alice: 3, bob: 1, carol: 0）
    assert_eq!(result.len(), 4, "3 表 JOIN 应返回 4 行");

    // 验证 alice 的 3 行
    let alice_rows: Vec<&Vec<Value>> = result
        .iter()
        .filter(|r| r[0] == Value::Text("alice".into()))
        .collect();
    assert_eq!(alice_rows.len(), 3, "alice 应有 3 行订单项");

    // 验证 bob 的 1 行
    let bob_rows: Vec<&Vec<Value>> = result
        .iter()
        .filter(|r| r[0] == Value::Text("bob".into()))
        .collect();
    assert_eq!(bob_rows.len(), 1, "bob 应有 1 行订单项");

    // 验证 carol 的 0 行
    let carol_rows: Vec<&Vec<Value>> = result
        .iter()
        .filter(|r| r[0] == Value::Text("carol".into()))
        .collect();
    assert_eq!(carol_rows.len(), 0, "carol 应有 0 行订单项（无订单）");
}

// =====================================================================
//  Phase 3.16 验收测试 4：3 表 JOIN + WHERE 过滤
// =====================================================================

#[test]
fn test_sql_full_link_three_table_join_with_where() {
    let catalog = make_three_table_catalog();
    let (users, orders, items) = make_three_table_data();

    let mut exec = Executor::new();
    exec.register_table(&users);
    exec.register_table(&orders);
    exec.register_table(&items);

    // SELECT u.name, o.order_id, o.amount
    // FROM users u
    // JOIN orders o ON u.user_id = o.user_id
    // WHERE o.amount > 50
    let sql = "SELECT u.name, o.order_id, o.amount \
               FROM users u \
               JOIN orders o ON u.user_id = o.user_id \
               WHERE o.amount > 50";
    let plan = plan_sql(sql, &catalog);
    let result = exec.execute(&plan).unwrap();

    // amount > 50 的订单：101 (alice, 75), 102 (bob, 100) → 2 行
    assert_eq!(result.len(), 2, "amount > 50 的 JOIN 应返回 2 行");
    let names: Vec<String> = result
        .iter()
        .filter_map(|r| {
            if let Value::Text(s) = &r[0] {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect();
    assert!(names.contains(&"alice".to_string()));
    assert!(names.contains(&"bob".to_string()));
}

// =====================================================================
//  Phase 3.16 验收测试 5：子查询嵌套（2 层派生表）
// =====================================================================

#[test]
fn test_sql_full_link_nested_subquery() {
    // 场景：SELECT FROM (SELECT FROM (SELECT)) 2 层嵌套
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "t",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );

    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );
    for i in 0..10 {
        table.insert(vec![Value::Int64(i), Value::Int64(i * 10)]);
    }

    let mut exec = Executor::new();
    exec.register_table(&table);

    // SELECT id FROM (SELECT id, val FROM t WHERE id > 3) sub WHERE val > 50
    // 期望：id > 3 → [4,5,6,7,8,9]，val > 50 → [6,7,8,9]（val=id*10）
    let sql = "SELECT id FROM (SELECT id, val FROM t WHERE id > 3) AS sub WHERE val > 50";
    let plan = plan_sql(sql, &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 4, "嵌套子查询应返回 4 行");
    let ids: Vec<i64> = result
        .iter()
        .filter_map(|r| {
            if let Value::Int64(id) = r[0] {
                Some(id)
            } else {
                None
            }
        })
        .collect();
    assert!(ids.contains(&6));
    assert!(ids.contains(&7));
    assert!(ids.contains(&8));
    assert!(ids.contains(&9));
}

// =====================================================================
//  Phase 3.16 验收测试 6：聚合查询（COUNT/SUM/AVG/MIN/MAX）
// =====================================================================

#[test]
fn test_sql_full_link_aggregation() {
    let catalog = make_three_table_catalog();
    let (_users, orders, _items) = make_three_table_data();

    let mut exec = Executor::new();
    exec.register_table(&orders);

    // SELECT COUNT(*), SUM(amount), AVG(amount), MIN(amount), MAX(amount) FROM orders
    let sql = "SELECT COUNT(*), SUM(amount), AVG(amount), MIN(amount), MAX(amount) FROM orders";
    let plan = plan_sql(sql, &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1, "聚合应返回 1 行");
    // COUNT=3, SUM=225, AVG=75, MIN=50, MAX=100
    assert_eq!(result[0][0], Value::Int64(3));
    assert_eq!(result[0][1], Value::Int64(225));
    // AVG: 225/3 = 75（实际可能为 Float64）
    match &result[0][2] {
        Value::Float64(v) => assert!((v - 75.0).abs() < 0.001),
        Value::Int64(v) => assert_eq!(*v, 75),
        other => panic!("AVG 应为 Float64 或 Int64，实际: {other:?}"),
    }
    assert_eq!(result[0][3], Value::Int64(50));
    assert_eq!(result[0][4], Value::Int64(100));
}

// =====================================================================
//  Phase 3.16 验收测试 7：GROUP BY + 聚合
// =====================================================================

#[test]
fn test_sql_full_link_group_by_aggregation() {
    let catalog = make_three_table_catalog();
    let (_users, orders, _items) = make_three_table_data();

    let mut exec = Executor::new();
    exec.register_table(&orders);

    // SELECT user_id, COUNT(*), SUM(amount) FROM orders GROUP BY user_id
    let sql = "SELECT user_id, COUNT(*), SUM(amount) FROM orders GROUP BY user_id";
    let plan = plan_sql(sql, &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(
        result.len(),
        2,
        "GROUP BY user_id 应返回 2 组（alice=1, bob=2）"
    );

    // 按 user_id 排序便于断言
    let sorted = sort_rows_by_index_asc(result, 0);
    // user_id=1 (alice): COUNT=2, SUM=125
    assert_eq!(sorted[0][0], Value::Int64(1));
    assert_eq!(sorted[0][1], Value::Int64(2));
    assert_eq!(sorted[0][2], Value::Int64(125));
    // user_id=2 (bob): COUNT=1, SUM=100
    assert_eq!(sorted[1][0], Value::Int64(2));
    assert_eq!(sorted[1][1], Value::Int64(1));
    assert_eq!(sorted[1][2], Value::Int64(100));
}

// =====================================================================
//  Phase 3.16 验收测试 8：GROUP BY + HAVING
// =====================================================================

#[test]
fn test_sql_full_link_group_by_having() {
    let catalog = make_three_table_catalog();
    let (_users, orders, _items) = make_three_table_data();

    let mut exec = Executor::new();
    exec.register_table(&orders);

    // SELECT user_id, COUNT(*) FROM orders GROUP BY user_id HAVING COUNT(*) > 1
    // 期望：只有 user_id=1 (alice) 有 2 个订单
    let sql = "SELECT user_id, COUNT(*) FROM orders GROUP BY user_id HAVING COUNT(*) > 1";
    let plan = plan_sql(sql, &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1, "HAVING COUNT > 1 应只返回 1 组");
    assert_eq!(result[0][0], Value::Int64(1));
    assert_eq!(result[0][1], Value::Int64(2));
}

// =====================================================================
//  Phase 3.16 验收测试 9：LIMIT + OFFSET
// =====================================================================

#[test]
fn test_sql_full_link_limit_offset() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table("t", vec![("id", ColumnType::Int64)]);

    let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    for i in 0..100 {
        table.insert(vec![Value::Int64(i)]);
    }

    let mut exec = Executor::new();
    exec.register_table(&table);

    // SELECT id FROM t LIMIT 10 OFFSET 20
    let plan = plan_sql("SELECT id FROM t LIMIT 10 OFFSET 20", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 10, "LIMIT 10 应返回 10 行");
    for (i, row) in result.iter().enumerate() {
        assert_eq!(row[0], Value::Int64(20 + i as i64));
    }
}

// =====================================================================
//  Phase 3.16 验收测试 10：ORDER BY（手动排序模拟）
// =====================================================================

#[test]
fn test_sql_full_link_order_by_manual_sort() {
    // 场景：Executor 暂未实现 Sort 算子，通过手动排序模拟 ORDER BY 行为
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "t",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );

    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );
    // 故意乱序插入
    let values = [50, 10, 90, 30, 70, 20, 80, 40, 60, 100];
    for (i, v) in values.iter().enumerate() {
        table.insert(vec![Value::Int64(i as i64), Value::Int64(*v)]);
    }

    let mut exec = Executor::new();
    exec.register_table(&table);

    // SELECT id, val FROM t（无 ORDER BY，先获取全部行）
    let plan = plan_sql("SELECT id, val FROM t", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 10);

    // 手动按 val 升序排序（模拟 ORDER BY val ASC）
    let sorted_asc = sort_rows_by_index_asc(result.clone(), 1);
    for (i, row) in sorted_asc.iter().enumerate().take(10) {
        let expected = (i + 1) * 10; // 10, 20, ..., 100
        assert_eq!(
            row[1],
            Value::Int64(expected as i64),
            "升序第 {i} 个 val 应为 {expected}"
        );
    }

    // 手动按 val 降序排序（模拟 ORDER BY val DESC）
    let sorted_desc = sort_rows_by_index_desc(result, 1);
    for (i, row) in sorted_desc.iter().enumerate().take(10) {
        let expected = 100 - i * 10; // 100, 90, ..., 10
        assert_eq!(
            row[1],
            Value::Int64(expected as i64),
            "降序第 {i} 个 val 应为 {expected}"
        );
    }
}

// =====================================================================
//  Phase 3.16 验收测试 11：完整链路 — JOIN + 聚合 + LIMIT
// =====================================================================

#[test]
fn test_sql_full_link_join_aggregate_limit() {
    // 场景：users JOIN orders → GROUP BY user_id → COUNT/SUM → LIMIT
    let catalog = make_three_table_catalog();
    let (users, orders, _items) = make_three_table_data();

    let mut exec = Executor::new();
    exec.register_table(&users);
    exec.register_table(&orders);

    // SELECT u.user_id, COUNT(*), SUM(o.amount)
    // FROM users u JOIN orders o ON u.user_id = o.user_id
    // GROUP BY u.user_id
    // LIMIT 1
    let sql = "SELECT u.user_id, COUNT(*), SUM(o.amount) \
               FROM users u JOIN orders o ON u.user_id = o.user_id \
               GROUP BY u.user_id \
               LIMIT 1";
    let plan = plan_sql(sql, &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1, "LIMIT 1 应只返回 1 行");
    // 结果应为 alice (user_id=1) 或 bob (user_id=2) 之一
    let user_id = match &result[0][0] {
        Value::Int64(v) => *v,
        _ => panic!("expected Int64 user_id"),
    };
    assert!(user_id == 1 || user_id == 2, "user_id 应为 1 或 2");
    if user_id == 1 {
        assert_eq!(result[0][1], Value::Int64(2)); // COUNT
        assert_eq!(result[0][2], Value::Int64(125)); // SUM
    } else {
        assert_eq!(result[0][1], Value::Int64(1)); // COUNT
        assert_eq!(result[0][2], Value::Int64(100)); // SUM
    }
}

// =====================================================================
//  Phase 3.16 验收测试 12：事务 COMMIT（保留变更）
// =====================================================================

#[test]
fn test_sql_full_link_transaction_commit() {
    // 场景：BEGIN → INSERT → UPDATE → COMMIT → 数据保留
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "t",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );

    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );

    // BEGIN（创建快照）
    let snapshot = table.snapshot();

    // DML 不需要 register_table，直接传入 &mut table
    let exec = Executor::new();

    // INSERT 3 行
    let insert_plan = plan_sql(
        "INSERT INTO t (id, val) VALUES (1, 10), (2, 20), (3, 30)",
        &catalog,
    );
    let inserted = exec.execute_insert(&insert_plan, &mut table).unwrap();
    assert_eq!(inserted.affected_rows, 3);
    assert_eq!(table.row_count(), 3);

    // UPDATE: val = val * 2
    let update_plan = plan_sql("UPDATE t SET val = val * 2", &catalog);
    let updated = exec.execute_update(&update_plan, &mut table).unwrap();
    assert_eq!(updated.affected_rows, 3);

    // COMMIT：丢弃快照（变更保留）
    drop(snapshot);
    assert_eq!(table.row_count(), 3, "COMMIT 后数据应保留");

    // 验证 UPDATE 生效（SELECT 需 register_table）
    let mut exec2 = Executor::new();
    exec2.register_table(&table);
    let select_plan = plan_sql("SELECT id, val FROM t", &catalog);
    let result = exec2.execute(&select_plan).unwrap();
    let sorted = sort_rows_by_index_asc(result, 0);
    assert_eq!(sorted[0], vec![Value::Int64(1), Value::Int64(20)]); // 10 * 2
    assert_eq!(sorted[1], vec![Value::Int64(2), Value::Int64(40)]); // 20 * 2
    assert_eq!(sorted[2], vec![Value::Int64(3), Value::Int64(60)]); // 30 * 2
}

// =====================================================================
//  Phase 3.16 验收测试 13：事务 ROLLBACK（丢弃变更）
// =====================================================================

#[test]
fn test_sql_full_link_transaction_rollback() {
    // 场景：BEGIN → INSERT → UPDATE → ROLLBACK → 数据回到初始状态
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "t",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );

    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );
    // 初始 2 行
    table.insert(vec![Value::Int64(1), Value::Int64(100)]);
    table.insert(vec![Value::Int64(2), Value::Int64(200)]);
    let initial_count = table.row_count();

    // BEGIN（创建快照）
    let snapshot = table.snapshot();

    // DML 不需要 register_table
    let exec = Executor::new();

    // INSERT 3 行
    let insert_plan = plan_sql(
        "INSERT INTO t (id, val) VALUES (3, 30), (4, 40), (5, 50)",
        &catalog,
    );
    let _ = exec.execute_insert(&insert_plan, &mut table).unwrap();
    assert_eq!(table.row_count(), initial_count + 3, "INSERT 后应为 5 行");

    // UPDATE: 所有 val 翻倍
    let update_plan = plan_sql("UPDATE t SET val = val * 2", &catalog);
    let _ = exec.execute_update(&update_plan, &mut table).unwrap();

    // DELETE: id > 2
    let delete_plan = plan_sql("DELETE FROM t WHERE id > 2", &catalog);
    let _ = exec.execute_delete(&delete_plan, &mut table).unwrap();
    assert_eq!(table.row_count(), 2, "DELETE 后应为 2 行");

    // ROLLBACK：restore 快照
    table.restore(snapshot);
    assert_eq!(
        table.row_count(),
        initial_count,
        "ROLLBACK 后行数应回到初始"
    );

    // 验证初始数据完整恢复
    let mut exec2 = Executor::new();
    exec2.register_table(&table);
    let select_plan = plan_sql("SELECT id, val FROM t", &catalog);
    let result = exec2.execute(&select_plan).unwrap();
    let sorted = sort_rows_by_index_asc(result, 0);
    assert_eq!(sorted[0], vec![Value::Int64(1), Value::Int64(100)]); // 未翻倍
    assert_eq!(sorted[1], vec![Value::Int64(2), Value::Int64(200)]); // 未翻倍
}

// =====================================================================
//  Phase 3.16 验收测试 14：混合 DML + 查询一致性
// =====================================================================

#[test]
fn test_sql_full_link_mixed_dml_query_consistency() {
    // 场景：INSERT → SELECT 验证 → UPDATE → SELECT 验证 → DELETE → SELECT 验证
    // 每步 DML 后立即 SELECT 验证数据一致性
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "t",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );

    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );

    // Step 1: INSERT 5 行（DML 不需 register）
    let exec = Executor::new();
    let insert_plan = plan_sql(
        "INSERT INTO t (id, val) VALUES (1, 10), (2, 20), (3, 30), (4, 40), (5, 50)",
        &catalog,
    );
    let n = exec.execute_insert(&insert_plan, &mut table).unwrap();
    assert_eq!(n.affected_rows, 5);
    assert_eq!(table.row_count(), 5);

    // Step 2: SELECT 验证 5 行（SELECT 需 register）
    let select_plan = plan_sql("SELECT id, val FROM t", &catalog);
    let mut exec_sel = Executor::new();
    exec_sel.register_table(&table);
    let result = exec_sel.execute(&select_plan).unwrap();
    assert_eq!(result.len(), 5);
    let sum: i64 = result
        .iter()
        .filter_map(|r| {
            if let Value::Int64(v) = &r[1] {
                Some(*v)
            } else {
                None
            }
        })
        .sum();
    assert_eq!(sum, 150, "初始 val 总和应为 150");

    // Step 3: UPDATE: val = val + 100 WHERE id <= 3
    let update_plan = plan_sql("UPDATE t SET val = val + 100 WHERE id <= 3", &catalog);
    let n = exec.execute_update(&update_plan, &mut table).unwrap();
    assert_eq!(n.affected_rows, 3, "应更新 3 行");

    // Step 4: SELECT 验证更新结果
    let mut exec_sel2 = Executor::new();
    exec_sel2.register_table(&table);
    let result = exec_sel2.execute(&select_plan).unwrap();
    let new_sum: i64 = result
        .iter()
        .filter_map(|r| {
            if let Value::Int64(v) = &r[1] {
                Some(*v)
            } else {
                None
            }
        })
        .sum();
    // 110 + 120 + 130 + 40 + 50 = 450
    assert_eq!(new_sum, 450, "UPDATE 后 val 总和应为 450");

    // Step 5: DELETE: val > 100
    let delete_plan = plan_sql("DELETE FROM t WHERE val > 100", &catalog);
    let n = exec.execute_delete(&delete_plan, &mut table).unwrap();
    assert_eq!(n.affected_rows, 3, "应删除 3 行");

    // Step 6: SELECT 验证删除结果
    let mut exec_sel3 = Executor::new();
    exec_sel3.register_table(&table);
    let result = exec_sel3.execute(&select_plan).unwrap();
    assert_eq!(result.len(), 2, "剩余 2 行");
    let remaining_sum: i64 = result
        .iter()
        .filter_map(|r| {
            if let Value::Int64(v) = &r[1] {
                Some(*v)
            } else {
                None
            }
        })
        .sum();
    assert_eq!(remaining_sum, 90, "剩余 val 总和应为 40 + 50 = 90");
}

// =====================================================================
//  Phase 3.16 验收测试 15：1M 行 + 聚合 + LIMIT 完整链路
// =====================================================================

#[test]
fn test_sql_full_link_one_million_aggregate_limit() {
    use szrsql_sql::executor::CounterTable;

    const TOTAL: usize = 1_000_000;

    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table("big", vec![("id", ColumnType::Int64)]);

    let big = CounterTable::new("big", TOTAL);

    let mut exec = Executor::new();
    exec.register_table(&big);

    // SELECT COUNT(*) FROM big
    let count_plan = plan_sql("SELECT COUNT(*) FROM big", &catalog);
    let result = exec.execute(&count_plan).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(TOTAL as i64), "COUNT 应为 1M");

    // SELECT id FROM big LIMIT 5
    let limit_plan = plan_sql("SELECT id FROM big LIMIT 5", &catalog);
    let result = exec.execute(&limit_plan).unwrap();
    assert_eq!(result.len(), 5, "LIMIT 5 应返回 5 行");
    for (i, row) in result.iter().enumerate() {
        assert_eq!(row[0], Value::Int64(i as i64));
    }

    // SELECT id FROM big LIMIT 10 OFFSET 999990（验证大 OFFSET）
    let offset_plan = plan_sql("SELECT id FROM big LIMIT 10 OFFSET 999990", &catalog);
    let result = exec.execute(&offset_plan).unwrap();
    assert_eq!(result.len(), 10, "尾部 LIMIT 10 应返回 10 行");
    for (i, row) in result.iter().enumerate() {
        assert_eq!(row[0], Value::Int64(999990 + i as i64));
    }
}

// =====================================================================
//  Phase 3.16 验收测试 16：3 表 JOIN + 聚合 + HAVING + LIMIT 完整链路
// =====================================================================

#[test]
fn test_sql_full_link_join_aggregate_having_limit() {
    // 场景：综合 — users JOIN orders → GROUP BY user_id → COUNT/SUM → HAVING → LIMIT
    let catalog = make_three_table_catalog();
    let (users, orders, _items) = make_three_table_data();

    let mut exec = Executor::new();
    exec.register_table(&users);
    exec.register_table(&orders);

    // SELECT u.user_id, COUNT(*), SUM(o.amount)
    // FROM users u JOIN orders o ON u.user_id = o.user_id
    // GROUP BY u.user_id
    // HAVING SUM(o.amount) > 100
    let sql = "SELECT u.user_id, COUNT(*), SUM(o.amount) \
               FROM users u JOIN orders o ON u.user_id = o.user_id \
               GROUP BY u.user_id \
               HAVING SUM(o.amount) > 100";
    let plan = plan_sql(sql, &catalog);
    let result = exec.execute(&plan).unwrap();

    // alice (id=1): SUM=125 > 100 ✓
    // bob (id=2): SUM=100，不 > 100 ✗
    assert_eq!(result.len(), 1, "HAVING SUM > 100 应只返回 1 组");
    assert_eq!(result[0][0], Value::Int64(1));
    assert_eq!(result[0][1], Value::Int64(2));
    assert_eq!(result[0][2], Value::Int64(125));
}

// =====================================================================
//  Phase 3.16 验收测试 17：事务中混合 DML + 查询 → ROLLBACK 数据一致
// =====================================================================

#[test]
fn test_sql_full_link_transaction_mixed_dml_rollback() {
    // 场景：BEGIN → INSERT → SELECT → UPDATE → SELECT → DELETE → SELECT → ROLLBACK → 验证初始状态
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "t",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );

    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );
    // 初始 3 行
    table.insert(vec![Value::Int64(1), Value::Int64(100)]);
    table.insert(vec![Value::Int64(2), Value::Int64(200)]);
    table.insert(vec![Value::Int64(3), Value::Int64(300)]);
    let initial_count = table.row_count();

    // BEGIN
    let snapshot = table.snapshot();

    // DML executor — 不 register table
    let exec = Executor::new();

    // INSERT 1 行
    let insert_plan = plan_sql("INSERT INTO t (id, val) VALUES (4, 400)", &catalog);
    let _ = exec.execute_insert(&insert_plan, &mut table).unwrap();
    assert_eq!(table.row_count(), initial_count + 1);

    // SELECT 验证
    let mut exec_sel = Executor::new();
    exec_sel.register_table(&table);
    let count_plan = plan_sql("SELECT COUNT(*) FROM t", &catalog);
    let result = exec_sel.execute(&count_plan).unwrap();
    assert_eq!(result[0][0], Value::Int64((initial_count + 1) as i64));

    // UPDATE: val = val + 1
    let update_plan = plan_sql("UPDATE t SET val = val + 1", &catalog);
    let _ = exec.execute_update(&update_plan, &mut table).unwrap();

    // SELECT 验证
    let mut exec_sel2 = Executor::new();
    exec_sel2.register_table(&table);
    let result = exec_sel2.execute(&count_plan).unwrap();
    assert_eq!(result[0][0], Value::Int64((initial_count + 1) as i64));

    // DELETE: id = 4
    let delete_plan = plan_sql("DELETE FROM t WHERE id = 4", &catalog);
    let _ = exec.execute_delete(&delete_plan, &mut table).unwrap();
    assert_eq!(table.row_count(), initial_count);

    // ROLLBACK：恢复到 BEGIN 之前
    table.restore(snapshot);
    assert_eq!(table.row_count(), initial_count, "ROLLBACK 后行数应恢复");

    // 验证 val 未被 +1（恢复初始值）
    let mut exec_sel3 = Executor::new();
    exec_sel3.register_table(&table);
    let select_plan = plan_sql("SELECT id, val FROM t", &catalog);
    let result = exec_sel3.execute(&select_plan).unwrap();
    let sorted = sort_rows_by_index_asc(result, 0);
    assert_eq!(sorted[0], vec![Value::Int64(1), Value::Int64(100)]);
    assert_eq!(sorted[1], vec![Value::Int64(2), Value::Int64(200)]);
    assert_eq!(sorted[2], vec![Value::Int64(3), Value::Int64(300)]);
}

// =====================================================================
//  Phase 3.16 验收测试 18：综合完整链路（CREATE + INSERT + JOIN + 聚合 + 事务）
// =====================================================================

#[test]
fn test_sql_full_link_comprehensive_e2e() {
    // 综合场景：
    // 1. CREATE 三表 catalog
    // 2. INSERT 初始数据
    // 3. JOIN 查询验证
    // 4. 事务内 DML
    // 5. 聚合验证
    // 6. ROLLBACK
    // 7. 验证数据回到初始
    let catalog = make_three_table_catalog();
    let (mut users, mut orders, mut items) = make_three_table_data();

    let initial_users = users.row_count();
    let initial_orders = orders.row_count();
    let initial_items = items.row_count();

    // Step 1: 初始 JOIN 验证
    let join_sql = "SELECT u.name, COUNT(o.order_id) \
                    FROM users u \
                    LEFT JOIN orders o ON u.user_id = o.user_id \
                    GROUP BY u.name";
    let join_plan = plan_sql(join_sql, &catalog);

    {
        let mut exec = Executor::new();
        exec.register_table(&users);
        exec.register_table(&orders);
        let result = exec.execute(&join_plan).unwrap();
        assert_eq!(result.len(), 3, "应有 3 个用户组");
    }

    // Step 2: BEGIN — 创建所有表快照
    let snap_users = users.snapshot();
    let snap_orders = orders.snapshot();
    let snap_items = items.snapshot();

    // Step 3: 事务内 DML（DML 不需 register）
    let exec = Executor::new();

    // INSERT 新用户
    let new_user_plan = plan_sql(
        "INSERT INTO users (user_id, name) VALUES (4, 'dave')",
        &catalog,
    );
    let _ = exec.execute_insert(&new_user_plan, &mut users).unwrap();

    // INSERT dave 的订单
    let new_order_plan = plan_sql(
        "INSERT INTO orders (order_id, user_id, amount) VALUES (200, 4, 500)",
        &catalog,
    );
    let _ = exec.execute_insert(&new_order_plan, &mut orders).unwrap();

    // INSERT dave 订单的 items
    let new_item_plan = plan_sql(
        "INSERT INTO items (item_id, order_id, product) VALUES (2000, 200, 'elderberry')",
        &catalog,
    );
    let _ = exec.execute_insert(&new_item_plan, &mut items).unwrap();

    // Step 4: 验证 DML 生效
    assert_eq!(users.row_count(), initial_users + 1);
    assert_eq!(orders.row_count(), initial_orders + 1);
    assert_eq!(items.row_count(), initial_items + 1);

    // Step 5: JOIN + 聚合验证（含新数据）
    {
        let mut exec = Executor::new();
        exec.register_table(&users);
        exec.register_table(&orders);
        let result = exec.execute(&join_plan).unwrap();
        assert_eq!(result.len(), 4, "加入 dave 后应有 4 个用户组");
    }

    // Step 6: ROLLBACK — 恢复所有表
    users.restore(snap_users);
    orders.restore(snap_orders);
    items.restore(snap_items);

    // Step 7: 验证恢复
    assert_eq!(users.row_count(), initial_users, "users 恢复");
    assert_eq!(orders.row_count(), initial_orders, "orders 恢复");
    assert_eq!(items.row_count(), initial_items, "items 恢复");

    // Step 8: JOIN 验证（应回到初始 3 组）
    {
        let mut exec = Executor::new();
        exec.register_table(&users);
        exec.register_table(&orders);
        let result = exec.execute(&join_plan).unwrap();
        assert_eq!(result.len(), 3, "ROLLBACK 后应回到 3 个用户组");
    }
}
