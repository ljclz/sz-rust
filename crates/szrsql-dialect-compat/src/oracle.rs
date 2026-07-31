//! Oracle 兼容性测试模块。
//!
//! 验证 SzRSQL 解析器对 Oracle 方言的兼容性，覆盖：
//! - DDL：CREATE TABLE、SEQUENCE、SYNONYM、约束、注释
//! - DML：SELECT/INSERT/UPDATE/DELETE、ROWNUM、DUAL、MINUS
//! - 数据类型：NUMBER/VARCHAR2/CHAR/DATE/CLOB/BLOB/RAW
//! - 函数：SYSDATE、DECODE、NVL/NVL2、TO_DATE/TO_NUMBER/TO_CHAR、ADD_MONTHS
//! - 运算符：|| 拼接、seq.NEXTVAL/CURRVAL
//! - 子查询、JOIN、CONNECT BY（不支持）

use crate::CompatStatus;
use serde::{Deserialize, Serialize};
use szrsql_sql::dialect::{parse_with_dialect, Dialect};

/// Oracle 兼容性检查分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OracleCategory {
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

/// 单项 Oracle 兼容性检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleCompatResult {
    /// 检查项名称
    pub name: String,
    /// 分类
    pub category: OracleCategory,
    /// 被测试的 SQL 语句
    pub sql: String,
    /// 兼容性状态
    pub status: CompatStatus,
    /// 详细说明
    pub detail: String,
}

/// Oracle 兼容性测试套件
pub struct OracleCompat;

impl OracleCompat {
    /// 运行全部 Oracle 兼容性检查
    pub fn run_all() -> Vec<OracleCompatResult> {
        let mut results = Vec::new();
        results.extend(Self::test_ddl());
        results.extend(Self::test_dml());
        results.extend(Self::test_types());
        results.extend(Self::test_functions());
        results.extend(Self::test_operators());
        results.extend(Self::test_identifiers());
        results
    }

