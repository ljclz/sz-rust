//! Phase 3.14 集成测试 — RBAC + RLS 组合验证。
//!
//! # 验收标准（SzRSQL实施进度.md Phase 3.14）
//!
//! - 角色 A 有 RLS 策略 → SELECT 自动过滤
//! - 角色 B 无 RLS 策略 → SELECT 全表
//! - RLS + 子查询 + JOIN 混合
//!
//! # 测试策略
//!
//! 直接在 `RbacManager` + `RlsManager` + `PolicyEvaluator` 层测试组合：
//! 1. 用 `RbacManager::check(user, Privilege::Select, &table)` 验证表级权限
//! 2. 用 `PolicyEvaluator::filter_select(...)` 验证行级过滤
//! 3. 模拟"角色 A 有 RLS 策略"= RBAC 允许 + RLS 过滤；"角色 B 无 RLS 策略"= RBAC 允许 + 无策略匹配
//! 4. JOIN 混合：两表各自过滤后手动 join

use szrsql_catalog::{
    rbac::{DatabaseObject, Privilege, RbacManager},
    rls::{Policy, PolicyEvaluator, RlsManager},
};
use szrsql_sql::ast::{BinaryOp, Expr, TableName};
use szrsql_types::value::Value;

use std::collections::HashSet;

// =====================================================================
//  辅助函数
// =====================================================================

fn table(name: &str) -> TableName {
    TableName::new(name)
}

/// Helper: wrap table name as DatabaseObject::Table
fn db_table(name: &str) -> DatabaseObject {
    DatabaseObject::Table(table(name))
}

/// 构造 `col = current_user` 表达式
fn col_eq_current_user(col: &str) -> Expr {
    Expr::BinaryOp {
        left: Box::new(Expr::Identifier(vec![col.to_string()])),
        op: BinaryOp::Eq,
        right: Box::new(Expr::Identifier(vec!["current_user".to_string()])),
    }
}

/// 构造 `col = literal_text(value)` 表达式
fn col_eq_text(col: &str, value: &str) -> Expr {
    Expr::BinaryOp {
        left: Box::new(Expr::Identifier(vec![col.to_string()])),
        op: BinaryOp::Eq,
        right: Box::new(Expr::Literal(Value::Text(value.to_string()))),
    }
}

/// 构造 `col = literal_int(value)` 表达式
fn col_eq_int(col: &str, value: i64) -> Expr {
    Expr::BinaryOp {
        left: Box::new(Expr::Identifier(vec![col.to_string()])),
        op: BinaryOp::Eq,
        right: Box::new(Expr::Literal(Value::Int64(value))),
    }
}

/// 构造 `left = right` 表达式
fn eq_expr(left: Expr, right: Expr) -> Expr {
    Expr::BinaryOp {
        left: Box::new(left),
        op: BinaryOp::Eq,
        right: Box::new(right),
    }
}

/// 列名: ["id", "tenant_id", "data"]
fn tenant_columns() -> Vec<String> {
    vec![
        "id".to_string(),
        "tenant_id".to_string(),
        "data".to_string(),
    ]
}

/// 构造测试行: [id, tenant_id, data]
fn tenant_row(id: i64, tenant: &str, data: &str) -> Vec<Value> {
    vec![
        Value::Int64(id),
        Value::Text(tenant.to_string()),
        Value::Text(data.to_string()),
    ]
}

/// 提取行的 id
fn row_id(row: &[Value]) -> i64 {
    if let Value::Int64(id) = row[0] {
        id
    } else {
        panic!("expected Int64 id, got {:?}", row[0])
    }
}

/// 提取行的 tenant_id
fn row_tenant(row: &[Value]) -> String {
    if let Value::Text(t) = &row[1] {
        t.clone()
    } else {
        panic!("expected Text tenant_id, got {:?}", row[1])
    }
}

/// 模拟 RBAC + RLS 组合的 SELECT 过滤流程
///
/// 1. RBAC 检查：用户是否有 SELECT 权限？
///    - 若无 → 返回空（权限拒绝）
///    - 若有 → 继续
/// 2. RLS 过滤：应用匹配的策略
#[allow(clippy::too_many_arguments)]
fn rbac_rls_select(
    rows: &[Vec<Value>],
    column_names: &[String],
    table_name: &TableName,
    user: &str,
    user_roles: &[String],
    rbac: &RbacManager,
    rls: &RlsManager,
    evaluator: &PolicyEvaluator,
    bypass_rls: bool,
) -> Vec<Vec<Value>> {
    // Step 1: RBAC 检查
    let obj = DatabaseObject::Table(table_name.clone());
    if !rbac.check(user, Privilege::Select, &obj) {
        return Vec::new(); // 权限拒绝
    }
    // Step 2: RLS 过滤
    evaluator.filter_select(
        rows,
        column_names,
        table_name,
        user,
        user_roles,
        rls,
        bypass_rls,
    )
}

