//! SQLite 兼容性测试模块。
//!
//! 验证 SzRSQL 解析器对 SQLite 方言的兼容性，覆盖：
//! - DDL：CREATE TABLE、AUTOINCREMENT、WITHOUT ROWID、PRAGMA
//! - DML：SELECT/INSERT/UPDATE/DELETE、LIMIT offset,count、REPLACE INTO
//! - 数据类型：INTEGER/REAL/TEXT/BLOB/NUMERIC、动态类型
//! - 函数：DATE/TIME/DATETIME/STRFTIME、JSON_EXTRACT、GROUP_CONCAT
//! - 运算符：GLOB、MATCH（FTS5）、|| 拼接
//! - 标识符：方括号、反引号、双引号

use crate::CompatStatus;
use serde::{Deserialize, Serialize};
use szrsql_sql::dialect::{parse_with_dialect, Dialect};

/// SQLite 兼容性检查分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SqliteCategory {
    /// DDL 语句
    Ddl,
    /// DML 语句
    Dml,
    /// 数据类型
    Type,
    /// 内置函数
    Function,
    /// 运算符
    Operator,
    /// 标识符
    Identifier,
    /// PRAGMA 指令
    Pragma,
}

/// 单项 SQLite 兼容性检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqliteCompatResult {
    /// 检查项名称
    pub name: String,
    /// 分类
    pub category: SqliteCategory,
    /// 被测试的 SQL 语句
    pub sql: String,
    /// 兼容性状态
    pub status: CompatStatus,
    /// 详细说明
    pub detail: String,
}

/// SQLite 兼容性测试套件
pub struct SqliteCompat;

impl SqliteCompat {
    /// 运行全部 SQLite 兼容性检查
    pub fn run_all() -> Vec<SqliteCompatResult> {
        let mut results = Vec::new();
        results.extend(Self::test_ddl());
        results.extend(Self::test_dml());
        results.extend(Self::test_types());
        results.extend(Self::test_functions());
        results.extend(Self::test_operators());
        results.extend(Self::test_identifiers());
        results.extend(Self::test_pragma());
        results
    }

