//! Phase 3.13 RLS 单元测试 — 行级安全策略验证。
//!
//! 覆盖：
//! - Policy / PolicyCommand 单元测试
//! - RlsManager 策略管理（创建/删除/启用/禁用/强制）
//! - PolicyEvaluator 行过滤（SELECT/INSERT/UPDATE/DELETE）
//! - 特殊变量 current_user / session_user / current_role
//! - PERMISSIVE vs RESTRICTIVE 组合
//! - 角色过滤
//! - **Phase 3.13 验收测试**：CREATE POLICY p ON t FOR SELECT USING (tenant_id = current_user)
//!   → user1 只能看到自己的行；user2 只能看到自己的行

use crate::rls::{Policy, PolicyCommand, PolicyEvaluator, RlsContext, RlsError, RlsManager};
use szrsql_sql::{
    ast::{BinaryOp, Expr, TableName},
    expr::RowContext,
};
use szrsql_types::value::Value;

// =====================================================================
//  辅助函数
// =====================================================================

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

/// 构造 `col1 = val1 AND col2 = val2` 表达式
fn and(left: Expr, right: Expr) -> Expr {
    Expr::BinaryOp {
        left: Box::new(left),
        op: BinaryOp::And,
        right: Box::new(right),
    }
}

/// 构造 `col1 = val1 OR col2 = val2` 表达式
fn or(left: Expr, right: Expr) -> Expr {
    Expr::BinaryOp {
        left: Box::new(left),
        op: BinaryOp::Or,
        right: Box::new(right),
    }
}

fn table(name: &str) -> TableName {
    TableName::new(name)
}

/// 构造测试行: [id, tenant_id, data]
fn tenant_row(id: i64, tenant: &str, data: &str) -> Vec<Value> {
    vec![
        Value::Int64(id),
        Value::Text(tenant.to_string()),
        Value::Text(data.to_string()),
    ]
}

/// 列名: ["id", "tenant_id", "data"]
fn tenant_columns() -> Vec<String> {
    vec![
        "id".to_string(),
        "tenant_id".to_string(),
        "data".to_string(),
    ]
}

// =====================================================================
//  PolicyCommand 单元测试
// =====================================================================

#[test]
fn test_policy_command_matches() {
    assert!(PolicyCommand::All.matches(PolicyCommand::Select));
    assert!(PolicyCommand::All.matches(PolicyCommand::Insert));
    assert!(PolicyCommand::All.matches(PolicyCommand::Update));
    assert!(PolicyCommand::All.matches(PolicyCommand::Delete));
    assert!(PolicyCommand::All.matches(PolicyCommand::All));

    assert!(PolicyCommand::Select.matches(PolicyCommand::Select));
    assert!(!PolicyCommand::Select.matches(PolicyCommand::Insert));
    assert!(!PolicyCommand::Select.matches(PolicyCommand::Update));
    assert!(!PolicyCommand::Select.matches(PolicyCommand::Delete));
}

// =====================================================================
//  Policy 构造与角色匹配
// =====================================================================

#[test]
fn test_policy_new_select() {
    let p = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    assert_eq!(p.name, "p1");
    assert_eq!(p.command, PolicyCommand::Select);
    assert!(p.permissive);
    assert!(p.using.is_some());
    assert!(p.with_check.is_none());
    assert!(p.roles.is_empty()); // PUBLIC
}

#[test]
fn test_policy_new_insert() {
    let p = Policy::new_insert("p1", table("t"), col_eq_current_user("owner"));
    assert_eq!(p.name, "p1");
    assert_eq!(p.command, PolicyCommand::Insert);
    assert!(p.permissive);
    assert!(p.with_check.is_some());
    assert!(p.using.is_none());
}

#[test]
fn test_policy_with_roles() {
    let p = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"))
        .with_roles(vec!["admin".to_string(), "manager".to_string()]);
    assert_eq!(p.roles, vec!["admin", "manager"]);
}

#[test]
fn test_policy_with_permissive() {
    let p_permissive = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    assert!(p_permissive.permissive);

    let p_restrictive = Policy::new_select("p2", table("t"), col_eq_current_user("tenant_id"))
        .with_permissive(false);
    assert!(!p_restrictive.permissive);
}

#[test]
fn test_policy_applies_to_roles_public() {
    // Empty roles = PUBLIC = applies to all users
    let p = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    assert!(p.applies_to_roles(&[])); // No roles
    assert!(p.applies_to_roles(&["user1".to_string()]));
    assert!(p.applies_to_roles(&["admin".to_string(), "user1".to_string()]));
}

#[test]
fn test_policy_applies_to_roles_specific() {
    let p = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"))
        .with_roles(vec!["admin".to_string(), "manager".to_string()]);
    assert!(!p.applies_to_roles(&[])); // No roles → no match
    assert!(p.applies_to_roles(&["admin".to_string()])); // admin matches
    assert!(p.applies_to_roles(&["manager".to_string(), "user1".to_string()])); // manager matches
    assert!(!p.applies_to_roles(&["user1".to_string()])); // Neither admin nor manager
}