// =====================================================================
//  Phase 3.14 验收测试 1：角色 A 有 RLS 策略 → SELECT 自动过滤
// =====================================================================

#[test]
fn test_role_a_with_rls_policy_select_filtered() {
    // 场景：
    // - 表 t 包含 alice 和 bob 的数据
    // - alice 属于 role_a，role_a 有 RLS 策略 (tenant_id = current_user)
    // - alice 有 SELECT 权限
    // - 期望：alice 只看到自己的行

    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    // 设置 RBAC
    rbac.create_user("alice").unwrap();
    rbac.create_role("role_a").unwrap();
    rbac.grant_role("role_a", "alice").unwrap();
    rbac.grant(Privilege::Select, db_table("t"), "role_a", false)
        .unwrap();

    // 设置 RLS
    rls.enable_rls(&table("t")).unwrap();
    let policy = Policy::new_select("p_a", table("t"), col_eq_current_user("tenant_id"))
        .with_roles(vec!["role_a".to_string()]);
    rls.create_policy(policy).unwrap();

    // 表数据
    let rows = vec![
        tenant_row(1, "alice", "alice's data 1"),
        tenant_row(2, "bob", "bob's data 1"),
        tenant_row(3, "alice", "alice's data 2"),
    ];

    let evaluator = PolicyEvaluator::new();
    let result = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &["role_a".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false,
    );

    // alice 通过 RBAC 检查 + RLS 过滤后只看到自己的 2 行
    assert_eq!(result.len(), 2);
    for row in &result {
        assert_eq!(row_tenant(row), "alice");
    }
    let ids: HashSet<i64> = result.iter().map(|r| row_id(r)).collect();
    assert!(ids.contains(&1));
    assert!(ids.contains(&3));
}

// =====================================================================
//  Phase 3.14 验收测试 2：角色 B 无 RLS 策略 → SELECT 全表
// =====================================================================

#[test]
fn test_role_b_without_rls_policy_select_all_via_owner_bypass() {
    // 场景：
    // - 表 t 包含 alice 和 bob 的数据
    // - bob 属于 role_b，role_b 有 SELECT 权限但无匹配的 RLS 策略
    // - bob 是表所有者（bypass_rls=true）
    // - 期望：bob 看到全表（所有者绕过 RLS）

    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    // 设置 RBAC
    rbac.create_user("bob").unwrap();
    rbac.create_role("role_b").unwrap();
    rbac.grant_role("role_b", "bob").unwrap();
    rbac.grant(Privilege::Select, db_table("t"), "role_b", false)
        .unwrap();

    // 设置 RLS：只有 role_a 的策略，role_b 无匹配策略
    rls.enable_rls(&table("t")).unwrap();
    let policy = Policy::new_select("p_a", table("t"), col_eq_current_user("tenant_id"))
        .with_roles(vec!["role_a".to_string()]);
    rls.create_policy(policy).unwrap();

    // 表数据
    let rows = vec![
        tenant_row(1, "alice", "alice's data 1"),
        tenant_row(2, "bob", "bob's data 1"),
        tenant_row(3, "alice", "alice's data 2"),
    ];

    let evaluator = PolicyEvaluator::new();
    // bob 是表所有者 → bypass_rls=true
    let result = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "bob",
        &["role_b".to_string()],
        &rbac,
        &rls,
        &evaluator,
        true, // bypass_rls
    );

    // bob 通过 RBAC + bypass_rls → 看到全表
    assert_eq!(result.len(), 3);
}

#[test]
fn test_role_b_without_rls_policy_no_bypass_denies_all() {
    // 场景：
    // - bob 属于 role_b，role_b 有 SELECT 权限但无匹配的 RLS 策略
    // - bob 不是表所有者（bypass_rls=false）
    // - 期望：bob 看不到任何行（deny-all default，PG 语义）

    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    rbac.create_user("bob").unwrap();
    rbac.create_role("role_b").unwrap();
    rbac.grant_role("role_b", "bob").unwrap();
    rbac.grant(Privilege::Select, db_table("t"), "role_b", false)
        .unwrap();

    rls.enable_rls(&table("t")).unwrap();
    // 只有 role_a 的策略
    let policy = Policy::new_select("p_a", table("t"), col_eq_current_user("tenant_id"))
        .with_roles(vec!["role_a".to_string()]);
    rls.create_policy(policy).unwrap();

    let rows = vec![
        tenant_row(1, "alice", "data1"),
        tenant_row(2, "bob", "data2"),
    ];

    let evaluator = PolicyEvaluator::new();
    let result = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "bob",
        &["role_b".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false, // 不绕过 RLS
    );

    // bob 通过 RBAC，但无匹配的 PERMISSIVE 策略 → deny-all
    assert_eq!(result.len(), 0);
}