    fn check(name: &str, category: OracleCategory, sql: &str) -> OracleCompatResult {
        match parse_with_dialect(sql, &Dialect::Oracle) {
            Ok(stmts) if !stmts.is_empty() => OracleCompatResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Pass,
                detail: format!("解析成功，返回 {} 条语句", stmts.len()),
            },
            Ok(_) => OracleCompatResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Fail,
                detail: "解析成功但返回空语句列表".to_string(),
            },
            Err(e) => OracleCompatResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Fail,
                detail: format!("解析失败: {e}"),
            },
        }
    }

    fn test_ddl() -> Vec<OracleCompatResult> {
        vec![
            Self::check("CREATE TABLE 基本语法", OracleCategory::Ddl,
                "CREATE TABLE users (id NUMBER PRIMARY KEY, name VARCHAR2(100))"),
            Self::check("CREATE TABLE with DEFAULT", OracleCategory::Ddl,
                "CREATE TABLE t (id NUMBER, name VARCHAR2(50) DEFAULT 'unknown')"),
            Self::check("CREATE TABLE with CHECK", OracleCategory::Ddl,
                "CREATE TABLE t (age NUMBER CHECK(age > 0))"),
            Self::check("CREATE TABLE with UNIQUE", OracleCategory::Ddl,
                "CREATE TABLE t (id NUMBER, email VARCHAR2(100) UNIQUE)"),
            Self::check("CREATE TABLE with FOREIGN KEY", OracleCategory::Ddl,
                "CREATE TABLE orders (id NUMBER PRIMARY KEY, user_id NUMBER, FOREIGN KEY(user_id) REFERENCES users(id))"),
            Self::check("CREATE SEQUENCE", OracleCategory::Ddl,
                "CREATE SEQUENCE seq_users START WITH 1 INCREMENT BY 1"),
            Self::check("CREATE INDEX", OracleCategory::Ddl,
                "CREATE INDEX idx_name ON users(name)"),
            Self::check("CREATE UNIQUE INDEX", OracleCategory::Ddl,
                "CREATE UNIQUE INDEX idx_email ON users(email)"),
            Self::check("CREATE VIEW", OracleCategory::Ddl,
                "CREATE VIEW v_users AS SELECT id, name FROM users"),
            Self::check("CREATE SYNONYM", OracleCategory::Ddl,
                "CREATE SYNONYM syn_users FOR users"),
            Self::check("DROP TABLE", OracleCategory::Ddl,
                "DROP TABLE users"),
            Self::check("DROP TABLE CASCADE", OracleCategory::Ddl,
                "DROP TABLE users CASCADE CONSTRAINTS"),
            Self::check("DROP SEQUENCE", OracleCategory::Ddl,
                "DROP SEQUENCE seq_users"),
            Self::check("DROP VIEW", OracleCategory::Ddl,
                "DROP VIEW v_users"),
            Self::check("ALTER TABLE ADD COLUMN", OracleCategory::Ddl,
                "ALTER TABLE users ADD (email VARCHAR2(100))"),
            Self::check("ALTER TABLE DROP COLUMN", OracleCategory::Ddl,
                "ALTER TABLE users DROP (email)"),
            Self::check("ALTER TABLE MODIFY COLUMN", OracleCategory::Ddl,
                "ALTER TABLE users MODIFY (name VARCHAR2(200) NOT NULL)"),
            Self::check("TRUNCATE TABLE", OracleCategory::Ddl,
                "TRUNCATE TABLE users"),
            Self::check("COMMENT ON TABLE", OracleCategory::Ddl,
                "COMMENT ON TABLE users IS '用户表'"),
            Self::check("COMMENT ON COLUMN", OracleCategory::Ddl,
                "COMMENT ON COLUMN users.name IS '用户姓名'"),
        ]
    }

    fn test_dml() -> Vec<OracleCompatResult> {
        vec![
            Self::check("SELECT 基本语法", OracleCategory::Dml,
                "SELECT id, name FROM users WHERE age > 18"),
            Self::check("SELECT FROM DUAL", OracleCategory::Dml,
                "SELECT 1 FROM dual"),
            Self::check("SELECT SYSDATE FROM DUAL", OracleCategory::Dml,
                "SELECT SYSDATE FROM dual"),
            Self::check("SELECT ROWNUM <= N", OracleCategory::Dml,
                "SELECT * FROM users WHERE ROWNUM <= 10"),
            Self::check("SELECT ROWNUM < N", OracleCategory::Dml,
                "SELECT * FROM users WHERE ROWNUM < 11"),
            Self::check("SELECT JOIN", OracleCategory::Dml,
                "SELECT u.id, o.id FROM users u INNER JOIN orders o ON u.id = o.user_id"),
            Self::check("SELECT LEFT JOIN", OracleCategory::Dml,
                "SELECT u.id FROM users u LEFT JOIN orders o ON u.id = o.user_id"),
            Self::check("Oracle 旧式 JOIN 逗号", OracleCategory::Dml,
                "SELECT u.id, o.id FROM users u, orders o WHERE u.id = o.user_id"),
            Self::check("SELECT GROUP BY", OracleCategory::Dml,
                "SELECT department, COUNT(*) FROM employees GROUP BY department"),
            Self::check("SELECT HAVING", OracleCategory::Dml,
                "SELECT department, AVG(salary) FROM employees GROUP BY department HAVING AVG(salary) > 50000"),
            Self::check("SELECT ORDER BY", OracleCategory::Dml,
                "SELECT * FROM users ORDER BY name ASC, age DESC"),
            Self::check("SELECT DISTINCT", OracleCategory::Dml,
                "SELECT DISTINCT department FROM employees"),
            Self::check("SELECT 子查询", OracleCategory::Dml,
                "SELECT * FROM (SELECT id, name FROM users) sub"),
            Self::check("SELECT UNION", OracleCategory::Dml,
                "SELECT id FROM users UNION SELECT id FROM orders"),
            Self::check("SELECT UNION ALL", OracleCategory::Dml,
                "SELECT id FROM users UNION ALL SELECT id FROM orders"),
            Self::check("SELECT MINUS", OracleCategory::Dml,
                "SELECT id FROM users MINUS SELECT id FROM orders"),
            Self::check("SELECT INTERSECT", OracleCategory::Dml,
                "SELECT id FROM users INTERSECT SELECT id FROM orders"),
            Self::check("INSERT 基本语法", OracleCategory::Dml,
                "INSERT INTO users (id, name) VALUES (1, 'Alice')"),
            Self::check("INSERT 多行", OracleCategory::Dml,
                "INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob'), (3, 'Carol')"),
            Self::check("INSERT SELECT", OracleCategory::Dml,
                "INSERT INTO users SELECT * FROM temp_users"),
            Self::check("UPDATE", OracleCategory::Dml,
                "UPDATE users SET name = 'Bob' WHERE id = 1"),
            Self::check("DELETE", OracleCategory::Dml,
                "DELETE FROM users WHERE id = 1"),
            Self::check("SELECT with CTE", OracleCategory::Dml,
                "WITH cte AS (SELECT id FROM users) SELECT * FROM cte"),
            Self::check("SELECT with EXISTS", OracleCategory::Dml,
                "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE o.user_id = u.id)"),
        ]
    }

    fn test_types() -> Vec<OracleCompatResult> {
        vec![
            Self::check("NUMBER 类型", OracleCategory::Type,
                "CREATE TABLE t (id NUMBER)"),
            Self::check("NUMBER(p) 类型", OracleCategory::Type,
                "CREATE TABLE t (id NUMBER(10))"),
            Self::check("NUMBER(p,s) 类型", OracleCategory::Type,
                "CREATE TABLE t (price NUMBER(10, 2))"),
            Self::check("INTEGER 类型", OracleCategory::Type,
                "CREATE TABLE t (id INTEGER)"),
            Self::check("FLOAT 类型", OracleCategory::Type,
                "CREATE TABLE t (price FLOAT)"),
            Self::check("BINARY_FLOAT 类型", OracleCategory::Type,
                "CREATE TABLE t (price BINARY_FLOAT)"),
            Self::check("BINARY_DOUBLE 类型", OracleCategory::Type,
                "CREATE TABLE t (price BINARY_DOUBLE)"),
            Self::check("VARCHAR2 类型", OracleCategory::Type,
                "CREATE TABLE t (name VARCHAR2(100))"),
            Self::check("NVARCHAR2 类型", OracleCategory::Type,
                "CREATE TABLE t (name NVARCHAR2(100))"),
            Self::check("CHAR 类型", OracleCategory::Type,
                "CREATE TABLE t (code CHAR(10))"),
            Self::check("NCHAR 类型", OracleCategory::Type,
                "CREATE TABLE t (code NCHAR(10))"),
            Self::check("CLOB 类型", OracleCategory::Type,
                "CREATE TABLE t (content CLOB)"),
            Self::check("BLOB 类型", OracleCategory::Type,
                "CREATE TABLE t (data BLOB)"),
            Self::check("NCLOB 类型", OracleCategory::Type,
                "CREATE TABLE t (content NCLOB)"),
            Self::check("DATE 类型", OracleCategory::Type,
                "CREATE TABLE t (birthday DATE)"),
            Self::check("TIMESTAMP 类型", OracleCategory::Type,
                "CREATE TABLE t (created_at TIMESTAMP)"),
            Self::check("TIMESTAMP WITH TIME ZONE", OracleCategory::Type,
                "CREATE TABLE t (ts TIMESTAMP WITH TIME ZONE)"),
            Self::check("RAW 类型", OracleCategory::Type,
                "CREATE TABLE t (data RAW(100))"),
            Self::check("LONG 类型", OracleCategory::Type,
                "CREATE TABLE t (data LONG)"),
            Self::check("ROWID 类型", OracleCategory::Type,
                "CREATE TABLE t (rid ROWID)"),
            Self::check("JSON 类型", OracleCategory::Type,
                "CREATE TABLE t (data JSON)"),
        ]
    }

    fn test_functions() -> Vec<OracleCompatResult> {
        vec![
            Self::check("SYSDATE 函数", OracleCategory::Function,
                "SELECT SYSDATE FROM dual"),
            Self::check("CURRENT_DATE 函数", OracleCategory::Function,
                "SELECT CURRENT_DATE FROM dual"),
            Self::check("CURRENT_TIMESTAMP 函数", OracleCategory::Function,
                "SELECT CURRENT_TIMESTAMP FROM dual"),
            Self::check("DECODE 函数", OracleCategory::Function,
                "SELECT DECODE(status, 1, 'active', 2, 'inactive', 'unknown') FROM users"),
            Self::check("DECODE 无默认值", OracleCategory::Function,
                "SELECT DECODE(status, 1, 'active') FROM users"),
            Self::check("NVL 函数", OracleCategory::Function,
                "SELECT NVL(name, 'unknown') FROM users"),
            Self::check("NVL2 函数", OracleCategory::Function,
                "SELECT NVL2(name, name, 'unknown') FROM users"),
            Self::check("COALESCE 函数", OracleCategory::Function,
                "SELECT COALESCE(name, 'unknown') FROM users"),
            Self::check("TO_DATE 函数", OracleCategory::Function,
                "SELECT TO_DATE('2024-01-01', 'YYYY-MM-DD') FROM dual"),
            Self::check("TO_NUMBER 函数", OracleCategory::Function,
                "SELECT TO_NUMBER('123') FROM dual"),
            Self::check("TO_CHAR 函数", OracleCategory::Function,
                "SELECT TO_CHAR(123) FROM dual"),
            Self::check("COUNT 函数", OracleCategory::Function,
                "SELECT COUNT(*) FROM users"),
            Self::check("SUM/AVG/MAX/MIN 函数", OracleCategory::Function,
                "SELECT SUM(salary), AVG(salary), MAX(salary), MIN(salary) FROM employees"),
            Self::check("LENGTH 函数", OracleCategory::Function,
                "SELECT LENGTH(name) FROM users"),
            Self::check("UPPER/LOWER 函数", OracleCategory::Function,
                "SELECT UPPER(name), LOWER(name) FROM users"),
            Self::check("SUBSTR 函数", OracleCategory::Function,
                "SELECT SUBSTR(name, 1, 3) FROM users"),
            Self::check("INSTR 函数", OracleCategory::Function,
                "SELECT INSTR(name, 'a') FROM users"),
            Self::check("TRIM 函数", OracleCategory::Function,
                "SELECT TRIM('  hello  ') FROM dual"),
            Self::check("CAST 函数", OracleCategory::Function,
                "SELECT CAST('123' AS NUMBER) FROM dual"),
            Self::check("CASE WHEN 表达式", OracleCategory::Function,
                "SELECT CASE WHEN age > 18 THEN 'adult' ELSE 'minor' END FROM users"),
            Self::check("USER 函数", OracleCategory::Function,
                "SELECT USER FROM dual"),
        ]
    }

    fn test_operators() -> Vec<OracleCompatResult> {
        vec![
            Self::check("|| 字符串拼接", OracleCategory::Operator,
                "SELECT 'a' || 'b' || 'c' FROM dual"),
            Self::check("LIKE 运算符", OracleCategory::Operator,
                "SELECT * FROM users WHERE name LIKE 'A%'"),
            Self::check("BETWEEN 运算符", OracleCategory::Operator,
                "SELECT * FROM users WHERE age BETWEEN 18 AND 65"),
            Self::check("IN 运算符", OracleCategory::Operator,
                "SELECT * FROM users WHERE id IN (1, 2, 3)"),
            Self::check("IS NULL 运算符", OracleCategory::Operator,
                "SELECT * FROM users WHERE name IS NULL"),
            Self::check("IS NOT NULL 运算符", OracleCategory::Operator,
                "SELECT * FROM users WHERE name IS NOT NULL"),
            Self::check("AND 运算符", OracleCategory::Operator,
                "SELECT * FROM users WHERE age > 18 AND age < 65"),
            Self::check("OR 运算符", OracleCategory::Operator,
                "SELECT * FROM users WHERE age < 18 OR age > 65"),
            Self::check("NOT 运算符", OracleCategory::Operator,
                "SELECT * FROM users WHERE NOT (age > 65)"),
            Self::check("seq.NEXTVAL 运算符", OracleCategory::Operator,
                "SELECT seq_users.NEXTVAL FROM dual"),
            Self::check("seq.CURRVAL 运算符", OracleCategory::Operator,
                "SELECT seq_users.CURRVAL FROM dual"),
            Self::check("比较运算符", OracleCategory::Operator,
                "SELECT * FROM users WHERE age >= 18 AND age <= 65"),
            Self::check("算术运算符", OracleCategory::Operator,
                "SELECT 1 + 2 * 3 - 4 / 2 FROM dual"),
            Self::check("|| 拼接列", OracleCategory::Operator,
                "SELECT first_name || ' ' || last_name FROM users"),
        ]
    }

    fn test_identifiers() -> Vec<OracleCompatResult> {
        vec![
            Self::check("双引号标识符", OracleCategory::Identifier,
                "SELECT \"id\", \"name\" FROM \"users\""),
            Self::check("带 schema 前缀", OracleCategory::Identifier,
                "SELECT * FROM hr.users"),
            Self::check("别名 AS", OracleCategory::Identifier,
                "SELECT id AS user_id, name AS user_name FROM users"),
            Self::check("别名省略 AS", OracleCategory::Identifier,
                "SELECT id user_id, name user_name FROM users"),
            Self::check("表别名", OracleCategory::Identifier,
                "SELECT u.id, u.name FROM users u"),
            Self::check("限定列名", OracleCategory::Identifier,
                "SELECT users.id, users.name FROM users"),
            Self::check("schema.table 列限定", OracleCategory::Identifier,
                "SELECT hr.users.id FROM hr.users"),
            Self::check("列名带下划线", OracleCategory::Identifier,
                "SELECT user_id, first_name, last_name FROM users"),
            Self::check("大小写敏感标识符", OracleCategory::Identifier,
                "SELECT \"CamelCase\" FROM users"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_returns_nonempty() {
        let results = OracleCompat::run_all();
        assert!(!results.is_empty(), "Oracle 检查项不应为空");
        assert!(results.len() >= 50, "Oracle 检查项应至少 50 项，实际: {}", results.len());
    }

    #[test]
    fn basic_select_passes() {
        let results = OracleCompat::run_all();
        let select_basic = results.iter().find(|r| r.name == "SELECT 基本语法")
            .expect("应包含 SELECT 基本语法 测试");
        assert_eq!(select_basic.status, CompatStatus::Pass);
    }

    #[test]
    fn nvl_function_passes() {
        let results = OracleCompat::run_all();
        let nvl = results.iter().find(|r| r.name == "NVL 函数")
            .expect("应包含 NVL 函数测试");
        assert_eq!(nvl.status, CompatStatus::Pass);
    }

    #[test]
    fn decode_function_passes() {
        let results = OracleCompat::run_all();
        let decode = results.iter().find(|r| r.name == "DECODE 函数")
            .expect("应包含 DECODE 函数测试");
        assert_eq!(decode.status, CompatStatus::Pass);
    }
}