#[test]
fn test_policy_applies_to_roles_case_insensitive() {
    let p = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"))
        .with_roles(vec!["Admin".to_string()]);
    assert!(p.applies_to_roles(&["admin".to_string()])); // Case insensitive
    assert!(p.applies_to_roles(&["ADMIN".to_string()]));
}

// =====================================================================
//  RlsManager — 启用/禁用/强制 RLS
// =====================================================================

#[test]
fn test_enable_rls_basic() {
    let mut mgr = RlsManager::new();
    let t = table("t1");
    assert!(!mgr.is_enabled(&t));
    assert!(mgr.enable_rls(&t).is_ok());
    assert!(mgr.is_enabled(&t));
}

#[test]
fn test_enable_rls_duplicate() {
    let mut mgr = RlsManager::new();
    let t = table("t1");
    mgr.enable_rls(&t).unwrap();
    let result = mgr.enable_rls(&t);
    assert!(matches!(result, Err(RlsError::AlreadyEnabled(_))));
}

#[test]
fn test_disable_rls_basic() {
    let mut mgr = RlsManager::new();
    let t = table("t1");
    mgr.enable_rls(&t).unwrap();
    assert!(mgr.disable_rls(&t).is_ok());
    assert!(!mgr.is_enabled(&t));
}

#[test]
fn test_disable_rls_not_enabled() {
    let mut mgr = RlsManager::new();
    let t = table("t1");
    let result = mgr.disable_rls(&t);
    assert!(matches!(result, Err(RlsError::NotEnabled(_))));
}

#[test]
fn test_disable_rls_also_removes_forced() {
    let mut mgr = RlsManager::new();
    let t = table("t1");
    mgr.enable_rls(&t).unwrap();
    mgr.force_rls(&t).unwrap();
    assert!(mgr.is_forced(&t));
    mgr.disable_rls(&t).unwrap();
    assert!(!mgr.is_forced(&t));
}

#[test]
fn test_force_rls_requires_enabled() {
    let mut mgr = RlsManager::new();
    let t = table("t1");
    let result = mgr.force_rls(&t);
    assert!(matches!(result, Err(RlsError::NotEnabled(_))));
}

#[test]
fn test_force_rls_basic() {
    let mut mgr = RlsManager::new();
    let t = table("t1");
    mgr.enable_rls(&t).unwrap();
    assert!(mgr.force_rls(&t).is_ok());
    assert!(mgr.is_forced(&t));
}

#[test]
fn test_unforce_rls() {
    let mut mgr = RlsManager::new();
    let t = table("t1");
    mgr.enable_rls(&t).unwrap();
    mgr.force_rls(&t).unwrap();
    assert!(mgr.unforce_rls(&t).is_ok());
    assert!(!mgr.is_forced(&t));
    // RLS still enabled
    assert!(mgr.is_enabled(&t));
}

// =====================================================================
//  RlsManager — CREATE/DROP POLICY
// =====================================================================

#[test]
fn test_create_policy_basic() {
    let mut mgr = RlsManager::new();
    let p = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    assert!(mgr.create_policy(p).is_ok());
    assert_eq!(mgr.policy_count(), 1);
}

#[test]
fn test_create_policy_duplicate_name() {
    let mut mgr = RlsManager::new();
    let p1 = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    let p2 = Policy::new_select("p1", table("t"), col_eq_current_user("owner"));
    mgr.create_policy(p1).unwrap();
    let result = mgr.create_policy(p2);
    assert!(matches!(result, Err(RlsError::PolicyAlreadyExists(_))));
}

#[test]
fn test_create_policy_duplicate_name_different_tables() {
    let mut mgr = RlsManager::new();
    let p1 = Policy::new_select("p1", table("t1"), col_eq_current_user("tenant_id"));
    let p2 = Policy::new_select("p1", table("t2"), col_eq_current_user("tenant_id"));
    assert!(mgr.create_policy(p1).is_ok());
    assert!(mgr.create_policy(p2).is_ok()); // Same name, different table → OK
    assert_eq!(mgr.policy_count(), 2);
}

#[test]
fn test_create_policy_case_insensitive_name() {
    let mut mgr = RlsManager::new();
    let p1 = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    let p2 = Policy::new_select("P1", table("t"), col_eq_current_user("owner"));
    mgr.create_policy(p1).unwrap();
    let result = mgr.create_policy(p2);
    assert!(matches!(result, Err(RlsError::PolicyAlreadyExists(_))));
}

#[test]
fn test_create_policy_invalid_select_no_using() {
    let mut mgr = RlsManager::new();
    let p = Policy {
        name: "p1".to_string(),
        table: table("t"),
        command: PolicyCommand::Select,
        roles: Vec::new(),
        using: None,
        with_check: None,
        permissive: true,
    };
    let result = mgr.create_policy(p);
    assert!(matches!(result, Err(RlsError::InvalidPolicy(_))));
}

