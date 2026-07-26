//! Phase 3.9 multi-tenant SqlRewriter tests — AST-level table name prefix rewriting.
//!
//! Coverage:
//! - TenantContext basics (3): new / with_prefix / is_exempt
//! - System table exemption (2): pg_tables / information_schema.*
//! - Basic SELECT rewrite (2): FROM table / FROM schema.table
//! - JOIN rewrite (3): INNER / LEFT / 3-table chain
//! - Subquery rewrite (3): scalar subquery / IN subquery / EXISTS subquery
//! - Nested subquery (1): subquery inside subquery
//! - DML rewrite (3): INSERT / UPDATE / DELETE
//! - DDL rewrite (4): CREATE TABLE / DROP TABLE / CREATE INDEX / DROP INDEX
//! - Foreign key reference (1): FK target table rewritten
//! - String literal safety (1): "users" in literal not rewritten
//! - Column name safety (1): column named "users" not rewritten
//! - Custom exempt (1): user-added exempt table
//!
//! 25 test cases.

use super::multitenant::{RewriteError, SqlRewriter, TenantContext};
use szrsql_sql::ast::{
    Expr, InsertSource, JoinType, Select, SelectItem, Statement, TableConstraint, TableFactor,
    TableName,
};
use szrsql_sql::parser::parse_sql;
use szrsql_types::value::Value;

// =====================================================================
//  Helpers
// =====================================================================

fn parse_one(sql: &str) -> Statement {
    let stmts = parse_sql(sql).expect("parse failed");
    assert_eq!(stmts.len(), 1, "expected exactly 1 statement");
    stmts.into_iter().next().unwrap()
}

fn rewrite(sql: &str, tenant: &TenantContext) -> Statement {
    let rewriter = SqlRewriter::new(tenant.clone());
    let stmt = parse_one(sql);
    rewriter.rewrite_statement(stmt).expect("rewrite failed")
}

fn extract_select(stmt: &Statement) -> &Select {
    match stmt {
        Statement::Select(s) => s.as_ref(),
        _ => panic!("expected Select statement"),
    }
}

fn first_table_name(select: &Select) -> TableName {
    match &select.from[0].relation {
        TableFactor::Table { name, .. } => name.clone(),
        _ => panic!("expected Table factor"),
    }
}

// =====================================================================
//  TenantContext basics (3)
// =====================================================================

#[test]
fn test_tenant_context_new_default_prefix() {
    let ctx = TenantContext::new("tenant_001");
    assert_eq!(ctx.tenant_id, "tenant_001");
    assert_eq!(ctx.prefix, "tenant_001_");
    assert!(!ctx.exempt.is_empty());
}

#[test]
fn test_tenant_context_custom_prefix() {
    let ctx = TenantContext::with_prefix("acme", "tenant_");
    assert_eq!(ctx.tenant_id, "acme");
    assert_eq!(ctx.prefix, "tenant_");
}

#[test]
fn test_tenant_context_exempt_builder() {
    let ctx = TenantContext::new("t1").with_exempt("my_sys_table");
    assert!(ctx.is_exempt(&TableName::new("my_sys_table")));
    assert!(ctx.is_exempt(&TableName::new("pg_tables"))); // still in default exempt
    assert!(!ctx.is_exempt(&TableName::new("users")));
}

// =====================================================================
//  System table exemption (2)
// =====================================================================

#[test]
fn test_exempt_pg_tables_not_rewritten() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite("SELECT * FROM pg_tables", &tenant);
    let select = extract_select(&stmt);
    assert_eq!(first_table_name(select).name, "pg_tables");
}

#[test]
fn test_exempt_information_schema_not_rewritten() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite(
        "SELECT * FROM information_schema.tables WHERE table_name = 'users'",
        &tenant,
    );
    let select = extract_select(&stmt);
    match &select.from[0].relation {
        TableFactor::Table { name, .. } => {
            assert_eq!(name.schema.as_deref(), Some("information_schema"));
            assert_eq!(name.name, "tables");
        }
        _ => panic!("expected Table factor"),
    }
}

// =====================================================================
//  Basic SELECT rewrite (2)
// =====================================================================