#[test]
fn test_role_b_with_public_permissive_policy_sees_all() {
    // 场景：
    // - role_a 有 RLS 策略 (tenant_id = current_user)
    // - PUBLIC 有 permissive 策略 (1=1, 始终通过) → role_b 通过 PUBLIC 策略看到全表
    // - 期望：role_b 看到 PUBLIC 策略允许的全表

    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    rbac.create_user("alice").unwrap();
    rbac.create_user("bob").unwrap();
    rbac.create_role("role_a").unwrap();
    rbac.create_role("role_b").unwrap();
    rbac.grant_role("role_a", "alice").unwrap();
    rbac.grant_role("role_b", "bob").unwrap();
    rbac.grant(Privilege::Select, db_table("t"), "role_a", false)
        .unwrap();
    rbac.grant(Privilege::Select, db_table("t"), "role_b", false)
        .unwrap();

    rls.enable_rls(&table("t")).unwrap();
    // role_a 策略：tenant_id = current_user
    let p_a = Policy::new_select("p_a", table("t"), col_eq_current_user("tenant_id"))
        .with_roles(vec!["role_a".to_string()]);
    // PUBLIC 策略：始终通过（1=1）
    let p_public = Policy::new_select(
        "p_public",
        table("t"),
        eq_expr(col_eq_int("id", 1), col_eq_int("id", 1)), // id=1 AND id=1 → always true (simplification)
    );
    rls.create_policy(p_a).unwrap();
    rls.create_policy(p_public).unwrap();

    let rows = vec![
        tenant_row(1, "alice", "alice data"),
        tenant_row(2, "bob", "bob data"),
        tenant_row(3, "carol", "carol data"),
    ];

    let evaluator = PolicyEvaluator::new();

    // alice (role_a) → 匹配 p_a (tenant_id=alice) + p_public (always true) → OR → 看到 alice + 全表
    let alice_result = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &["role_a".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false,
    );
    // alice 匹配 p_public (always true) → 全表（3 行）
    assert_eq!(alice_result.len(), 3);

    // bob (role_b) → 不匹配 p_a (role_a only)，匹配 p_public (PUBLIC) → 全表
    let bob_result = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "bob",
        &["role_b".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false,
    );
    assert_eq!(bob_result.len(), 3);
}

// =====================================================================
//  Phase 3.14 验收测试 3：RLS + 子查询 + JOIN 混合
// =====================================================================

