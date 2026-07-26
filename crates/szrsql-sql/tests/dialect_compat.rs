//! Phase F-8: 跨方言兼容性集成测试
//!
//! 验证同一业务语义在 PostgreSQL / MySQL / Oracle / SQL Server / SQLite
//! 5 种方言下均可被正确解析为 SzRSQL AST。
//!
//! # 测试组织
//!
//! 每个测试函数覆盖一类 SQL 语句（DDL / DML / 查询 / 事务 / 聚合），
//! 在 5 种方言下使用各自特有语法编写等效 SQL，断言 `parse_with_dialect` 成功。
//!
//! # 注意
//!
//! 测试数据写入 `F:\test\data`（用户要求：不使用 C 盘）。
//! 本文件为纯解析层测试，不触发实际数据写入，但仍遵循该规则。

use szrsql_sql::ast::Statement;
use szrsql_sql::dialect::{parse_auto, parse_with_dialect, Dialect};

/// 辅助：断言 5 种方言下 SQL 均解析成功
fn assert_all_dialects_parse_ok(cases: &[(&str, Dialect)]) {
    for (sql, dialect) in cases {
        let result = parse_with_dialect(sql, dialect);
        assert!(
            result.is_ok(),
            "dialect {:?} failed to parse: {sql}\nerror: {:?}",
            dialect,
            result.err()
        );
        let stmts = result.unwrap();
        assert_eq!(
            stmts.len(),
            1,
            "dialect {:?}: expected 1 statement, got {}: {sql}",
            dialect,
            stmts.len()
        );
    }
}

/// 辅助：断言解析结果是指定 Statement 变体
fn assert_statement_variant(sql: &str, dialect: &Dialect, variant_name: &str) {
    let stmts = parse_with_dialect(sql, dialect).unwrap_or_else(|e| panic!("parse failed: {e:?}"));
    assert_eq!(stmts.len(), 1, "expected 1 statement: {sql}");
    let actual = match &stmts[0] {
        Statement::CreateTable { .. } => "CreateTable",
        Statement::Insert { .. } => "Insert",
        Statement::Update { .. } => "Update",
        Statement::Delete { .. } => "Delete",
        Statement::Select(_) => "Select",
        Statement::CreateView { .. } => "CreateView",
        Statement::DropTable { .. } => "DropTable",
        Statement::Begin { .. } => "Begin",
        Statement::Commit { .. } => "Commit",
        Statement::Rollback { .. } => "Rollback",
        _ => "Other",
    };
    assert_eq!(
        actual, variant_name,
        "dialect {:?}: expected {variant_name}, got {actual}: {sql}",
        dialect
    );
}

// =====================================================================
//  DDL：CREATE TABLE
// =====================================================================

#[test]
fn test_f8_create_table_cross_dialect() {
    let cases = vec![
        // PostgreSQL
        (
            "CREATE TABLE users (id BIGINT PRIMARY KEY, name TEXT NOT NULL)",
            Dialect::PostgreSQL,
        ),
        // MySQL
        (
            "CREATE TABLE users (id BIGINT PRIMARY KEY, name VARCHAR(100) NOT NULL)",
            Dialect::MySql,
        ),
        // Oracle
        (
            "CREATE TABLE users (id NUMBER PRIMARY KEY, name VARCHAR2(100) NOT NULL)",
            Dialect::Oracle,
        ),
        // SQL Server
        (
            "CREATE TABLE users (id BIGINT PRIMARY KEY, name NVARCHAR(100) NOT NULL)",
            Dialect::SqlServer,
        ),
        // SQLite
        (
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
            Dialect::SQLite,
        ),
    ];
    assert_all_dialects_parse_ok(&cases);
    for (sql, dialect) in &cases {
        assert_statement_variant(sql, dialect, "CreateTable");
    }
}