#[test]
fn test_rewrite_simple_select() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite("SELECT id, name FROM users", &tenant);
    let select = extract_select(&stmt);
    assert_eq!(first_table_name(select).name, "t1_users");
}

#[test]
fn test_rewrite_qualified_table_name() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite("SELECT * FROM public.users", &tenant);
    let select = extract_select(&stmt);
    let name = first_table_name(select);
    assert_eq!(name.schema.as_deref(), Some("public"));
    assert_eq!(name.name, "t1_users");
}

// =====================================================================
//  JOIN rewrite (3)
// =====================================================================

#[test]
fn test_rewrite_inner_join() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite(
        "SELECT u.id, d.name FROM users u INNER JOIN depts d ON u.dept_id = d.id",
        &tenant,
    );
    let select = extract_select(&stmt);
    assert_eq!(select.from[0].joins.len(), 1);
    match &select.from[0].relation {
        TableFactor::Table { name, .. } => assert_eq!(name.name, "t1_users"),
        _ => panic!(),
    }
    match &select.from[0].joins[0].relation {
        TableFactor::Table { name, .. } => assert_eq!(name.name, "t1_depts"),
        _ => panic!(),
    }
}

#[test]
fn test_rewrite_left_join() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite(
        "SELECT * FROM users LEFT JOIN orders ON users.id = orders.user_id",
        &tenant,
    );
    let select = extract_select(&stmt);
    assert_eq!(select.from[0].joins[0].join_type, JoinType::LeftOuter);
    match &select.from[0].joins[0].relation {
        TableFactor::Table { name, .. } => assert_eq!(name.name, "t1_orders"),
        _ => panic!(),
    }
}

#[test]
fn test_rewrite_3_table_chain_join() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite(
        "SELECT * FROM a JOIN b ON a.id = b.a_id JOIN c ON b.id = c.b_id",
        &tenant,
    );
    let select = extract_select(&stmt);
    assert_eq!(select.from[0].joins.len(), 2);
    // All 3 tables should be prefixed
    match &select.from[0].relation {
        TableFactor::Table { name, .. } => assert_eq!(name.name, "t1_a"),
        _ => panic!(),
    }
    match &select.from[0].joins[0].relation {
        TableFactor::Table { name, .. } => assert_eq!(name.name, "t1_b"),
        _ => panic!(),
    }
    match &select.from[0].joins[1].relation {
        TableFactor::Table { name, .. } => assert_eq!(name.name, "t1_c"),
        _ => panic!(),
    }
}

// =====================================================================
//  Subquery rewrite (3)
// =====================================================================

#[test]
fn test_rewrite_scalar_subquery() {
    let tenant = TenantContext::new("t1");
    // SELECT (SELECT MAX(id) FROM logs) AS max_id FROM users
    let stmt = rewrite("SELECT (SELECT MAX(id) FROM logs) FROM users", &tenant);
    let select = extract_select(&stmt);
    // Check outer query FROM users → t1_users
    assert_eq!(first_table_name(select).name, "t1_users");
    // Check inner subquery FROM logs → t1_logs
    if let SelectItem::UnnamedExpr(Expr::Subquery(inner)) = &select.projection[0] {
        assert_eq!(first_table_name(inner).name, "t1_logs");
    } else {
        panic!(
            "expected Subquery in projection: {:?}",
            select.projection[0]
        );
    }
}

#[test]
fn test_rewrite_in_subquery() {
    let tenant = TenantContext::new("t1");
    // SELECT * FROM users WHERE id IN (SELECT user_id FROM orders)
    let stmt = rewrite(
        "SELECT * FROM users WHERE id IN (SELECT user_id FROM orders)",
        &tenant,
    );
    let select = extract_select(&stmt);
    assert_eq!(first_table_name(select).name, "t1_users");
    if let Some(Expr::InSubquery { subquery, .. }) = &select.where_clause {
        assert_eq!(first_table_name(subquery).name, "t1_orders");
    } else {
        panic!("expected InSubquery in WHERE");
    }
}