#[test]
fn test_rls_with_join_two_tables_filtered() {
    // 场景：
    // - 表 orders: [order_id, user_id, amount]
    // - 表 items: [item_id, order_id, product]
    // - 两表都启用 RLS，策略均为 user_id/current_user 相关
    // - alice 查询 orders JOIN items → 只看到自己的订单和订单项
    //
    // 模拟 JOIN：
    // 1. 对 orders 应用 RLS 过滤
    // 2. 对 items 应用 RLS 过滤
    // 3. 手动 join（order_id 匹配）

    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    rbac.create_user("alice").unwrap();
    rbac.create_user("bob").unwrap();
    rbac.create_role("role_user").unwrap();
    rbac.grant_role("role_user", "alice").unwrap();
    rbac.grant_role("role_user", "bob").unwrap();
    // GRANT SELECT ON orders, items TO role_user
    rbac.grant(Privilege::Select, db_table("orders"), "role_user", false)
        .unwrap();
    rbac.grant(Privilege::Select, db_table("items"), "role_user", false)
        .unwrap();

    // RLS on orders: user_id = current_user
    rls.enable_rls(&table("orders")).unwrap();
    let p_orders = Policy::new_select("p_orders", table("orders"), col_eq_current_user("user_id"));
    rls.create_policy(p_orders).unwrap();

    // RLS on items: items 表通过 order_id 关联 orders，需要 join 才能确定可见性
    // 简化：items 表也添加 user_id 列，策略为 user_id = current_user
    // 实际生产中可用子查询：USING (order_id IN (SELECT order_id FROM orders WHERE user_id = current_user))
    rls.enable_rls(&table("items")).unwrap();
    let p_items = Policy::new_select("p_items", table("items"), col_eq_current_user("user_id"));
    rls.create_policy(p_items).unwrap();

    // 表数据
    let order_cols = vec![
        "order_id".to_string(),
        "user_id".to_string(),
        "amount".to_string(),
    ];
    let item_cols = vec![
        "item_id".to_string(),
        "order_id".to_string(),
        "user_id".to_string(),
        "product".to_string(),
    ];

    let orders = vec![
        vec![
            Value::Int64(100),
            Value::Text("alice".to_string()),
            Value::Int64(50),
        ],
        vec![
            Value::Int64(101),
            Value::Text("bob".to_string()),
            Value::Int64(75),
        ],
        vec![
            Value::Int64(102),
            Value::Text("alice".to_string()),
            Value::Int64(100),
        ],
    ];

    let items = vec![
        vec![
            Value::Int64(1),
            Value::Int64(100),
            Value::Text("alice".to_string()),
            Value::Text("apple".to_string()),
        ],
        vec![
            Value::Int64(2),
            Value::Int64(100),
            Value::Text("alice".to_string()),
            Value::Text("banana".to_string()),
        ],
        vec![
            Value::Int64(3),
            Value::Int64(101),
            Value::Text("bob".to_string()),
            Value::Text("cherry".to_string()),
        ],
        vec![
            Value::Int64(4),
            Value::Int64(102),
            Value::Text("alice".to_string()),
            Value::Text("date".to_string()),
        ],
    ];

    let evaluator = PolicyEvaluator::new();

    // Step 1: 过滤 orders（alice 只看到自己的订单）
    let alice_orders = rbac_rls_select(
        &orders,
        &order_cols,
        &table("orders"),
        "alice",
        &["role_user".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false,
    );
    assert_eq!(alice_orders.len(), 2); // order_id 100, 102

    // Step 2: 过滤 items（alice 只看到自己的订单项）
    let alice_items = rbac_rls_select(
        &items,
        &item_cols,
        &table("items"),
        "alice",
        &["role_user".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false,
    );
    assert_eq!(alice_items.len(), 3); // item_id 1, 2, 4

    // Step 3: 手动 JOIN (orders.order_id = items.order_id)
    let mut joined: Vec<(i64, String, i64, i64, String)> = Vec::new();
    for order in &alice_orders {
        let order_id = if let Value::Int64(id) = order[0] {
            id
        } else {
            panic!("expected Int64")
        };
        let user_id = if let Value::Text(u) = &order[1] {
            u.clone()
        } else {
            panic!("expected Text")
        };
        let amount = if let Value::Int64(a) = order[2] {
            a
        } else {
            panic!("expected Int64")
        };
        for item in &alice_items {
            let item_order_id = if let Value::Int64(id) = item[1] {
                id
            } else {
                panic!("expected Int64")
            };
            if item_order_id == order_id {
                let item_id = if let Value::Int64(id) = item[0] {
                    id
                } else {
                    panic!("expected Int64")
                };
                let product = if let Value::Text(p) = &item[3] {
                    p.clone()
                } else {
                    panic!("expected Text")
                };
                joined.push((order_id, user_id.clone(), amount, item_id, product));
            }
        }
    }

    // alice 应看到 3 个 join 结果：order 100 (2 items) + order 102 (1 item)
    assert_eq!(joined.len(), 3);
    let order_ids: HashSet<i64> = joined.iter().map(|(oid, _, _, _, _)| *oid).collect();
    assert!(order_ids.contains(&100));
    assert!(order_ids.contains(&102));
    assert!(!order_ids.contains(&101)); // bob's order filtered out
}

#[test]
fn test_rls_with_subquery_semantics() {
    // 场景：模拟子查询 `SELECT * FROM items WHERE order_id IN (SELECT order_id FROM orders)`
    // 两表都启用 RLS，子查询和外表分别独立过滤
    //
    // 实际 SQL:
    //   SELECT * FROM items WHERE order_id IN (
    //     SELECT order_id FROM orders WHERE user_id = current_user
    //   )
    //
    // 拆解：
    //   1. 子查询：过滤 orders → alice 的 order_ids
    //   2. 外查询：过滤 items → 在 order_ids 集合中的 items

    let mut rls = RlsManager::new();

    rls.enable_rls(&table("orders")).unwrap();
    let p_orders = Policy::new_select("p_orders", table("orders"), col_eq_current_user("user_id"));
    rls.create_policy(p_orders).unwrap();

    // items 表不启用 RLS（子查询中 orders 过滤后，items 直接 IN 检查）
    // 实际生产中两表都应启用 RLS

    let order_cols = vec![
        "order_id".to_string(),
        "user_id".to_string(),
        "amount".to_string(),
    ];
    let _item_cols = [
        "item_id".to_string(),
        "order_id".to_string(),
        "product".to_string(),
    ];

    let orders = vec![
        vec![
            Value::Int64(100),
            Value::Text("alice".to_string()),
            Value::Int64(50),
        ],
        vec![
            Value::Int64(101),
            Value::Text("bob".to_string()),
            Value::Int64(75),
        ],
        vec![
            Value::Int64(102),
            Value::Text("alice".to_string()),
            Value::Int64(100),
        ],
    ];

    let items = [
        vec![
            Value::Int64(1),
            Value::Int64(100),
            Value::Text("apple".to_string()),
        ],
        vec![
            Value::Int64(2),
            Value::Int64(101),
            Value::Text("banana".to_string()),
        ],
        vec![
            Value::Int64(3),
            Value::Int64(102),
            Value::Text("cherry".to_string()),
        ],
        vec![
            Value::Int64(4),
            Value::Int64(103),
            Value::Text("date".to_string()),
        ],
    ];

    let evaluator = PolicyEvaluator::new();

    // Step 1: 子查询 — 过滤 orders（alice 只看到自己的订单）
    let alice_orders = evaluator.filter_select(
        &orders,
        &order_cols,
        &table("orders"),
        "alice",
        &[],
        &rls,
        false,
    );
    assert_eq!(alice_orders.len(), 2);

    // 提取 alice 的 order_ids
    let alice_order_ids: HashSet<i64> = alice_orders
        .iter()
        .filter_map(|r| {
            if let Value::Int64(id) = r[0] {
                Some(id)
            } else {
                None
            }
        })
        .collect();
    assert!(alice_order_ids.contains(&100));
    assert!(alice_order_ids.contains(&102));

    // Step 2: 外查询 — 从 items 中筛选 order_id 在 alice_order_ids 中的行
    let alice_items: Vec<_> = items
        .iter()
        .filter(|item| {
            if let Value::Int64(order_id) = item[1] {
                alice_order_ids.contains(&order_id)
            } else {
                false
            }
        })
        .cloned()
        .collect();

    // alice 看到 item_id 1 (order 100) 和 item_id 3 (order 102)
    assert_eq!(alice_items.len(), 2);
    let item_ids: HashSet<i64> = alice_items
        .iter()
        .filter_map(|r| {
            if let Value::Int64(id) = r[0] {
                Some(id)
            } else {
                None
            }
        })
        .collect();
    assert!(item_ids.contains(&1));
    assert!(item_ids.contains(&3));
    assert!(!item_ids.contains(&2)); // bob's order
    assert!(!item_ids.contains(&4)); // order 103 doesn't exist
}

// =====================================================================
//  综合场景：RBAC 拒绝 + RLS
// =====================================================================

#[test]
fn test_rbac_deny_overrides_rls() {
    // 场景：
    // - user_charlie 没有 SELECT 权限
    // - 即使 RLS 策略允许，RBAC 先拒绝 → 返回空
    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    rbac.create_user("charlie").unwrap();
    // charlie 没有任何 GRANT

    rls.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select("p", table("t"), col_eq_current_user("tenant_id"));
    rls.create_policy(p).unwrap();

    let rows = vec![
        tenant_row(1, "charlie", "charlie's data"),
        tenant_row(2, "alice", "alice's data"),
    ];

    let evaluator = PolicyEvaluator::new();
    let result = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "charlie",
        &[],
        &rbac,
        &rls,
        &evaluator,
        false,
    );

    // charlie 无 SELECT 权限 → RBAC 拒绝 → 空结果
    assert_eq!(result.len(), 0);
}

#[test]
fn test_rbac_allow_rls_deny() {
    // 场景：
    // - user_alice 有 SELECT 权限（RBAC 允许）
    // - RLS 策略不匹配 alice 的任何行 → 返回空
    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    rbac.create_user("alice").unwrap();
    rbac.create_role("role_a").unwrap();
    rbac.grant_role("role_a", "alice").unwrap();
    rbac.grant(Privilege::Select, db_table("t"), "role_a", false)
        .unwrap();

    rls.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select("p", table("t"), col_eq_current_user("tenant_id"));
    rls.create_policy(p).unwrap();

    let rows = vec![
        tenant_row(1, "bob", "bob's data"),
        tenant_row(2, "carol", "carol's data"),
    ];

    let evaluator = PolicyEvaluator::new();
    let result = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &["role_a".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false,
    );

    // alice 通过 RBAC，但 RLS 过滤后无匹配行 → 空结果
    assert_eq!(result.len(), 0);
}

#[test]
fn test_superuser_bypasses_rbac_but_rls_still_applies() {
    // 场景：
    // - admin 是 SUPERUSER → RBAC 总是允许
    // - 但 RLS 仍应用（除非 bypass_rls=true）
    // - 期望：admin 通过 RBAC，RLS 过滤后只看到自己的行
    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    rbac.create_superuser("admin").unwrap();

    rls.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select("p", table("t"), col_eq_current_user("tenant_id"));
    rls.create_policy(p).unwrap();

    let rows = vec![
        tenant_row(1, "admin", "admin's data"),
        tenant_row(2, "alice", "alice's data"),
        tenant_row(3, "admin", "more admin data"),
    ];

    let evaluator = PolicyEvaluator::new();
    let result = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "admin",
        &[],
        &rbac,
        &rls,
        &evaluator,
        false, // 不绕过 RLS
    );

    // admin 通过 RBAC（SUPERUSER），RLS 过滤后只看到自己的 2 行
    assert_eq!(result.len(), 2);
    for row in &result {
        assert_eq!(row_tenant(row), "admin");
    }
}