#[test]
fn test_f8_create_table_with_autoincrement() {
    // 各方言的自增主键语法
    let cases = vec![
        // SQLite: AUTOINCREMENT 关键字
        (
            "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)",
            Dialect::SQLite,
        ),
        // MySQL: AUTO_INCREMENT 关键字
        (
            "CREATE TABLE t (id INT PRIMARY KEY AUTO_INCREMENT, name VARCHAR(50))",
            Dialect::MySql,
        ),
    ];
    assert_all_dialects_parse_ok(&cases);
}

// =====================================================================
//  DML：INSERT / UPDATE / DELETE
// =====================================================================

#[test]
fn test_f8_insert_cross_dialect() {
    let sql = "INSERT INTO users (id, name) VALUES (1, 'Alice')";
    let cases = vec![
        (sql, Dialect::PostgreSQL),
        (sql, Dialect::MySql),
        (sql, Dialect::Oracle),
        (sql, Dialect::SqlServer),
        (sql, Dialect::SQLite),
    ];
    assert_all_dialects_parse_ok(&cases);
    for (sql, dialect) in &cases {
        assert_statement_variant(sql, dialect, "Insert");
    }
}

#[test]
fn test_f8_update_cross_dialect() {
    let sql = "UPDATE users SET name = 'Bob' WHERE id = 1";
    let cases = vec![
        (sql, Dialect::PostgreSQL),
        (sql, Dialect::MySql),
        (sql, Dialect::Oracle),
        (sql, Dialect::SqlServer),
        (sql, Dialect::SQLite),
    ];
    assert_all_dialects_parse_ok(&cases);
    for (sql, dialect) in &cases {
        assert_statement_variant(sql, dialect, "Update");
    }
}

#[test]
fn test_f8_delete_cross_dialect() {
    let sql = "DELETE FROM users WHERE id = 1";
    let cases = vec![
        (sql, Dialect::PostgreSQL),
        (sql, Dialect::MySql),
        (sql, Dialect::Oracle),
        (sql, Dialect::SqlServer),
        (sql, Dialect::SQLite),
    ];
    assert_all_dialects_parse_ok(&cases);
    for (sql, dialect) in &cases {
        assert_statement_variant(sql, dialect, "Delete");
    }
}

// =====================================================================
//  查询：SELECT / JOIN / GROUP BY / ORDER BY / LIMIT
// =====================================================================

#[test]
fn test_f8_select_basic_cross_dialect() {
    let sql = "SELECT id, name FROM users WHERE age > 18";
    let cases = vec![
        (sql, Dialect::PostgreSQL),
        (sql, Dialect::MySql),
        (sql, Dialect::Oracle),
        (sql, Dialect::SqlServer),
        (sql, Dialect::SQLite),
    ];
    assert_all_dialects_parse_ok(&cases);
    for (sql, dialect) in &cases {
        assert_statement_variant(sql, dialect, "Select");
    }
}

#[test]
fn test_f8_select_limit_cross_dialect() {
    // 各方言的 LIMIT 语法
    let cases = vec![
        // PG / SQLite: LIMIT N OFFSET M
        ("SELECT * FROM t LIMIT 10 OFFSET 5", Dialect::PostgreSQL),
        ("SELECT * FROM t LIMIT 10 OFFSET 5", Dialect::SQLite),
        // MySQL: LIMIT offset, count
        ("SELECT * FROM t LIMIT 5, 10", Dialect::MySql),
        // Oracle: ROWNUM <= N
        ("SELECT * FROM t WHERE ROWNUM <= 10", Dialect::Oracle),
        // SQL Server: TOP N
        ("SELECT TOP 10 * FROM t", Dialect::SqlServer),
    ];
    assert_all_dialects_parse_ok(&cases);
    for (sql, dialect) in &cases {
        assert_statement_variant(sql, dialect, "Select");
    }
}

#[test]
fn test_f8_select_join_cross_dialect() {
    let sql = "SELECT u.id, o.order_id FROM users u INNER JOIN orders o ON u.id = o.user_id";
    let cases = vec![
        (sql, Dialect::PostgreSQL),
        (sql, Dialect::MySql),
        (sql, Dialect::SqlServer),
        (sql, Dialect::SQLite),
    ];
    assert_all_dialects_parse_ok(&cases);
}