#[test]
fn test_rewrite_exists_subquery() {
    let tenant = TenantContext::new("t1");
    // SELECT * FROM users WHERE EXISTS (SELECT 1 FROM orders WHERE orders.user_id = users.id)
    let stmt = rewrite(
        "SELECT * FROM users WHERE EXISTS (SELECT 1 FROM orders WHERE orders.user_id = users.id)",
        &tenant,
    );
    let select = extract_select(&stmt);
    assert_eq!(first_table_name(select).name, "t1_users");
    if let Some(Expr::Exists { subquery, .. }) = &select.where_clause {
        assert_eq!(first_table_name(subquery).name, "t1_orders");
    } else {
        panic!("expected Exists in WHERE");
    }
}

// =====================================================================
//  Nested subquery (1)
// =====================================================================

#[test]
fn test_rewrite_nested_subquery() {
    let tenant = TenantContext::new("t1");
    // SELECT * FROM users WHERE id IN (
    //   SELECT user_id FROM orders WHERE product_id IN (SELECT id FROM products))
    let stmt = rewrite(
        "SELECT * FROM users WHERE id IN (\
         SELECT user_id FROM orders WHERE product_id IN (SELECT id FROM products))",
        &tenant,
    );
    let select = extract_select(&stmt);
    assert_eq!(first_table_name(select).name, "t1_users");
    if let Some(Expr::InSubquery {
        subquery: outer, ..
    }) = &select.where_clause
    {
        assert_eq!(first_table_name(outer).name, "t1_orders");
        if let Some(Expr::InSubquery {
            subquery: inner, ..
        }) = &outer.where_clause
        {
            assert_eq!(first_table_name(inner).name, "t1_products");
        } else {
            panic!("expected nested InSubquery");
        }
    } else {
        panic!("expected InSubquery");
    }
}

// =====================================================================
//  DML rewrite (3)
// =====================================================================

#[test]
fn test_rewrite_insert_values() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite("INSERT INTO users VALUES (1, 'alice')", &tenant);
    match stmt {
        Statement::Insert { table, .. } => assert_eq!(table.name, "t1_users"),
        _ => panic!("expected Insert"),
    }
}

#[test]
fn test_rewrite_insert_select_source() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite("INSERT INTO logs SELECT * FROM events", &tenant);
    match stmt {
        Statement::Insert {
            table,
            source: InsertSource::Select(s),
            ..
        } => {
            assert_eq!(table.name, "t1_logs");
            assert_eq!(first_table_name(&s).name, "t1_events");
        }
        _ => panic!("expected Insert with Select source"),
    }
}

#[test]
fn test_rewrite_update_with_where() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite("UPDATE users SET name = 'bob' WHERE id = 1", &tenant);
    match stmt {
        Statement::Update { table, .. } => assert_eq!(table.name, "t1_users"),
        _ => panic!("expected Update"),
    }
}

// =====================================================================
//  DDL rewrite (4)
// =====================================================================

#[test]
fn test_rewrite_create_table() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite("CREATE TABLE users (id BIGINT, name TEXT)", &tenant);
    match stmt {
        Statement::CreateTable { name, .. } => assert_eq!(name.name, "t1_users"),
        _ => panic!("expected CreateTable"),
    }
}

#[test]
fn test_rewrite_drop_table() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite("DROP TABLE users, orders", &tenant);
    match stmt {
        Statement::DropTable { names, .. } => {
            assert_eq!(names.len(), 2);
            assert_eq!(names[0].name, "t1_users");
            assert_eq!(names[1].name, "t1_orders");
        }
        _ => panic!("expected DropTable"),
    }
}

#[test]
fn test_rewrite_create_index() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite("CREATE INDEX idx_users_id ON users (id)", &tenant);
    match stmt {
        Statement::CreateIndex {
            name,
            table,
            columns,
            ..
        } => {
            assert_eq!(name, Some("idx_users_id".into()));
            assert_eq!(table.name, "t1_users");
            assert_eq!(columns.len(), 1);
            assert_eq!(columns[0].column, "id");
        }
        _ => panic!("expected CreateIndex"),
    }
}

#[test]
fn test_rewrite_drop_index() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite("DROP INDEX idx_users_id", &tenant);
    match stmt {
        Statement::DropIndex { names, .. } => {
            assert_eq!(names, vec!["idx_users_id".to_string()]);
        }
        _ => panic!("expected DropIndex"),
    }
}