#[test]
fn test_create_policy_invalid_insert_no_with_check() {
    let mut mgr = RlsManager::new();
    let p = Policy {
        name: "p1".to_string(),
        table: table("t"),
        command: PolicyCommand::Insert,
        roles: Vec::new(),
        using: None,
        with_check: None,
        permissive: true,
    };
    let result = mgr.create_policy(p);
    assert!(matches!(result, Err(RlsError::InvalidPolicy(_))));
}

#[test]
fn test_drop_policy_basic() {
    let mut mgr = RlsManager::new();
    let p = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    mgr.create_policy(p).unwrap();
    assert_eq!(mgr.policy_count(), 1);
    assert!(mgr.drop_policy(&table("t"), "p1").is_ok());
    assert_eq!(mgr.policy_count(), 0);
}

#[test]
fn test_drop_policy_not_found() {
    let mut mgr = RlsManager::new();
    let result = mgr.drop_policy(&table("t"), "nonexistent");
    assert!(matches!(result, Err(RlsError::PolicyNotFound(_))));
}

#[test]
fn test_drop_policy_case_insensitive() {
    let mut mgr = RlsManager::new();
    let p = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    mgr.create_policy(p).unwrap();
    assert!(mgr.drop_policy(&table("t"), "P1").is_ok());
}

#[test]
fn test_policies_for_table() {
    let mut mgr = RlsManager::new();
    let p1 = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    let p2 = Policy::new_select("p2", table("t"), col_eq_text("owner", "admin"));
    let p3 = Policy::new_select("p3", table("other"), col_eq_current_user("tenant_id"));
    mgr.create_policy(p1).unwrap();
    mgr.create_policy(p2).unwrap();
    mgr.create_policy(p3).unwrap();

    assert_eq!(mgr.policy_count_for_table(&table("t")), 2);
    assert_eq!(mgr.policy_count_for_table(&table("other")), 1);
    assert_eq!(mgr.policy_count_for_table(&table("nonexistent")), 0);
}

#[test]
fn test_matching_policies_command_filter() {
    let mut mgr = RlsManager::new();
    let p_select = Policy::new_select("p_sel", table("t"), col_eq_current_user("tenant_id"));
    let p_insert = Policy::new_insert("p_ins", table("t"), col_eq_current_user("owner"));
    mgr.create_policy(p_select).unwrap();
    mgr.create_policy(p_insert).unwrap();

    let select_matches = mgr.matching_policies(&table("t"), PolicyCommand::Select, &[]);
    assert_eq!(select_matches.len(), 1);
    assert_eq!(select_matches[0].name, "p_sel");

    let insert_matches = mgr.matching_policies(&table("t"), PolicyCommand::Insert, &[]);
    assert_eq!(insert_matches.len(), 1);
    assert_eq!(insert_matches[0].name, "p_ins");
}

#[test]
fn test_matching_policies_all_command() {
    let mut mgr = RlsManager::new();
    // ALL command policy matches all commands
    let p_all = Policy {
        name: "p_all".to_string(),
        table: table("t"),
        command: PolicyCommand::All,
        roles: Vec::new(),
        using: Some(col_eq_current_user("tenant_id")),
        with_check: None,
        permissive: true,
    };
    mgr.create_policy(p_all).unwrap();

    assert_eq!(
        mgr.matching_policies(&table("t"), PolicyCommand::Select, &[])
            .len(),
        1
    );
    assert_eq!(
        mgr.matching_policies(&table("t"), PolicyCommand::Insert, &[])
            .len(),
        1
    );
    assert_eq!(
        mgr.matching_policies(&table("t"), PolicyCommand::Update, &[])
            .len(),
        1
    );
    assert_eq!(
        mgr.matching_policies(&table("t"), PolicyCommand::Delete, &[])
            .len(),
        1
    );
}

#[test]
fn test_matching_policies_role_filter() {
    let mut mgr = RlsManager::new();
    let p_public = Policy::new_select("p_pub", table("t"), col_eq_current_user("tenant_id"));
    let p_admin = Policy::new_select("p_adm", table("t"), col_eq_text("owner", "admin"))
        .with_roles(vec!["admin".to_string()]);
    mgr.create_policy(p_public).unwrap();
    mgr.create_policy(p_admin).unwrap();

    // user with no special roles → only PUBLIC policy
    let user_matches = mgr.matching_policies(&table("t"), PolicyCommand::Select, &[]);
    assert_eq!(user_matches.len(), 1);
    assert_eq!(user_matches[0].name, "p_pub");

    // admin user → both PUBLIC and admin policies
    let admin_matches =
        mgr.matching_policies(&table("t"), PolicyCommand::Select, &["admin".to_string()]);
    assert_eq!(admin_matches.len(), 2);
}

// =====================================================================
//  RlsContext — 特殊变量解析
// =====================================================================