    /// 测试单条 SQL 在 SQLite 方言下的解析兼容性
    fn check(name: &str, category: SqliteCategory, sql: &str) -> SqliteCompatResult {
        match parse_with_dialect(sql, &Dialect::SQLite) {
            Ok(stmts) if !stmts.is_empty() => SqliteCompatResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Pass,
                detail: format!("解析成功，返回 {} 条语句", stmts.len()),
            },
            Ok(_) => SqliteCompatResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Fail,
                detail: "解析成功但返回空语句列表".to_string(),
            },
            Err(e) => SqliteCompatResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Fail,
                detail: format!("解析失败: {e}"),
            },
        }
    }

    /// DDL 兼容性测试
    fn test_ddl() -> Vec<SqliteCompatResult> {
        vec![
            Self::check("CREATE TABLE 基本语法", SqliteCategory::Ddl,
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)"),
            Self::check("CREATE TABLE with AUTOINCREMENT", SqliteCategory::Ddl,
                "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)"),
            Self::check("CREATE TABLE WITHOUT ROWID", SqliteCategory::Ddl,
                "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT) WITHOUT ROWID"),
            Self::check("CREATE TABLE with DEFAULT", SqliteCategory::Ddl,
                "CREATE TABLE t (id INTEGER, name TEXT DEFAULT 'unknown', created_at TEXT DEFAULT CURRENT_TIMESTAMP)"),
            Self::check("CREATE TABLE with CHECK", SqliteCategory::Ddl,
                "CREATE TABLE t (age INTEGER CHECK(age > 0))"),
            Self::check("CREATE TABLE with UNIQUE", SqliteCategory::Ddl,
                "CREATE TABLE t (id INTEGER, email TEXT UNIQUE)"),
            Self::check("CREATE TABLE with FOREIGN KEY", SqliteCategory::Ddl,
                "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER, FOREIGN KEY(user_id) REFERENCES users(id))"),
            Self::check("CREATE TABLE IF NOT EXISTS", SqliteCategory::Ddl,
                "CREATE TABLE IF NOT EXISTS users (id INTEGER PRIMARY KEY, name TEXT)"),
            Self::check("CREATE INDEX", SqliteCategory::Ddl,
                "CREATE INDEX idx_name ON users(name)"),
            Self::check("CREATE UNIQUE INDEX", SqliteCategory::Ddl,
                "CREATE UNIQUE INDEX idx_email ON users(email)"),
            Self::check("DROP TABLE", SqliteCategory::Ddl,
                "DROP TABLE IF EXISTS users"),
            Self::check("ALTER TABLE ADD COLUMN", SqliteCategory::Ddl,
                "ALTER TABLE users ADD COLUMN email TEXT"),
            Self::check("ALTER TABLE DROP COLUMN", SqliteCategory::Ddl,
                "ALTER TABLE users DROP COLUMN email"),
            Self::check("ALTER TABLE RENAME COLUMN", SqliteCategory::Ddl,
                "ALTER TABLE users RENAME COLUMN name TO full_name"),
            Self::check("ALTER TABLE RENAME TO", SqliteCategory::Ddl,
                "ALTER TABLE users RENAME TO accounts"),
            Self::check("CREATE VIEW", SqliteCategory::Ddl,
                "CREATE VIEW IF NOT EXISTS v_users AS SELECT id, name FROM users"),
            Self::check("CREATE VIRTUAL TABLE FTS5", SqliteCategory::Ddl,
                "CREATE VIRTUAL TABLE docs USING fts5(title, body)"),
            Self::check("DROP VIEW", SqliteCategory::Ddl,
                "DROP VIEW IF EXISTS v_users"),
            Self::check("DROP INDEX", SqliteCategory::Ddl,
                "DROP INDEX IF EXISTS idx_name"),
        ]
    }

    /// DML 兼容性测试
    fn test_dml() -> Vec<SqliteCompatResult> {
        vec![
            Self::check("SELECT 基本语法", SqliteCategory::Dml,
                "SELECT id, name FROM users WHERE age > 18"),
            Self::check("SELECT LIMIT count", SqliteCategory::Dml,
                "SELECT * FROM t LIMIT 10"),
            Self::check("SELECT LIMIT offset, count", SqliteCategory::Dml,
                "SELECT * FROM t LIMIT 5, 10"),
            Self::check("SELECT LIMIT OFFSET", SqliteCategory::Dml,
                "SELECT * FROM t LIMIT 10 OFFSET 5"),
            Self::check("SELECT JOIN", SqliteCategory::Dml,
                "SELECT u.id, o.id FROM users u INNER JOIN orders o ON u.id = o.user_id"),
            Self::check("SELECT LEFT JOIN", SqliteCategory::Dml,
                "SELECT u.id FROM users u LEFT JOIN orders o ON u.id = o.user_id"),
            Self::check("SELECT GROUP BY", SqliteCategory::Dml,
                "SELECT department, COUNT(*) FROM employees GROUP BY department"),
            Self::check("SELECT HAVING", SqliteCategory::Dml,
                "SELECT department, AVG(salary) FROM employees GROUP BY department HAVING AVG(salary) > 50000"),
            Self::check("SELECT ORDER BY", SqliteCategory::Dml,
                "SELECT * FROM users ORDER BY name ASC, age DESC"),
            Self::check("SELECT DISTINCT", SqliteCategory::Dml,
                "SELECT DISTINCT department FROM employees"),
            Self::check("SELECT 子查询", SqliteCategory::Dml,
                "SELECT * FROM (SELECT id, name FROM users) AS sub"),
            Self::check("SELECT UNION", SqliteCategory::Dml,
                "SELECT id FROM users UNION SELECT id FROM orders"),
            Self::check("SELECT UNION ALL", SqliteCategory::Dml,
                "SELECT id FROM users UNION ALL SELECT id FROM orders"),
            Self::check("INSERT 基本语法", SqliteCategory::Dml,
                "INSERT INTO users (id, name) VALUES (1, 'Alice')"),
            Self::check("INSERT 多行", SqliteCategory::Dml,
                "INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob'), (3, 'Carol')"),
            Self::check("INSERT OR REPLACE", SqliteCategory::Dml,
                "INSERT OR REPLACE INTO users (id, name) VALUES (1, 'Alice')"),
            Self::check("INSERT OR IGNORE", SqliteCategory::Dml,
                "INSERT OR IGNORE INTO users (id, name) VALUES (1, 'Alice')"),
            Self::check("REPLACE INTO", SqliteCategory::Dml,
                "REPLACE INTO users (id, name) VALUES (1, 'Alice')"),
            Self::check("UPDATE", SqliteCategory::Dml,
                "UPDATE users SET name = 'Bob' WHERE id = 1"),
            Self::check("UPDATE 多列", SqliteCategory::Dml,
                "UPDATE users SET name = 'Bob', age = 30 WHERE id = 1"),
            Self::check("DELETE", SqliteCategory::Dml,
                "DELETE FROM users WHERE id = 1"),
            Self::check("SELECT with CTE", SqliteCategory::Dml,
                "WITH cte AS (SELECT id FROM users) SELECT * FROM cte"),
            Self::check("SELECT 递归 CTE", SqliteCategory::Dml,
                "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM r WHERE n < 10) SELECT n FROM r"),
            Self::check("SELECT with EXISTS", SqliteCategory::Dml,
                "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE o.user_id = u.id)"),
        ]
    }

    /// 数据类型兼容性测试
    fn test_types() -> Vec<SqliteCompatResult> {
        vec![
            Self::check("INTEGER 类型", SqliteCategory::Type,
                "CREATE TABLE t (id INTEGER)"),
            Self::check("INT 类型", SqliteCategory::Type,
                "CREATE TABLE t (id INT)"),
            Self::check("REAL 类型", SqliteCategory::Type,
                "CREATE TABLE t (price REAL)"),
            Self::check("DOUBLE 类型", SqliteCategory::Type,
                "CREATE TABLE t (price DOUBLE)"),
            Self::check("FLOAT 类型", SqliteCategory::Type,
                "CREATE TABLE t (price FLOAT)"),
            Self::check("TEXT 类型", SqliteCategory::Type,
                "CREATE TABLE t (name TEXT)"),
            Self::check("VARCHAR 类型", SqliteCategory::Type,
                "CREATE TABLE t (name VARCHAR(255))"),
            Self::check("CHAR 类型", SqliteCategory::Type,
                "CREATE TABLE t (code CHAR(10))"),
            Self::check("BLOB 类型", SqliteCategory::Type,
                "CREATE TABLE t (data BLOB)"),
            Self::check("NUMERIC 类型", SqliteCategory::Type,
                "CREATE TABLE t (price NUMERIC(10, 2))"),
            Self::check("DECIMAL 类型", SqliteCategory::Type,
                "CREATE TABLE t (price DECIMAL(10, 2))"),
            Self::check("BOOLEAN 类型", SqliteCategory::Type,
                "CREATE TABLE t (is_active BOOLEAN)"),
            Self::check("DATE 类型", SqliteCategory::Type,
                "CREATE TABLE t (birthday DATE)"),
            Self::check("DATETIME 类型", SqliteCategory::Type,
                "CREATE TABLE t (created_at DATETIME)"),
            Self::check("TIMESTAMP 类型", SqliteCategory::Type,
                "CREATE TABLE t (updated_at TIMESTAMP)"),
            Self::check("BIGINT 类型", SqliteCategory::Type,
                "CREATE TABLE t (id BIGINT)"),
            Self::check("JSON 类型", SqliteCategory::Type,
                "CREATE TABLE t (data JSON)"),
        ]
    }

    /// 内置函数兼容性测试
    fn test_functions() -> Vec<SqliteCompatResult> {
        vec![
            Self::check("DATE 函数", SqliteCategory::Function,
                "SELECT DATE('2024-01-01')"),
            Self::check("TIME 函数", SqliteCategory::Function,
                "SELECT TIME('10:30:00')"),
            Self::check("DATETIME 函数", SqliteCategory::Function,
                "SELECT DATETIME('2024-01-01 10:30:00')"),
            Self::check("STRFTIME 函数", SqliteCategory::Function,
                "SELECT STRFTIME('%Y-%m-%d', '2024-01-01')"),
            Self::check("CURRENT_DATE 函数", SqliteCategory::Function,
                "SELECT CURRENT_DATE"),
            Self::check("CURRENT_TIME 函数", SqliteCategory::Function,
                "SELECT CURRENT_TIME"),
            Self::check("CURRENT_TIMESTAMP 函数", SqliteCategory::Function,
                "SELECT CURRENT_TIMESTAMP"),
            Self::check("COUNT 函数", SqliteCategory::Function,
                "SELECT COUNT(*) FROM users"),
            Self::check("SUM/AVG/MAX/MIN 函数", SqliteCategory::Function,
                "SELECT SUM(salary), AVG(salary), MAX(salary), MIN(salary) FROM employees"),
            Self::check("GROUP_CONCAT 函数", SqliteCategory::Function,
                "SELECT department, GROUP_CONCAT(name) FROM employees GROUP BY department"),
            Self::check("LENGTH 函数", SqliteCategory::Function,
                "SELECT LENGTH(name) FROM users"),
            Self::check("UPPER/LOWER 函数", SqliteCategory::Function,
                "SELECT UPPER(name), LOWER(name) FROM users"),
            Self::check("SUBSTRING 函数", SqliteCategory::Function,
                "SELECT SUBSTRING(name, 1, 3) FROM users"),
            Self::check("TRIM 函数", SqliteCategory::Function,
                "SELECT TRIM('  hello  ')"),
            Self::check("COALESCE 函数", SqliteCategory::Function,
                "SELECT COALESCE(name, 'unknown') FROM users"),
            Self::check("IFNULL 函数", SqliteCategory::Function,
                "SELECT IFNULL(name, 'unknown') FROM users"),
            Self::check("NULLIF 函数", SqliteCategory::Function,
                "SELECT NULLIF(a, b) FROM t"),
            Self::check("CAST 函数", SqliteCategory::Function,
                "SELECT CAST('123' AS INTEGER)"),
            Self::check("CASE WHEN 表达式", SqliteCategory::Function,
                "SELECT CASE WHEN age > 18 THEN 'adult' ELSE 'minor' END FROM users"),
            Self::check("JSON_EXTRACT 函数", SqliteCategory::Function,
                "SELECT JSON_EXTRACT('{\"a\":1}', '$.a')"),
            Self::check("JSON_ARRAY 函数", SqliteCategory::Function,
                "SELECT JSON_ARRAY(1, 2, 3)"),
            Self::check("JSON_OBJECT 函数", SqliteCategory::Function,
                "SELECT JSON_OBJECT('a', 1, 'b', 2)"),
            Self::check("ABS 函数", SqliteCategory::Function,
                "SELECT ABS(-5)"),
            Self::check("ROUND 函数", SqliteCategory::Function,
                "SELECT ROUND(3.14159, 2)"),
        ]
    }

    /// 运算符兼容性测试
    fn test_operators() -> Vec<SqliteCompatResult> {
        vec![
            Self::check("GLOB 运算符", SqliteCategory::Operator,
                "SELECT * FROM users WHERE name GLOB 'A*'"),
            Self::check("LIKE 运算符", SqliteCategory::Operator,
                "SELECT * FROM users WHERE name LIKE 'A%'"),
            Self::check("BETWEEN 运算符", SqliteCategory::Operator,
                "SELECT * FROM users WHERE age BETWEEN 18 AND 65"),
            Self::check("IN 运算符", SqliteCategory::Operator,
                "SELECT * FROM users WHERE id IN (1, 2, 3)"),
            Self::check("IS NULL 运算符", SqliteCategory::Operator,
                "SELECT * FROM users WHERE name IS NULL"),
            Self::check("IS NOT NULL 运算符", SqliteCategory::Operator,
                "SELECT * FROM users WHERE name IS NOT NULL"),
            Self::check("AND 运算符", SqliteCategory::Operator,
                "SELECT * FROM users WHERE age > 18 AND age < 65"),
            Self::check("OR 运算符", SqliteCategory::Operator,
                "SELECT * FROM users WHERE age < 18 OR age > 65"),
            Self::check("NOT 运算符", SqliteCategory::Operator,
                "SELECT * FROM users WHERE NOT (age > 65)"),
            Self::check("|| 字符串拼接", SqliteCategory::Operator,
                "SELECT 'a' || 'b' || 'c'"),
            Self::check("比较运算符", SqliteCategory::Operator,
                "SELECT * FROM users WHERE age >= 18 AND age <= 65"),
            Self::check("算术运算符", SqliteCategory::Operator,
                "SELECT 1 + 2 * 3 - 4 / 2"),
            Self::check("位运算符 &", SqliteCategory::Operator,
                "SELECT 5 & 3"),
            Self::check("位运算符 |", SqliteCategory::Operator,
                "SELECT 5 | 3"),
            Self::check("位运算符 <<", SqliteCategory::Operator,
                "SELECT 1 << 4"),
            Self::check("MATCH 运算符 FTS5", SqliteCategory::Operator,
                "SELECT * FROM docs WHERE docs MATCH 'hello'"),
        ]
    }

    /// 标识符兼容性测试
    fn test_identifiers() -> Vec<SqliteCompatResult> {
        vec![
            Self::check("方括号标识符", SqliteCategory::Identifier,
                "SELECT [id], [name] FROM [users]"),
            Self::check("反引号标识符", SqliteCategory::Identifier,
                "SELECT `id`, `name` FROM `users`"),
            Self::check("双引号标识符", SqliteCategory::Identifier,
                "SELECT \"id\", \"name\" FROM \"users\""),
            Self::check("别名 AS", SqliteCategory::Identifier,
                "SELECT id AS user_id, name AS user_name FROM users"),
            Self::check("别名省略 AS", SqliteCategory::Identifier,
                "SELECT id user_id, name user_name FROM users"),
            Self::check("表别名", SqliteCategory::Identifier,
                "SELECT u.id, u.name FROM users u"),
            Self::check("限定列名", SqliteCategory::Identifier,
                "SELECT users.id, users.name FROM users"),
            Self::check("schema.table 限定", SqliteCategory::Identifier,
                "SELECT * FROM main.users"),
            Self::check("rowid 隐式列", SqliteCategory::Identifier,
                "SELECT rowid, id FROM users"),
        ]
    }

    /// PRAGMA 指令兼容性测试
    fn test_pragma() -> Vec<SqliteCompatResult> {
        vec![
            Self::check("PRAGMA foreign_keys", SqliteCategory::Pragma,
                "PRAGMA foreign_keys = ON"),
            Self::check("PRAGMA foreign_keys OFF", SqliteCategory::Pragma,
                "PRAGMA foreign_keys = OFF"),
            Self::check("PRAGMA journal_mode", SqliteCategory::Pragma,
                "PRAGMA journal_mode = WAL"),
            Self::check("PRAGMA synchronous", SqliteCategory::Pragma,
                "PRAGMA synchronous = NORMAL"),
            Self::check("PRAGMA table_info", SqliteCategory::Pragma,
                "PRAGMA table_info(users)"),
            Self::check("PRAGMA database_list", SqliteCategory::Pragma,
                "PRAGMA database_list"),
            Self::check("PRAGMA compile_options", SqliteCategory::Pragma,
                "PRAGMA compile_options"),
            Self::check("PRAGMA user_version", SqliteCategory::Pragma,
                "PRAGMA user_version = 1"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_returns_nonempty() {
        let results = SqliteCompat::run_all();
        assert!(!results.is_empty(), "SQLite 检查项不应为空");
        assert!(results.len() >= 50, "SQLite 检查项应至少 50 项，实际: {}", results.len());
    }

    #[test]
    fn basic_select_passes() {
        let results = SqliteCompat::run_all();
        let select_basic = results.iter().find(|r| r.name == "SELECT 基本语法")
            .expect("应包含 SELECT 基本语法 测试");
        assert_eq!(select_basic.status, CompatStatus::Pass);
    }

    #[test]
    fn bracket_identifier_passes() {
        let results = SqliteCompat::run_all();
        let bracket = results.iter().find(|r| r.name == "方括号标识符")
            .expect("应包含方括号标识符测试");
        assert_eq!(bracket.status, CompatStatus::Pass);
    }

    #[test]
    fn pragma_replaced_passes() {
        let results = SqliteCompat::run_all();
        let pragma = results.iter().find(|r| r.name == "PRAGMA foreign_keys")
            .expect("应包含 PRAGMA 测试");
        // PRAGMA 被预处理替换为 SELECT 1，应能解析
        assert_eq!(pragma.status, CompatStatus::Pass);
    }
}