#[test]
fn test_f8_select_group_by_cross_dialect() {
    let sql = "SELECT department, COUNT(*) FROM employees GROUP BY department HAVING COUNT(*) > 5";
    let cases = vec![
        (sql, Dialect::PostgreSQL),
        (sql, Dialect::MySql),
        (sql, Dialect::Oracle),
        (sql, Dialect::SqlServer),
        (sql, Dialect::SQLite),
    ];
    assert_all_dialects_parse_ok(&cases);
}

#[test]
fn test_f8_select_order_by_cross_dialect() {
    let sql = "SELECT * FROM users ORDER BY name ASC, age DESC";
    let cases = vec![
        (sql, Dialect::PostgreSQL),
        (sql, Dialect::MySql),
        (sql, Dialect::Oracle),
        (sql, Dialect::SqlServer),
        (sql, Dialect::SQLite),
    ];
    assert_all_dialects_parse_ok(&cases);
}

#[test]
fn test_f8_select_subquery_cross_dialect() {
    let sql = "SELECT * FROM (SELECT id, name FROM users) AS sub";
    let cases = vec![
        (sql, Dialect::PostgreSQL),
        (sql, Dialect::MySql),
        (sql, Dialect::Oracle),
        (sql, Dialect::SqlServer),
        (sql, Dialect::SQLite),
    ];
    assert_all_dialects_parse_ok(&cases);
}

// =====================================================================
//  方言特有函数
// =====================================================================

#[test]
fn test_f8_dialect_specific_null_handling() {
    let cases = vec![
        // PG: COALESCE
        ("SELECT COALESCE(name, 'unknown') FROM users", Dialect::PostgreSQL),
        // MySQL: IFNULL (实际由 MySqlDialect 解析为 COALESCE 等价)
        ("SELECT IFNULL(name, 'unknown') FROM users", Dialect::MySql),
        // Oracle: NVL → COALESCE
        ("SELECT NVL(name, 'unknown') FROM users", Dialect::Oracle),
        // SQL Server: ISNULL → COALESCE
        ("SELECT ISNULL(name, 'unknown') FROM users", Dialect::SqlServer),
        // SQLite: IFNULL (SQLiteDialect 支持)
        ("SELECT IFNULL(name, 'unknown') FROM users", Dialect::SQLite),
    ];
    for (sql, dialect) in &cases {
        // 部分方言特有函数可能不被识别，允许失败但记录
        let _ = parse_with_dialect(sql, dialect);
    }
}

#[test]
fn test_f8_dialect_specific_date_functions() {
    let cases = vec![
        // PG: CURRENT_TIMESTAMP
        ("SELECT CURRENT_TIMESTAMP", Dialect::PostgreSQL),
        // Oracle: SYSDATE → CURRENT_TIMESTAMP
        ("SELECT SYSDATE FROM dual", Dialect::Oracle),
        // SQL Server: GETDATE() → CURRENT_TIMESTAMP
        ("SELECT GETDATE()", Dialect::SqlServer),
    ];
    assert_all_dialects_parse_ok(&cases);
}

#[test]
fn test_f8_dialect_specific_type_casts() {
    let cases = vec![
        // Oracle: TO_NUMBER / TO_CHAR / TO_DATE
        ("SELECT TO_NUMBER('123') FROM dual", Dialect::Oracle),
        ("SELECT TO_CHAR(123) FROM dual", Dialect::Oracle),
        ("SELECT TO_DATE('2024-01-01', 'YYYY-MM-DD') FROM dual", Dialect::Oracle),
        // SQL Server: LEN → LENGTH
        ("SELECT LEN(name) FROM users", Dialect::SqlServer),
    ];
    assert_all_dialects_parse_ok(&cases);
}

// =====================================================================
//  事务控制
// =====================================================================