#[test]
fn test_superuser_with_bypass_rls_sees_all() {
    // 场景：
    // - admin 是 SUPERUSER + bypass_rls=true → 看到全表
    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    rbac.create_superuser("admin").unwrap();

    rls.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select("p", table("t"), col_eq_current_user("tenant_id"));
    rls.create_policy(p).unwrap();

    let rows = vec![
        tenant_row(1, "admin", "admin's data"),
        tenant_row(2, "alice", "alice's data"),
    ];

    let evaluator = PolicyEvaluator::new();
    let result = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "admin",
        &[],
        &rbac,
        &rls,
        &evaluator,
        true, // bypass_rls
    );

    assert_eq!(result.len(), 2); // 全表
}

// =====================================================================
//  DML 组合测试：INSERT/UPDATE/DELETE 与 RBAC
// =====================================================================

#[test]
fn test_rbac_deny_insert_blocks_rls_check() {
    // 场景：user 没有 INSERT 权限 → RBAC 拒绝 → 不进行 RLS 检查
    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    rbac.create_user("alice").unwrap();
    // alice 没有 INSERT 权限

    rls.enable_rls(&table("t")).unwrap();
    let p = Policy::new_insert("p_ins", table("t"), col_eq_current_user("owner"));
    rls.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let cols = vec!["id".to_string(), "owner".to_string(), "data".to_string()];
    let row = vec![
        Value::Int64(1),
        Value::Text("alice".to_string()),
        Value::Text("data".to_string()),
    ];

    // RBAC 先检查
    let obj = DatabaseObject::Table(table("t"));
    let rbac_allowed = rbac.check("alice", Privilege::Insert, &obj);
    assert!(!rbac_allowed); // alice 无 INSERT 权限

    // 即使 RLS WITH CHECK 会通过，整体 INSERT 仍被拒绝
    let rls_check = evaluator.check_insert(&row, &cols, &table("t"), "alice", &[], &rls, false);
    assert!(rls_check); // RLS 会通过

    // 但 RBAC 拒绝 → 整体 INSERT 失败
    let final_allowed = rbac_allowed && rls_check;
    assert!(!final_allowed);
}

