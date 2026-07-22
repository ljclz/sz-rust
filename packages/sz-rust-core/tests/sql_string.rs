//! Phase 0.8 — 编译时 SQL 检查宏集成测试
//!
//! 验证 `sz_rust_core::sql_string!` 宏的重导出可用性。
//!
//! 注意：`sql_string!` 是编译时宏，校验失败会触发 `compile_error!`，
//! 因此本测试文件本身通过编译即证明合法 SQL 被正确接受。
//! 对于非法 SQL 的拒绝路径，由 `sz-orm-macros` 自身的单元测试覆盖
//! （见 `sz-orm/packages/sz-orm-macros/src/lib.rs` 中的 `validate_sql_content` 测试）。

use sz_rust_core::sql_string;

/// 合法 SELECT 应通过编译时校验，返回 &str
#[test]
fn test_sql_string_valid_select() {
    let sql = sql_string!("SELECT * FROM users WHERE id = 1");
    assert_eq!(sql, "SELECT * FROM users WHERE id = 1");
}

/// 带参数占位符的 SELECT 应通过校验
#[test]
fn test_sql_string_valid_select_with_params() {
    let sql = sql_string!("SELECT id, name FROM users WHERE id = ? AND status = ?");
    assert!(sql.contains("SELECT"));
    assert!(sql.contains("FROM"));
    assert_eq!(sql.matches('?').count(), 2);
}

/// 带参数数量校验的 SELECT 应通过
#[test]
fn test_sql_string_with_param_count_match() {
    let sql = sql_string!("SELECT * FROM users WHERE id = ?"; params: 1);
    assert_eq!(sql.matches('?').count(), 1);
}

/// 合法 INSERT 应通过编译时校验
#[test]
fn test_sql_string_valid_insert() {
    let sql = sql_string!("INSERT INTO users (id, name) VALUES (1, 'Alice')");
    assert!(sql.starts_with("INSERT"));
    assert!(sql.contains("INTO"));
    assert!(sql.contains("VALUES"));
}

/// 合法 UPDATE 应通过编译时校验
#[test]
fn test_sql_string_valid_update() {
    let sql = sql_string!("UPDATE users SET name = 'Bob' WHERE id = 1");
    assert!(sql.starts_with("UPDATE"));
    assert!(sql.contains("SET"));
}

/// 合法 DELETE 应通过编译时校验
#[test]
fn test_sql_string_valid_delete() {
    let sql = sql_string!("DELETE FROM users WHERE id = 1");
    assert!(sql.starts_with("DELETE"));
    assert!(sql.contains("FROM"));
}

/// 带子查询的复杂 SELECT 应通过校验
#[test]
fn test_sql_string_complex_select_with_subquery() {
    let sql = sql_string!(
        "SELECT id, name FROM users WHERE id IN (SELECT user_id FROM orders WHERE total > 100)"
    );
    assert!(sql.contains("IN"));
    assert!(sql.contains("SELECT"));
}

/// 带括号的合法 SQL 应通过括号平衡校验
#[test]
fn test_sql_string_balanced_parens() {
    let sql = sql_string!("SELECT * FROM users WHERE (id = 1 OR id = 2) AND status = 1");
    assert_eq!(sql.matches('(').count(), sql.matches(')').count());
}

/// 宏返回的 SQL 是 &'static str 类型
#[test]
fn test_sql_string_returns_static_str() {
    let sql: &'static str = sql_string!("SELECT 1 FROM dual");
    assert_eq!(sql, "SELECT 1 FROM dual");
}

/// query! 宏也应可用（与 sql_string! 等价，无 db-verify feature 时）
#[test]
fn test_query_macro_valid_select() {
    use sz_rust_core::query;
    let sql = query!("SELECT * FROM users WHERE id = 1");
    assert_eq!(sql, "SELECT * FROM users WHERE id = 1");
}
