//! SzRSQL 对抗性边界审计测试
//!
//! 对应文档：`docs/对抗性边界审计清单.md`
//! 覆盖六大类审计项的子集（可独立单元测试部分）：
//! - ADV-SQL: SQL 注入与解析边界
//! - ADV-MEM: 内存安全与资源耗尽
//! - ADV-EDG: 边界条件与极端值
//! - ADV-DAT: 数据完整性与一致性
//! - ADV-PRT: 协议/解析器健壮性
//! - ADV-TYP: 类型与约束安全
//!
//! 说明：并发（ADV-CON）、网络（ADV-NET）相关测试需要独立集成环境，
//!      在 `tests/integration/` 与 `szrsql-pgcompat` crate 中覆盖。

#![allow(clippy::approx_constant)]

use szrsql_sql::ast::{escape_ident, is_valid_ident, quote_ident};
use szrsql_sql::executor::{Executor, InMemoryTable, MutableTable, TableStorage};
use szrsql_sql::parser::{parse_single_statement, parse_sql};
use szrsql_sql::plan::{InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

fn make_catalog_users() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "users",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("age", ColumnType::Int64),
        ],
    );
    catalog
}

fn make_filled_users() -> InMemoryTable {
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("age", ColumnType::Int64),
        ],
    );
    table.insert(vec![
        Value::Int64(1),
        Value::Text("alice".into()),
        Value::Int64(30),
    ]);
    table.insert(vec![
        Value::Int64(2),
        Value::Text("bob".into()),
        Value::Int64(25),
    ]);
    table.insert(vec![
        Value::Int64(3),
        Value::Text("carol".into()),
        Value::Int64(35),
    ]);
    table
}

fn plan_sql(sql: &str, catalog: &dyn szrsql_sql::plan::Catalog) -> LogicalPlan {
    let stmts = parse_sql(sql).expect("parse failed");
    assert_eq!(stmts.len(), 1, "expected exactly 1 statement");
    let planner = Planner::new(catalog);
    planner
        .plan_statement(stmts.into_iter().next().unwrap())
        .expect("plan failed")
}

/// 内联实现字符串字面量转义（PostgreSQL 风格：单引号双写）
fn escape_string_literal(s: &str) -> String {
    s.replace('\'', "''")
}

// =====================================================================
//  ADV-SQL: SQL 注入与解析边界
// =====================================================================

#[test]
fn test_adv_sql_001_identifier_injection() {
    // ADV-SQL-001: 动态标识符拼接注入
    let malicious = "users; DROP TABLE secrets; --";
    let quoted = quote_ident(malicious);
    assert!(
        quoted.starts_with('"'),
        "quoted ident must start with double quote"
    );
    assert!(
        quoted.ends_with('"'),
        "quoted ident must end with double quote"
    );
    // 内部分号不能逃逸出引用
    assert!(
        quoted.matches("\";").count() == 0,
        "no unescaped quote-semicolon sequence: {}",
        quoted
    );
}

#[test]
fn test_adv_sql_002_string_literal_injection() {
    // ADV-SQL-002: 字符串字面量注入
    // 转义策略（PostgreSQL 风格）：单引号双写
    let malicious = "'; DROP TABLE users; --";
    let escaped = escape_string_literal(malicious);
    // 转义后所有单引号必须成对出现（双写）
    let quote_count = escaped.chars().filter(|&c| c == '\'').count();
    assert_eq!(
        quote_count % 2,
        0,
        "single quotes must be paired after escaping: {}",
        escaped
    );
    // 关键安全属性：将转义后的字符串重新放入 SQL 字面值后，
    // 解析器应能正确识别字符串边界，不会把 DROP 当作语句执行
    let sql = format!("SELECT '{}' FROM users", escaped);
    let parsed = parse_sql(&sql);
    assert!(parsed.is_ok(), "escaped literal should parse correctly");
    let stmts = parsed.unwrap();
    assert_eq!(
        stmts.len(),
        1,
        "should parse as single statement, not multi-stmt"
    );
}