#[test]
fn test_rls_context_resolves_current_user() {
    use szrsql_sql::expr::EvalContext;
    let row_ctx = RowContext::new();
    let rls_ctx = RlsContext::new(&row_ctx, "alice");
    assert_eq!(
        rls_ctx.lookup_column("current_user").unwrap(),
        Value::Text("alice".to_string())
    );
    assert_eq!(
        rls_ctx.lookup_column("user").unwrap(),
        Value::Text("alice".to_string())
    );
}

#[test]
fn test_rls_context_resolves_session_user() {
    use szrsql_sql::expr::EvalContext;
    let row_ctx = RowContext::new();
    let rls_ctx = RlsContext::new(&row_ctx, "alice").with_session_user("root");
    assert_eq!(
        rls_ctx.lookup_column("session_user").unwrap(),
        Value::Text("root".to_string())
    );
    assert_eq!(
        rls_ctx.lookup_column("current_user").unwrap(),
        Value::Text("alice".to_string())
    );
}

#[test]
fn test_rls_context_resolves_current_role() {
    use szrsql_sql::expr::EvalContext;
    let row_ctx = RowContext::new();
    let rls_ctx = RlsContext::new(&row_ctx, "alice").with_current_role("admin");
    assert_eq!(
        rls_ctx.lookup_column("current_role").unwrap(),
        Value::Text("admin".to_string())
    );
}

#[test]
fn test_rls_context_falls_back_to_row() {
    use szrsql_sql::expr::EvalContext;
    let row_ctx = RowContext::new().with("tenant_id", Value::Text("alice".to_string()));
    let rls_ctx = RlsContext::new(&row_ctx, "alice");
    assert_eq!(
        rls_ctx.lookup_column("tenant_id").unwrap(),
        Value::Text("alice".to_string())
    );
}

#[test]
fn test_rls_context_case_insensitive_special_vars() {
    use szrsql_sql::expr::EvalContext;
    let row_ctx = RowContext::new();
    let rls_ctx = RlsContext::new(&row_ctx, "alice");
    assert_eq!(
        rls_ctx.lookup_column("CURRENT_USER").unwrap(),
        Value::Text("alice".to_string())
    );
    assert_eq!(
        rls_ctx.lookup_column("Session_User").unwrap(),
        Value::Text("alice".to_string())
    );
}

// =====================================================================
//  PolicyEvaluator — SELECT 过滤（核心验收）
// =====================================================================

#[test]
fn test_filter_select_rls_not_enabled_returns_all() {
    // RLS 未启用 → 返回所有行（无过滤）
    let mgr = RlsManager::new();
    let evaluator = PolicyEvaluator::new();
    let rows = vec![
        tenant_row(1, "alice", "data1"),
        tenant_row(2, "bob", "data2"),
    ];
    let result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false,
    );
    assert_eq!(result.len(), 2); // No filtering
}

#[test]
fn test_filter_select_rls_enabled_no_policies_denies_all() {
    // RLS 启用但无策略 → 拒绝所有（deny-all default）
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let evaluator = PolicyEvaluator::new();
    let rows = vec![tenant_row(1, "alice", "data1")];
    let result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false,
    );
    assert_eq!(result.len(), 0); // Deny all
}

#[test]
fn test_filter_select_bypass_rls_returns_all() {
    // bypass_rls=true (表所有者) → 返回所有行
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![
        tenant_row(1, "alice", "data1"),
        tenant_row(2, "bob", "data2"),
    ];
    let result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        true, // bypass_rls
    );
    assert_eq!(result.len(), 2); // Owner bypasses RLS
}

#[test]
fn test_filter_select_force_rls_overrides_bypass() {
    // FORCE RLS → 即使 bypass_rls=true 也强制过滤
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    mgr.force_rls(&table("t")).unwrap();
    let p = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![
        tenant_row(1, "alice", "data1"),
        tenant_row(2, "bob", "data2"),
    ];
    let result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        true, // bypass_rls=true but FORCE overrides
    );
    assert_eq!(result.len(), 1); // Force RLS applies even with bypass
    assert_eq!(result[0][1], Value::Text("alice".to_string()));
}

#[test]
fn test_filter_select_policy_filters_rows() {
    // 基本过滤：policy = `tenant_id = current_user`
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![
        tenant_row(1, "alice", "data1"),
        tenant_row(2, "bob", "data2"),
        tenant_row(3, "alice", "data3"),
        tenant_row(4, "carol", "data4"),
    ];

    // alice 只能看到 alice 的行
    let alice_result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false,
    );
    assert_eq!(alice_result.len(), 2);
    assert_eq!(alice_result[0][1], Value::Text("alice".to_string()));
    assert_eq!(alice_result[1][1], Value::Text("alice".to_string()));

    // bob 只能看到 bob 的行
    let bob_result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "bob",
        &[],
        &mgr,
        false,
    );
    assert_eq!(bob_result.len(), 1);
    assert_eq!(bob_result[0][1], Value::Text("bob".to_string()));
}

#[test]
fn test_filter_select_policy_literal_value() {
    // Policy 使用字面量值而非 current_user
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select("p1", table("t"), col_eq_text("tenant_id", "alice"));
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![
        tenant_row(1, "alice", "data1"),
        tenant_row(2, "bob", "data2"),
    ];

    // Any user — policy always filters to alice's rows
    let result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "carol",
        &[],
        &mgr,
        false,
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][1], Value::Text("alice".to_string()));
}

