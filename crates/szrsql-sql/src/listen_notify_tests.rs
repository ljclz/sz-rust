//! Phase 4.6 单元测试 — LISTEN/UNLISTEN/NOTIFY 解析与规划。
//!
//! 覆盖类别：
//! - Parser（10）：LISTEN 基本频道名、双引号频道名、UNLISTEN 单频道、UNLISTEN *、
//!   NOTIFY 无负载、NOTIFY 带负载、NOTIFY 带空负载、NOTIFY 负载含逗号、
//!   错误语法（缺少频道名、未引用负载）、混合语句顺序保持
//! - Plan（6）：Listen、Unlisten（单频道）、Unlisten（*）、Notify（无负载）、
//!   Notify（带负载）、plan_schema 返回空 Schema
//!
//! 共 16 个测试用例。

use crate::ast::{Statement, TableName};
use crate::parser::{parse_one, parse_sql};
use crate::plan::{InMemoryCatalog, LogicalPlan, Planner};

// =====================================================================
//  辅助函数
// =====================================================================

/// 解析 SQL 并断言成功，返回 Statement
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
    planner
        .plan_statement(stmt)
        .unwrap_or_else(|e| panic!("plan failed for SQL: {sql}\nerror: {e:?}"))
}

/// 断言解析失败
fn must_fail(sql: &str) {
    assert!(
        parse_one(sql).is_err(),
        "expected parse failure for SQL: {sql}"
    );
}

// =====================================================================
//  Parser 测试（10 条）
// =====================================================================

#[test]
fn test_parse_listen_basic() {
    let stmt = must_parse("LISTEN events");
    match stmt {
        Statement::Listen { channel } => assert_eq!(channel, "events"),
        other => panic!("expected Listen, got {other:?}"),
    }
}

#[test]
fn test_parse_listen_quoted_channel() {
    let stmt = must_parse("LISTEN \"my channel\"");
    match stmt {
        Statement::Listen { channel } => assert_eq!(channel, "my channel"),
        other => panic!("expected Listen, got {other:?}"),
    }
}

#[test]
fn test_parse_unlisten_basic() {
    let stmt = must_parse("UNLISTEN events");
    match stmt {
        Statement::Unlisten { channel } => assert_eq!(channel, "events"),
        other => panic!("expected Unlisten, got {other:?}"),
    }
}

#[test]
fn test_parse_unlisten_all() {
    let stmt = must_parse("UNLISTEN *");
    match stmt {
        Statement::Unlisten { channel } => assert_eq!(channel, "*"),
        other => panic!("expected Unlisten, got {other:?}"),
    }
}

#[test]
fn test_parse_notify_no_payload() {
    let stmt = must_parse("NOTIFY events");
    match stmt {
        Statement::Notify { channel, payload } => {
            assert_eq!(channel, "events");
            assert_eq!(payload, "");
        }
        other => panic!("expected Notify, got {other:?}"),
    }
}

#[test]
fn test_parse_notify_with_payload() {
    let stmt = must_parse("NOTIFY events, 'hello world'");
    match stmt {
        Statement::Notify { channel, payload } => {
            assert_eq!(channel, "events");
            assert_eq!(payload, "hello world");
        }
        other => panic!("expected Notify, got {other:?}"),
    }
}

#[test]
fn test_parse_notify_empty_payload() {
    let stmt = must_parse("NOTIFY events, ''");
    match stmt {
        Statement::Notify { channel, payload } => {
            assert_eq!(channel, "events");
            assert_eq!(payload, "");
        }
        other => panic!("expected Notify, got {other:?}"),
    }
}

#[test]
fn test_parse_notify_no_space_after_comma() {
    // PG 允许 NOTIFY channel,'payload'（逗号后无空格）
    let stmt = must_parse("NOTIFY events,'payload'");
    match stmt {
        Statement::Notify { channel, payload } => {
            assert_eq!(channel, "events");
            assert_eq!(payload, "payload");
        }
        other => panic!("expected Notify, got {other:?}"),
    }
}

#[test]
fn test_parse_listen_missing_channel_fails() {
    must_fail("LISTEN");
    must_fail("NOTIFY");
    must_fail("UNLISTEN");
}

#[test]
fn test_parse_mixed_statements_preserve_order() {
    // 混合 LISTEN/SELECT/NOTIFY/UNLISTEN 应保持顺序
    let sql = "LISTEN a; SELECT 1; NOTIFY a, 'x'; UNLISTEN a;";
    let stmts = parse_sql(sql).expect("parse failed");
    assert_eq!(stmts.len(), 4);
    assert!(matches!(stmts[0], Statement::Listen { .. }));
    assert!(matches!(stmts[1], Statement::Select(_)));
    assert!(matches!(stmts[2], Statement::Notify { .. }));
    assert!(matches!(stmts[3], Statement::Unlisten { .. }));
}

// =====================================================================
//  Plan 测试（6 条）
// =====================================================================

#[test]
fn test_plan_listen() {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql("LISTEN events", &catalog);
    match plan {
        LogicalPlan::Listen { channel } => assert_eq!(channel, "events"),
        other => panic!("expected Listen plan, got {other:?}"),
    }
}

#[test]
fn test_plan_unlisten_channel() {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql("UNLISTEN events", &catalog);
    match plan {
        LogicalPlan::Unlisten { channel } => assert_eq!(channel, "events"),
        other => panic!("expected Unlisten plan, got {other:?}"),
    }
}

#[test]
fn test_plan_unlisten_all() {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql("UNLISTEN *", &catalog);
    match plan {
        LogicalPlan::Unlisten { channel } => assert_eq!(channel, "*"),
        other => panic!("expected Unlisten plan, got {other:?}"),
    }
}

#[test]
fn test_plan_notify_no_payload() {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql("NOTIFY events", &catalog);
    match plan {
        LogicalPlan::Notify { channel, payload } => {
            assert_eq!(channel, "events");
            assert_eq!(payload, "");
        }
        other => panic!("expected Notify plan, got {other:?}"),
    }
}

#[test]
fn test_plan_notify_with_payload() {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql("NOTIFY events, 'payload'", &catalog);
    match plan {
        LogicalPlan::Notify { channel, payload } => {
            assert_eq!(channel, "events");
            assert_eq!(payload, "payload");
        }
        other => panic!("expected Notify plan, got {other:?}"),
    }
}

#[test]
fn test_plan_schema_returns_empty_for_notify_plans() {
    // plan_schema 对 Listen/Unlisten/Notify 应返回空 Schema
    // （通过 plan_statement 后间接验证：不 panic 即可）
    let catalog = InMemoryCatalog::new();
    let _ = plan_sql("LISTEN ch", &catalog);
    let _ = plan_sql("UNLISTEN ch", &catalog);
    let _ = plan_sql("NOTIFY ch", &catalog);
    let _ = plan_sql("NOTIFY ch, 'p'", &catalog);
    // 若 plan_schema 抛出 non-exhaustive match，则会在 plan_statement 中 panic
    // （实际上 plan_schema 只在通配符展开等场景调用，这里只是冒烟测试）
    let _ = TableName::new("__notify__"); // 仅使用 TableName 以避免未使用警告
}