#[test]
fn test_rbac_allow_insert_rls_check_passes() {
    // 场景：user 有 INSERT 权限 + RLS WITH CHECK 通过 → 整体允许
    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    rbac.create_user("alice").unwrap();
    rbac.create_role("role_a").unwrap();
    rbac.grant_role("role_a", "alice").unwrap();
    rbac.grant(Privilege::Insert, db_table("t"), "role_a", false)
        .unwrap();

    rls.enable_rls(&table("t")).unwrap();
    let p = Policy::new_insert("p_ins", table("t"), col_eq_current_user("owner"));
    rls.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let cols = vec!["id".to_string(), "owner".to_string(), "data".to_string()];
    let row = vec![
        Value::Int64(1),
        Value::Text("alice".to_string()),
        Value::Text("data".to_string()),
    ];

    let obj = DatabaseObject::Table(table("t"));
    let rbac_allowed = rbac.check("alice", Privilege::Insert, &obj);
    assert!(rbac_allowed);

    let rls_check = evaluator.check_insert(&row, &cols, &table("t"), "alice", &[], &rls, false);
    assert!(rls_check);

    assert!(rbac_allowed && rls_check); // 整体允许
}

#[test]
fn test_rbac_allow_insert_rls_check_fails() {
    // 场景：user 有 INSERT 权限 + RLS WITH CHECK 失败 → 整体拒绝
    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    rbac.create_user("alice").unwrap();
    rbac.create_role("role_a").unwrap();
    rbac.grant_role("role_a", "alice").unwrap();
    rbac.grant(Privilege::Insert, db_table("t"), "role_a", false)
        .unwrap();

    rls.enable_rls(&table("t")).unwrap();
    let p = Policy::new_insert("p_ins", table("t"), col_eq_current_user("owner"));
    rls.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let cols = vec!["id".to_string(), "owner".to_string(), "data".to_string()];
    // alice 试图插入 owner=bob 的行 → RLS WITH CHECK 失败
    let row = vec![
        Value::Int64(1),
        Value::Text("bob".to_string()),
        Value::Text("data".to_string()),
    ];

    let obj = DatabaseObject::Table(table("t"));
    let rbac_allowed = rbac.check("alice", Privilege::Insert, &obj);
    assert!(rbac_allowed);

    let rls_check = evaluator.check_insert(&row, &cols, &table("t"), "alice", &[], &rls, false);
    assert!(!rls_check);

    assert!(!(rbac_allowed && rls_check)); // 整体拒绝（RLS 失败：RBAC 通过 + RLS 拒绝 → 拒绝）
}

