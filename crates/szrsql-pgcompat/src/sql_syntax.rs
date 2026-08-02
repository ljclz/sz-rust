//! SQL 语法兼容性测试模块。
//!
//! 验证 SzRSQL 解析器能否正确解析 PostgreSQL 特有语法。
//! 使用 `szrsql_sql::dialect::parse_with_dialect` 以 PostgreSQL 方言解析 SQL，
//! 根据解析结果判定兼容性状态。
//!
//! # 测试分类
//!
//! - [`SyntaxCategory::Ddl`]：DDL 语句（CREATE/ALTER/DROP TABLE/INDEX/VIEW）
//! - [`SyntaxCategory::Dml`]：DML 语句（SELECT/INSERT/UPDATE/DELETE）
//! - [`SyntaxCategory::Function`]：内置函数调用
//! - [`SyntaxCategory::Type`]：PostgreSQL 特有数据类型
//! - [`SyntaxCategory::Operator`]：PostgreSQL 特有运算符

use crate::CompatStatus;
use serde::{Deserialize, Serialize};
use szrsql_sql::dialect::{parse_with_dialect, Dialect};

/// SQL 语法兼容性检查分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyntaxCategory {
    /// DDL 语句
    Ddl,
    /// DML 语句
    Dml,
    /// 内置函数
    Function,
    /// 数据类型
    Type,
    /// 运算符
    Operator,
}

/// 单项 SQL 语法兼容性检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlSyntaxResult {
    /// 检查项名称
    pub name: String,
    /// 分类
    pub category: SyntaxCategory,
    /// 被测试的 SQL 语句
    pub sql: String,
    /// 兼容性状态
    pub status: CompatStatus,
    /// 详细说明
    pub detail: String,
}

/// SQL 语法兼容性测试套件
pub struct SqlSyntaxCompat;

impl SqlSyntaxCompat {
    /// 运行全部 SQL 语法兼容性检查
    pub fn run_all() -> Vec<SqlSyntaxResult> {
        let mut results = Vec::new();
        results.extend(Self::test_ddl());
        results.extend(Self::test_dml());
        results.extend(Self::test_functions());
        results.extend(Self::test_types());
        results.extend(Self::test_operators());
        results
    }

