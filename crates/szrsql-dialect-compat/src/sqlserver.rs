//! SQL Server (T-SQL) 兼容性测试模块。
//!
//! 验证 SzRSQL 解析器对 SQL Server 方言的兼容性，覆盖：
//! - DDL：CREATE TABLE、IDENTITY、约束、索引、视图
//! - DML：SELECT TOP N、INSERT/UPDATE/DELETE、OUTPUT、MERGE
//! - 数据类型：INT/BIGINT/TINYINT、VARCHAR/NVARCHAR、DATETIME2/UNIQUEIDENTIFIER
//! - 函数：GETDATE/GETUTCDATE、ISNULL、LEN、LEFT/RIGHT、CONVERT
//! - 运算符：TOP、+ 拼接、字符串运算
//! - 标识符：方括号、双引号、schema.dbo.table

use crate::CompatStatus;
use serde::{Deserialize, Serialize};
use szrsql_sql::dialect::{parse_with_dialect, Dialect};

/// SQL Server 兼容性检查分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SqlserverCategory {
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

/// 单项 SQL Server 兼容性检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlserverCompatResult {
    /// 检查项名称
    pub name: String,
    /// 分类
    pub category: SqlserverCategory,
    /// 被测试的 SQL 语句
    pub sql: String,
    /// 兼容性状态
    pub status: CompatStatus,
    /// 详细说明
    pub detail: String,
}

/// SQL Server 兼容性测试套件
pub struct SqlserverCompat;

impl SqlserverCompat {
    /// 运行全部 SQL Server 兼容性检查
    pub fn run_all() -> Vec<SqlserverCompatResult> {
        let mut results = Vec::new();
        results.extend(Self::test_ddl());
        results.extend(Self::test_dml());
        results.extend(Self::test_types());
        results.extend(Self::test_functions());
        results.extend(Self::test_operators());
        results.extend(Self::test_identifiers());
        results
    }