// =====================================================================
//  REVOKE 与 RLS 的组合
// =====================================================================

#[test]
fn test_revoke_select_blocks_rls_filtering() {
    // 场景：
    // 1. alice 有 SELECT 权限 + RLS 策略 → 看到自己的行
    // 2. REVOKE SELECT → alice 看不到任何行（RBAC 拒绝）
    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    rbac.create_user("alice").unwrap();
    rbac.create_role("role_a").unwrap();
    rbac.grant_role("role_a", "alice").unwrap();
    rbac.grant(Privilege::Select, db_table("t"), "role_a", false)
        .unwrap();

    rls.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select("p", table("t"), col_eq_current_user("tenant_id"));
    rls.create_policy(p).unwrap();

    let rows = vec![
        tenant_row(1, "alice", "data1"),
        tenant_row(2, "bob", "data2"),
    ];

    let evaluator = PolicyEvaluator::new();

    // 初始：alice 看到 1 行
    let before = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &["role_a".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false,
    );
    assert_eq!(before.len(), 1);

    // REVOKE SELECT
    rbac.revoke(Privilege::Select, &db_table("t"), "role_a")
        .unwrap();

    // REVOKE 后：alice 看不到任何行（RBAC 拒绝）
    let after = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &["role_a".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false,
    );
    assert_eq!(after.len(), 0);
}

// =====================================================================
//  策略生命周期与 RBAC 组合
// =====================================================================

#[test]
fn test_drop_rls_policy_keeps_rbac_check() {
    // 场景：
    // 1. alice 有 SELECT 权限 + RLS 策略 → 看到自己的行
    // 2. DROP POLICY → alice 无匹配策略 → deny-all（RLS 启用但无策略）
    // 3. DISABLE RLS → alice 看到全表（RLS 未启用）
    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    rbac.create_user("alice").unwrap();
    rbac.create_role("role_a").unwrap();
    rbac.grant_role("role_a", "alice").unwrap();
    rbac.grant(Privilege::Select, db_table("t"), "role_a", false)
        .unwrap();

    rls.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select("p", table("t"), col_eq_current_user("tenant_id"));
    rls.create_policy(p).unwrap();

    let rows = vec![
        tenant_row(1, "alice", "data1"),
        tenant_row(2, "bob", "data2"),
    ];

    let evaluator = PolicyEvaluator::new();

    // 初始：alice 通过 RBAC + RLS 过滤 → 1 行
    let step1 = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &["role_a".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false,
    );
    assert_eq!(step1.len(), 1);

    // DROP POLICY → alice 无匹配策略 → deny-all
    rls.drop_policy(&table("t"), "p").unwrap();
    let step2 = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &["role_a".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false,
    );
    assert_eq!(step2.len(), 0); // RLS 启用无策略 → deny-all

    // DISABLE RLS → alice 看到全表
    rls.disable_rls(&table("t")).unwrap();
    let step3 = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &["role_a".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false,
    );
    assert_eq!(step3.len(), 2); // RLS 未启用 → 全表
}

// =====================================================================
//  PERMISSIVE/RESTRICTIVE 与 RBAC 组合
// =====================================================================