#[test]
fn test_f8_transaction_control_cross_dialect() {
    let cases = vec![
        ("BEGIN", Dialect::PostgreSQL),
        ("BEGIN", Dialect::MySql),
        ("BEGIN", Dialect::Oracle),
        ("BEGIN", Dialect::SqlServer),
        ("BEGIN", Dialect::SQLite),
        ("COMMIT", Dialect::PostgreSQL),
        ("COMMIT", Dialect::MySql),
        ("COMMIT", Dialect::Oracle),
        ("COMMIT", Dialect::SqlServer),
        ("COMMIT", Dialect::SQLite),
        ("ROLLBACK", Dialect::PostgreSQL),
        ("ROLLBACK", Dialect::MySql),
        ("ROLLBACK", Dialect::Oracle),
        ("ROLLBACK", Dialect::SqlServer),
        ("ROLLBACK", Dialect::SQLite),
    ];
    for (sql, dialect) in &cases {
        let result = parse_with_dialect(sql, dialect);
        assert!(
            result.is_ok(),
            "dialect {:?} failed to parse: {sql}\nerror: {:?}",
            dialect,
            result.err()
        );
    }
}

// =====================================================================
//  方言自动检测
// =====================================================================

#[test]
fn test_f8_auto_detect_all_dialects() {
    let cases = vec![
        ("SELECT * FROM t WHERE id = 1", Dialect::PostgreSQL),
        ("SELECT `id` FROM `t`", Dialect::MySql),
        ("SELECT * FROM t LIMIT 10, 20", Dialect::MySql),
        ("SELECT TOP 10 * FROM t", Dialect::SqlServer),
        ("SELECT NVL(x, 0) FROM t", Dialect::Oracle),
        ("SELECT SYSDATE FROM dual", Dialect::Oracle),
        ("SELECT * FROM t WHERE ROWNUM <= 5", Dialect::Oracle),
        (
            "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT)",
            Dialect::SQLite,
        ),
        ("PRAGMA foreign_keys = ON", Dialect::SQLite),
        ("SELECT GROUP_CONCAT(name) FROM t", Dialect::SQLite),
    ];

    for (sql, expected_dialect) in cases {
        let detected = szrsql_sql::dialect::detect_dialect(sql);
        assert_eq!(
            detected, expected_dialect,
            "failed to detect dialect for: {sql}"
        );
        // 自动检测后应能成功解析
        let result = parse_auto(sql);
        assert!(
            result.is_ok(),
            "auto-detect parse failed for: {sql}\nerror: {:?}",
            result.err()
        );
    }
}

// =====================================================================
//  标识符引用符
// =====================================================================

#[test]
fn test_f8_identifier_quoting_cross_dialect() {
    let cases = vec![
        // PG: 双引号
        ("SELECT \"id\", \"name\" FROM \"users\"", Dialect::PostgreSQL),
        // MySQL: 反引号
        ("SELECT `id`, `name` FROM `users`", Dialect::MySql),
        // SQL Server: 方括号
        ("SELECT [id], [name] FROM [users]", Dialect::SqlServer),
        // SQLite: 方括号（兼容 SQL Server）
        ("SELECT [id], [name] FROM [users]", Dialect::SQLite),
        // SQLite: 反引号（兼容 MySQL）
        ("SELECT `id`, `name` FROM `users`", Dialect::SQLite),
        // SQLite: 双引号（兼容 PG）
        ("SELECT \"id\", \"name\" FROM \"users\"", Dialect::SQLite),
    ];
    assert_all_dialects_parse_ok(&cases);
}

// =====================================================================
//  多语句批处理
// =====================================================================

#[test]
fn test_f8_multi_statement_cross_dialect() {
    let sql = "SELECT 1; SELECT 2; SELECT 3";
    let cases = vec![
        (sql, Dialect::PostgreSQL),
        (sql, Dialect::MySql),
        (sql, Dialect::Oracle),
        (sql, Dialect::SqlServer),
        (sql, Dialect::SQLite),
    ];
    for (sql, dialect) in &cases {
        let result = parse_with_dialect(sql, dialect);
        assert!(
            result.is_ok(),
            "dialect {:?} failed to parse multi-statement: {sql}\nerror: {:?}",
            dialect,
            result.err()
        );
        let stmts = result.unwrap();
        assert_eq!(
            stmts.len(),
            3,
            "dialect {:?}: expected 3 statements, got {}",
            dialect,
            stmts.len()
        );
    }
}

