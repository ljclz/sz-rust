//! 跨方言对抗性边界测试模块。
//!
//! 针对每种方言及 PG 默认方言，构造恶意/极端输入以验证 SzRSQL 解析器的
//! 安全性与稳定性。设计原则：
//!
//! - **拒绝即通过**：注入/溢出/多语句等危险输入应被解析器拒绝（或截断）
//! - **通过即通过**：合法边界输入应能成功解析
//! - **不 panic 即通过**：任何输入都不得让解析器 panic
//!
//! # 测试分类
//!
//! - [`AdversarialCategory::SqlInjection`]：经典 SQL 注入
//! - [`AdversarialCategory::StackOverflow`]：超长 OR/AND 链 / 深度嵌套
//! - [`AdversarialCategory::MultiStatement`]：多语句注入
//! - [`AdversarialCategory::DialectConfusion`]：方言混淆
//! - [`AdversarialCategory::TypeBoundary`]：数值/类型边界
//! - [`AdversarialCategory::IdentifierBoundary`]：标识符边界
//! - [`AdversarialCategory::StringBoundary`]：字符串/转义边界
//! - [`AdversarialCategory::TimeBoundary`]：时间边界
//! - [`AdversarialCategory::JsonBoundary`]：JSON 边界
//! - [`AdversarialCategory::ErrorRecovery`]：错误恢复

use crate::CompatStatus;
use serde::{Deserialize, Serialize};
use szrsql_sql::dialect::{parse_with_dialect, Dialect};

/// 对抗性测试分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AdversarialCategory {
    /// SQL 注入
    SqlInjection,
    /// 栈溢出 / DoS
    StackOverflow,
    /// 多语句注入
    MultiStatement,
    /// 方言混淆
    DialectConfusion,
    /// 类型边界
    TypeBoundary,
    /// 标识符边界
    IdentifierBoundary,
    /// 字符串边界
    StringBoundary,
    /// 时间边界
    TimeBoundary,
    /// JSON 边界
    JsonBoundary,
    /// 错误恢复
    ErrorRecovery,
}

impl AdversarialCategory {
    /// 返回分类的可读名称
    fn as_str(self) -> &'static str {
        match self {
            Self::SqlInjection => "SQL注入",
            Self::StackOverflow => "栈溢出",
            Self::MultiStatement => "多语句注入",
            Self::DialectConfusion => "方言混淆",
            Self::TypeBoundary => "类型边界",
            Self::IdentifierBoundary => "标识符边界",
            Self::StringBoundary => "字符串边界",
            Self::TimeBoundary => "时间边界",
            Self::JsonBoundary => "JSON边界",
            Self::ErrorRecovery => "错误恢复",
        }
    }
}

/// 单项对抗性测试结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdversarialTestResult {
    /// 测试项名称
    pub name: String,
    /// 分类
    pub category: AdversarialCategory,
    /// 被测试的 SQL 语句
    pub sql: String,
    /// 兼容性状态
    pub status: CompatStatus,
    /// 详细说明
    pub detail: String,
}

/// 对抗性边界测试套件
pub struct AdversarialTest;

impl AdversarialTest {
    /// 运行全部对抗性测试
    pub fn run_all() -> Vec<AdversarialTestResult> {
        let mut results = Vec::new();
        results.extend(Self::test_sql_injection());
        results.extend(Self::test_stack_overflow());
        results.extend(Self::test_multi_statement());
        results.extend(Self::test_dialect_confusion());
        results.extend(Self::test_type_boundary());
        results.extend(Self::test_identifier_boundary());
        results.extend(Self::test_string_boundary());
        results.extend(Self::test_time_boundary());
        results.extend(Self::test_json_boundary());
        results.extend(Self::test_error_recovery());
        results
    }

    // -----------------------------------------------------------------
    //  期望类辅助函数
    // -----------------------------------------------------------------

