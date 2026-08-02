//! MySQL 兼容性测试模块。
//!
//! 验证 SzRSQL 解析器对 MySQL 方言的兼容性，覆盖：
//! - DDL：CREATE TABLE 选项、AUTO_INCREMENT、UNSIGNED、ENGINE、CHARSET
//! - DML：SELECT/INSERT/UPDATE/DELETE、LIMIT offset,count、REPLACE INTO
//! - 数据类型：TINYINT/SMALLINT/MEDIUMINT/INT/BIGINT、VARCHAR、TEXT/BLOB、DATETIME/YEAR
//! - 函数：NOW()/CURDATE()/IFNULL()/IF()/CONCAT()/GROUP_CONCAT()/DATE_FORMAT()
//! - 运算符：反引号标识符、REGEXP、RLIKE
//! - 子查询、JOIN、UNION

use crate::CompatStatus;
use serde::{Deserialize, Serialize};
use szrsql_sql::dialect::{parse_with_dialect, Dialect};

/// MySQL 兼容性检查分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MysqlCategory {
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
}

/// 单项 MySQL 兼容性检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MysqlCompatResult {
    /// 检查项名称
    pub name: String,
    /// 分类
    pub category: MysqlCategory,
    /// 被测试的 SQL 语句
    pub sql: String,
    /// 兼容性状态
    pub status: CompatStatus,
    /// 详细说明
    pub detail: String,
}

/// MySQL 兼容性测试套件
pub struct MysqlCompat;

impl MysqlCompat {
    /// 运行全部 MySQL 兼容性检查
    pub fn run_all() -> Vec<MysqlCompatResult> {
        let mut results = Vec::new();
        results.extend(Self::test_ddl());
        results.extend(Self::test_dml());
        results.extend(Self::test_types());
        results.extend(Self::test_functions());
        results.extend(Self::test_operators());
        results.extend(Self::test_identifiers());
        results
    }