// =====================================================================
//  Foreign key reference (1)
// =====================================================================

#[test]
fn test_rewrite_foreign_key_reference() {
    let tenant = TenantContext::new("t1");
    let stmt = rewrite(
        "CREATE TABLE orders (\
         id BIGINT PRIMARY KEY, \
         user_id BIGINT, \
         FOREIGN KEY (user_id) REFERENCES users(id))",
        &tenant,
    );
    match stmt {
        Statement::CreateTable { constraints, .. } => {
            assert_eq!(constraints.len(), 1);
            match &constraints[0] {
                TableConstraint::ForeignKey { reference, .. } => {
                    assert_eq!(reference.table.name, "t1_users");
                }
                _ => panic!("expected ForeignKey constraint"),
            }
        }
        _ => panic!("expected CreateTable"),
    }
}

// =====================================================================
//  String literal safety (1)
// =====================================================================

#[test]
fn test_string_literal_not_rewritten() {
    let tenant = TenantContext::new("t1");
    // The string 'users' must NOT be rewritten — AST-level rewriter ignores literals
    let stmt = rewrite("INSERT INTO users VALUES (1, 'users')", &tenant);
    match stmt {
        Statement::Insert {
            table,
            source: InsertSource::Values(rows),
            ..
        } => {
            assert_eq!(table.name, "t1_users");
            assert_eq!(rows[0][1], Expr::Literal(Value::Text("users".into())));
        }
        _ => panic!("expected Insert"),
    }
}

// =====================================================================
//  Column name safety (1)
// =====================================================================

#[test]
fn test_column_named_users_not_rewritten() {
    let tenant = TenantContext::new("t1");
    // Column named "users" in SELECT — must NOT be prefixed (only table refs are)
    let stmt = rewrite("SELECT users FROM logs", &tenant);
    let select = extract_select(&stmt);
    // Table "logs" → t1_logs
    assert_eq!(first_table_name(select).name, "t1_logs");
    // Column "users" stays as Identifier(["users"])
    match &select.projection[0] {
        SelectItem::UnnamedExpr(Expr::Identifier(parts)) => {
            assert_eq!(parts, &vec!["users".to_string()]);
        }
        other => panic!("expected Identifier column, got {:?}", other),
    }
}

// =====================================================================
//  Custom exempt (1)
// =====================================================================

#[test]
fn test_custom_exempt_table_not_rewritten() {
    let tenant = TenantContext::new("t1").with_exempt("shared_config");
    let stmt = rewrite("SELECT * FROM shared_config", &tenant);
    let select = extract_select(&stmt);
    assert_eq!(first_table_name(select).name, "shared_config");

    // Other tables still rewritten
    let stmt = rewrite("SELECT * FROM users", &tenant);
    let select = extract_select(&stmt);
    assert_eq!(first_table_name(select).name, "t1_users");
}

// =====================================================================
//  Direct API test — rewrite_table_name (extra sanity)
// =====================================================================

#[test]
fn test_rewrite_table_name_direct() {
    let tenant = TenantContext::new("t1");
    let rewriter = SqlRewriter::new(tenant);

    let rewritten = rewriter.rewrite_table_name(TableName::new("users"));
    assert_eq!(rewritten.name, "t1_users");
    assert_eq!(rewritten.schema, None);

    let rewritten = rewriter.rewrite_table_name(TableName::with_schema("public", "users"));
    assert_eq!(rewritten.schema, Some("public".into()));
    assert_eq!(rewritten.name, "t1_users");

    let rewritten = rewriter.rewrite_table_name(TableName::new("pg_tables"));
    assert_eq!(rewritten.name, "pg_tables"); // exempt
}

// =====================================================================
//  Unsupported statement (1) — EXPLAIN wraps a Statement, may need handling
// =====================================================================

#[test]
fn test_rewrite_unsupported_returns_error() {
    let tenant = TenantContext::new("t1");
    let rewriter = SqlRewriter::new(tenant);
    // EXPLAIN is not yet supported by the rewriter
    let stmt = parse_one("EXPLAIN SELECT * FROM users");
    let result = rewriter.rewrite_statement(stmt);
    assert!(matches!(result, Err(RewriteError::Unsupported(_))));
}