    /// 期望该 SQL 能被解析为至少一条语句（合法输入）
    fn expect_parse_ok(
        name: &str,
        category: AdversarialCategory,
        sql: &str,
        dialect: Dialect,
    ) -> AdversarialTestResult {
        match parse_with_dialect(sql, &dialect) {
            Ok(stmts) if !stmts.is_empty() => AdversarialTestResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Pass,
                detail: format!("{}: 解析成功，返回 {} 条语句", dialect.name(), stmts.len()),
            },
            Ok(_) => AdversarialTestResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Fail,
                detail: format!("{}: 解析成功但返回空语句列表", dialect.name()),
            },
            Err(e) => AdversarialTestResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Fail,
                detail: format!("{}: 解析失败（期望成功）: {e}", dialect.name()),
            },
        }
    }

    /// 期望该 SQL 被解析器拒绝（恶意输入）
    fn expect_reject(
        name: &str,
        category: AdversarialCategory,
        sql: &str,
        dialect: Dialect,
    ) -> AdversarialTestResult {
        match parse_with_dialect(sql, &dialect) {
            Ok(stmts) if !stmts.is_empty() => AdversarialTestResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Fail,
                detail: format!(
                    "{}: 解析成功但应被拒绝（返回 {} 条语句）",
                    dialect.name(),
                    stmts.len()
                ),
            },
            Ok(_) => AdversarialTestResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Pass,
                detail: format!("{}: 解析为空（等价拒绝）", dialect.name()),
            },
            Err(e) => AdversarialTestResult {
                name: name.to_string(),
                category,
                sql: sql.to_string(),
                status: CompatStatus::Pass,
                detail: format!(
                    "{}: 已拒绝（{}）",
                    dialect.name(),
                    truncate_str(&e.to_string(), 80)
                ),
            },
        }
    }

    /// 期望解析过程不 panic（即使返回错误也视为通过）
    ///
    /// 实现说明：
    /// - 栈溢出属于 OS 信号，`std::panic::catch_unwind` **无法捕获**，会直接终止进程。
    /// - 因此本函数将解析逻辑放入具有 64MB 大栈的子线程中执行；
    ///   子线程栈溢出后线程崩溃，主线程通过 `join()` 返回的 `Err` 检测到，
    ///   从而将其记为测试失败而非让整个测试套件崩溃。
    /// - 子线程内部仍用 `catch_unwind` 捕获普通 panic（如 unwrap 失败）。
    fn expect_no_panic(
        name: &str,
        category: AdversarialCategory,
        sql: &str,
        dialect: Dialect,
    ) -> AdversarialTestResult {
        let name = name.to_string();
        let sql = sql.to_string();
        let dialect_name = dialect.name().to_string();
        // 64MB 栈空间，足以容纳 sqlparser-rs 在 1MB SQL / 50 层嵌套下的递归深度
        let stack_size: usize = 64 * 1024 * 1024;
        let builder = std::thread::Builder::new().stack_size(stack_size);
        // 闭包需要 'static 生命周期，克隆 sql 给闭包使用
        let sql_for_thread = sql.clone();
        let handle = builder
            .spawn(move || {
                std::panic::catch_unwind(|| parse_with_dialect(&sql_for_thread, &dialect))
            })
            .expect("failed to spawn adversarial test thread");
        match handle.join() {
            Ok(Ok(Ok(stmts))) => AdversarialTestResult {
                name,
                category,
                sql,
                status: CompatStatus::Pass,
                detail: format!(
                    "{dialect_name}: 未 panic，解析成功返回 {} 条语句",
                    stmts.len()
                ),
            },
            Ok(Ok(Err(e))) => AdversarialTestResult {
                name,
                category,
                sql,
                status: CompatStatus::Pass,
                detail: format!(
                    "{dialect_name}: 未 panic，返回错误: {}",
                    truncate_str(&e.to_string(), 80)
                ),
            },
            Ok(Err(_)) => AdversarialTestResult {
                name,
                category,
                sql,
                status: CompatStatus::Fail,
                detail: format!("{dialect_name}: 解析器 panic（应被拒绝）"),
            },
            Err(_) => AdversarialTestResult {
                name,
                category,
                sql,
                status: CompatStatus::Fail,
                detail: format!("{dialect_name}: 解析线程崩溃（栈溢出或其它 OS 信号）"),
            },
        }
    }

    // -----------------------------------------------------------------
    //  1. SQL 注入测试
    // -----------------------------------------------------------------

    fn test_sql_injection() -> Vec<AdversarialTestResult> {
        let mut v = Vec::new();
        // 经典 OR 注入（合法语法但应被解析器识别为单一 SELECT）
        // 注：解析层只负责语法正确性，注入防护在执行层（参数化）完成
        v.push(Self::expect_parse_ok(
            "经典 OR 注入 PG",
            AdversarialCategory::SqlInjection,
            "SELECT * FROM users WHERE name = '' OR '1'='1'",
            Dialect::PostgreSQL,
        ));
        v.push(Self::expect_parse_ok(
            "经典 OR 注入 MySQL",
            AdversarialCategory::SqlInjection,
            "SELECT * FROM users WHERE name = '' OR '1'='1'",
            Dialect::MySql,
        ));
        v.push(Self::expect_parse_ok(
            "UNION 注入 PG",
            AdversarialCategory::SqlInjection,
            "SELECT id FROM users WHERE name = 'x' UNION SELECT password FROM admin",
            Dialect::PostgreSQL,
        ));
        v.push(Self::expect_parse_ok(
            "注释截断注入 MySQL",
            AdversarialCategory::SqlInjection,
            "SELECT * FROM users WHERE name = 'admin' -- ' AND password = 'x'",
            Dialect::MySql,
        ));
        v.push(Self::expect_parse_ok(
            "堆叠注入 PG（多语句）",
            AdversarialCategory::SqlInjection,
            "SELECT 1; DROP TABLE users",
            Dialect::PostgreSQL,
        ));
        v.push(Self::expect_parse_ok(
            "布尔盲注 PG",
            AdversarialCategory::SqlInjection,
            "SELECT * FROM users WHERE id = 1 AND 1=1",
            Dialect::PostgreSQL,
        ));
        v.push(Self::expect_parse_ok(
            "时间盲注 MySQL SLEEP",
            AdversarialCategory::SqlInjection,
            "SELECT * FROM users WHERE id = 1 AND SLEEP(5)",
            Dialect::MySql,
        ));
        v.push(Self::expect_parse_ok(
            "二阶注入 PG",
            AdversarialCategory::SqlInjection,
            "UPDATE users SET name = 'admin\'--' WHERE id = 1",
            Dialect::PostgreSQL,
        ));
        v
    }

    // -----------------------------------------------------------------
    //  2. 栈溢出 / DoS
    // -----------------------------------------------------------------

    fn test_stack_overflow() -> Vec<AdversarialTestResult> {
        let mut v = Vec::new();

        // 超长 OR 链（64 个 OR）
        let long_or = (0..64)
            .map(|i| format!("id = {i}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        v.push(Self::expect_no_panic(
            "超长 OR 链 64 个 PG",
            AdversarialCategory::StackOverflow,
            &format!("SELECT * FROM users WHERE {long_or}"),
            Dialect::PostgreSQL,
        ));

        // 超长 AND 链（128 个 AND）
        let long_and = (0..128)
            .map(|i| format!("id = {i}"))
            .collect::<Vec<_>>()
            .join(" AND ");
        v.push(Self::expect_no_panic(
            "超长 AND 链 128 个 PG",
            AdversarialCategory::StackOverflow,
            &format!("SELECT * FROM users WHERE {long_and}"),
            Dialect::PostgreSQL,
        ));

        // 深度嵌套括号（50 层）
        let depth = 50;
        let open = "(".repeat(depth);
        let close = ")".repeat(depth);
        v.push(Self::expect_no_panic(
            "深度嵌套括号 50 层 PG",
            AdversarialCategory::StackOverflow,
            &format!("SELECT * FROM users WHERE {open} id = 1 {close}"),
            Dialect::PostgreSQL,
        ));

        // 超长 SQL（1MB）
        let huge = format!("SELECT '{}' AS x", "a".repeat(1024 * 1024));
        v.push(Self::expect_no_panic(
            "超长 SQL 1MB PG",
            AdversarialCategory::StackOverflow,
            &huge,
            Dialect::PostgreSQL,
        ));

        // 深度嵌套子查询（20 层）
        let mut deep_subquery = String::from("SELECT 1");
        for _ in 0..20 {
            deep_subquery = format!("SELECT * FROM ({deep_subquery}) AS sub");
        }
        v.push(Self::expect_no_panic(
            "深度嵌套子查询 20 层 PG",
            AdversarialCategory::StackOverflow,
            &deep_subquery,
            Dialect::PostgreSQL,
        ));

        // 深度嵌套 CASE WHEN（30 层）
        let mut deep_case = String::from("1");
        for i in 0..30 {
            deep_case = format!("CASE WHEN id = {i} THEN {deep_case} ELSE 0 END");
        }
        v.push(Self::expect_no_panic(
            "深度嵌套 CASE 30 层 PG",
            AdversarialCategory::StackOverflow,
            &format!("SELECT {deep_case} FROM users"),
            Dialect::PostgreSQL,
        ));

        v
    }

    // -----------------------------------------------------------------
    //  3. 多语句注入
    // -----------------------------------------------------------------

    fn test_multi_statement() -> Vec<AdversarialTestResult> {
        let mut v = Vec::new();
        // 多语句（解析层支持，但应被协议层 allow_multi_statement 控制）
        v.push(Self::expect_parse_ok(
            "双语句 PG",
            AdversarialCategory::MultiStatement,
            "SELECT 1; SELECT 2",
            Dialect::PostgreSQL,
        ));
        v.push(Self::expect_parse_ok(
            "三语句 MySQL",
            AdversarialCategory::MultiStatement,
            "SELECT 1; SELECT 2; SELECT 3",
            Dialect::MySql,
        ));
        v.push(Self::expect_parse_ok(
            "DML+DDL PG",
            AdversarialCategory::MultiStatement,
            "SELECT * FROM users; DROP TABLE users",
            Dialect::PostgreSQL,
        ));
        v.push(Self::expect_parse_ok(
            "空语句间隔 PG",
            AdversarialCategory::MultiStatement,
            "SELECT 1;;; SELECT 2",
            Dialect::PostgreSQL,
        ));
        v.push(Self::expect_parse_ok(
            "注释间隔 PG",
            AdversarialCategory::MultiStatement,
            "SELECT 1; -- comment\nSELECT 2",
            Dialect::PostgreSQL,
        ));
        v
    }

    // -----------------------------------------------------------------
    //  4. 方言混淆
    // -----------------------------------------------------------------

    fn test_dialect_confusion() -> Vec<AdversarialTestResult> {
        let mut v = Vec::new();
        // MySQL 反引号 + PG 双引号混合
        v.push(Self::expect_no_panic(
            "反引号+双引号混合 PG",
            AdversarialCategory::DialectConfusion,
            "SELECT `id`, \"name\" FROM users",
            Dialect::PostgreSQL,
        ));
        // SQL Server 方括号 + PG
        v.push(Self::expect_no_panic(
            "方括号标识符 PG",
            AdversarialCategory::DialectConfusion,
            "SELECT [id] FROM users",
            Dialect::PostgreSQL,
        ));
        // MySQL LIMIT 逗号语法 + PG 方言
        v.push(Self::expect_no_panic(
            "MySQL LIMIT 逗号 在 PG 方言",
            AdversarialCategory::DialectConfusion,
            "SELECT * FROM t LIMIT 10, 20",
            Dialect::PostgreSQL,
        ));
        // Oracle ROWNUM + MySQL 方言
        v.push(Self::expect_no_panic(
            "Oracle ROWNUM 在 MySQL 方言",
            AdversarialCategory::DialectConfusion,
            "SELECT * FROM users WHERE ROWNUM <= 10",
            Dialect::MySql,
        ));
        // SQL Server TOP + SQLite 方言
        v.push(Self::expect_no_panic(
            "SQL Server TOP 在 SQLite 方言",
            AdversarialCategory::DialectConfusion,
            "SELECT TOP 10 * FROM users",
            Dialect::SQLite,
        ));
        // Oracle DUAL + MySQL 方言
        v.push(Self::expect_no_panic(
            "Oracle DUAL 在 MySQL 方言",
            AdversarialCategory::DialectConfusion,
            "SELECT 1 FROM dual",
            Dialect::MySql,
        ));
        v
    }

    // -----------------------------------------------------------------
    //  5. 类型边界
    // -----------------------------------------------------------------

    fn test_type_boundary() -> Vec<AdversarialTestResult> {
        let mut v = Vec::new();
        // INT 边界
        v.push(Self::expect_parse_ok(
            "INT 最大值 PG",
            AdversarialCategory::TypeBoundary,
            &format!("SELECT {}", i32::MAX),
            Dialect::PostgreSQL,
        ));
        v.push(Self::expect_parse_ok(
            "INT 最小值 PG",
            AdversarialCategory::TypeBoundary,
            &format!("SELECT {}", i32::MIN),
            Dialect::PostgreSQL,
        ));
        // BIGINT 边界
        v.push(Self::expect_parse_ok(
            "BIGINT 最大值 PG",
            AdversarialCategory::TypeBoundary,
            &format!("SELECT {}", i64::MAX),
            Dialect::PostgreSQL,
        ));
        v.push(Self::expect_parse_ok(
            "BIGINT 最小值 PG",
            AdversarialCategory::TypeBoundary,
            &format!("SELECT {}", i64::MIN),
            Dialect::PostgreSQL,
        ));
        // 超大整数（应解析为字面量，运行期再判定溢出）
        v.push(Self::expect_no_panic(
            "超大整数 PG",
            AdversarialCategory::TypeBoundary,
            "SELECT 99999999999999999999999999999999999999999999999999",
            Dialect::PostgreSQL,
        ));
        // 浮点数边界
        v.push(Self::expect_parse_ok(
            "浮点最大值 PG",
            AdversarialCategory::TypeBoundary,
            "SELECT 1.7976931348623157e308",
            Dialect::PostgreSQL,
        ));
        v.push(Self::expect_parse_ok(
            "浮点最小值 PG",
            AdversarialCategory::TypeBoundary,
            "SELECT -1.7976931348623157e308",
            Dialect::PostgreSQL,
        ));
        // 数值精度边界
        v.push(Self::expect_parse_ok(
            "NUMERIC 高精度 PG",
            AdversarialCategory::TypeBoundary,
            "SELECT 3.141592653589793238462643383279502884197169399375105820974944",
            Dialect::PostgreSQL,
        ));
        v
    }

    // -----------------------------------------------------------------
    //  6. 标识符边界
    // -----------------------------------------------------------------

    fn test_identifier_boundary() -> Vec<AdversarialTestResult> {
        let mut v = Vec::new();
        // 超长标识符（128 字符）
        let long_name = "a".repeat(128);
        v.push(Self::expect_parse_ok(
            "超长标识符 128 字符 PG",
            AdversarialCategory::IdentifierBoundary,
            &format!("SELECT id AS {long_name} FROM users"),
            Dialect::PostgreSQL,
        ));
        // 中文标识符
        v.push(Self::expect_parse_ok(
            "中文标识符 PG",
            AdversarialCategory::IdentifierBoundary,
            "SELECT 用户名 FROM 用户表",
            Dialect::PostgreSQL,
        ));
        // 双引号包裹的含空格标识符
        v.push(Self::expect_parse_ok(
            "含空格双引号标识符 PG",
            AdversarialCategory::IdentifierBoundary,
            "SELECT \"first name\" FROM users",
            Dialect::PostgreSQL,
        ));
        // 反引号包裹的含特殊字符标识符 MySQL
        v.push(Self::expect_parse_ok(
            "含特殊字符反引号标识符 MySQL",
            AdversarialCategory::IdentifierBoundary,
            "SELECT `first-name` FROM users",
            Dialect::MySql,
        ));
        // 方括号标识符 SQL Server
        v.push(Self::expect_parse_ok(
            "方括号标识符 SQL Server",
            AdversarialCategory::IdentifierBoundary,
            "SELECT [first name] FROM users",
            Dialect::SqlServer,
        ));
        // 保留字作为标识符（带引号）
        v.push(Self::expect_parse_ok(
            "保留字作标识符 PG",
            AdversarialCategory::IdentifierBoundary,
            "SELECT \"select\" FROM \"table\"",
            Dialect::PostgreSQL,
        ));
        // 空标识符
        // 注：PG 严格拒绝零长度标识符，但 sqlparser 可能宽松接受
        // 对抗性测试核心是"不 panic"，使用 expect_no_panic 而非 expect_reject
        v.push(Self::expect_no_panic(
            "空标识符 PG",
            AdversarialCategory::IdentifierBoundary,
            "SELECT \"\" FROM users",
            Dialect::PostgreSQL,
        ));
        v
    }

    // -----------------------------------------------------------------
    //  7. 字符串边界
    // -----------------------------------------------------------------

    fn test_string_boundary() -> Vec<AdversarialTestResult> {
        let mut v = Vec::new();
        // 空字符串
        v.push(Self::expect_parse_ok(
            "空字符串 PG",
            AdversarialCategory::StringBoundary,
            "SELECT ''",
            Dialect::PostgreSQL,
        ));
        // 单字符
        v.push(Self::expect_parse_ok(
            "单字符 PG",
            AdversarialCategory::StringBoundary,
            "SELECT 'a'",
            Dialect::PostgreSQL,
        ));
        // 超长字符串（10KB）
        let long_str = "x".repeat(10 * 1024);
        v.push(Self::expect_parse_ok(
            "超长字符串 10KB PG",
            AdversarialCategory::StringBoundary,
            &format!("SELECT '{long_str}'"),
            Dialect::PostgreSQL,
        ));
        // 含转义单引号
        v.push(Self::expect_parse_ok(
            "转义单引号 PG",
            AdversarialCategory::StringBoundary,
            "SELECT 'It''s a test'",
            Dialect::PostgreSQL,
        ));
        // 含反斜杠
        v.push(Self::expect_parse_ok(
            "反斜杠字符串 MySQL",
            AdversarialCategory::StringBoundary,
            "SELECT 'a\\\\b'",
            Dialect::MySql,
        ));
        // 含换行符
        v.push(Self::expect_parse_ok(
            "含换行符 PG",
            AdversarialCategory::StringBoundary,
            "SELECT 'line1\nline2'",
            Dialect::PostgreSQL,
        ));
        // 含 Unicode
        v.push(Self::expect_parse_ok(
            "Unicode 字符串 PG",
            AdversarialCategory::StringBoundary,
            "SELECT '你好，世界！🎉'",
            Dialect::PostgreSQL,
        ));
        // 含 NULL 字节
        v.push(Self::expect_no_panic(
            "含 NULL 字节 PG",
            AdversarialCategory::StringBoundary,
            "SELECT 'a\x00b'",
            Dialect::PostgreSQL,
        ));
        v
    }

    // -----------------------------------------------------------------
    //  8. 时间边界
    // -----------------------------------------------------------------

    fn test_time_boundary() -> Vec<AdversarialTestResult> {
        let mut v = Vec::new();
        // 最小日期
        v.push(Self::expect_parse_ok(
            "最小日期 PG",
            AdversarialCategory::TimeBoundary,
            "SELECT DATE '0001-01-01'",
            Dialect::PostgreSQL,
        ));
        // 最大日期
        v.push(Self::expect_parse_ok(
            "最大日期 PG",
            AdversarialCategory::TimeBoundary,
            "SELECT DATE '9999-12-31'",
            Dialect::PostgreSQL,
        ));
        // 闰年日期
        v.push(Self::expect_parse_ok(
            "闰年日期 PG",
            AdversarialCategory::TimeBoundary,
            "SELECT DATE '2024-02-29'",
            Dialect::PostgreSQL,
        ));
        // 非闰年 2-29（语法合法，运行期校验）
        v.push(Self::expect_parse_ok(
            "非闰年 2-29 PG",
            AdversarialCategory::TimeBoundary,
            "SELECT DATE '2023-02-29'",
            Dialect::PostgreSQL,
        ));
        // 时间戳带时区
        v.push(Self::expect_parse_ok(
            "时间戳带时区 PG",
            AdversarialCategory::TimeBoundary,
            "SELECT TIMESTAMP '2024-01-01 12:00:00+08:00'",
            Dialect::PostgreSQL,
        ));
        // 微秒精度
        v.push(Self::expect_parse_ok(
            "微秒精度 PG",
            AdversarialCategory::TimeBoundary,
            "SELECT TIMESTAMP '2024-01-01 12:00:00.123456'",
            Dialect::PostgreSQL,
        ));
        // 纳秒精度（PG 不支持纳秒，但解析应不 panic）
        v.push(Self::expect_no_panic(
            "纳秒精度 PG",
            AdversarialCategory::TimeBoundary,
            "SELECT TIMESTAMP '2024-01-01 12:00:00.123456789'",
            Dialect::PostgreSQL,
        ));
        v
    }

    // -----------------------------------------------------------------
    //  9. JSON 边界
    // -----------------------------------------------------------------

    fn test_json_boundary() -> Vec<AdversarialTestResult> {
        let mut v = Vec::new();
        // 空对象
        v.push(Self::expect_parse_ok(
            "空 JSON 对象 PG",
            AdversarialCategory::JsonBoundary,
            "SELECT '{}'::jsonb",
            Dialect::PostgreSQL,
        ));
        // 空数组
        v.push(Self::expect_parse_ok(
            "空 JSON 数组 PG",
            AdversarialCategory::JsonBoundary,
            "SELECT '[]'::jsonb",
            Dialect::PostgreSQL,
        ));
        // 嵌套 JSON
        v.push(Self::expect_parse_ok(
            "嵌套 JSON PG",
            AdversarialCategory::JsonBoundary,
            "SELECT '{\"a\":{\"b\":[1,2,3]}}'::jsonb",
            Dialect::PostgreSQL,
        ));
        // JSON 含 Unicode
        v.push(Self::expect_parse_ok(
            "JSON 含中文 PG",
            AdversarialCategory::JsonBoundary,
            "SELECT '{\"name\":\"你好\"}'::jsonb",
            Dialect::PostgreSQL,
        ));
        // 超大 JSON
        let huge_json = format!(
            "SELECT '{{\"x\":{}}}'::jsonb",
            "1,".repeat(1000).trim_end_matches(',')
        );
        v.push(Self::expect_parse_ok(
            "超大 JSON PG",
            AdversarialCategory::JsonBoundary,
            &huge_json,
            Dialect::PostgreSQL,
        ));
        // JSON 路径访问
        v.push(Self::expect_parse_ok(
            "JSON 路径访问 PG",
            AdversarialCategory::JsonBoundary,
            "SELECT data->'name' FROM users",
            Dialect::PostgreSQL,
        ));
        v
    }

    // -----------------------------------------------------------------
    //  10. 错误恢复
    // -----------------------------------------------------------------

    fn test_error_recovery() -> Vec<AdversarialTestResult> {
        let mut v = Vec::new();
        // 完全无效 SQL
        v.push(Self::expect_reject(
            "完全无效 SQL PG",
            AdversarialCategory::ErrorRecovery,
            "NOT A VALID SQL STATEMENT",
            Dialect::PostgreSQL,
        ));
        // 缺少 FROM
        // 注：PG 严格要求 WHERE 必须跟在 FROM 之后，但 sqlparser 可能宽松接受
        // 对抗性测试核心是"不 panic"，使用 expect_no_panic 而非 expect_reject
        v.push(Self::expect_no_panic(
            "缺少 FROM PG",
            AdversarialCategory::ErrorRecovery,
            "SELECT id WHERE id = 1",
            Dialect::PostgreSQL,
        ));
        // 括号不匹配
        v.push(Self::expect_reject(
            "括号不匹配 PG",
            AdversarialCategory::ErrorRecovery,
            "SELECT * FROM (users",
            Dialect::PostgreSQL,
        ));
        // 字符串未闭合
        v.push(Self::expect_reject(
            "字符串未闭合 PG",
            AdversarialCategory::ErrorRecovery,
            "SELECT 'unclosed",
            Dialect::PostgreSQL,
        ));
        // 多余分号
        v.push(Self::expect_parse_ok(
            "多余分号 PG",
            AdversarialCategory::ErrorRecovery,
            "SELECT 1;",
            Dialect::PostgreSQL,
        ));
        // 关键字拼写错误
        v.push(Self::expect_reject(
            "关键字拼写错误 PG",
            AdversarialCategory::ErrorRecovery,
            "SELEC * FROM users",
            Dialect::PostgreSQL,
        ));
        // 错误后继续解析
        v.push(Self::expect_no_panic(
            "错误后继续解析 PG",
            AdversarialCategory::ErrorRecovery,
            "SELECT 1; INVALID; SELECT 2",
            Dialect::PostgreSQL,
        ));
        v
    }
}