#[test]
fn test_filter_select_no_matching_rows() {
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![
        tenant_row(1, "alice", "data1"),
        tenant_row(2, "bob", "data2"),
    ];

    // dave has no matching rows
    let result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "dave",
        &[],
        &mgr,
        false,
    );
    assert_eq!(result.len(), 0);
}

#[test]
fn test_filter_select_empty_rows() {
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows: Vec<Vec<Value>> = vec![];
    let result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false,
    );
    assert_eq!(result.len(), 0);
}

// =====================================================================
//  Phase 3.13 验收测试 — CREATE POLICY p ON t FOR SELECT USING (tenant_id = current_user)
//  → user1 只能看到自己的行；user2 只能看到自己的行
// =====================================================================

#[test]
fn test_phase_3_13_acceptance_user1_sees_only_own_rows() {
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    // CREATE POLICY p ON t FOR SELECT USING (tenant_id = current_user)
    let policy = Policy::new_select("p", table("t"), col_eq_current_user("tenant_id"));
    mgr.create_policy(policy).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![
        tenant_row(1, "user1", "row1_of_user1"),
        tenant_row(2, "user1", "row2_of_user1"),
        tenant_row(3, "user2", "row1_of_user2"),
        tenant_row(4, "user2", "row2_of_user2"),
        tenant_row(5, "user1", "row3_of_user1"),
    ];

    let user1_visible = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "user1",
        &[],
        &mgr,
        false,
    );
    assert_eq!(user1_visible.len(), 3); // user1 sees 3 rows
    for row in &user1_visible {
        assert_eq!(row[1], Value::Text("user1".to_string()));
    }

    let user2_visible = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "user2",
        &[],
        &mgr,
        false,
    );
    assert_eq!(user2_visible.len(), 2); // user2 sees 2 rows
    for row in &user2_visible {
        assert_eq!(row[1], Value::Text("user2".to_string()));
    }

    // 验证用户 1 和用户 2 看到的行不重叠
    let user1_ids: HashSet<i64> = user1_visible
        .iter()
        .filter_map(|r| {
            if let Value::Int64(id) = r[0] {
                Some(id)
            } else {
                None
            }
        })
        .collect();
    let user2_ids: HashSet<i64> = user2_visible
        .iter()
        .filter_map(|r| {
            if let Value::Int64(id) = r[0] {
                Some(id)
            } else {
                None
            }
        })
        .collect();
    let intersection: HashSet<_> = user1_ids.intersection(&user2_ids).cloned().collect();
    assert!(
        intersection.is_empty(),
        "user1 and user2 visible rows must not overlap"
    );
}

#[test]
fn test_phase_3_13_acceptance_user_with_no_rows_sees_nothing() {
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let policy = Policy::new_select("p", table("t"), col_eq_current_user("tenant_id"));
    mgr.create_policy(policy).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![
        tenant_row(1, "user1", "row1"),
        tenant_row(2, "user2", "row2"),
    ];

    // user3 has no rows → sees nothing
    let user3_visible = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "user3",
        &[],
        &mgr,
        false,
    );
    assert_eq!(user3_visible.len(), 0);
}

// =====================================================================
//  PolicyEvaluator — INSERT WITH CHECK
// =====================================================================

#[test]
fn test_check_insert_basic_pass() {
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy::new_insert("p_ins", table("t"), col_eq_current_user("owner"));
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let cols = vec!["id".to_string(), "owner".to_string(), "data".to_string()];
    let row = vec![
        Value::Int64(1),
        Value::Text("alice".to_string()),
        Value::Text("data1".to_string()),
    ];
    assert!(evaluator.check_insert(&row, &cols, &table("t"), "alice", &[], &mgr, false));
}

#[test]
fn test_check_insert_basic_fail() {
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy::new_insert("p_ins", table("t"), col_eq_current_user("owner"));
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let cols = vec!["id".to_string(), "owner".to_string(), "data".to_string()];
    let row = vec![
        Value::Int64(1),
        Value::Text("bob".to_string()), // owner=bob but current_user=alice → fail
        Value::Text("data1".to_string()),
    ];
    assert!(!evaluator.check_insert(&row, &cols, &table("t"), "alice", &[], &mgr, false));
}

#[test]
fn test_check_insert_rls_not_enabled_always_pass() {
    let mgr = RlsManager::new();
    let evaluator = PolicyEvaluator::new();
    let row = vec![Value::Int64(1)];
    assert!(evaluator.check_insert(
        &row,
        &["id".to_string()],
        &table("t"),
        "alice",
        &[],
        &mgr,
        false
    ));
}