    /// 测试单条 SQL 的解析兼容性
    fn check(name: &str, category: SyntaxCategory, sql: &str) -> SqlSyntaxResult {
        match parse_with_dialect(sql, &Dialect::PostgreSQL) {
            Ok(statements) => {
                if statements.is_empty() {
                    SqlSyntaxResult {
                        name: name.to_string(),
                        category,
                        sql: sql.to_string(),
                        status: CompatStatus::Fail,
                        detail: "解析成功但返回空语句列表".to_string(),
                    }
                } else {
                    SqlSyntaxResult {
                        name: name.to_string(),
                        category,
                        sql: sql.to_string(),
                        status: CompatStatus::Pass,
                        detail: format!("解析成功，返回 {} 条语句", statements.len()),
                    }
                }
            }
            Err(e) => SqlSyntaxResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Fail,
                detail: format!("解析失败: {e}"),
            },
        }
    }

    /// DDL 兼容性测试
    fn test_ddl() -> Vec<SqlSyntaxResult> {
        vec![
            Self::check(
                "CREATE TABLE 基本语法",
                SyntaxCategory::Ddl,
                "CREATE TABLE users (id BIGINT PRIMARY KEY, name TEXT NOT NULL)",
            ),
            Self::check(
                "CREATE TABLE with DEFAULT",
                SyntaxCategory::Ddl,
                "CREATE TABLE t (id INT, created_at TIMESTAMP DEFAULT NOW())",
            ),
            Self::check(
                "CREATE TABLE with CHECK 约束",
                SyntaxCategory::Ddl,
                "CREATE TABLE t (id INT, age INT CHECK (age >= 0))",
            ),
            Self::check(
                "CREATE TABLE with FOREIGN KEY",
                SyntaxCategory::Ddl,
                "CREATE TABLE orders (id INT, user_id INT REFERENCES users(id))",
            ),
            Self::check(
                "CREATE TABLE with UNIQUE",
                SyntaxCategory::Ddl,
                "CREATE TABLE t (id INT, email TEXT UNIQUE)",
            ),
            Self::check(
                "CREATE INDEX 基本语法",
                SyntaxCategory::Ddl,
                "CREATE INDEX idx_name ON users(name)",
            ),
            Self::check(
                "CREATE UNIQUE INDEX",
                SyntaxCategory::Ddl,
                "CREATE UNIQUE INDEX idx_email ON users(email)",
            ),
            Self::check(
                "DROP TABLE",
                SyntaxCategory::Ddl,
                "DROP TABLE IF EXISTS users",
            ),
            Self::check(
                "ALTER TABLE ADD COLUMN",
                SyntaxCategory::Ddl,
                "ALTER TABLE users ADD COLUMN age INT",
            ),
            Self::check(
                "CREATE VIEW",
                SyntaxCategory::Ddl,
                "CREATE VIEW v_users AS SELECT id, name FROM users",
            ),
        ]
    }

    /// DML 兼容性测试
    fn test_dml() -> Vec<SqlSyntaxResult> {
        vec![
            Self::check(
                "SELECT with LIMIT",
                SyntaxCategory::Dml,
                "SELECT * FROM users LIMIT 10",
            ),
            Self::check(
                "SELECT with LIMIT OFFSET",
                SyntaxCategory::Dml,
                "SELECT * FROM users LIMIT 10 OFFSET 20",
            ),
            Self::check(
                "SELECT with WHERE",
                SyntaxCategory::Dml,
                "SELECT id, name FROM users WHERE age > 18 AND status = 'active'",
            ),
            Self::check(
                "SELECT with JOIN",
                SyntaxCategory::Dml,
                "SELECT u.id, o.id FROM users u JOIN orders o ON u.id = o.user_id",
            ),
            Self::check(
                "SELECT with LEFT JOIN",
                SyntaxCategory::Dml,
                "SELECT u.id FROM users u LEFT JOIN orders o ON u.id = o.user_id",
            ),
            Self::check(
                "SELECT with GROUP BY",
                SyntaxCategory::Dml,
                "SELECT user_id, COUNT(*) FROM orders GROUP BY user_id",
            ),
            Self::check(
                "SELECT with HAVING",
                SyntaxCategory::Dml,
                "SELECT user_id, COUNT(*) FROM orders GROUP BY user_id HAVING COUNT(*) > 5",
            ),
            Self::check(
                "SELECT with ORDER BY",
                SyntaxCategory::Dml,
                "SELECT * FROM users ORDER BY name ASC, id DESC",
            ),
            Self::check(
                "SELECT with DISTINCT",
                SyntaxCategory::Dml,
                "SELECT DISTINCT department FROM employees",
            ),
            Self::check(
                "SELECT with subquery",
                SyntaxCategory::Dml,
                "SELECT * FROM users WHERE id IN (SELECT user_id FROM orders)",
            ),
            Self::check(
                "SELECT with CTE",
                SyntaxCategory::Dml,
                "WITH active_users AS (SELECT * FROM users WHERE status = 'active') SELECT * FROM active_users",
            ),
            Self::check(
                "INSERT with VALUES",
                SyntaxCategory::Dml,
                "INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob')",
            ),
            Self::check(
                "INSERT with RETURNING",
                SyntaxCategory::Dml,
                "INSERT INTO users (id, name) VALUES (1, 'Alice') RETURNING id",
            ),
            Self::check(
                "UPDATE with WHERE",
                SyntaxCategory::Dml,
                "UPDATE users SET name = 'Bob' WHERE id = 1",
            ),
            Self::check(
                "DELETE with WHERE",
                SyntaxCategory::Dml,
                "DELETE FROM users WHERE id = 1",
            ),
            Self::check(
                "UPSERT (ON CONFLICT)",
                SyntaxCategory::Dml,
                "INSERT INTO users (id, name) VALUES (1, 'Alice') ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
            ),
        ]
    }

    /// 内置函数兼容性测试
    fn test_functions() -> Vec<SqlSyntaxResult> {
        vec![
            Self::check(
                "COUNT 函数",
                SyntaxCategory::Function,
                "SELECT COUNT(*) FROM users",
            ),
            Self::check(
                "SUM/AVG/MAX/MIN 函数",
                SyntaxCategory::Function,
                "SELECT SUM(amount), AVG(amount), MAX(amount), MIN(amount) FROM orders",
            ),
            Self::check(
                "字符串函数",
                SyntaxCategory::Function,
                "SELECT UPPER(name), LOWER(name), LENGTH(name) FROM users",
            ),
            Self::check(
                "COALESCE 函数",
                SyntaxCategory::Function,
                "SELECT COALESCE(nickname, 'unknown') FROM users",
            ),
            Self::check(
                "NOW/CURRENT_TIMESTAMP",
                SyntaxCategory::Function,
                "SELECT NOW(), CURRENT_TIMESTAMP",
            ),
            Self::check(
                "CAST 表达式",
                SyntaxCategory::Function,
                "SELECT CAST(id AS TEXT) FROM users",
            ),
            Self::check(
                "CASE WHEN 表达式",
                SyntaxCategory::Function,
                "SELECT CASE WHEN age >= 18 THEN 'adult' ELSE 'minor' END FROM users",
            ),
            Self::check(
                "SUBSTRING 函数",
                SyntaxCategory::Function,
                "SELECT SUBSTRING(name FROM 1 FOR 3) FROM users",
            ),
        ]
    }

    /// PostgreSQL 特有数据类型测试
    fn test_types() -> Vec<SqlSyntaxResult> {
        vec![
            Self::check(
                "SERIAL 类型",
                SyntaxCategory::Type,
                "CREATE TABLE t (id SERIAL PRIMARY KEY)",
            ),
            Self::check(
                "BIGSERIAL 类型",
                SyntaxCategory::Type,
                "CREATE TABLE t (id BIGSERIAL PRIMARY KEY)",
            ),
            Self::check(
                "TEXT 类型",
                SyntaxCategory::Type,
                "CREATE TABLE t (content TEXT)",
            ),
            Self::check(
                "TIMESTAMP 类型",
                SyntaxCategory::Type,
                "CREATE TABLE t (created_at TIMESTAMP)",
            ),
            Self::check(
                "TIMESTAMPTZ 类型",
                SyntaxCategory::Type,
                "CREATE TABLE t (created_at TIMESTAMPTZ)",
            ),
            Self::check(
                "DATE 类型",
                SyntaxCategory::Type,
                "CREATE TABLE t (birthday DATE)",
            ),
            Self::check(
                "BOOLEAN 类型",
                SyntaxCategory::Type,
                "CREATE TABLE t (is_active BOOLEAN)",
            ),
            Self::check(
                "NUMERIC/DECIMAL 类型",
                SyntaxCategory::Type,
                "CREATE TABLE t (price NUMERIC(10, 2))",
            ),
            Self::check(
                "JSON/JSONB 类型",
                SyntaxCategory::Type,
                "CREATE TABLE t (data JSONB)",
            ),
            Self::check(
                "BYTEA 类型",
                SyntaxCategory::Type,
                "CREATE TABLE t (blob BYTEA)",
            ),
            Self::check(
                "UUID 类型",
                SyntaxCategory::Type,
                "CREATE TABLE t (id UUID PRIMARY KEY)",
            ),
            Self::check(
                "ARRAY 类型",
                SyntaxCategory::Type,
                "CREATE TABLE t (tags TEXT[])",
            ),
        ]
    }

    /// PostgreSQL 特有运算符测试
    fn test_operators() -> Vec<SqlSyntaxResult> {
        vec![
            Self::check(
                "字符串连接运算符 ||",
                SyntaxCategory::Operator,
                "SELECT first_name || ' ' || last_name FROM users",
            ),
            Self::check(
                "ILIKE 运算符（大小写不敏感 LIKE）",
                SyntaxCategory::Operator,
                "SELECT * FROM users WHERE name ILIKE '%alice%'",
            ),
            Self::check(
                "SIMILAR TO 运算符",
                SyntaxCategory::Operator,
                "SELECT * FROM users WHERE name SIMILAR TO 'A%'",
            ),
            Self::check(
                "IS NULL / IS NOT NULL",
                SyntaxCategory::Operator,
                "SELECT * FROM users WHERE nickname IS NULL",
            ),
            Self::check(
                "IS DISTINCT FROM",
                SyntaxCategory::Operator,
                "SELECT * FROM users WHERE id IS DISTINCT FROM 1",
            ),
            Self::check(
                "ANY/ALL 运算符",
                SyntaxCategory::Operator,
                "SELECT * FROM users WHERE id = ANY(ARRAY[1, 2, 3])",
            ),
            Self::check(
                ":: 类型转换运算符",
                SyntaxCategory::Operator,
                "SELECT id::TEXT FROM users",
            ),
            Self::check(
                "BETWEEN 运算符",
                SyntaxCategory::Operator,
                "SELECT * FROM users WHERE age BETWEEN 18 AND 65",
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_returns_nonempty() {
        let results = SqlSyntaxCompat::run_all();
        assert!(!results.is_empty(), "应返回至少一项检查结果");
    }

    #[test]
    fn covers_all_categories() {
        let results = SqlSyntaxCompat::run_all();
        let has_ddl = results.iter().any(|r| r.category == SyntaxCategory::Ddl);
        let has_dml = results.iter().any(|r| r.category == SyntaxCategory::Dml);
        let has_func = results
            .iter()
            .any(|r| r.category == SyntaxCategory::Function);
        let has_type = results.iter().any(|r| r.category == SyntaxCategory::Type);
        let has_op = results
            .iter()
            .any(|r| r.category == SyntaxCategory::Operator);
        assert!(
            has_ddl && has_dml && has_func && has_type && has_op,
            "应覆盖所有分类"
        );
    }

    #[test]
    fn basic_select_should_pass() {
        let result = SqlSyntaxCompat::check("basic select", SyntaxCategory::Dml, "SELECT 1");
        assert_eq!(result.status, CompatStatus::Pass, "SELECT 1 应可解析");
    }

    #[test]
    fn invalid_sql_should_fail() {
        let result = SqlSyntaxCompat::check("invalid", SyntaxCategory::Dml, "SELECT FROM WHERE");
        assert_eq!(result.status, CompatStatus::Fail, "非法 SQL 应解析失败");
    }
}