    /// 测试单条 SQL 在 SQL Server 方言下的解析兼容性
    fn check(name: &str, category: SqlserverCategory, sql: &str) -> SqlserverCompatResult {
        match parse_with_dialect(sql, &Dialect::SqlServer) {
            Ok(stmts) if !stmts.is_empty() => SqlserverCompatResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Pass,
                detail: format!("解析成功，返回 {} 条语句", stmts.len()),
            },
            Ok(_) => SqlserverCompatResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Fail,
                detail: "解析成功但返回空语句列表".to_string(),
            },
            Err(e) => SqlserverCompatResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Fail,
                detail: format!("解析失败: {e}"),
            },
        }
    }

    /// DDL 兼容性测试
    fn test_ddl() -> Vec<SqlserverCompatResult> {
        vec![
            Self::check("CREATE TABLE 基本语法", SqlserverCategory::Ddl,
                "CREATE TABLE users (id INT PRIMARY KEY, name VARCHAR(100))"),
            Self::check("CREATE TABLE with IDENTITY", SqlserverCategory::Ddl,
                "CREATE TABLE t (id INT IDENTITY(1, 1) PRIMARY KEY, name NVARCHAR(50))"),
            Self::check("CREATE TABLE with DEFAULT", SqlserverCategory::Ddl,
                "CREATE TABLE t (id INT, name NVARCHAR(50) DEFAULT N'unknown')"),
            Self::check("CREATE TABLE with CHECK", SqlserverCategory::Ddl,
                "CREATE TABLE t (age INT CHECK(age > 0))"),
            Self::check("CREATE TABLE with UNIQUE", SqlserverCategory::Ddl,
                "CREATE TABLE t (id INT, email NVARCHAR(100) UNIQUE)"),
            Self::check("CREATE TABLE with FOREIGN KEY", SqlserverCategory::Ddl,
                "CREATE TABLE orders (id INT PRIMARY KEY, user_id INT, FOREIGN KEY(user_id) REFERENCES users(id))"),
            Self::check("CREATE TABLE with NOT NULL", SqlserverCategory::Ddl,
                "CREATE TABLE t (id INT NOT NULL, name NVARCHAR(50) NOT NULL)"),
            Self::check("CREATE TABLE IF NOT EXISTS", SqlserverCategory::Ddl,
                "CREATE TABLE IF NOT EXISTS users (id INT PRIMARY KEY, name NVARCHAR(100))"),
            Self::check("CREATE INDEX", SqlserverCategory::Ddl,
                "CREATE INDEX idx_name ON users(name)"),
            Self::check("CREATE UNIQUE INDEX", SqlserverCategory::Ddl,
                "CREATE UNIQUE INDEX idx_email ON users(email)"),
            Self::check("CREATE CLUSTERED INDEX", SqlserverCategory::Ddl,
                "CREATE CLUSTERED INDEX idx_id ON users(id)"),
            Self::check("CREATE NONCLUSTERED INDEX", SqlserverCategory::Ddl,
                "CREATE NONCLUSTERED INDEX idx_name ON users(name)"),
            Self::check("DROP TABLE", SqlserverCategory::Ddl,
                "DROP TABLE IF EXISTS users"),
            Self::check("ALTER TABLE ADD COLUMN", SqlserverCategory::Ddl,
                "ALTER TABLE users ADD email NVARCHAR(100)"),
            Self::check("ALTER TABLE DROP COLUMN", SqlserverCategory::Ddl,
                "ALTER TABLE users DROP COLUMN email"),
            Self::check("ALTER TABLE ALTER COLUMN", SqlserverCategory::Ddl,
                "ALTER TABLE users ALTER COLUMN name NVARCHAR(200) NOT NULL"),
            Self::check("CREATE VIEW", SqlserverCategory::Ddl,
                "CREATE VIEW v_users AS SELECT id, name FROM users"),
            Self::check("TRUNCATE TABLE", SqlserverCategory::Ddl,
                "TRUNCATE TABLE users"),
            Self::check("CREATE SCHEMA", SqlserverCategory::Ddl,
                "CREATE SCHEMA hr"),
        ]
    }

    /// DML 兼容性测试
    fn test_dml() -> Vec<SqlserverCompatResult> {
        vec![
            Self::check("SELECT 基本语法", SqlserverCategory::Dml,
                "SELECT id, name FROM users WHERE age > 18"),
            Self::check("SELECT TOP N", SqlserverCategory::Dml,
                "SELECT TOP 10 * FROM users"),
            Self::check("SELECT TOP N PERCENT", SqlserverCategory::Dml,
                "SELECT TOP 10 PERCENT * FROM users"),
            Self::check("SELECT TOP N WITH TIES", SqlserverCategory::Dml,
                "SELECT TOP 10 WITH TIES * FROM users ORDER BY age"),
            Self::check("SELECT JOIN", SqlserverCategory::Dml,
                "SELECT u.id, o.id FROM users u INNER JOIN orders o ON u.id = o.user_id"),
            Self::check("SELECT LEFT JOIN", SqlserverCategory::Dml,
                "SELECT u.id FROM users u LEFT JOIN orders o ON u.id = o.user_id"),
            Self::check("SELECT RIGHT JOIN", SqlserverCategory::Dml,
                "SELECT u.id FROM users u RIGHT JOIN orders o ON u.id = o.user_id"),
            Self::check("SELECT FULL JOIN", SqlserverCategory::Dml,
                "SELECT u.id FROM users u FULL OUTER JOIN orders o ON u.id = o.user_id"),
            Self::check("SELECT CROSS JOIN", SqlserverCategory::Dml,
                "SELECT u.id, o.id FROM users u CROSS JOIN orders o"),
            Self::check("SELECT GROUP BY", SqlserverCategory::Dml,
                "SELECT department, COUNT(*) FROM employees GROUP BY department"),
            Self::check("SELECT HAVING", SqlserverCategory::Dml,
                "SELECT department, AVG(salary) FROM employees GROUP BY department HAVING AVG(salary) > 50000"),
            Self::check("SELECT ORDER BY", SqlserverCategory::Dml,
                "SELECT * FROM users ORDER BY name ASC, age DESC"),
            Self::check("SELECT DISTINCT", SqlserverCategory::Dml,
                "SELECT DISTINCT department FROM employees"),
            Self::check("SELECT 子查询", SqlserverCategory::Dml,
                "SELECT * FROM (SELECT id, name FROM users) AS sub"),
            Self::check("SELECT UNION", SqlserverCategory::Dml,
                "SELECT id FROM users UNION SELECT id FROM orders"),
            Self::check("SELECT UNION ALL", SqlserverCategory::Dml,
                "SELECT id FROM users UNION ALL SELECT id FROM orders"),
            Self::check("SELECT EXCEPT", SqlserverCategory::Dml,
                "SELECT id FROM users EXCEPT SELECT id FROM orders"),
            Self::check("SELECT INTERSECT", SqlserverCategory::Dml,
                "SELECT id FROM users INTERSECT SELECT id FROM orders"),
            Self::check("INSERT 基本语法", SqlserverCategory::Dml,
                "INSERT INTO users (id, name) VALUES (1, 'Alice')"),
            Self::check("INSERT 多行", SqlserverCategory::Dml,
                "INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob'), (3, 'Carol')"),
            Self::check("INSERT SELECT", SqlserverCategory::Dml,
                "INSERT INTO users SELECT * FROM temp_users"),
            Self::check("UPDATE", SqlserverCategory::Dml,
                "UPDATE users SET name = 'Bob' WHERE id = 1"),
            Self::check("UPDATE 多列", SqlserverCategory::Dml,
                "UPDATE users SET name = 'Bob', age = 30 WHERE id = 1"),
            Self::check("DELETE", SqlserverCategory::Dml,
                "DELETE FROM users WHERE id = 1"),
            Self::check("MERGE 语句", SqlserverCategory::Dml,
                "MERGE target AS t USING source AS s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.name = s.name"),
            Self::check("SELECT with CTE", SqlserverCategory::Dml,
                "WITH cte AS (SELECT id FROM users) SELECT * FROM cte"),
            Self::check("SELECT with EXISTS", SqlserverCategory::Dml,
                "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE o.user_id = u.id)"),
        ]
    }

    /// 数据类型兼容性测试
    fn test_types() -> Vec<SqlserverCompatResult> {
        vec![
            Self::check(
                "INT 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (id INT)",
            ),
            Self::check(
                "BIGINT 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (id BIGINT)",
            ),
            Self::check(
                "SMALLINT 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (id SMALLINT)",
            ),
            Self::check(
                "TINYINT 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (id TINYINT)",
            ),
            Self::check(
                "BIT 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (flag BIT)",
            ),
            Self::check(
                "DECIMAL 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (price DECIMAL(10, 2))",
            ),
            Self::check(
                "NUMERIC 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (price NUMERIC(10, 2))",
            ),
            Self::check(
                "MONEY 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (price MONEY)",
            ),
            Self::check(
                "SMALLMONEY 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (price SMALLMONEY)",
            ),
            Self::check(
                "FLOAT 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (price FLOAT)",
            ),
            Self::check(
                "REAL 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (price REAL)",
            ),
            Self::check(
                "VARCHAR 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (name VARCHAR(100))",
            ),
            Self::check(
                "VARCHAR(MAX) 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (content VARCHAR(MAX))",
            ),
            Self::check(
                "NVARCHAR 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (name NVARCHAR(100))",
            ),
            Self::check(
                "NVARCHAR(MAX) 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (content NVARCHAR(MAX))",
            ),
            Self::check(
                "CHAR 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (code CHAR(10))",
            ),
            Self::check(
                "NCHAR 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (code NCHAR(10))",
            ),
            Self::check(
                "TEXT 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (content TEXT)",
            ),
            Self::check(
                "NTEXT 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (content NTEXT)",
            ),
            Self::check(
                "DATE 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (birthday DATE)",
            ),
            Self::check(
                "DATETIME 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (created_at DATETIME)",
            ),
            Self::check(
                "DATETIME2 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (created_at DATETIME2)",
            ),
            Self::check(
                "SMALLDATETIME 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (created_at SMALLDATETIME)",
            ),
            Self::check(
                "DATETIMEOFFSET 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (created_at DATETIMEOFFSET)",
            ),
            Self::check(
                "TIME 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (duration TIME)",
            ),
            Self::check(
                "UNIQUEIDENTIFIER 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (id UNIQUEIDENTIFIER)",
            ),
            Self::check(
                "VARBINARY 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (data VARBINARY(100))",
            ),
            Self::check(
                "IMAGE 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (data IMAGE)",
            ),
            Self::check(
                "JSON 类型",
                SqlserverCategory::Type,
                "CREATE TABLE t (data JSON)",
            ),
        ]
    }

    /// 内置函数兼容性测试
    fn test_functions() -> Vec<SqlserverCompatResult> {
        vec![
            Self::check(
                "GETDATE 函数",
                SqlserverCategory::Function,
                "SELECT GETDATE()",
            ),
            Self::check(
                "GETUTCDATE 函数",
                SqlserverCategory::Function,
                "SELECT GETUTCDATE()",
            ),
            Self::check(
                "SYSDATETIME 函数",
                SqlserverCategory::Function,
                "SELECT SYSDATETIME()",
            ),
            Self::check(
                "SYSDATETIMEOFFSET 函数",
                SqlserverCategory::Function,
                "SELECT SYSDATETIMEOFFSET()",
            ),
            Self::check(
                "ISNULL 函数",
                SqlserverCategory::Function,
                "SELECT ISNULL(name, 'unknown') FROM users",
            ),
            Self::check(
                "COALESCE 函数",
                SqlserverCategory::Function,
                "SELECT COALESCE(name, 'unknown') FROM users",
            ),
            Self::check(
                "LEN 函数",
                SqlserverCategory::Function,
                "SELECT LEN(name) FROM users",
            ),
            Self::check(
                "LEFT 函数",
                SqlserverCategory::Function,
                "SELECT LEFT(name, 3) FROM users",
            ),
            Self::check(
                "RIGHT 函数",
                SqlserverCategory::Function,
                "SELECT RIGHT(name, 3) FROM users",
            ),
            Self::check(
                "CHARINDEX 函数",
                SqlserverCategory::Function,
                "SELECT CHARINDEX('a', name) FROM users",
            ),
            Self::check(
                "SUBSTRING 函数",
                SqlserverCategory::Function,
                "SELECT SUBSTRING(name, 1, 3) FROM users",
            ),
            Self::check(
                "UPPER/LOWER 函数",
                SqlserverCategory::Function,
                "SELECT UPPER(name), LOWER(name) FROM users",
            ),
            Self::check(
                "TRIM 函数",
                SqlserverCategory::Function,
                "SELECT TRIM('  hello  ')",
            ),
            Self::check(
                "CONVERT 函数",
                SqlserverCategory::Function,
                "SELECT CONVERT(VARCHAR(10), 123)",
            ),
            Self::check(
                "CAST 函数",
                SqlserverCategory::Function,
                "SELECT CAST('123' AS INT)",
            ),
            Self::check(
                "DATEADD 函数",
                SqlserverCategory::Function,
                "SELECT DATEADD(day, 30, GETDATE())",
            ),
            Self::check(
                "DATEDIFF 函数",
                SqlserverCategory::Function,
                "SELECT DATEDIFF(day, '2024-01-01', '2024-12-31')",
            ),
            Self::check(
                "DATEPART 函数",
                SqlserverCategory::Function,
                "SELECT DATEPART(year, GETDATE())",
            ),
            Self::check(
                "DATENAME 函数",
                SqlserverCategory::Function,
                "SELECT DATENAME(month, GETDATE())",
            ),
            Self::check(
                "YEAR 函数",
                SqlserverCategory::Function,
                "SELECT YEAR(birthday) FROM users",
            ),
            Self::check(
                "MONTH 函数",
                SqlserverCategory::Function,
                "SELECT MONTH(birthday) FROM users",
            ),
            Self::check(
                "DAY 函数",
                SqlserverCategory::Function,
                "SELECT DAY(birthday) FROM users",
            ),
            Self::check(
                "COUNT 函数",
                SqlserverCategory::Function,
                "SELECT COUNT(*) FROM users",
            ),
            Self::check(
                "SUM/AVG/MAX/MIN 函数",
                SqlserverCategory::Function,
                "SELECT SUM(salary), AVG(salary), MAX(salary), MIN(salary) FROM employees",
            ),
            Self::check("ABS 函数", SqlserverCategory::Function, "SELECT ABS(-5)"),
            Self::check(
                "ROUND 函数",
                SqlserverCategory::Function,
                "SELECT ROUND(3.14159, 2)",
            ),
            Self::check(
                "CASE WHEN 表达式",
                SqlserverCategory::Function,
                "SELECT CASE WHEN age > 18 THEN 'adult' ELSE 'minor' END FROM users",
            ),
            Self::check("NEWID 函数", SqlserverCategory::Function, "SELECT NEWID()"),
        ]
    }

    /// 运算符兼容性测试
    fn test_operators() -> Vec<SqlserverCompatResult> {
        vec![
            Self::check("TOP N 运算符", SqlserverCategory::Operator,
                "SELECT TOP 10 * FROM users"),
            Self::check("+ 字符串拼接", SqlserverCategory::Operator,
                "SELECT 'a' + 'b' + 'c'"),
            Self::check("LIKE 运算符", SqlserverCategory::Operator,
                "SELECT * FROM users WHERE name LIKE 'A%'"),
            Self::check("BETWEEN 运算符", SqlserverCategory::Operator,
                "SELECT * FROM users WHERE age BETWEEN 18 AND 65"),
            Self::check("IN 运算符", SqlserverCategory::Operator,
                "SELECT * FROM users WHERE id IN (1, 2, 3)"),
            Self::check("IS NULL 运算符", SqlserverCategory::Operator,
                "SELECT * FROM users WHERE name IS NULL"),
            Self::check("IS NOT NULL 运算符", SqlserverCategory::Operator,
                "SELECT * FROM users WHERE name IS NOT NULL"),
            Self::check("AND 运算符", SqlserverCategory::Operator,
                "SELECT * FROM users WHERE age > 18 AND age < 65"),
            Self::check("OR 运算符", SqlserverCategory::Operator,
                "SELECT * FROM users WHERE age < 18 OR age > 65"),
            Self::check("NOT 运算符", SqlserverCategory::Operator,
                "SELECT * FROM users WHERE NOT (age > 65)"),
            Self::check("比较运算符", SqlserverCategory::Operator,
                "SELECT * FROM users WHERE age >= 18 AND age <= 65"),
            Self::check("算术运算符", SqlserverCategory::Operator,
                "SELECT 1 + 2 * 3 - 4 / 2"),
            Self::check("+ 拼接列", SqlserverCategory::Operator,
                "SELECT first_name + ' ' + last_name FROM users"),
            Self::check("ALL 运算符", SqlserverCategory::Operator,
                "SELECT * FROM users WHERE age > ALL (SELECT age FROM minors)"),
            Self::check("ANY 运算符", SqlserverCategory::Operator,
                "SELECT * FROM users WHERE age > ANY (SELECT age FROM minors)"),
            Self::check("EXISTS 运算符", SqlserverCategory::Operator,
                "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE o.user_id = u.id)"),
        ]
    }

    /// 标识符兼容性测试
    fn test_identifiers() -> Vec<SqlserverCompatResult> {
        vec![
            Self::check(
                "方括号标识符",
                SqlserverCategory::Identifier,
                "SELECT [id], [name] FROM [users]",
            ),
            Self::check(
                "方括号保留字",
                SqlserverCategory::Identifier,
                "SELECT [order], [group] FROM [table]",
            ),
            Self::check(
                "双引号标识符",
                SqlserverCategory::Identifier,
                "SELECT \"id\", \"name\" FROM \"users\"",
            ),
            Self::check(
                "带 schema 前缀",
                SqlserverCategory::Identifier,
                "SELECT * FROM dbo.users",
            ),
            Self::check(
                "带 db.schema.table",
                SqlserverCategory::Identifier,
                "SELECT * FROM mydb.dbo.users",
            ),
            Self::check(
                "别名 AS",
                SqlserverCategory::Identifier,
                "SELECT id AS user_id, name AS user_name FROM users",
            ),
            Self::check(
                "别名省略 AS",
                SqlserverCategory::Identifier,
                "SELECT id user_id, name user_name FROM users",
            ),
            Self::check(
                "表别名",
                SqlserverCategory::Identifier,
                "SELECT u.id, u.name FROM users u",
            ),
            Self::check(
                "限定列名",
                SqlserverCategory::Identifier,
                "SELECT users.id, users.name FROM users",
            ),
            Self::check(
                "列名带下划线",
                SqlserverCategory::Identifier,
                "SELECT user_id, first_name, last_name FROM users",
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_returns_nonempty() {
        let results = SqlserverCompat::run_all();
        assert!(!results.is_empty(), "SQL Server 检查项不应为空");
        assert!(
            results.len() >= 50,
            "SQL Server 检查项应至少 50 项，实际: {}",
            results.len()
        );
    }

    #[test]
    fn basic_select_passes() {
        let results = SqlserverCompat::run_all();
        let select_basic = results
            .iter()
            .find(|r| r.name == "SELECT 基本语法")
            .expect("应包含 SELECT 基本语法 测试");
        assert_eq!(select_basic.status, CompatStatus::Pass);
    }

    #[test]
    fn top_n_passes() {
        let results = SqlserverCompat::run_all();
        let top = results
            .iter()
            .find(|r| r.name == "SELECT TOP N")
            .expect("应包含 SELECT TOP N 测试");
        assert_eq!(top.status, CompatStatus::Pass);
    }

    #[test]
    fn isnull_function_passes() {
        let results = SqlserverCompat::run_all();
        let isnull = results
            .iter()
            .find(|r| r.name == "ISNULL 函数")
            .expect("应包含 ISNULL 函数测试");
        assert_eq!(isnull.status, CompatStatus::Pass);
    }
}