#[test]
fn test_rbac_with_permissive_and_restrictive_rls() {
    // 场景：
    // - alice (role_user) 有 SELECT 权限
    // - permissive 策略：tenant_id = current_user OR shared = true
    // - restrictive 策略：archived = false
    // - 期望：alice 看到 (自己的 OR 共享的) AND (未归档的)

    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    rbac.create_user("alice").unwrap();
    rbac.create_role("role_user").unwrap();
    rbac.grant_role("role_user", "alice").unwrap();
    rbac.grant(Privilege::Select, db_table("docs"), "role_user", false)
        .unwrap();

    rls.enable_rls(&table("docs")).unwrap();
    // permissive: tenant_id = current_user OR shared = 'true'
    let p_perm = Policy::new_select(
        "p_perm",
        table("docs"),
        szrsql_sql::ast::Expr::BinaryOp {
            left: Box::new(col_eq_current_user("tenant_id")),
            op: BinaryOp::Or,
            right: Box::new(col_eq_text("shared", "true")),
        },
    );
    // restrictive: archived = 'false'
    let p_rest = Policy::new_select("p_rest", table("docs"), col_eq_text("archived", "false"))
        .with_permissive(false);
    rls.create_policy(p_perm).unwrap();
    rls.create_policy(p_rest).unwrap();

    let cols = vec![
        "id".to_string(),
        "tenant_id".to_string(),
        "shared".to_string(),
        "archived".to_string(),
    ];
    let rows = vec![
        vec![
            // id=1, alice 的, 不共享, 未归档 → (alice OR false) AND true → 通过
            Value::Int64(1),
            Value::Text("alice".to_string()),
            Value::Text("false".to_string()),
            Value::Text("false".to_string()),
        ],
        vec![
            // id=2, bob 的, 共享, 未归档 → (false OR true) AND true → 通过
            Value::Int64(2),
            Value::Text("bob".to_string()),
            Value::Text("true".to_string()),
            Value::Text("false".to_string()),
        ],
        vec![
            // id=3, alice 的, 不共享, 已归档 → (alice OR false) AND false → 拒绝
            Value::Int64(3),
            Value::Text("alice".to_string()),
            Value::Text("false".to_string()),
            Value::Text("true".to_string()),
        ],
        vec![
            // id=4, bob 的, 共享, 已归档 → (false OR true) AND false → 拒绝
            Value::Int64(4),
            Value::Text("bob".to_string()),
            Value::Text("true".to_string()),
            Value::Text("true".to_string()),
        ],
    ];

    let evaluator = PolicyEvaluator::new();
    let result = rbac_rls_select(
        &rows,
        &cols,
        &table("docs"),
        "alice",
        &["role_user".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false,
    );

    // alice 看到 id=1 (自己的, 未归档) + id=2 (共享的, 未归档)
    assert_eq!(result.len(), 2);
    let ids: HashSet<i64> = result
        .iter()
        .filter_map(|r| {
            if let Value::Int64(id) = r[0] {
                Some(id)
            } else {
                None
            }
        })
        .collect();
    assert!(ids.contains(&1));
    assert!(ids.contains(&2));
}

// =====================================================================
//  Phase 3.14 综合验收：多用户多角色多策略
// =====================================================================

#[test]
fn test_phase_3_14_comprehensive_multi_user_multi_role() {
    // 场景：
    // - alice (role_user): 有 SELECT 权限 + RLS (tenant_id = current_user)
    // - admin_user (role_admin, SUPERUSER): 有 SELECT 权限（SUPERUSER 覆盖）+ bypass_rls
    // - charlie (role_user): 有 SELECT 权限，但表数据中没有 charlie 的行
    // - 期望：
    //   - alice 看到自己的行
    //   - admin_user 看到全表（bypass_rls）
    //   - charlie 看不到任何行（RLS 过滤后无匹配）

    let mut rbac = RbacManager::new();
    let mut rls = RlsManager::new();

    // 用户与角色
    rbac.create_user("alice").unwrap();
    rbac.create_superuser("admin_user").unwrap();
    rbac.create_user("charlie").unwrap();
    rbac.create_role("role_user").unwrap();
    rbac.create_role("role_admin").unwrap();
    rbac.grant_role("role_user", "alice").unwrap();
    rbac.grant_role("role_admin", "admin_user").unwrap();
    rbac.grant_role("role_user", "charlie").unwrap();

    // GRANT SELECT ON t TO role_user, role_admin
    rbac.grant(Privilege::Select, db_table("t"), "role_user", false)
        .unwrap();
    rbac.grant(Privilege::Select, db_table("t"), "role_admin", false)
        .unwrap();

    // RLS 策略：tenant_id = current_user
    rls.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select("p", table("t"), col_eq_current_user("tenant_id"));
    rls.create_policy(p).unwrap();

    let rows = vec![
        tenant_row(1, "alice", "alice's data"),
        tenant_row(2, "bob", "bob's data"),
        tenant_row(3, "alice", "more alice data"),
    ];

    let evaluator = PolicyEvaluator::new();

    // alice (role_user) → 看到 2 行（id=1, 3）
    let alice_result = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &["role_user".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false,
    );
    assert_eq!(alice_result.len(), 2);

    // admin_user (SUPERUSER + bypass_rls) → 看到全表（3 行）
    let admin_result = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "admin_user",
        &["role_admin".to_string()],
        &rbac,
        &rls,
        &evaluator,
        true, // bypass_rls
    );
    assert_eq!(admin_result.len(), 3);

    // charlie (role_user) → 看到 0 行（无匹配数据）
    let charlie_result = rbac_rls_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "charlie",
        &["role_user".to_string()],
        &rbac,
        &rls,
        &evaluator,
        false,
    );
    assert_eq!(charlie_result.len(), 0);
}