#[test]
fn test_check_insert_bypass_rls() {
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy::new_insert("p_ins", table("t"), col_eq_current_user("owner"));
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let row = vec![
        Value::Int64(1),
        Value::Text("bob".to_string()),
        Value::Text("data".to_string()),
    ];
    // bypass_rls=true → always pass
    assert!(evaluator.check_insert(
        &row,
        &["id".to_string(), "owner".to_string(), "data".to_string()],
        &table("t"),
        "alice",
        &[],
        &mgr,
        true
    ));
}

// =====================================================================
//  PolicyEvaluator — DELETE USING
// =====================================================================

#[test]
fn test_check_delete_basic_pass() {
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select("p_del", table("t"), col_eq_current_user("tenant_id"));
    // Note: DELETE policy should use PolicyCommand::Delete, but for simplicity
    // we use Select here — in real PG, you'd create a separate DELETE policy.
    // For this test, we'll create a proper DELETE policy.
    let _ = p;
    mgr.drop_policy(&table("t"), "p_del").ok();
    let delete_policy = Policy {
        name: "p_del".to_string(),
        table: table("t"),
        command: PolicyCommand::Delete,
        roles: Vec::new(),
        using: Some(col_eq_current_user("tenant_id")),
        with_check: None,
        permissive: true,
    };
    mgr.create_policy(delete_policy).unwrap();

    let evaluator = PolicyEvaluator::new();
    let row = tenant_row(1, "alice", "data1");
    assert!(evaluator.check_delete(
        &row,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false
    ));
}

#[test]
fn test_check_delete_basic_fail() {
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy {
        name: "p_del".to_string(),
        table: table("t"),
        command: PolicyCommand::Delete,
        roles: Vec::new(),
        using: Some(col_eq_current_user("tenant_id")),
        with_check: None,
        permissive: true,
    };
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let row = tenant_row(1, "alice", "data1");
    // bob cannot delete alice's row
    assert!(!evaluator.check_delete(
        &row,
        &tenant_columns(),
        &table("t"),
        "bob",
        &[],
        &mgr,
        false
    ));
}

// =====================================================================
//  PolicyEvaluator — UPDATE USING + WITH CHECK
// =====================================================================

#[test]
fn test_check_update_basic_pass() {
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    // UPDATE policy with both USING (old row) and WITH CHECK (new row)
    let p = Policy {
        name: "p_upd".to_string(),
        table: table("t"),
        command: PolicyCommand::Update,
        roles: Vec::new(),
        using: Some(col_eq_current_user("tenant_id")),
        with_check: Some(col_eq_current_user("tenant_id")),
        permissive: true,
    };
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let old_row = tenant_row(1, "alice", "old_data");
    let new_row = tenant_row(1, "alice", "new_data"); // Still alice's row
    assert!(evaluator.check_update(
        &old_row,
        &new_row,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false
    ));
}

#[test]
fn test_check_update_fail_using() {
    // Old row doesn't satisfy USING (user can't update others' rows)
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy {
        name: "p_upd".to_string(),
        table: table("t"),
        command: PolicyCommand::Update,
        roles: Vec::new(),
        using: Some(col_eq_current_user("tenant_id")),
        with_check: Some(col_eq_current_user("tenant_id")),
        permissive: true,
    };
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let old_row = tenant_row(1, "alice", "old_data");
    let new_row = tenant_row(1, "alice", "new_data");
    // bob cannot update alice's row (USING fails on old row)
    assert!(!evaluator.check_update(
        &old_row,
        &new_row,
        &tenant_columns(),
        &table("t"),
        "bob",
        &[],
        &mgr,
        false
    ));
}

#[test]
fn test_check_update_fail_with_check() {
    // New row doesn't satisfy WITH CHECK (can't reassign to another user)
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy {
        name: "p_upd".to_string(),
        table: table("t"),
        command: PolicyCommand::Update,
        roles: Vec::new(),
        using: Some(col_eq_current_user("tenant_id")),
        with_check: Some(col_eq_current_user("tenant_id")),
        permissive: true,
    };
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let old_row = tenant_row(1, "alice", "old_data");
    let new_row = tenant_row(1, "bob", "new_data"); // Trying to reassign to bob
                                                    // alice's USING passes, but WITH CHECK fails (new tenant_id != current_user=alice)
    assert!(!evaluator.check_update(
        &old_row,
        &new_row,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false
    ));
}

// =====================================================================
//  PERMISSIVE vs RESTRICTIVE 组合
// =====================================================================

#[test]
fn test_permissive_policies_combine_with_or() {
    // Two permissive policies → OR (any passing is enough)
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    // p1: tenant_id = current_user
    let p1 = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    // p2: tenant_id = 'public' (always matches 'public' rows)
    let p2 = Policy::new_select("p2", table("t"), col_eq_text("tenant_id", "public"));
    mgr.create_policy(p1).unwrap();
    mgr.create_policy(p2).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![
        tenant_row(1, "alice", "alice's data"),
        tenant_row(2, "public", "public data"),
        tenant_row(3, "bob", "bob's data"),
    ];

    // alice sees her row + public row (OR combination)
    let result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false,
    );
    assert_eq!(result.len(), 2);
}