    /// 测试单条 SQL 在 MySQL 方言下的解析兼容性
    fn check(name: &str, category: MysqlCategory, sql: &str) -> MysqlCompatResult {
        match parse_with_dialect(sql, &Dialect::MySql) {
            Ok(stmts) if !stmts.is_empty() => MysqlCompatResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Pass,
                detail: format!("解析成功，返回 {} 条语句", stmts.len()),
            },
            Ok(_) => MysqlCompatResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Fail,
                detail: "解析成功但返回空语句列表".to_string(),
            },
            Err(e) => MysqlCompatResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Fail,
                detail: format!("解析失败: {e}"),
            },
        }
    }

    /// DDL 兼容性测试
    fn test_ddl() -> Vec<MysqlCompatResult> {
        vec![
            Self::check("CREATE TABLE 基本语法", MysqlCategory::Ddl,
                "CREATE TABLE users (id INT PRIMARY KEY, name VARCHAR(100))"),
            Self::check("CREATE TABLE with AUTO_INCREMENT", MysqlCategory::Ddl,
                "CREATE TABLE t (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(50))"),
            Self::check("CREATE TABLE with ENGINE=InnoDB", MysqlCategory::Ddl,
                "CREATE TABLE t (id INT) ENGINE=InnoDB"),
            Self::check("CREATE TABLE with CHARSET", MysqlCategory::Ddl,
                "CREATE TABLE t (id INT) DEFAULT CHARSET=utf8mb4"),
            Self::check("CREATE TABLE with COMMENT", MysqlCategory::Ddl,
                "CREATE TABLE t (id INT COMMENT '主键', name VARCHAR(50) COMMENT '姓名')"),
            Self::check("CREATE TABLE with UNSIGNED", MysqlCategory::Ddl,
                "CREATE TABLE t (id INT UNSIGNED PRIMARY KEY, age TINYINT UNSIGNED)"),
            Self::check("CREATE TABLE with NOT NULL DEFAULT", MysqlCategory::Ddl,
                "CREATE TABLE t (id INT NOT NULL AUTO_INCREMENT, name VARCHAR(50) NOT NULL DEFAULT 'unknown')"),
            Self::check("CREATE INDEX", MysqlCategory::Ddl,
                "CREATE INDEX idx_name ON users(name)"),
            Self::check("CREATE UNIQUE INDEX", MysqlCategory::Ddl,
                "CREATE UNIQUE INDEX idx_email ON users(email)"),
            Self::check("DROP TABLE", MysqlCategory::Ddl,
                "DROP TABLE IF EXISTS users"),
            Self::check("ALTER TABLE ADD COLUMN", MysqlCategory::Ddl,
                "ALTER TABLE users ADD COLUMN email VARCHAR(100)"),
            Self::check("ALTER TABLE DROP COLUMN", MysqlCategory::Ddl,
                "ALTER TABLE users DROP COLUMN email"),
            Self::check("ALTER TABLE MODIFY COLUMN", MysqlCategory::Ddl,
                "ALTER TABLE users MODIFY COLUMN name VARCHAR(200) NOT NULL"),
            Self::check("CREATE VIEW", MysqlCategory::Ddl,
                "CREATE VIEW v_users AS SELECT id, name FROM users"),
            Self::check("TRUNCATE TABLE", MysqlCategory::Ddl,
                "TRUNCATE TABLE users"),
        ]
    }

    /// DML 兼容性测试
    fn test_dml() -> Vec<MysqlCompatResult> {
        vec![
            Self::check("SELECT 基本语法", MysqlCategory::Dml,
                "SELECT id, name FROM users WHERE age > 18"),
            Self::check("SELECT LIMIT offset, count", MysqlCategory::Dml,
                "SELECT * FROM t LIMIT 10, 20"),
            Self::check("SELECT LIMIT count", MysqlCategory::Dml,
                "SELECT * FROM t LIMIT 10"),
            Self::check("SELECT JOIN", MysqlCategory::Dml,
                "SELECT u.id, o.id FROM users u INNER JOIN orders o ON u.id = o.user_id"),
            Self::check("SELECT LEFT JOIN", MysqlCategory::Dml,
                "SELECT u.id FROM users u LEFT JOIN orders o ON u.id = o.user_id"),
            Self::check("SELECT GROUP BY", MysqlCategory::Dml,
                "SELECT department, COUNT(*) FROM employees GROUP BY department"),
            Self::check("SELECT HAVING", MysqlCategory::Dml,
                "SELECT department, AVG(salary) AS avg_sal FROM employees GROUP BY department HAVING AVG(salary) > 50000"),
            Self::check("SELECT ORDER BY", MysqlCategory::Dml,
                "SELECT * FROM users ORDER BY name ASC, age DESC"),
            Self::check("SELECT DISTINCT", MysqlCategory::Dml,
                "SELECT DISTINCT department FROM employees"),
            Self::check("SELECT 子查询", MysqlCategory::Dml,
                "SELECT * FROM (SELECT id, name FROM users) AS sub"),
            Self::check("SELECT UNION", MysqlCategory::Dml,
                "SELECT id FROM users UNION SELECT id FROM orders"),
            Self::check("SELECT UNION ALL", MysqlCategory::Dml,
                "SELECT id FROM users UNION ALL SELECT id FROM orders"),
            Self::check("INSERT 基本语法", MysqlCategory::Dml,
                "INSERT INTO users (id, name) VALUES (1, 'Alice')"),
            Self::check("INSERT 多行", MysqlCategory::Dml,
                "INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob'), (3, 'Carol')"),
            Self::check("INSERT IGNORE", MysqlCategory::Dml,
                "INSERT IGNORE INTO users (id, name) VALUES (1, 'Alice')"),
            Self::check("REPLACE INTO", MysqlCategory::Dml,
                "REPLACE INTO users (id, name) VALUES (1, 'Alice')"),
            Self::check("UPDATE", MysqlCategory::Dml,
                "UPDATE users SET name = 'Bob' WHERE id = 1"),
            Self::check("UPDATE 多列", MysqlCategory::Dml,
                "UPDATE users SET name = 'Bob', age = 30 WHERE id = 1"),
            Self::check("DELETE", MysqlCategory::Dml,
                "DELETE FROM users WHERE id = 1"),
            Self::check("ON DUPLICATE KEY UPDATE", MysqlCategory::Dml,
                "INSERT INTO users (id, name) VALUES (1, 'Alice') ON DUPLICATE KEY UPDATE name = VALUES(name)"),
            Self::check("SELECT with CTE", MysqlCategory::Dml,
                "WITH cte AS (SELECT id FROM users) SELECT * FROM cte"),
            Self::check("SELECT with EXISTS", MysqlCategory::Dml,
                "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE o.user_id = u.id)"),
        ]
    }

    /// 数据类型兼容性测试
    fn test_types() -> Vec<MysqlCompatResult> {
        vec![
            Self::check(
                "TINYINT 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (id TINYINT)",
            ),
            Self::check(
                "SMALLINT 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (id SMALLINT)",
            ),
            Self::check(
                "MEDIUMINT 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (id MEDIUMINT)",
            ),
            Self::check("INT 类型", MysqlCategory::Type, "CREATE TABLE t (id INT)"),
            Self::check(
                "BIGINT 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (id BIGINT)",
            ),
            Self::check(
                "VARCHAR 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (name VARCHAR(255))",
            ),
            Self::check(
                "CHAR 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (code CHAR(10))",
            ),
            Self::check(
                "TEXT 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (content TEXT)",
            ),
            Self::check(
                "MEDIUMTEXT 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (content MEDIUMTEXT)",
            ),
            Self::check(
                "LONGTEXT 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (content LONGTEXT)",
            ),
            Self::check(
                "BLOB 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (data BLOB)",
            ),
            Self::check(
                "FLOAT 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (score FLOAT)",
            ),
            Self::check(
                "DOUBLE 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (price DOUBLE)",
            ),
            Self::check(
                "DECIMAL 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (price DECIMAL(10, 2))",
            ),
            Self::check(
                "DATE 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (birthday DATE)",
            ),
            Self::check(
                "DATETIME 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (created_at DATETIME)",
            ),
            Self::check(
                "TIMESTAMP 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (updated_at TIMESTAMP)",
            ),
            Self::check(
                "TIME 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (duration TIME)",
            ),
            Self::check(
                "YEAR 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (year YEAR)",
            ),
            Self::check(
                "BOOLEAN 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (is_active BOOLEAN)",
            ),
            Self::check(
                "JSON 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (data JSON)",
            ),
            Self::check(
                "ENUM 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (status ENUM('active', 'inactive'))",
            ),
            Self::check(
                "BIT 类型",
                MysqlCategory::Type,
                "CREATE TABLE t (flags BIT(8))",
            ),
        ]
    }

    /// 内置函数兼容性测试
    fn test_functions() -> Vec<MysqlCompatResult> {
        vec![
            Self::check("NOW() 函数", MysqlCategory::Function, "SELECT NOW()"),
            Self::check(
                "CURDATE() 函数",
                MysqlCategory::Function,
                "SELECT CURDATE()",
            ),
            Self::check(
                "CURTIME() 函数",
                MysqlCategory::Function,
                "SELECT CURTIME()",
            ),
            Self::check(
                "IFNULL 函数",
                MysqlCategory::Function,
                "SELECT IFNULL(name, 'unknown') FROM users",
            ),
            Self::check(
                "IF 函数",
                MysqlCategory::Function,
                "SELECT IF(age > 18, 'adult', 'minor') FROM users",
            ),
            Self::check(
                "CONCAT 函数",
                MysqlCategory::Function,
                "SELECT CONCAT(first_name, ' ', last_name) FROM users",
            ),
            Self::check(
                "CONCAT_WS 函数",
                MysqlCategory::Function,
                "SELECT CONCAT_WS(',', first_name, last_name) FROM users",
            ),
            Self::check(
                "GROUP_CONCAT 函数",
                MysqlCategory::Function,
                "SELECT department, GROUP_CONCAT(name) FROM employees GROUP BY department",
            ),
            Self::check(
                "DATE_FORMAT 函数",
                MysqlCategory::Function,
                "SELECT DATE_FORMAT(NOW(), '%Y-%m-%d')",
            ),
            Self::check(
                "STR_TO_DATE 函数",
                MysqlCategory::Function,
                "SELECT STR_TO_DATE('2024-01-01', '%Y-%m-%d')",
            ),
            Self::check(
                "UNIX_TIMESTAMP 函数",
                MysqlCategory::Function,
                "SELECT UNIX_TIMESTAMP()",
            ),
            Self::check(
                "FROM_UNIXTIME 函数",
                MysqlCategory::Function,
                "SELECT FROM_UNIXTIME(1700000000)",
            ),
            Self::check(
                "COUNT 函数",
                MysqlCategory::Function,
                "SELECT COUNT(*) FROM users",
            ),
            Self::check(
                "SUM/AVG/MAX/MIN 函数",
                MysqlCategory::Function,
                "SELECT SUM(salary), AVG(salary), MAX(salary), MIN(salary) FROM employees",
            ),
            Self::check(
                "LENGTH 函数",
                MysqlCategory::Function,
                "SELECT LENGTH(name) FROM users",
            ),
            Self::check(
                "UPPER/LOWER 函数",
                MysqlCategory::Function,
                "SELECT UPPER(name), LOWER(name) FROM users",
            ),
            Self::check(
                "SUBSTRING 函数",
                MysqlCategory::Function,
                "SELECT SUBSTRING(name, 1, 3) FROM users",
            ),
            Self::check(
                "TRIM 函数",
                MysqlCategory::Function,
                "SELECT TRIM('  hello  ')",
            ),
            Self::check(
                "COALESCE 函数",
                MysqlCategory::Function,
                "SELECT COALESCE(name, 'unknown') FROM users",
            ),
            Self::check(
                "CAST 函数",
                MysqlCategory::Function,
                "SELECT CAST('123' AS SIGNED)",
            ),
            Self::check(
                "CASE WHEN 表达式",
                MysqlCategory::Function,
                "SELECT CASE WHEN age > 18 THEN 'adult' ELSE 'minor' END FROM users",
            ),
        ]
    }

    /// 运算符兼容性测试
    fn test_operators() -> Vec<MysqlCompatResult> {
        vec![
            Self::check(
                "REGEXP 运算符",
                MysqlCategory::Operator,
                "SELECT * FROM users WHERE name REGEXP '^A'",
            ),
            Self::check(
                "RLIKE 运算符",
                MysqlCategory::Operator,
                "SELECT * FROM users WHERE name RLIKE '^A'",
            ),
            Self::check(
                "LIKE 运算符",
                MysqlCategory::Operator,
                "SELECT * FROM users WHERE name LIKE 'A%'",
            ),
            Self::check(
                "BETWEEN 运算符",
                MysqlCategory::Operator,
                "SELECT * FROM users WHERE age BETWEEN 18 AND 65",
            ),
            Self::check(
                "IN 运算符",
                MysqlCategory::Operator,
                "SELECT * FROM users WHERE id IN (1, 2, 3)",
            ),
            Self::check(
                "IS NULL 运算符",
                MysqlCategory::Operator,
                "SELECT * FROM users WHERE name IS NULL",
            ),
            Self::check(
                "IS NOT NULL 运算符",
                MysqlCategory::Operator,
                "SELECT * FROM users WHERE name IS NOT NULL",
            ),
            Self::check(
                "AND 运算符",
                MysqlCategory::Operator,
                "SELECT * FROM users WHERE age > 18 AND age < 65",
            ),
            Self::check(
                "OR 运算符",
                MysqlCategory::Operator,
                "SELECT * FROM users WHERE age < 18 OR age > 65",
            ),
            Self::check(
                "NOT 运算符",
                MysqlCategory::Operator,
                "SELECT * FROM users WHERE NOT (age > 65)",
            ),
            Self::check(
                "字符串拼接 CONCAT",
                MysqlCategory::Operator,
                "SELECT CONCAT('a', 'b')",
            ),
            Self::check("DIV 整除运算符", MysqlCategory::Operator, "SELECT 10 DIV 3"),
            Self::check("MOD 取模运算符", MysqlCategory::Operator, "SELECT 10 MOD 3"),
            Self::check(
                "比较运算符",
                MysqlCategory::Operator,
                "SELECT * FROM users WHERE age >= 18 AND age <= 65",
            ),
            Self::check(
                "算术运算符",
                MysqlCategory::Operator,
                "SELECT 1 + 2 * 3 - 4 / 2",
            ),
        ]
    }

    /// 标识符兼容性测试
    fn test_identifiers() -> Vec<MysqlCompatResult> {
        vec![
            Self::check(
                "反引号标识符",
                MysqlCategory::Identifier,
                "SELECT `id`, `name` FROM `users`",
            ),
            Self::check(
                "反引号保留字",
                MysqlCategory::Identifier,
                "SELECT `order`, `group` FROM `table`",
            ),
            Self::check(
                "带数据库名前缀",
                MysqlCategory::Identifier,
                "SELECT * FROM mydb.users",
            ),
            Self::check(
                "别名 AS",
                MysqlCategory::Identifier,
                "SELECT id AS user_id, name AS user_name FROM users",
            ),
            Self::check(
                "别名省略 AS",
                MysqlCategory::Identifier,
                "SELECT id user_id, name user_name FROM users",
            ),
            Self::check(
                "表别名",
                MysqlCategory::Identifier,
                "SELECT u.id, u.name FROM users u",
            ),
            Self::check(
                "列名带下划线",
                MysqlCategory::Identifier,
                "SELECT user_id, first_name, last_name FROM users",
            ),
            Self::check(
                "双引号字符串字面量",
                MysqlCategory::Identifier,
                "SELECT \"hello\"",
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_returns_nonempty() {
        let results = MysqlCompat::run_all();
        assert!(!results.is_empty(), "MySQL 检查项不应为空");
        assert!(
            results.len() >= 50,
            "MySQL 检查项应至少 50 项，实际: {}",
            results.len()
        );
    }

    #[test]
    fn basic_select_passes() {
        let results = MysqlCompat::run_all();
        let select_basic = results
            .iter()
            .find(|r| r.name == "SELECT 基本语法")
            .expect("应包含 SELECT 基本语法 测试");
        assert_eq!(select_basic.status, CompatStatus::Pass);
    }

    #[test]
    fn limit_offset_comma_passes() {
        let results = MysqlCompat::run_all();
        let limit = results
            .iter()
            .find(|r| r.name == "SELECT LIMIT offset, count")
            .expect("应包含 LIMIT offset, count 测试");
        assert_eq!(limit.status, CompatStatus::Pass);
    }

    #[test]
    fn backtick_identifier_passes() {
        let results = MysqlCompat::run_all();
        let backtick = results
            .iter()
            .find(|r| r.name == "反引号标识符")
            .expect("应包含反引号标识符测试");
        assert_eq!(backtick.status, CompatStatus::Pass);
    }
}