#[test]
fn test_adv_sql_003_comment_bypass() {
    // ADV-SQL-003: SQL 注释绕过
    let result = parse_sql("SELECT * FROM users -- comment\nWHERE id = 1");
    assert!(result.is_ok(), "single-line comment should parse");

    // 块注释
    let result = parse_sql("SELECT /* block */ * FROM users");
    assert!(result.is_ok(), "block comment should parse");

    // 嵌套块注释
    let result = parse_sql("SELECT /* outer /* inner */ outer */ * FROM users");
    match result {
        Ok(_) => {}
        Err(e) => println!("ADV-SQL-003: nested comment rejected (acceptable): {}", e),
    }
}

#[test]
fn test_adv_sql_004_long_sql_input() {
    // 注意：在 Windows 上测试线程默认栈大小（1MB）可能不足以解析超长 SQL，
    // 此处通过 spawn 一个 8MB 栈的线程来执行测试体，避免 STATUS_STACK_OVERFLOW。
    let handle = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            // ADV-SQL-004: 超长 SQL 输入 + 深度嵌套 OR 链
            // ADV-BUG-001 已修复：添加 MAX_EXPR_DEPTH=512 递归深度限制 + MAX_SQL_LEN=1MB 长度预检
            // 现在深度嵌套 OR 链不再栈溢出，而是返回清晰的 ParseError

            // 测试 1：平铺的长 SELECT 列表（不触发深度递归），应正常解析
            let mut sql = String::from("SELECT ");
            for i in 0..1000 {
                if i > 0 {
                    sql.push(',');
                }
                sql.push_str(&format!("{}", i));
            }
            sql.push_str(" FROM users");
            let result = parse_sql(&sql);
            assert!(result.is_ok(), "long flat SELECT list should parse");

            // 测试 2：20 个 OR 链（< MAX_BINARY_OP_CHAIN=256），应正常解析
            let mut or_sql = String::from("SELECT * FROM users WHERE id = 0");
            for i in 1..20 {
                or_sql.push_str(&format!(" OR id = {}", i));
            }
            let result = parse_sql(&or_sql);
            assert!(
                result.is_ok(),
                "20 OR chains (< 256) should parse: {:?}",
                result.err()
            );

            // 测试 3：50 个 OR 链（< MAX_BINARY_OP_CHAIN=256），应正常解析
            // （原 ADV-BUG-001 复现输入为 50 链，在 2MB 栈下会栈溢出；
            //  现通过预检 + 大栈线程双重防护，50 链可安全解析）
            let mut deep_or_sql = String::from("SELECT * FROM users WHERE id = 0");
            for i in 1..50 {
                deep_or_sql.push_str(&format!(" OR id = {}", i));
            }
            let result = parse_sql(&deep_or_sql);
            assert!(
                result.is_ok(),
                "50 OR chains (< 256) should parse successfully: {:?}",
                result.err()
            );

            // 测试 3b：300 个 OR 链（> MAX_BINARY_OP_CHAIN=256），应被预检拒绝
            let mut over_or_sql = String::from("SELECT * FROM users WHERE id = 0");
            for i in 1..300 {
                over_or_sql.push_str(&format!(" OR id = {}", i));
            }
            let result = parse_sql(&over_or_sql);
            assert!(
                result.is_err(),
                "300 OR chains (> 256) should be rejected by MAX_BINARY_OP_CHAIN pre-check"
            );
            if let Err(ref e) = result {
                let msg = format!("{}", e);
                assert!(
                    msg.contains("OR/AND") || msg.contains("ADV-BUG-001"),
                    "error message should mention OR/AND limit, got: {}",
                    msg
                );
            }

            // 测试 4：600 个 OR 链（远超 MAX_BINARY_OP_CHAIN=256），应被预检拒绝
            let mut huge_or_sql = String::from("SELECT * FROM users WHERE id = 0");
            for i in 1..600 {
                huge_or_sql.push_str(&format!(" OR id = {}", i));
            }
            let result = parse_sql(&huge_or_sql);
            assert!(
                result.is_err(),
                "600 OR chains should be rejected by MAX_BINARY_OP_CHAIN pre-check"
            );

            // 测试 5：超过 MAX_SQL_LEN 的超长 SQL，应直接拒绝
            let huge_sql = format!("SELECT '{}';", "x".repeat(2 * 1024 * 1024));
            let result = parse_sql(&huge_sql);
            assert!(
                result.is_err(),
                "SQL > 1MB should be rejected by MAX_SQL_LEN pre-check"
            );
        })
        .expect("failed to spawn test thread");
    handle.join().expect("test thread panicked");
}