#[test]
fn test_restrictive_policies_combine_with_and() {
    // Restrictive policies combine with AND, but require at least one permissive policy
    // (PostgreSQL: no permissive policies → deny-all default)
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    // p_base (permissive): always true — grants base access to all rows
    let p_base = Policy::new_select("p_base", table("t"), Expr::Literal(Value::Bool(true)));
    // r1 (restrictive): tenant_id = current_user
    let r1 = Policy::new_select("r1", table("t"), col_eq_current_user("tenant_id"))
        .with_permissive(false);
    // r2 (restrictive): id = 2
    let r2 = Policy::new_select("r2", table("t"), col_eq_int("id", 2)).with_permissive(false);
    mgr.create_policy(p_base).unwrap();
    mgr.create_policy(r1).unwrap();
    mgr.create_policy(r2).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![
        tenant_row(1, "alice", "row1"), // id=1, tenant=alice → r1 pass, r2 fail
        tenant_row(2, "alice", "row2"), // id=2, tenant=alice → both pass
        tenant_row(2, "bob", "row2_bob"), // id=2, tenant=bob → r1 fail for alice
    ];

    let result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false,
    );
    // p_base (permissive, always true) AND r1 (restrictive, tenant=alice) AND r2 (restrictive, id=2)
    // → only row2 of alice
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(2));
    assert_eq!(result[0][1], Value::Text("alice".to_string()));
}

#[test]
fn test_permissive_and_restrictive_combine() {
    // PERMISSIVE (OR) + RESTRICTIVE (AND) → final = permissive_pass && restrictive_pass
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    // p1 (permissive): tenant_id = current_user
    let p1 = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    // r1 (restrictive): id = 2
    let r1 = Policy::new_select("r1", table("t"), col_eq_int("id", 2)).with_permissive(false);
    mgr.create_policy(p1).unwrap();
    mgr.create_policy(r1).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![
        tenant_row(1, "alice", "row1"),   // p1 passes, r1 fails
        tenant_row(2, "alice", "row2"),   // p1 passes, r1 passes
        tenant_row(2, "bob", "row2_bob"), // p1 fails for alice, r1 passes
    ];

    let result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false,
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(2));
    assert_eq!(result[0][1], Value::Text("alice".to_string()));
}

#[test]
fn test_only_restrictive_no_permissive_denies_all() {
    // No permissive policies → deny all (even if restrictive passes)
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let r1 = Policy::new_select("r1", table("t"), col_eq_current_user("tenant_id"))
        .with_permissive(false);
    mgr.create_policy(r1).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![tenant_row(1, "alice", "data1")];

    let result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false,
    );
    assert_eq!(result.len(), 0); // No permissive → deny all
}

// =====================================================================
//  角色过滤
// =====================================================================

#[test]
fn test_role_specific_policy() {
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    // admin policy: admin can see all rows (1=1, always true)
    let admin_policy = Policy {
        name: "p_admin".to_string(),
        table: table("t"),
        command: PolicyCommand::Select,
        roles: vec!["admin".to_string()],
        using: Some(Expr::Literal(Value::Bool(true))),
        with_check: None,
        permissive: true,
    };
    // user policy: tenant_id = current_user (for non-admin)
    let user_policy = Policy::new_select("p_user", table("t"), col_eq_current_user("tenant_id"));
    mgr.create_policy(admin_policy).unwrap();
    mgr.create_policy(user_policy).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![
        tenant_row(1, "alice", "alice_data"),
        tenant_row(2, "bob", "bob_data"),
        tenant_row(3, "carol", "carol_data"),
    ];

    // admin sees all rows
    let admin_result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "admin_user",
        &["admin".to_string()],
        &mgr,
        false,
    );
    assert_eq!(admin_result.len(), 3);

    // regular user (alice) sees only her rows
    let alice_result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &["user".to_string()],
        &mgr,
        false,
    );
    assert_eq!(alice_result.len(), 1);
    assert_eq!(alice_result[0][1], Value::Text("alice".to_string()));
}

// =====================================================================
//  综合场景
// =====================================================================