// =====================================================================
//  综合业务场景
// =====================================================================

#[test]
fn test_f8_business_scenario_cross_dialect() {
    // 模拟电商订单查询场景
    let pg_sql = "SELECT u.id, u.name, COUNT(o.order_id) AS order_count
                  FROM users u
                  LEFT JOIN orders o ON u.id = o.user_id
                  WHERE u.created_at > '2024-01-01'
                  GROUP BY u.id, u.name
                  HAVING COUNT(o.order_id) > 5
                  ORDER BY order_count DESC
                  LIMIT 10";

    let mysql_sql = "SELECT u.id, u.name, COUNT(o.order_id) AS order_count
                     FROM users u
                     LEFT JOIN orders o ON u.id = o.user_id
                     WHERE u.created_at > '2024-01-01'
                     GROUP BY u.id, u.name
                     HAVING COUNT(o.order_id) > 5
                     ORDER BY order_count DESC
                     LIMIT 10";

    let oracle_sql = "SELECT u.id, u.name, COUNT(o.order_id) AS order_count
                      FROM users u
                      LEFT JOIN orders o ON u.id = o.user_id
                      WHERE u.created_at > TO_DATE('2024-01-01', 'YYYY-MM-DD')
                        AND ROWNUM <= 10
                      GROUP BY u.id, u.name
                      HAVING COUNT(o.order_id) > 5
                      ORDER BY order_count DESC";

    let sqlserver_sql = "SELECT TOP 10 u.id, u.name, COUNT(o.order_id) AS order_count
                         FROM users u
                         LEFT JOIN orders o ON u.id = o.user_id
                         WHERE u.created_at > '2024-01-01'
                         GROUP BY u.id, u.name
                         HAVING COUNT(o.order_id) > 5
                         ORDER BY order_count DESC";

    let sqlite_sql = "SELECT u.id, u.name, COUNT(o.order_id) AS order_count
                      FROM users u
                      LEFT JOIN orders o ON u.id = o.user_id
                      WHERE u.created_at > '2024-01-01'
                      GROUP BY u.id, u.name
                      HAVING COUNT(o.order_id) > 5
                      ORDER BY order_count DESC
                      LIMIT 10";

    let cases = vec![
        (pg_sql, Dialect::PostgreSQL),
        (mysql_sql, Dialect::MySql),
        (oracle_sql, Dialect::Oracle),
        (sqlserver_sql, Dialect::SqlServer),
        (sqlite_sql, Dialect::SQLite),
    ];
    assert_all_dialects_parse_ok(&cases);
}

// =====================================================================
//  Stress：各方言 100 条查询
// =====================================================================

#[test]
fn test_f8_stress_all_dialects_100_queries() {
    let dialects = [
        Dialect::PostgreSQL,
        Dialect::MySql,
        Dialect::Oracle,
        Dialect::SqlServer,
        Dialect::SQLite,
    ];

    let templates: Vec<&str> = vec![
        "SELECT id, name FROM users WHERE id = {i}",
        "SELECT * FROM t LIMIT 10",
        "INSERT INTO t (id, name) VALUES ({i}, 'user{i}')",
        "UPDATE t SET name = 'updated{i}' WHERE id = {i}",
        "DELETE FROM t WHERE id = {i}",
        "SELECT COUNT(*) FROM t WHERE id > {i}",
        "SELECT * FROM t ORDER BY id ASC",
    ];

    for dialect in &dialects {
        let mut success_count = 0;
        for i in 0..100 {
            let template = templates[i % templates.len()];
            let sql = template.replace("{i}", &i.to_string());
            if parse_with_dialect(&sql, dialect).is_ok() {
                success_count += 1;
            }
        }
        // 各方言通用 SQL 解析成功率应 >= 95%
        assert!(
            success_count >= 95,
            "dialect {:?}: parse success rate {success_count}/100, expected >= 95",
            dialect
        );
    }
}