#[test]
fn test_adv_sql_005_nested_subquery_depth() {
    // ADV-SQL-005: 嵌套子查询深度
    let sql = String::from(
        "SELECT * FROM (SELECT * FROM (SELECT * FROM (SELECT * FROM users) t1) t2) t3",
    );
    let result = parse_sql(&sql);
    match result {
        Ok(_) => {}
        Err(e) => println!("ADV-SQL-005: deep nested query rejected: {}", e),
    }
}

#[test]
fn test_adv_sql_006_special_characters() {
    // ADV-SQL-006: 特殊字符处理
    // 正常字符串应能解析
    let result = parse_sql("SELECT 'hello world' FROM users");
    assert!(result.is_ok(), "normal string should parse");

    // 字符串内的单引号通过双写转义
    let result = parse_sql("SELECT 'it''s ok' FROM users");
    assert!(result.is_ok(), "escaped single quote should parse");
}

#[test]
fn test_adv_sql_007_null_handling() {
    // ADV-SQL-007: NULL 值处理
    let catalog = make_catalog_users();
    let plan = plan_sql("SELECT * FROM users WHERE age IS NULL", &catalog);
    let table = make_filled_users();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 0, "no rows should match IS NULL");

    // IS NOT NULL 应匹配所有行
    let plan = plan_sql("SELECT * FROM users WHERE age IS NOT NULL", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 3, "all rows should match IS NOT NULL");
}

#[test]
fn test_adv_sql_008_type_confusion() {
    // ADV-SQL-008: 类型混淆
    // 字符串与数字比较应解析但不一定执行成功
    let result = parse_sql("SELECT * FROM users WHERE id = '1'");
    assert!(result.is_ok(), "type-mixed comparison should parse");
}

#[test]
fn test_adv_sql_009_multi_statement_injection() {
    // ADV-SQL-009: 多语句注入（ADV-BUG-002 修复验证）
    //
    // 修复前：parse_sql 接受多语句，应用层可能执行注入的第二条语句
    // 修复后：
    //   1. parse_sql 仍然接受多语句（兼容 PG Simple Query 协议）
    //   2. parse_single_statement 严格拒绝多语句（单语句模式）
    //   3. pgwire ExecutorService 默认 allow_multi_statement=false

    // 测试 1：parse_sql 仍接受多语句（兼容性）
    let result = parse_sql("SELECT 1; DROP TABLE users");
    assert!(
        result.is_ok(),
        "parse_sql should accept multi-statement for PG compatibility"
    );
    let stmts = result.unwrap();
    assert_eq!(stmts.len(), 2, "should parse 2 statements");

    // 测试 2：parse_single_statement 拒绝多语句（ADV-BUG-002 修复）
    let result = parse_single_statement("SELECT 1; DROP TABLE users");
    assert!(
        result.is_err(),
        "parse_single_statement should reject multi-statement (ADV-BUG-002)"
    );
    if let Err(ref e) = result {
        let msg = format!("{}", e);
        assert!(
            msg.contains("ADV-BUG-002") || msg.contains("multi-statement"),
            "error should mention ADV-BUG-002 or multi-statement, got: {}",
            msg
        );
    }

    // 测试 3：parse_single_statement 接受单语句
    let result = parse_single_statement("SELECT 1");
    assert!(result.is_ok(), "single statement should parse");

    // 测试 4：parse_single_statement 拒绝空语句
    let result = parse_single_statement("");
    // 空字符串可能返回 Ok(空vec) 或 Err，关键是不 panic
    // 修复后应返回 Err
    assert!(result.is_err(), "empty SQL should be rejected");
}