#[test]
fn test_comprehensive_rls_scenario() {
    // 综合场景：多用户、多角色、多策略组合
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("documents")).unwrap();

    // 策略1 (permissive, PUBLIC): owner = current_user OR shared = true
    let p_owner = Policy::new_select(
        "p_owner",
        table("documents"),
        or(col_eq_current_user("owner"), col_eq_text("shared", "true")),
    );
    mgr.create_policy(p_owner).unwrap();

    // 策略2 (restrictive, PUBLIC): NOT archived (archived = 'false')
    let r_not_archived = Policy::new_select(
        "r_not_archived",
        table("documents"),
        col_eq_text("archived", "false"),
    )
    .with_permissive(false);
    mgr.create_policy(r_not_archived).unwrap();

    let evaluator = PolicyEvaluator::new();
    let cols = vec![
        "id".to_string(),
        "owner".to_string(),
        "shared".to_string(),
        "archived".to_string(),
    ];
    let rows = vec![
        vec![
            // id=1, owner=alice, shared=false, archived=false → alice sees (owner match, not archived)
            Value::Int64(1),
            Value::Text("alice".to_string()),
            Value::Text("false".to_string()),
            Value::Text("false".to_string()),
        ],
        vec![
            // id=2, owner=bob, shared=true, archived=false → alice sees (shared, not archived)
            Value::Int64(2),
            Value::Text("bob".to_string()),
            Value::Text("true".to_string()),
            Value::Text("false".to_string()),
        ],
        vec![
            // id=3, owner=alice, shared=false, archived=true → alice doesn't see (archived)
            Value::Int64(3),
            Value::Text("alice".to_string()),
            Value::Text("false".to_string()),
            Value::Text("true".to_string()),
        ],
        vec![
            // id=4, owner=bob, shared=true, archived=true → alice doesn't see (archived)
            Value::Int64(4),
            Value::Text("bob".to_string()),
            Value::Text("true".to_string()),
            Value::Text("true".to_string()),
        ],
    ];

    let alice_result =
        evaluator.filter_select(&rows, &cols, &table("documents"), "alice", &[], &mgr, false);
    // Alice should see: row 1 (owner, not archived) + row 2 (shared, not archived) = 2 rows
    assert_eq!(alice_result.len(), 2);
    let alice_ids: HashSet<i64> = alice_result
        .iter()
        .filter_map(|r| {
            if let Value::Int64(id) = r[0] {
                Some(id)
            } else {
                None
            }
        })
        .collect();
    assert!(alice_ids.contains(&1));
    assert!(alice_ids.contains(&2));
    assert!(!alice_ids.contains(&3));
    assert!(!alice_ids.contains(&4));
}

#[test]
fn test_drop_policy_removes_filtering() {
    // Drop policy → no policies → deny-all (if RLS still enabled)
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select("p1", table("t"), col_eq_current_user("tenant_id"));
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![tenant_row(1, "alice", "data1")];

    // Initially: alice sees her row
    let before_drop = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false,
    );
    assert_eq!(before_drop.len(), 1);

    // Drop policy → deny all
    mgr.drop_policy(&table("t"), "p1").unwrap();
    let after_drop = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false,
    );
    assert_eq!(after_drop.len(), 0);

    // Disable RLS → no filtering
    mgr.disable_rls(&table("t")).unwrap();
    let after_disable = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false,
    );
    assert_eq!(after_disable.len(), 1);
}

#[test]
fn test_multiple_tables_isolated_policies() {
    // Different tables have independent RLS configurations
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t1")).unwrap();
    // t2 does NOT have RLS enabled

    let p1 = Policy::new_select("p1", table("t1"), col_eq_current_user("tenant_id"));
    mgr.create_policy(p1).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![tenant_row(1, "alice", "data1")];

    // t1 has RLS → alice sees her row
    let t1_result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t1"),
        "alice",
        &[],
        &mgr,
        false,
    );
    assert_eq!(t1_result.len(), 1);

    // t2 has no RLS → all rows returned (no filtering)
    let t2_result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t2"),
        "alice",
        &[],
        &mgr,
        false,
    );
    assert_eq!(t2_result.len(), 1); // No filtering (RLS not enabled)
}

// =====================================================================
//  复杂表达式策略
// =====================================================================

#[test]
fn test_complex_policy_and_expression() {
    // Policy: tenant_id = current_user AND id >= 2
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select(
        "p1",
        table("t"),
        and(
            col_eq_current_user("tenant_id"),
            col_eq_int("id", 2), // Simplified: id = 2 (we don't have >= in helper)
        ),
    );
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![
        tenant_row(1, "alice", "row1"),   // id=1 → fails (id != 2)
        tenant_row(2, "alice", "row2"),   // id=2 → passes
        tenant_row(2, "bob", "row2_bob"), // id=2 but tenant=bob → fails for alice
    ];

    let result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false,
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(2));
    assert_eq!(result[0][1], Value::Text("alice".to_string()));
}

#[test]
fn test_complex_policy_or_expression() {
    // Policy: tenant_id = current_user OR id = 999 (special public row)
    let mut mgr = RlsManager::new();
    mgr.enable_rls(&table("t")).unwrap();
    let p = Policy::new_select(
        "p1",
        table("t"),
        or(col_eq_current_user("tenant_id"), col_eq_int("id", 999)),
    );
    mgr.create_policy(p).unwrap();

    let evaluator = PolicyEvaluator::new();
    let rows = vec![
        tenant_row(1, "alice", "alice_row"), // tenant=alice → matches for alice
        tenant_row(999, "system", "public_row"), // id=999 → always matches
        tenant_row(2, "bob", "bob_row"),     // no match for alice
    ];

    let result = evaluator.filter_select(
        &rows,
        &tenant_columns(),
        &table("t"),
        "alice",
        &[],
        &mgr,
        false,
    );
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
    assert!(ids.contains(&999));
}

// 引入 HashSet 用于交集测试
use std::collections::HashSet;