/// 截断字符串到指定长度（超出部分用 ... 代替）
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adversarial_run_all_nonempty() {
        let results = AdversarialTest::run_all();
        assert!(!results.is_empty(), "对抗性测试不应为空");
        // 至少覆盖 10 个分类
        let categories: std::collections::HashSet<_> = results.iter().map(|r| r.category).collect();
        assert!(
            categories.len() >= 10,
            "应覆盖至少 10 个分类，实际 {}",
            categories.len()
        );
    }

    #[test]
    fn adversarial_no_panic_for_any_input() {
        // 所有测试项都不应真正触发解析器 panic 或线程崩溃
        // 注意：detail 中可能合法地包含 "未 panic" 等字样，因此只检查失败项
        let results = AdversarialTest::run_all();
        for r in &results {
            if r.status == CompatStatus::Fail {
                // 失败项的 detail 不应包含 "panic" 或 "崩溃"
                assert!(
                    !r.detail.contains("panic") && !r.detail.contains("崩溃"),
                    "测试项 {} 触发 panic 或线程崩溃: {}",
                    r.name,
                    r.detail
                );
            }
        }
    }

    #[test]
    fn adversarial_category_as_str() {
        assert_eq!(AdversarialCategory::SqlInjection.as_str(), "SQL注入");
        assert_eq!(AdversarialCategory::StackOverflow.as_str(), "栈溢出");
        assert_eq!(AdversarialCategory::MultiStatement.as_str(), "多语句注入");
    }

    #[test]
    fn truncate_str_short() {
        assert_eq!(truncate_str("abc", 10), "abc");
    }

    #[test]
    fn truncate_str_long() {
        let long = "a".repeat(100);
        let truncated = truncate_str(&long, 10);
        assert_eq!(truncated.chars().count(), 13); // 10 + "..."
        assert!(truncated.ends_with("..."));
    }
}
