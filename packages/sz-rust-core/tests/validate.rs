//! Phase 0.9 — 运行时 SQL 校验集成测试
//!
//! 验证 `sz_rust_core` 重导出的 `sz-orm-sql-validator` API 可用性，
//! 覆盖合法 SQL 接受、非法 SQL 拒绝、SQL 注入检测、参数数量校验、
//! 标识符校验、语句类型检测等核心场景。

use sz_rust_core::{
    detect_statement_type, validate, validate_column_name, validate_delete, validate_insert,
    validate_parameter_count, validate_select, validate_sql, validate_sql_runtime,
    validate_table_name, validate_update, SqlStatementType, SqlValidationError,
};

// ============================================================================
// validate_sql_runtime 便捷函数测试
// ============================================================================

#[test]
fn test_validate_sql_runtime_valid_select() {
    assert!(validate_sql_runtime("SELECT * FROM users WHERE id = 1").is_ok());
}

#[test]
fn test_validate_sql_runtime_invalid_select_missing_from() {
    let result = validate_sql_runtime("SELECT * users");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("FROM"));
}

#[test]
fn test_validate_sql_runtime_injection_or_1_1() {
    let result = validate_sql_runtime("SELECT * FROM users WHERE name = 'x' OR '1'='1'");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("injection"));
}

#[test]
fn test_validate_sql_runtime_injection_drop_table() {
    let result = validate_sql_runtime("'; DROP TABLE users; --");
    assert!(result.is_err());
}

#[test]
fn test_validate_sql_runtime_injection_union_select() {
    let result = validate_sql_runtime("1 UNION SELECT * FROM users");
    assert!(result.is_err());
}

#[test]
fn test_validate_sql_runtime_injection_comment() {
    let result = validate_sql_runtime("SELECT * FROM users -- comment");
    assert!(result.is_err());
}

#[test]
fn test_validate_sql_runtime_empty_sql() {
    assert!(validate_sql_runtime("").is_err());
    assert!(validate_sql_runtime("   ").is_err());
}

// ============================================================================
// validate_select / insert / update / delete 类型特定校验
// ============================================================================

#[test]
fn test_validate_select_valid() {
    assert!(validate_select("SELECT id, name FROM users WHERE id = 1").is_ok());
    assert!(
        validate_select("SELECT u.id FROM users u INNER JOIN orders o ON u.id = o.user_id").is_ok()
    );
}

#[test]
fn test_validate_select_missing_from() {
    assert!(validate_select("SELECT *").is_err());
}

#[test]
fn test_validate_insert_valid() {
    assert!(validate_insert("INSERT INTO users (name) VALUES ('alice')").is_ok());
    assert!(validate_insert("INSERT INTO users (name, age) VALUES ('bob', 25)").is_ok());
}

#[test]
fn test_validate_insert_missing_values() {
    assert!(validate_insert("INSERT INTO users (name)").is_err());
}

#[test]
fn test_validate_update_valid() {
    assert!(validate_update("UPDATE users SET name = 'alice' WHERE id = 1").is_ok());
}

#[test]
fn test_validate_update_missing_set() {
    assert!(validate_update("UPDATE users WHERE id = 1").is_err());
}

#[test]
fn test_validate_delete_valid() {
    assert!(validate_delete("DELETE FROM users WHERE id = 1").is_ok());
}

#[test]
fn test_validate_delete_missing_from() {
    assert!(validate_delete("DELETE users").is_err());
}

// ============================================================================
// validate_sql 通用校验（自动检测类型）
// ============================================================================

#[test]
fn test_validate_sql_all_statement_types() {
    assert!(validate_sql("SELECT * FROM users").is_ok());
    assert!(validate_sql("INSERT INTO users (name) VALUES ('a')").is_ok());
    assert!(validate_sql("UPDATE users SET name = 'a' WHERE id = 1").is_ok());
    assert!(validate_sql("DELETE FROM users WHERE id = 1").is_ok());
    assert!(validate_sql("CREATE TABLE users (id INT)").is_ok());
}

#[test]
fn test_validate_sql_empty() {
    assert!(validate_sql("").is_err());
}

// ============================================================================
// validate 完整校验
// ============================================================================

#[test]
fn test_validate_complex_query() {
    let sql = "SELECT u.*, o.total FROM users u LEFT JOIN orders o ON u.id = o.user_id \
               WHERE u.status = 'active' AND u.created_at > '2024-01-01' \
               GROUP BY u.id HAVING COUNT(o.id) > 5 ORDER BY u.name ASC LIMIT 10 OFFSET 20";
    assert!(validate(sql).is_ok());
}

// ============================================================================
// detect_statement_type 语句类型检测
// ============================================================================