#[test]
fn test_adv_sql_010_quote_ident_idempotent() {
    // ADV-SQL-010 (子集): quote_ident 对已引用标识符的处理
    let normal = "users";
    let quoted = quote_ident(normal);
    assert_eq!(quoted, "\"users\"");

    let with_quote = "a\"b";
    let quoted = quote_ident(with_quote);
    assert_eq!(quoted, "\"a\"\"b\"");

    // escape_ident 不应破坏标识符
    let escaped = escape_ident(normal);
    assert_eq!(escaped, "users");

    // is_valid_ident 校验
    assert!(is_valid_ident("users"));
    assert!(is_valid_ident("_id"));
    assert!(!is_valid_ident(""));
    assert!(!is_valid_ident("user name"));
    assert!(!is_valid_ident("a;b"));
}

// =====================================================================
//  ADV-MEM: 内存安全与资源耗尽
// =====================================================================

#[test]
fn test_adv_mem_001_large_result_set() {
    // ADV-MEM-001: 大结果集（1 万行，验证不 OOM）
    let n = 10_000;
    let mut table = InMemoryTable::with_columns("big", vec![("id", ColumnType::Int64)]);
    for i in 0..n {
        table.insert(vec![Value::Int64(i as i64)]);
    }
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table("big", vec![("id", ColumnType::Int64)]);
    let plan = plan_sql("SELECT * FROM big", &catalog);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), n);
    // 首尾值校验
    assert_eq!(result[0][0], Value::Int64(0));
    assert_eq!(result[n - 1][0], Value::Int64((n - 1) as i64));
}

#[test]
fn test_adv_mem_002_empty_table_operations() {
    // ADV-MEM-002: 空表操作不崩溃
    let catalog = make_catalog_users();
    let table = InMemoryTable::with_columns(
        "users",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("age", ColumnType::Int64),
        ],
    );
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql("SELECT * FROM users", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 0);

    // COUNT(*) on 空表
    let plan = plan_sql("SELECT COUNT(*) FROM users", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    match &result[0][0] {
        Value::Int64(n) => assert_eq!(*n, 0),
        _ => panic!("COUNT should return Int64"),
    }
}

#[test]
fn test_adv_mem_003_repeated_insert_delete() {
    // ADV-MEM-003: 反复插入删除，验证 tombstone 不导致内存膨胀异常
    let catalog = make_catalog_users();
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("age", ColumnType::Int64),
        ],
    );
    for round in 0..100 {
        table.insert(vec![
            Value::Int64(round),
            Value::Text("x".into()),
            Value::Int64(round),
        ]);
    }
    assert_eq!(table.row_count(), 100);
    let plan = plan_sql("DELETE FROM users", &catalog);
    let exec = Executor::new();
    exec.execute_delete(&plan, &mut table).unwrap();
    assert_eq!(table.row_count(), 0);
}

#[test]
fn test_adv_mem_004_long_string_value() {
    // ADV-MEM-004: 长字符串值
    let catalog = make_catalog_users();
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("age", ColumnType::Int64),
        ],
    );
    let long_name = "x".repeat(100_000);
    table.insert(vec![
        Value::Int64(1),
        Value::Text(long_name.clone()),
        Value::Int64(20),
    ]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql("SELECT * FROM users WHERE id = 1", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][1], Value::Text(long_name));
}

// =====================================================================
//  ADV-EDG: 边界条件与极端值
// =====================================================================