#[test]
fn test_detect_statement_type_all() {
    assert_eq!(
        detect_statement_type("SELECT * FROM t"),
        SqlStatementType::Select
    );
    assert_eq!(
        detect_statement_type("INSERT INTO t VALUES (1)"),
        SqlStatementType::Insert
    );
    assert_eq!(
        detect_statement_type("UPDATE t SET a=1"),
        SqlStatementType::Update
    );
    assert_eq!(
        detect_statement_type("DELETE FROM t"),
        SqlStatementType::Delete
    );
    assert_eq!(
        detect_statement_type("CREATE TABLE t"),
        SqlStatementType::Create
    );
    assert_eq!(
        detect_statement_type("DROP TABLE t"),
        SqlStatementType::Drop
    );
    assert_eq!(
        detect_statement_type("ALTER TABLE t ADD COLUMN a"),
        SqlStatementType::Alter
    );
    assert_eq!(
        detect_statement_type("TRUNCATE TABLE t"),
        SqlStatementType::Truncate
    );
    assert_eq!(
        detect_statement_type("EXPLAIN SELECT * FROM t"),
        SqlStatementType::Other
    );
}

// ============================================================================
// validate_parameter_count 参数数量校验
// ============================================================================

#[test]
fn test_validate_parameter_count_match() {
    assert!(validate_parameter_count("SELECT * FROM users WHERE id = ?", 1).is_ok());
    assert!(validate_parameter_count("SELECT * FROM users WHERE id = ? AND name = ?", 2).is_ok());
}

#[test]
fn test_validate_parameter_count_mismatch() {
    assert!(validate_parameter_count("SELECT * FROM users WHERE id = ?", 2).is_err());
    assert!(validate_parameter_count("SELECT * FROM users WHERE id = ? AND name = ?", 1).is_err());
}

#[test]
fn test_validate_parameter_count_postgresql_style() {
    // PostgreSQL 风格 $1 $2 也应被识别
    assert!(validate_parameter_count("SELECT * FROM users WHERE id = $1", 1).is_ok());
    assert!(validate_parameter_count("SELECT * FROM users WHERE id = $1 AND name = $2", 2).is_ok());
}

#[test]
fn test_validate_parameter_count_mismatch_error_type() {
    let result = validate_parameter_count("SELECT * FROM users WHERE id = ?", 2);
    assert!(matches!(
        result,
        Err(SqlValidationError::ParameterCountMismatch { .. })
    ));
}

// ============================================================================
// validate_table_name / validate_column_name 标识符校验
// ============================================================================

#[test]
fn test_validate_table_name_valid() {
    assert!(validate_table_name("users").is_ok());
    assert!(validate_table_name("user_orders").is_ok());
    assert!(validate_table_name("`users`").is_ok());
    assert!(validate_table_name("\"users\"").is_ok());
}

#[test]
fn test_validate_table_name_invalid() {
    assert!(validate_table_name("").is_err());
    assert!(validate_table_name("users; DROP TABLE").is_err());
    assert!(validate_table_name("users--").is_err());
}

#[test]
fn test_validate_column_name_valid() {
    assert!(validate_column_name("id").is_ok());
    assert!(validate_column_name("*").is_ok());
    assert!(validate_column_name("users.name").is_ok());
    assert!(validate_column_name("`name`").is_ok());
}

#[test]
fn test_validate_column_name_invalid() {
    assert!(validate_column_name("name; DROP").is_err());
    assert!(validate_column_name("name--").is_err());
}

// ============================================================================
// 错误类型变体覆盖
// ============================================================================

#[test]
fn test_error_syntax_error_variant() {
    let result = validate_select("NOT_SELECT");
    assert!(matches!(result, Err(SqlValidationError::SyntaxError(_))));
}

#[test]
fn test_error_missing_keyword_variant() {
    let result = validate_select("SELECT *");
    assert!(matches!(result, Err(SqlValidationError::MissingKeyword(_))));
}

#[test]
fn test_error_injection_detected_variant() {
    let result = validate_sql("SELECT * FROM users WHERE name = 'x' OR '1'='1'");
    assert!(matches!(
        result,
        Err(SqlValidationError::InjectionDetected(_))
    ));
}

#[test]
fn test_error_unbalanced_parentheses_variant() {
    let result = validate_sql("SELECT * FROM (users");
    assert!(matches!(
        result,
        Err(SqlValidationError::UnbalancedParentheses(_))
    ));
}

#[test]
fn test_error_unclosed_string_variant() {
    let result = validate_sql("SELECT * FROM users WHERE name = 'alice");
    assert!(matches!(result, Err(SqlValidationError::UnclosedString(_))));
}