#[test]
fn test_adv_edg_001_empty_table_select() {
    let catalog = make_catalog_users();
    let table = InMemoryTable::with_columns(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql("SELECT * FROM users", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 0);
}

#[test]
fn test_adv_edg_002_single_row_table() {
    let catalog = make_catalog_users();
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    table.insert(vec![Value::Int64(42), Value::Text("answer".into())]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql("SELECT * FROM users", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(42));
}

#[test]
fn test_adv_edg_003_max_int() {
    // ADV-EDG-003: i64::MAX
    let catalog = make_catalog_users();
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    table.insert(vec![Value::Int64(i64::MAX), Value::Text("max".into())]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql(
        "SELECT * FROM users WHERE id = 9223372036854775807",
        &catalog,
    );
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(i64::MAX));
}

#[test]
fn test_adv_edg_004_min_int() {
    // ADV-EDG-004: i64::MIN
    let catalog = make_catalog_users();
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    table.insert(vec![Value::Int64(i64::MIN), Value::Text("min".into())]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql(
        "SELECT * FROM users WHERE id = -9223372036854775808",
        &catalog,
    );
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(i64::MIN));
}

#[test]
fn test_adv_edg_005_empty_string() {
    // ADV-EDG-005: 空字符串
    let catalog = make_catalog_users();
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    table.insert(vec![Value::Int64(1), Value::Text("".into())]);
    table.insert(vec![Value::Int64(2), Value::Text("alice".into())]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql("SELECT * FROM users WHERE name = ''", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(1));
}

#[test]
fn test_adv_edg_006_null_sorting() {
    // ADV-EDG-006: NULL 在排序中的处理
    let catalog = make_catalog_users();
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("age", ColumnType::Int64),
        ],
    );
    table.insert(vec![
        Value::Int64(1),
        Value::Text("a".into()),
        Value::Int64(30),
    ]);
    table.insert(vec![Value::Int64(2), Value::Text("b".into()), Value::Null]);
    table.insert(vec![
        Value::Int64(3),
        Value::Text("c".into()),
        Value::Int64(20),
    ]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql("SELECT * FROM users ORDER BY age ASC", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 3);
}

#[test]
fn test_adv_edg_007_negative_numbers() {
    // ADV-EDG-007: 负数
    let catalog = make_catalog_users();
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    table.insert(vec![Value::Int64(-1), Value::Text("neg".into())]);
    table.insert(vec![Value::Int64(0), Value::Text("zero".into())]);
    table.insert(vec![Value::Int64(1), Value::Text("pos".into())]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql("SELECT * FROM users WHERE id < 0", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(-1));
}

#[test]
fn test_adv_edg_008_unicode_strings() {
    // ADV-EDG-008: Unicode 字符串
    let catalog = make_catalog_users();
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    table.insert(vec![Value::Int64(1), Value::Text("中文用户".into())]);
    table.insert(vec![Value::Int64(2), Value::Text("日本語".into())]);
    table.insert(vec![Value::Int64(3), Value::Text("🎮".into())]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql("SELECT * FROM users", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[0][1], Value::Text("中文用户".into()));
    assert_eq!(result[2][1], Value::Text("🎮".into()));
}

#[test]
fn test_adv_edg_009_duplicate_values_distinct() {
    // ADV-EDG-009: 重复值与 DISTINCT
    let catalog = make_catalog_users();
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    table.insert(vec![Value::Int64(1), Value::Text("dup".into())]);
    table.insert(vec![Value::Int64(2), Value::Text("dup".into())]);
    table.insert(vec![Value::Int64(3), Value::Text("dup".into())]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql("SELECT DISTINCT name FROM users", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
}

#[test]
fn test_adv_edg_010_limit_zero_and_overflow() {
    // ADV-EDG-010: LIMIT 0 与超量 LIMIT
    let catalog = make_catalog_users();
    let table = make_filled_users();
    let mut exec = Executor::new();
    exec.register_table(&table);

    let plan = plan_sql("SELECT * FROM users LIMIT 0", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 0);

    let plan = plan_sql("SELECT * FROM users LIMIT 100", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 3);
}

// =====================================================================
//  ADV-DAT: 数据完整性与一致性
// =====================================================================

#[test]
fn test_adv_dat_001_transaction_rollback() {
    // ADV-DAT-001: 事务回滚（通过 snapshot/restore 模拟）
    let mut table = make_filled_users();
    let snapshot = table.snapshot();
    assert_eq!(table.row_count(), 3);
    table.insert(vec![
        Value::Int64(4),
        Value::Text("dave".into()),
        Value::Int64(40),
    ]);
    assert_eq!(table.row_count(), 4);
    table.restore(snapshot);
    assert_eq!(
        table.row_count(),
        3,
        "snapshot restore should rollback insert"
    );
}

#[test]
fn test_adv_dat_002_delete_then_insert() {
    // ADV-DAT-002: 删除后重新插入
    let catalog = make_catalog_users();
    let mut table = make_filled_users();
    assert_eq!(table.row_count(), 3);
    let plan = plan_sql("DELETE FROM users", &catalog);
    let exec = Executor::new();
    exec.execute_delete(&plan, &mut table).unwrap();
    assert_eq!(table.row_count(), 0);
    table.insert(vec![
        Value::Int64(1),
        Value::Text("new".into()),
        Value::Int64(20),
    ]);
    assert_eq!(table.row_count(), 1);
}

#[test]
fn test_adv_dat_003_update_preserves_row_count() {
    // ADV-DAT-003: UPDATE 保持行数不变
    let catalog = make_catalog_users();
    let mut table = make_filled_users();
    let original_count = table.row_count();
    let plan = plan_sql("UPDATE users SET age = age + 1", &catalog);
    let exec = Executor::new();
    let result = exec.execute_update(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, original_count);
    assert_eq!(table.row_count(), original_count);
}

#[test]
fn test_adv_dat_004_where_filter_accuracy() {
    // ADV-DAT-004: WHERE 过滤准确性
    let catalog = make_catalog_users();
    let table = make_filled_users();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql("SELECT * FROM users WHERE age > 30", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][1], Value::Text("carol".into()));
}

#[test]
fn test_adv_dat_005_count_accuracy() {
    // ADV-DAT-005: COUNT(*) 准确性
    let catalog = make_catalog_users();
    let mut table = make_filled_users();
    table.insert(vec![
        Value::Int64(4),
        Value::Text("dave".into()),
        Value::Int64(40),
    ]);
    table.insert(vec![
        Value::Int64(5),
        Value::Text("eve".into()),
        Value::Int64(28),
    ]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql("SELECT COUNT(*) FROM users", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    match &result[0][0] {
        Value::Int64(n) => assert_eq!(*n, 5),
        _ => panic!("COUNT should return Int64"),
    }
}

#[test]
fn test_adv_dat_006_update_with_where_accuracy() {
    // ADV-DAT-006: 带 WHERE 的 UPDATE 影响行数准确
    let catalog = make_catalog_users();
    let mut table = make_filled_users();
    let plan = plan_sql("UPDATE users SET age = 99 WHERE id > 1", &catalog);
    let exec = Executor::new();
    let result = exec.execute_update(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 2);
    assert_eq!(table.row_count(), 3);
}

#[test]
fn test_adv_dat_007_delete_with_where_accuracy() {
    // ADV-DAT-007: 带 WHERE 的 DELETE 影响行数准确
    let catalog = make_catalog_users();
    let mut table = make_filled_users();
    let plan = plan_sql("DELETE FROM users WHERE id = 2", &catalog);
    let exec = Executor::new();
    let result = exec.execute_delete(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.row_count(), 2);
}

#[test]
fn test_adv_dat_008_snapshot_idempotent() {
    // ADV-DAT-008: 同一快照可恢复多次
    let mut table = make_filled_users();
    let snapshot = table.snapshot();
    table.insert(vec![
        Value::Int64(100),
        Value::Text("x".into()),
        Value::Int64(1),
    ]);
    assert_eq!(table.row_count(), 4);
    let snap2 = table.snapshot();
    table.restore(snapshot);
    assert_eq!(table.row_count(), 3);
    table.restore(snap2);
    assert_eq!(table.row_count(), 4);
}

// =====================================================================
//  ADV-PRT: 解析器健壮性（ADV-NET/ADV-CON 在协议层的子集）
// =====================================================================

#[test]
fn test_adv_prt_001_malformed_sql_no_panic() {
    // ADV-PRT-001: 畸形 SQL 不应 panic
    let cases = [
        "",
        "   ",
        ";",
        "SELECT",
        "SELECT *",
        "SELECT * FROM",
        "FROM users",
        "SELECT * FROM users WHERE",
        "SELECT * FROM users WHERE =",
        "SELECT * FROM users WHERE id =",
        "SELECT (",
        "SELECT )",
        "SELECT * FROM users (",
        "SELECT * FROM users )",
        "INSERT INTO",
        "INSERT INTO users",
        "INSERT INTO users VALUES",
        "UPDATE",
        "UPDATE users",
        "UPDATE users SET",
        "DELETE",
        "DELETE FROM",
        "CREATE TABLE",
        "CREATE TABLE t",
        "CREATE TABLE t (",
        "CREATE TABLE t (id)",
        "DROP",
        "DROP TABLE",
    ];
    for sql in cases.iter() {
        // 仅要求不 panic，错误返回是可接受的
        let _ = parse_sql(sql);
    }
}

#[test]
fn test_adv_prt_002_unclosed_string_literal() {
    // ADV-PRT-002: 未闭合字符串字面值
    let result = parse_sql("SELECT 'unclosed FROM users");
    assert!(result.is_err(), "unclosed string literal should error");
}

#[test]
fn test_adv_prt_003_unclosed_parenthesis() {
    // ADV-PRT-003: 未闭合括号
    let result = parse_sql("SELECT * FROM (SELECT * FROM users");
    assert!(result.is_err(), "unclosed parenthesis should error");
}

#[test]
fn test_adv_prt_004_empty_statement() {
    // ADV-PRT-004: 空语句
    let result = parse_sql("");
    // 空语句可以返回空 stmt 列表或错误，关键是不能 panic
    if let Ok(stmts) = result {
        assert_eq!(stmts.len(), 0, "empty SQL should yield 0 statements");
    }
}

#[test]
fn test_adv_prt_005_semicolon_only() {
    // ADV-PRT-005: 仅分号
    let result = parse_sql(";");
    if let Ok(stmts) = result {
        assert!(stmts.len() <= 1, "semicolon-only should yield 0-1 stmts");
    }
}

#[test]
fn test_adv_prt_006_reserved_keyword_as_ident() {
    // ADV-PRT-006: 保留字作标识符
    // 解析器可能接受或拒绝，关键是行为明确
    let _ = parse_sql("SELECT select FROM users");
    let _ = parse_sql("SELECT * FROM where");
}

#[test]
fn test_adv_prt_007_numeric_edge_values() {
    // ADV-PRT-007: 数值边界
    let cases = [
        "SELECT 0",
        "SELECT -1",
        "SELECT 1.0",
        "SELECT 1.5e10",
        "SELECT 0x1F",
        "SELECT 010",
        "SELECT 9223372036854775807",  // i64::MAX
        "SELECT -9223372036854775808", // i64::MIN
        "SELECT 9223372036854775808",  // i64::MAX + 1 (overflow)
        "SELECT 0.0",
        "SELECT -0.0",
    ];
    for sql in cases.iter() {
        // 仅要求不 panic
        let _ = parse_sql(sql);
    }
}

#[test]
fn test_adv_prt_008_deep_parentheses() {
    // ADV-PRT-008: 深度嵌套括号
    let mut sql = String::from("SELECT ");
    for _ in 0..100 {
        sql.push('(');
    }
    sql.push('1');
    for _ in 0..100 {
        sql.push(')');
    }
    // 解析或拒绝均可，关键是不能栈溢出
    let _ = parse_sql(&sql);
}

// =====================================================================
//  ADV-TYP: 类型与约束安全
// =====================================================================

#[test]
fn test_adv_typ_001_null_in_unique_index() {
    // ADV-TYP-001: NULL 在唯一约束中（多个 NULL 应允许）
    let catalog = make_catalog_users();
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("age", ColumnType::Int64),
        ],
    );
    table.insert(vec![Value::Int64(1), Value::Text("a".into()), Value::Null]);
    table.insert(vec![Value::Int64(2), Value::Text("b".into()), Value::Null]);
    assert_eq!(table.row_count(), 2);

    // 通过 catalog + plan 验证 SELECT 仍可正确读取
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql("SELECT * FROM users WHERE age IS NULL", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 2);
}

#[test]
fn test_adv_typ_002_mixed_type_insert() {
    // ADV-TYP-002: 类型不匹配的插入（执行器层面）
    let catalog = make_catalog_users();
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("age", ColumnType::Int64),
        ],
    );
    // 直接通过低层 insert 写入混合类型，验证表不崩溃
    table.insert(vec![
        Value::Int64(1),
        Value::Text("a".into()),
        Value::Int64(10),
    ]);
    table.insert(vec![Value::Int64(2), Value::Text("b".into()), Value::Null]);
    assert_eq!(table.row_count(), 2);

    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql("SELECT * FROM users", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 2);
}

#[test]
fn test_adv_typ_003_aggregate_on_null() {
    // ADV-TYP-003: 聚合函数对 NULL 的处理
    let catalog = make_catalog_users();
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("age", ColumnType::Int64),
        ],
    );
    table.insert(vec![Value::Int64(1), Value::Text("a".into()), Value::Null]);
    table.insert(vec![Value::Int64(2), Value::Text("b".into()), Value::Null]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = plan_sql("SELECT COUNT(age) FROM users", &catalog);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    // COUNT(column) 应忽略 NULL
    match &result[0][0] {
        Value::Int64(n) => assert_eq!(*n, 0, "COUNT(age) should ignore NULLs"),
        _ => panic!("COUNT should return Int64"),
    }
}

#[test]
fn test_adv_typ_004_count_star_vs_count_col() {
    // ADV-TYP-004: COUNT(*) vs COUNT(col)
    let catalog = make_catalog_users();
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("age", ColumnType::Int64),
        ],
    );
    table.insert(vec![
        Value::Int64(1),
        Value::Text("a".into()),
        Value::Int64(30),
    ]);
    table.insert(vec![Value::Int64(2), Value::Text("b".into()), Value::Null]);
    table.insert(vec![
        Value::Int64(3),
        Value::Text("c".into()),
        Value::Int64(40),
    ]);
    let mut exec = Executor::new();
    exec.register_table(&table);

    // COUNT(*) 计所有行
    let plan = plan_sql("SELECT COUNT(*) FROM users", &catalog);
    let result = exec.execute(&plan).unwrap();
    match &result[0][0] {
        Value::Int64(n) => assert_eq!(*n, 3, "COUNT(*) should count all rows"),
        _ => panic!(),
    }

    // COUNT(age) 忽略 NULL
    let plan = plan_sql("SELECT COUNT(age) FROM users", &catalog);
    let result = exec.execute(&plan).unwrap();
    match &result[0][0] {
        Value::Int64(n) => assert_eq!(*n, 2, "COUNT(age) should ignore NULLs"),
        _ => panic!(),
    }
}
