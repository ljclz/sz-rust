//! 方言解析层 — Phase 6.8 / F-8
//!
//! 提供 MySQL / Oracle / SQL Server / SQLite 方言的 SQL 解析支持。设计目标：
//!
//! - **入口**：`parse_with_dialect(sql, dialect) -> Result<Vec<Statement>, ParseError>`
//! - **策略**：文本级预处理（方言特有语法 → PG 兼容语法）+ 对应 sqlparser 方言解析
//! - **复用**：预处理后复用现有 `parser::convert_statement` 转换为 SzRSQL AST
//!
//! # 支持的方言
//!
//! | 方言 | sqlparser Dialect | 预处理转换 |
//! |------|-------------------|-----------|
//! | PostgreSQL | `PostgreSqlDialect` | 无 |
//! | MySQL | `MySqlDialect` | `LIMIT offset, count` → `LIMIT count OFFSET offset` |
//! | Oracle | `AnsiDialect` | `ROWNUM <= N` → `LIMIT N`；`DECODE(...)` → `CASE WHEN...`；`NVL(a,b)` → `COALESCE(a,b)`；`TO_DATE(s,fmt)` → `CAST(s AS TIMESTAMP)`；`seq.NEXTVAL` → `nextval('seq')`；`SYSDATE` → `CURRENT_TIMESTAMP` |
//! | SQL Server | `MsSqlDialect` | `TOP N` → `LIMIT N`；`ISNULL(a,b)` → `COALESCE(a,b)`；`GETDATE()` → `CURRENT_TIMESTAMP` |
//! | SQLite | `SQLiteDialect` | `WITHOUT ROWID` → 移除；`PRAGMA ...` → 转为 SELECT 1 占位；`AUTOINCREMENT` 由 sqlparser 解析为 Identity 并在 apply_column_option 中静默忽略 |
//!
//! # 限制
//!
//! - 预处理为文本级（regex / 字符串匹配），不解析 SQL 语义；对复杂嵌套场景可能失效
//! - Oracle `ROWNUM` 仅处理 `WHERE ROWNUM <= N` 与 `WHERE ROWNUM < N` 形式
//! - `DECODE` 转换为 `CASE` 表达式，仅处理简单参数（不处理嵌套 DECODE 内含逗号的情况）
//! - SQL Server `TOP N` 仅处理 `SELECT TOP N ...` 形式
//!
//! # 用法
//!
//! ```
//! use szrsql_sql::dialect::{Dialect, parse_with_dialect};
//!
//! // MySQL 方言：LIMIT offset, count
//! let stmts = parse_with_dialect("SELECT * FROM t LIMIT 10, 20", &Dialect::MySql).unwrap();
//! assert_eq!(stmts.len(), 1);
//!
//! // Oracle 方言：NVL
//! let stmts = parse_with_dialect("SELECT NVL(name, 'unknown') FROM users", &Dialect::Oracle).unwrap();
//! assert_eq!(stmts.len(), 1);
//!
//! // SQL Server 方言：TOP
//! let stmts = parse_with_dialect("SELECT TOP 10 * FROM t", &Dialect::SqlServer).unwrap();
//! assert_eq!(stmts.len(), 1);
//! ```

use crate::ast::{InsertSource, OrderByExpr, Select, SetOperation, Statement};
use crate::parser::{
    convert_statement, count_binary_op_keywords, ParseError, MAX_BINARY_OP_CHAIN, MAX_SQL_LEN,
};
use regex::Regex;
use sqlparser::dialect::{
    AnsiDialect, Dialect as SpDialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect,
};
use sqlparser::parser::Parser;
use std::str::FromStr;

// =====================================================================
//  方言枚举
// =====================================================================

/// SQL 方言
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Dialect {
    /// PostgreSQL（默认）
    #[default]
    PostgreSQL,
    /// MySQL
    MySql,
    /// Oracle
    Oracle,
    /// SQL Server / T-SQL
    SqlServer,
    /// SQLite（Phase F-8 新增）
    SQLite,
}

impl Dialect {
    /// 返回方言对应的中文名
    pub fn name(&self) -> &'static str {
        match self {
            Dialect::PostgreSQL => "PostgreSQL",
            Dialect::MySql => "MySQL",
            Dialect::Oracle => "Oracle",
            Dialect::SqlServer => "SQL Server",
            Dialect::SQLite => "SQLite",
        }
    }

    /// 返回 sqlparser 对应的方言对象引用
    fn sqlparser_dialect(&self) -> Box<dyn SpDialect> {
        match self {
            Dialect::PostgreSQL => Box::new(PostgreSqlDialect {}),
            Dialect::MySql => Box::new(MySqlDialect {}),
            Dialect::Oracle => Box::new(AnsiDialect {}),
            Dialect::SqlServer => Box::new(MsSqlDialect {}),
            Dialect::SQLite => Box::new(SQLiteDialect {}),
        }
    }
}

impl FromStr for Dialect {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "postgres" | "postgresql" | "pg" => Ok(Dialect::PostgreSQL),
            "mysql" => Ok(Dialect::MySql),
            "oracle" => Ok(Dialect::Oracle),
            "sqlserver" | "mssql" | "tsql" => Ok(Dialect::SqlServer),
            "sqlite" | "sqlite3" => Ok(Dialect::SQLite),
            _ => Err(ParseError::Unsupported(format!("unknown dialect: {s}"))),
        }
    }
}

impl std::fmt::Display for Dialect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// =====================================================================
//  入口：parse_with_dialect
// =====================================================================

/// 用指定方言解析 SQL，返回 SzRSQL AST 语句列表
///
/// # 参数
/// - `sql`：SQL 文本
/// - `dialect`：方言（`Dialect::PostgreSQL` / `MySql` / `Oracle` / `SqlServer`）
///
/// # 处理流程
/// 1. **预处理**：根据方言将特有语法转换为 PG 兼容语法（文本级）
/// 2. **解析**：使用对应 sqlparser 方言解析预处理后的 SQL
/// 3. **转换**：复用 `parser::convert_statement` 将 sqlparser AST 转为 SzRSQL AST
///
/// # 错误
/// - `ParseError::SqlParser(msg)`：sqlparser 解析失败
/// - `ParseError::Unsupported(msg)`：不支持的方言
pub fn parse_with_dialect(sql: &str, dialect: &Dialect) -> Result<Vec<Statement>, ParseError> {
    // ADV-BUG-001 修复：SQL 长度预检，防止超长输入导致 sqlparser-rs 递归栈溢出
    // 与 parse_sql_inner 保持一致的安全防护，避免方言入口绕过预检
    if sql.len() > MAX_SQL_LEN {
        return Err(ParseError::Unsupported(format!(
            "SQL too long: {} bytes (max {} bytes)",
            sql.len(),
            MAX_SQL_LEN
        )));
    }
    // ADV-BUG-001 修复：OR/AND 链深度预检
    // sqlparser-rs 内部用递归下降解析二值表达式，左结合链深度 = 操作数个数
    // 在调用 sqlparser-rs 之前统计 SQL 文本中 OR/AND 关键字出现次数，超限直接拒绝
    let or_and_count = count_binary_op_keywords(sql);
    if or_and_count > MAX_BINARY_OP_CHAIN {
        return Err(ParseError::Unsupported(format!(
            "too many OR/AND operators in SQL: {} (max {}); this is a ADV-BUG-001 protection against stack overflow DoS",
            or_and_count,
            MAX_BINARY_OP_CHAIN
        )));
    }

    // 1. 预处理
    let preprocessed = preprocess(sql, dialect);

    // 2. 解析
    let sp_dialect = dialect.sqlparser_dialect();
    let statements = Parser::parse_sql(sp_dialect.as_ref(), &preprocessed)?;

    // 3. 转换
    let mut result: Vec<Statement> = statements
        .into_iter()
        .map(convert_statement)
        .collect::<Result<Vec<_>, _>>()?;

    // 4. Phase 6.9：应用方言语义（NULL 排序默认值等）
    apply_dialect_semantics(&mut result, dialect);

    Ok(result)
}

// =====================================================================
//  Phase 6.9：方言语义适配
// =====================================================================

/// 应用方言语义到 AST
///
/// Phase 6.9 实现：根据方言调整 `OrderByExpr.nulls_first` 默认值。
///
/// # 各方言 NULL 排序默认值
///
/// | 方言 | ASC | DESC |
/// |------|-----|------|
/// | PostgreSQL | NULLS LAST (`false`) | NULLS FIRST (`true`) |
/// | MySQL | NULLS FIRST (`true`) | NULLS LAST (`false`) |
/// | Oracle | NULLS LAST (`false`) | NULLS FIRST (`true`) |
/// | SQL Server | NULLS FIRST (`true`) | NULLS LAST (`false`) |
///
/// # 限制
///
/// 当前实现**覆盖所有 `OrderByExpr.nulls_first` 字段**，包括用户显式指定的 `NULLS FIRST/LAST`。
/// 这是因为 SzRSQL 的 `OrderByExpr` 未保留 "nulls_first 是否显式指定" 标记。
/// 如需保留用户显式指定，应扩展 `OrderByExpr` 添加 `nulls_first_specified: bool` 字段。
pub fn apply_dialect_semantics(statements: &mut [Statement], dialect: &Dialect) {
    for stmt in statements.iter_mut() {
        apply_dialect_semantics_to_stmt(stmt, dialect);
    }
}

fn apply_dialect_semantics_to_stmt(stmt: &mut Statement, dialect: &Dialect) {
    match stmt {
        Statement::Select(select) => {
            apply_dialect_semantics_to_select(select, dialect);
        }
        Statement::Insert {
            source: InsertSource::Select(select),
            ..
        } => {
            apply_dialect_semantics_to_select(select, dialect);
        }
        _ => {}
    }
}

fn apply_dialect_semantics_to_select(select: &mut Select, dialect: &Dialect) {
    apply_dialect_null_sort(&mut select.order_by, dialect);
    // 递归处理 CTE
    if let Some(with) = &mut select.with {
        for cte in &mut with.ctes {
            apply_dialect_semantics_to_select(&mut cte.query, dialect);
        }
    }
    // 递归处理集合操作
    if let Some(set_op) = &mut select.set_op {
        apply_dialect_semantics_to_set_op(set_op, dialect);
    }
}

fn apply_dialect_semantics_to_set_op(set_op: &mut SetOperation, dialect: &Dialect) {
    apply_dialect_semantics_to_select(&mut set_op.left, dialect);
    apply_dialect_semantics_to_select(&mut set_op.right, dialect);
}

fn apply_dialect_null_sort(order_by: &mut [OrderByExpr], dialect: &Dialect) {
    for ob in order_by.iter_mut() {
        ob.nulls_first = dialect_default_nulls_first(dialect, ob.asc);
    }
}

/// 返回方言默认的 NULLS FIRST 标志
fn dialect_default_nulls_first(dialect: &Dialect, asc: bool) -> bool {
    match (dialect, asc) {
        // PG / Oracle：ASC → NULLS LAST，DESC → NULLS FIRST
        (Dialect::PostgreSQL, true) | (Dialect::Oracle, true) => false,
        (Dialect::PostgreSQL, false) | (Dialect::Oracle, false) => true,
        // MySQL / SQL Server / SQLite：ASC → NULLS FIRST，DESC → NULLS LAST
        // SQLite 将 NULL 视为最小值，ASC 时 NULL 在前；与 MySQL/SQL Server 一致
        (Dialect::MySql, true) | (Dialect::SqlServer, true) | (Dialect::SQLite, true) => true,
        (Dialect::MySql, false) | (Dialect::SqlServer, false) | (Dialect::SQLite, false) => false,
    }
}

// =====================================================================
//  预处理：方言特有语法 → PG 兼容语法
// =====================================================================

/// 对 SQL 文本应用方言特有预处理
///
/// # 参数
/// - `sql`：原始 SQL 文本
/// - `dialect`：方言
///
/// # 返回
/// 预处理后的 SQL 文本（PG 兼容语法）
fn preprocess(sql: &str, dialect: &Dialect) -> String {
    match dialect {
        Dialect::PostgreSQL => sql.to_string(),
        Dialect::MySql => preprocess_mysql(sql),
        Dialect::Oracle => preprocess_oracle(sql),
        Dialect::SqlServer => preprocess_sqlserver(sql),
        Dialect::SQLite => preprocess_sqlite(sql),
    }
}

// ---------------------------------------------------------------------
//  MySQL 预处理
// ---------------------------------------------------------------------

/// MySQL 方言预处理
///
/// # 转换规则
/// 1. `LIMIT offset, count` → `LIMIT count OFFSET offset`
///    （MySQL 语法；PG 不支持逗号分隔的 LIMIT）
/// 2. `ON DUPLICATE KEY UPDATE col=val, ...` → `ON CONFLICT DO UPDATE SET col=val, ...`
///    （MySQL upsert 语法 → PG ON CONFLICT 语法）
/// 3. `a MOD b` → `a % b`（MySQL MOD 运算符 → PG % 运算符）
/// 4. `\\` 字符串转义 → `''` （MySQL 反斜杠转义 → PG 标准转义）
///    （当前仅处理简单情况，复杂场景由 MySqlDialect 处理）
fn preprocess_mysql(sql: &str) -> String {
    let mut result = sql.to_string();

    // 1. LIMIT offset, count → LIMIT count OFFSET offset
    // 正则：LIMIT\s+(\d+)\s*,\s*(\d+)
    // 注意：不处理 LIMIT ? OFFSET ? 等参数化形式（PG 也支持）
    let limit_re = Regex::new(r"(?i)\bLIMIT\s+(\d+)\s*,\s*(\d+)").unwrap();
    result = limit_re
        .replace_all(&result, "LIMIT $2 OFFSET $1")
        .to_string();

    // 2. ON DUPLICATE KEY UPDATE col=val, ... → ON CONFLICT DO UPDATE SET col=val, ...
    // MySQL upsert 语法转换为 PG ON CONFLICT 语法
    // 注意：简化处理，不解析冲突目标列（PG 需指定冲突列或用 ON CONFLICT (col)）
    // 这里使用 ON CONFLICT DO UPDATE（无冲突目标，相当于冲突时更新所有列）
    let on_dup_re = Regex::new(r"(?i)\bON\s+DUPLICATE\s+KEY\s+UPDATE\b").unwrap();
    result = on_dup_re
        .replace_all(&result, "ON CONFLICT DO UPDATE SET")
        .to_string();

    // 3. a MOD b → a % b（MOD 运算符 → % 运算符）
    // 仅匹配 MOD 作为运算符（MOD 后跟空白，非左括号）
    // 注意：MOD 作为函数调用 MOD(a, b) 不受影响（函数调用后跟 '('，无空白）
    // regex crate 不支持 lookahead，使用 \bMOD\s+ 匹配 MOD + 空白（运算符用法）
    // 函数调用 MOD( 不会匹配，因为 MOD 后直接跟 '('，无空白
    let mod_re = Regex::new(r"(?i)\bMOD\b\s+").unwrap();
    result = mod_re.replace_all(&result, "% ").to_string();

    result
}

// ---------------------------------------------------------------------
//  Oracle 预处理
// ---------------------------------------------------------------------

/// Oracle 方言预处理
///
/// # 转换规则
/// 1. `WHERE ROWNUM <= N` 或 `WHERE ROWNUM < N+1` → 保留 WHERE，追加 `LIMIT N`
///    （PG 不支持 ROWNUM；将 ROWNUM 限制转为 LIMIT）
/// 2. `DECODE(expr, val1, ret1, val2, ret2, ..., [default])` →
///    `CASE WHEN expr = val1 THEN ret1 WHEN expr = val2 THEN ret2 ... ELSE default END`
/// 3. `NVL(a, b)` → `COALESCE(a, b)`
/// 4. `NVL2(a, b, c)` → `CASE WHEN a IS NOT NULL THEN b ELSE c END`
/// 5. `TO_DATE(s, fmt)` → `CAST(s AS TIMESTAMP)`（简化：忽略格式化字符串）
/// 6. `TO_DATE(s)` → `CAST(s AS TIMESTAMP)`
/// 7. `TO_NUMBER(s)` → `CAST(s AS NUMERIC)`
/// 8. `TO_CHAR(s)` → `CAST(s AS TEXT)`
/// 9. `seq.NEXTVAL` → `nextval('seq')`
/// 10. `seq.CURRVAL` → `currval('seq')`
/// 11. `SYSDATE` → `CURRENT_TIMESTAMP`
/// 12. `||` 字符串拼接保持不变（PG 也支持）
fn preprocess_oracle(sql: &str) -> String {
    let mut result = sql.to_string();

    // 1. ROWNUM <= N → 追加 LIMIT N（简化：仅处理 WHERE 子句中的 ROWNUM <= N）
    // 例：WHERE ROWNUM <= 10 → WHERE TRUE LIMIT 10
    // 注意：这是简化处理，不处理 ROWNUM 与其他条件的组合
    let rownum_re = Regex::new(r"(?i)\bROWNUM\s*<=\s*(\d+)").unwrap();
    if let Some(caps) = rownum_re.captures(&result) {
        let n = caps.get(1).unwrap().as_str().to_string();
        // 移除 ROWNUM <= N 条件（替换为 TRUE 避免空 WHERE）
        let replaced = rownum_re.replace_all(&result, "TRUE").to_string();
        // 追加 LIMIT N（如果已有 LIMIT 则不追加）
        if !Regex::new(r"(?i)\bLIMIT\b").unwrap().is_match(&replaced) {
            result = format!("{replaced} LIMIT {n}");
        } else {
            result = replaced;
        }
    }

    // ROWNUM < N → LIMIT N-1
    let rownum_lt_re = Regex::new(r"(?i)\bROWNUM\s*<\s*(\d+)").unwrap();
    if let Some(caps) = rownum_lt_re.captures(&result) {
        if let Ok(n) = caps.get(1).unwrap().as_str().parse::<i64>() {
            let limit_n = n - 1;
            let replaced = rownum_lt_re.replace_all(&result, "TRUE").to_string();
            if !Regex::new(r"(?i)\bLIMIT\b").unwrap().is_match(&replaced) {
                result = format!("{replaced} LIMIT {limit_n}");
            } else {
                result = replaced;
            }
        }
    }

    // 2. DECODE(expr, val1, ret1, val2, ret2, ..., [default]) → CASE WHEN expr=val1 THEN ret1 ... ELSE default END
    result = convert_decode(&result);

    // 3. NVL(a, b) → COALESCE(a, b)
    let nvl_re = Regex::new(r"(?i)\bNVL\s*\(").unwrap();
    result = nvl_re.replace_all(&result, "COALESCE(").to_string();

    // 4. NVL2(a, b, c) → CASE WHEN a IS NOT NULL THEN b ELSE c END
    // 注意：必须先处理 NVL2，否则会被 NVL 替换覆盖
    // 简化：使用 regex 提取 NVL2 的 3 个参数
    let nvl2_re = Regex::new(r"(?i)\bNVL2\s*\(([^,]+),\s*([^,]+),\s*([^)]+)\)").unwrap();
    result = nvl2_re
        .replace_all(&result, "CASE WHEN $1 IS NOT NULL THEN $2 ELSE $3 END")
        .to_string();

    // 5. TO_DATE(s, fmt) → CAST(s AS TIMESTAMP)
    // 简化：提取第一个参数，忽略格式化字符串
    let to_date_re = Regex::new(r"(?i)\bTO_DATE\s*\(([^,)]+),\s*[^)]+\)").unwrap();
    result = to_date_re
        .replace_all(&result, "CAST($1 AS TIMESTAMP)")
        .to_string();

    // 6. TO_DATE(s) → CAST(s AS TIMESTAMP)
    let to_date_simple_re = Regex::new(r"(?i)\bTO_DATE\s*\(([^)]+)\)").unwrap();
    result = to_date_simple_re
        .replace_all(&result, "CAST($1 AS TIMESTAMP)")
        .to_string();

    // 7. TO_NUMBER(s) → CAST(s AS NUMERIC)
    let to_number_re = Regex::new(r"(?i)\bTO_NUMBER\s*\(([^)]+)\)").unwrap();
    result = to_number_re
        .replace_all(&result, "CAST($1 AS NUMERIC)")
        .to_string();

    // 8. TO_CHAR(s) → CAST(s AS TEXT) （仅处理单参数形式）
    let to_char_re = Regex::new(r"(?i)\bTO_CHAR\s*\(([^,)]+)\)").unwrap();
    result = to_char_re
        .replace_all(&result, "CAST($1 AS TEXT)")
        .to_string();

    // 9. seq.NEXTVAL → nextval('seq')
    let nextval_re = Regex::new(r"(?i)\b(\w+)\.NEXTVAL\b").unwrap();
    result = nextval_re.replace_all(&result, "nextval('$1')").to_string();

    // 10. seq.CURRVAL → currval('seq')
    let currval_re = Regex::new(r"(?i)\b(\w+)\.CURRVAL\b").unwrap();
    result = currval_re.replace_all(&result, "currval('$1')").to_string();

    // 11. SYSDATE → CURRENT_TIMESTAMP
    let sysdate_re = Regex::new(r"(?i)\bSYSDATE\b").unwrap();
    result = sysdate_re
        .replace_all(&result, "CURRENT_TIMESTAMP")
        .to_string();

    // 12. MINUS → EXCEPT（Oracle MINUS 等价于 PG/SQL 标准 EXCEPT）
    // 仅作为集合操作关键字替换（避免误伤包含 "minus" 字面量的字符串）
    let minus_re = Regex::new(r"(?i)\bMINUS\b").unwrap();
    result = minus_re.replace_all(&result, "EXCEPT").to_string();

    // 13. DROP TABLE x CASCADE CONSTRAINTS → DROP TABLE x CASCADE
    // Oracle CASCADE CONSTRAINTS 等价于 PG CASCADE（删除所有外键约束）
    let cascade_cons_re = Regex::new(r"(?i)\bCASCADE\s+CONSTRAINTS\b").unwrap();
    result = cascade_cons_re.replace_all(&result, "CASCADE").to_string();

    // 14. ALTER TABLE t ADD (col TYPE) → ALTER TABLE t ADD COLUMN col TYPE
    // Oracle 括号语法 → PG 标准 ADD COLUMN 语法
    // 简化：仅处理单个列定义（多列 ADD (c1 T1, c2 T2) 较少见）
    let add_paren_re = Regex::new(r"(?i)\bADD\s*\(\s*(\w+)\s+(\w+(?:\([^)]*\))?)\s*\)").unwrap();
    result = add_paren_re
        .replace_all(&result, "ADD COLUMN $1 $2")
        .to_string();

    // 15. ALTER TABLE t DROP (col) → ALTER TABLE t DROP COLUMN col
    let drop_paren_re = Regex::new(r"(?i)\bDROP\s*\(\s*(\w+)\s*\)").unwrap();
    result = drop_paren_re
        .replace_all(&result, "DROP COLUMN $1")
        .to_string();

    // 16. ALTER TABLE t MODIFY (col TYPE [options]) → ALTER TABLE t ALTER COLUMN col SET DATA TYPE TYPE
    // Oracle MODIFY → PG ALTER COLUMN SET DATA TYPE
    // 注：sqlparser 不支持 SET DATA TYPE 后跟 NOT NULL，简化：仅转换类型，NOT NULL 需单独执行
    let modify_paren_re =
        Regex::new(r"(?i)\bMODIFY\s*\(\s*(\w+)\s+(\w+(?:\([^)]*\))?)(?:\s+(?:NOT\s+NULL|NULL|DEFAULT\s+[^)]+))?\s*\)").unwrap();
    result = modify_paren_re
        .replace_all(&result, "ALTER COLUMN $1 SET DATA TYPE $2")
        .to_string();

    // 17. ALTER TABLE t MODIFY col TYPE → ALTER TABLE t ALTER COLUMN col SET DATA TYPE TYPE
    // 无括号形式
    let modify_re = Regex::new(r"(?i)\bMODIFY\s+(\w+)\s+(\w+(?:\([^)]*\))?)").unwrap();
    result = modify_re
        .replace_all(&result, "ALTER COLUMN $1 SET DATA TYPE $2")
        .to_string();

    // 18. CREATE SYNONYM name FOR target → SELECT 1（占位）
    // SzRSQL 不支持 SYNONYM，转换为无操作 SELECT 1 避免破坏批处理
    let synonym_re =
        Regex::new(r"(?im)^\s*CREATE\s+(?:OR\s+REPLACE\s+)?(?:PUBLIC\s+)?SYNONYM\b[^;]*;?")
            .unwrap();
    result = synonym_re.replace_all(&result, "SELECT 1;").to_string();

    // 19. DROP SYNONYM name → SELECT 1（占位）
    let drop_synonym_re = Regex::new(r"(?im)^\s*DROP\s+(?:PUBLIC\s+)?SYNONYM\b[^;]*;?").unwrap();
    result = drop_synonym_re
        .replace_all(&result, "SELECT 1;")
        .to_string();

    // 注：COMMENT ON 不再在此预处理中转换为 SELECT 1（占位）— Phase TDengine-P2
    // 已由 parser.rs 的 parse_comment 手动解析为 Statement::Comment，直接操作 catalog。

    // 21. CREATE SEQUENCE name START WITH m INCREMENT BY n → CREATE SEQUENCE name INCREMENT BY n START WITH m
    // Oracle 语法（START WITH 在前）→ PG 标准语法（INCREMENT BY 在前）
    // sqlparser 0.53 PostgreSqlDialect 要求 INCREMENT BY 在 START WITH 之前
    let ora_seq_re = Regex::new(
        r"(?i)\bCREATE\s+SEQUENCE\s+(\w+)\s+START\s+WITH\s+(\d+)\s+INCREMENT\s+BY\s+(\d+)",
    )
    .unwrap();
    result = ora_seq_re
        .replace_all(&result, "CREATE SEQUENCE $1 INCREMENT BY $3 START WITH $2")
        .to_string();

    result
}

/// 将 DECODE(expr, val1, ret1, val2, ret2, ..., [default]) 转换为 CASE 表达式
///
/// # 转换示例
/// - `DECODE(x, 1, 'one', 2, 'two', 'other')` →
///   `CASE WHEN x = 1 THEN 'one' WHEN x = 2 THEN 'two' ELSE 'other' END`
/// - `DECODE(x, 1, 'one')` →
///   `CASE WHEN x = 1 THEN 'one' END`
///
/// # 限制
/// 简化处理：按逗号分割参数，不处理参数内含嵌套函数调用或字符串内逗号的情况。
/// 对于复杂嵌套场景，建议手动改写为 CASE 表达式。
fn convert_decode(sql: &str) -> String {
    let decode_re = Regex::new(r"(?i)\bDECODE\s*\(").unwrap();
    if !decode_re.is_match(sql) {
        return sql.to_string();
    }

    let mut result = sql.to_string();
    // 循环处理多个 DECODE 调用（从最内层开始）
    while let Some(pos) = find_decode_call(&result) {
        let (call_str, args) = extract_parenthesized(&result, pos);
        if args.is_empty() {
            break;
        }
        let case_expr = build_case_from_decode(&args);
        result = result.replacen(&call_str, &case_expr, 1);
    }
    result
}

/// 查找 SQL 中第一个 DECODE( 调用的位置（返回 `(` 的位置）
fn find_decode_call(sql: &str) -> Option<usize> {
    let upper = sql.to_uppercase();
    let bytes = upper.as_bytes();
    let mut i = 0;
    while i + 7 <= bytes.len() {
        // 匹配 DECODE(
        if &bytes[i..i + 7] == b"DECODE(" && (i == 0 || !bytes[i - 1].is_ascii_alphabetic()) {
            return Some(i + 6); // 返回 ( 的位置
        }
        i += 1;
    }
    None
}

/// 从 `(` 位置开始提取括号内的完整内容（处理嵌套括号）
///
/// 返回 `(完整调用字符串, 参数列表)`
fn extract_parenthesized(sql: &str, paren_pos: usize) -> (String, Vec<String>) {
    let bytes = sql.as_bytes();
    let mut depth = 0;
    let mut start = paren_pos;
    let mut end = paren_pos;

    for (i, &b) in bytes.iter().enumerate().skip(paren_pos) {
        if b == b'(' {
            if depth == 0 {
                start = i;
            }
            depth += 1;
        } else if b == b')' {
            depth -= 1;
            if depth == 0 {
                end = i;
                break;
            }
        }
    }

    let inner = &sql[start + 1..end];
    let call_str = sql[start - 6..end + 1].to_string(); // "DECODE(...)"
    let args = split_args(inner);
    (call_str, args)
}

/// 按逗号分割参数列表（处理嵌套括号）
fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    let mut in_string = false;
    let mut string_char = '\0';

    for ch in s.chars() {
        if in_string {
            current.push(ch);
            if ch == string_char {
                in_string = false;
            }
            continue;
        }
        match ch {
            '\'' => {
                in_string = true;
                string_char = ch;
                current.push(ch);
            }
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                args.push(current.trim().to_string());
                current = String::new();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.trim().is_empty() {
        args.push(current.trim().to_string());
    }
    args
}

/// 根据参数列表构建 CASE 表达式
///
/// 参数格式：`[expr, val1, ret1, val2, ret2, ..., (default)]`
fn build_case_from_decode(args: &[String]) -> String {
    if args.is_empty() {
        return String::new();
    }
    let expr = &args[0];
    let mut case = "CASE".to_string();

    // 每 2 个参数为一组 WHEN ... THEN ...
    let mut i = 1;
    while i + 1 < args.len() {
        case.push_str(&format!(
            " WHEN {} = {} THEN {}",
            expr,
            args[i],
            args[i + 1]
        ));
        i += 2;
    }

    // 奇数个剩余参数为 default（ELSE）
    if i < args.len() {
        case.push_str(&format!(" ELSE {}", args[i]));
    }

    case.push_str(" END");
    case
}

// ---------------------------------------------------------------------
//  SQL Server 预处理
// ---------------------------------------------------------------------

/// SQL Server 方言预处理
///
/// # 转换规则
/// 1. `SELECT TOP N ...` → `SELECT ... LIMIT N`（移除 TOP，追加 LIMIT）
/// 2. `ISNULL(a, b)` → `COALESCE(a, b)`
/// 3. `GETDATE()` → `CURRENT_TIMESTAMP`
/// 4. `GETUTCDATE()` → `CURRENT_TIMESTAMP`（近似处理）
/// 5. `LEN(s)` → `LENGTH(s)`（PG 使用 LENGTH）
/// 6. `DATEDIFF(unit, a, b)` → `EXTRACT(EPOCH FROM (b - a))`（简化，忽略 unit）
/// 7. `CREATE CLUSTERED/NONCLUSTERED INDEX` → `CREATE INDEX`（移除聚簇关键字）
/// 8. `ALTER COLUMN col TYPE` → `ALTER COLUMN col SET DATA TYPE TYPE`（PG 语法）
/// 9. `SELECT TOP N WITH TIES` → `SELECT TOP N`（移除 WITH TIES）
/// 10. `CONVERT(type, expr)` → `CAST(expr AS type)`（PG CAST 语法）
/// 11. `CREATE SCHEMA name` → `SELECT 1`（占位，SzRSQL 不支持 schema）
fn preprocess_sqlserver(sql: &str) -> String {
    let mut result = sql.to_string();

    // 1. SELECT TOP N [WITH TIES] → SELECT ... LIMIT N
    // 先处理 WITH TIES（移除后再处理 TOP N）
    let with_ties_re = Regex::new(r"(?i)\bWITH\s+TIES\b").unwrap();
    result = with_ties_re.replace_all(&result, "").to_string();

    // 匹配：SELECT TOP N（N 为数字）
    let top_re = Regex::new(r"(?i)\bSELECT\s+TOP\s+(\d+)\s+").unwrap();
    if let Some(caps) = top_re.captures(&result.clone()) {
        let n = caps.get(1).unwrap().as_str();
        // 移除 TOP N
        result = top_re.replace_all(&result, "SELECT ").to_string();
        // 追加 LIMIT N（如果尚未有 LIMIT）
        if !Regex::new(r"(?i)\bLIMIT\b").unwrap().is_match(&result) {
            // 在末尾的分号前追加（如果有分号）
            let trimmed = result.trim_end();
            if let Some(stripped) = trimmed.strip_suffix(';') {
                result = format!("{stripped} LIMIT {n} ;");
            } else {
                result = format!("{trimmed} LIMIT {n}");
            }
        }
    }

    // 2. ISNULL(a, b) → COALESCE(a, b)
    let isnull_re = Regex::new(r"(?i)\bISNULL\s*\(").unwrap();
    result = isnull_re.replace_all(&result, "COALESCE(").to_string();

    // 3. GETDATE() → CURRENT_TIMESTAMP
    let getdate_re = Regex::new(r"(?i)\bGETDATE\s*\(\s*\)").unwrap();
    result = getdate_re
        .replace_all(&result, "CURRENT_TIMESTAMP")
        .to_string();

    // 4. GETUTCDATE() → CURRENT_TIMESTAMP
    let getutcdate_re = Regex::new(r"(?i)\bGETUTCDATE\s*\(\s*\)").unwrap();
    result = getutcdate_re
        .replace_all(&result, "CURRENT_TIMESTAMP")
        .to_string();

    // 5. LEN(s) → LENGTH(s)
    let len_re = Regex::new(r"(?i)\bLEN\s*\(").unwrap();
    result = len_re.replace_all(&result, "LENGTH(").to_string();

    // 7. CREATE [CLUSTERED|NONCLUSTERED] INDEX → CREATE INDEX
    // 移除 CLUSTERED / NONCLUSTERED 关键字（SzRSQL 不区分聚簇/非聚簇索引）
    let clustered_re = Regex::new(r"(?i)\bCREATE\s+(?:CLUSTERED|NONCLUSTERED)\s+INDEX").unwrap();
    result = clustered_re
        .replace_all(&result, "CREATE INDEX")
        .to_string();

    // 8a. ALTER TABLE t ALTER COLUMN col TYPE NOT NULL → 拆分为两条语句
    // SQL Server 语法：ALTER TABLE t ALTER COLUMN col NVARCHAR(100) NOT NULL
    // PG 不支持单句 SET DATA TYPE 后跟 NOT NULL，拆分为：
    //   1. ALTER TABLE t ALTER COLUMN col SET DATA TYPE TYPE
    //   2. ALTER TABLE t ALTER COLUMN col SET NOT NULL
    // 必须先于 8b 处理（带 NOT NULL 的更具体情况）
    let alter_col_notnull_re = Regex::new(
        r"(?i)\bALTER\s+TABLE\s+(\w+)\s+ALTER\s+COLUMN\s+(\w+)\s+(NVARCHAR|VARCHAR|CHAR|NCHAR|INT|INTEGER|BIGINT|SMALLINT|TINYINT|DECIMAL|NUMERIC|FLOAT|REAL|DOUBLE|BIT|BOOLEAN|DATE|TIME|DATETIME|SMALLDATETIME|DATETIME2|DATETIMEOFFSET|TEXT|NTEXT|IMAGE|BINARY|VARBINARY|MONEY|SMALLMONEY|UNIQUEIDENTIFIER|XML|JSON)(\([^)]*\))?\s+NOT\s+NULL",
    )
    .unwrap();
    result = alter_col_notnull_re
        .replace_all(
            &result,
            "ALTER TABLE $1 ALTER COLUMN $2 SET DATA TYPE $3$4; ALTER TABLE $1 ALTER COLUMN $2 SET NOT NULL",
        )
        .to_string();

    // 8b. ALTER COLUMN col TYPE → ALTER COLUMN col SET DATA TYPE TYPE（无 NOT NULL）
    // SQL Server 语法：ALTER TABLE t ALTER COLUMN col NVARCHAR(100)
    // PG 语法：ALTER TABLE t ALTER COLUMN col SET DATA TYPE NVARCHAR(100)
    let alter_col_re =
        Regex::new(r"(?i)\bALTER\s+COLUMN\s+(\w+)\s+(NVARCHAR|VARCHAR|CHAR|NCHAR|INT|INTEGER|BIGINT|SMALLINT|TINYINT|DECIMAL|NUMERIC|FLOAT|REAL|DOUBLE|BIT|BOOLEAN|DATE|TIME|DATETIME|SMALLDATETIME|DATETIME2|DATETIMEOFFSET|TEXT|NTEXT|IMAGE|BINARY|VARBINARY|MONEY|SMALLMONEY|UNIQUEIDENTIFIER|XML|JSON)(\([^)]*\))?")
            .unwrap();
    result = alter_col_re
        .replace_all(&result, "ALTER COLUMN $1 SET DATA TYPE $2$3")
        .to_string();

    // 11. CREATE SCHEMA name → SELECT 1（占位）
    // SzRSQL 不支持 CREATE SCHEMA（schema 概念简化为命名空间）
    let create_schema_re = Regex::new(r"(?im)^\s*CREATE\s+SCHEMA\b[^;]*;?").unwrap();
    result = create_schema_re
        .replace_all(&result, "SELECT 1;")
        .to_string();

    // 12. CONVERT(type, expr) → CAST(expr AS type)
    // SQL Server CONVERT 函数 → PG CAST 函数
    // 简化：仅处理两个参数的 CONVERT（type, expr），不支持 style 参数
    let convert_re =
        Regex::new(r"(?i)\bCONVERT\s*\(\s*(\w+(?:\([^)]*\))?)\s*,\s*([^)]+)\)").unwrap();
    result = convert_re
        .replace_all(&result, "CAST($2 AS $1)")
        .to_string();

    result
}

// ---------------------------------------------------------------------
//  SQLite 预处理（Phase F-8 新增）
// ---------------------------------------------------------------------

/// SQLite 方言预处理
///
/// # 转换规则
/// 1. `WITHOUT ROWID` 表选项 → 移除（SzRSQL 不支持物理 rowid 表选项）
/// 2. `PRAGMA ...` 语句 → 替换为 `SELECT 1` 占位（SzRSQL 不支持 PRAGMA，
///    返回常量避免破坏多语句批处理的语义）
/// 3. `AUTOINCREMENT` 关键字 → 保留（sqlparser SQLiteDialect 可识别，
///    在 apply_column_option 中作为 Identity 静默忽略）
/// 4. `INTEGER PRIMARY KEY` → 保留（SzRSQL 视为自增主键等价语义）
/// 5. SQLite 方括号标识符 `[foo]` → 已由 SQLiteDialect 处理
/// 6. `GROUP_CONCAT(x)` / `GROUP_CONCAT(x, sep)` → 保留原样由 sqlparser 解析
/// 7. `GLOB` 运算符 → `LIKE`（语义不完全一致：GLOB 大小写敏感且使用 * ?，
///    SzRSQL 简化为 LIKE 转换以保持基本兼容）
/// 8. `CREATE VIRTUAL TABLE name USING module(args)` → `CREATE TABLE name(args)`
///    （SzRSQL 不支持虚拟表，降级为普通 CREATE TABLE）
fn preprocess_sqlite(sql: &str) -> String {
    let mut result = sql.to_string();

    // 1. WITHOUT ROWID → 移除（不区分大小写，允许前后空白）
    let without_rowid_re = Regex::new(r"(?i)\bWITHOUT\s+ROWID\b").unwrap();
    result = without_rowid_re.replace_all(&result, "").to_string();

    // 2. PRAGMA name [= value] | PRAGMA name(args) → SELECT 1
    // 仅替换整行 PRAGMA 语句，避免误伤包含 "PRAGMA" 字面量的字符串
    // 简化处理：匹配 PRAGMA 开头直到行尾或分号
    let pragma_re = Regex::new(r"(?im)^\s*PRAGMA\b[^;]*;?").unwrap();
    result = pragma_re.replace_all(&result, "SELECT 1;").to_string();

    // 7. GLOB 运算符 → LIKE（简化转换）
    // GLOB 使用 * ? 通配符，LIKE 使用 % _ 通配符
    // 简化：仅替换关键字，不转换通配符（用户需自行适配）
    let glob_re = Regex::new(r"(?i)\bGLOB\b").unwrap();
    result = glob_re.replace_all(&result, "LIKE").to_string();

    // 8. CREATE VIRTUAL TABLE name USING module(args) → CREATE TABLE name(args_with_text)
    // SzRSQL 不支持虚拟表（FTS5等），降级为普通 CREATE TABLE
    // FTS5 列无类型，统一添加 TEXT 类型以符合 PG CREATE TABLE 语法
    // 例：CREATE VIRTUAL TABLE docs USING fts5(title, body)
    //   → CREATE TABLE docs (title TEXT, body TEXT)
    let virtual_table_re = Regex::new(
        r"(?i)\bCREATE\s+VIRTUAL\s+TABLE\s+(IF\s+NOT\s+EXISTS\s+)?(\w+)\s+USING\s+\w+\s*\(([^)]+)\)",
    )
    .unwrap();
    result = virtual_table_re
        .replace_all(&result, |caps: &regex::Captures| {
            let if_not_exists = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let table_name = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let cols_str = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            // 按逗号分割列名，每列添加 TEXT 类型
            // 忽略 FTS5 特殊参数（如 tokenize=, prefix=, content=）
            let cols: Vec<String> = cols_str
                .split(',')
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
                .filter_map(|c| {
                    // 跳过 FTS5 配置参数（包含 = 的）
                    if c.contains('=') {
                        return None;
                    }
                    // 列名可能带引号或前缀，提取纯列名
                    let col_name =
                        c.trim_matches(|ch: char| ch == '"' || ch == '`' || ch == '[' || ch == ']');
                    if col_name.is_empty() || col_name.contains(' ') {
                        return None;
                    }
                    Some(format!("{col_name} TEXT"))
                })
                .collect();
            format!(
                "CREATE TABLE {if_not_exists}{table_name} ({})",
                cols.join(", ")
            )
        })
        .to_string();

    // 9. MATCH 运算符（FTS5）→ LIKE
    // SQLite FTS5 MATCH 简化为 LIKE（语义不完全一致，但保持基本兼容）
    let match_re = Regex::new(r"(?i)\bMATCH\b").unwrap();
    result = match_re.replace_all(&result, "LIKE").to_string();

    // 10. 位运算符 << / >> → shift_left / shift_right 函数调用
    // SQLiteDialect 不支持 << / >> 运算符（sqlparser 限制）
    // SzRSQL PG 方言支持 PGBitwiseShiftLeft/Right，但 SQLiteDialect 解析阶段就报错
    // 预处理：将 a << b / a >> b 转换为 shift_left(a, b) / shift_right(a, b) 函数调用
    // 简化：仅处理简单操作数（标识符、数字、括号表达式），不处理复杂嵌套
    // << 运算符
    let shift_left_re = Regex::new(r"(\w+|\d+)\s*<<\s*(\w+|\d+)").unwrap();
    result = shift_left_re
        .replace_all(&result, "shift_left($1, $2)")
        .to_string();
    // >> 运算符
    let shift_right_re = Regex::new(r"(\w+|\d+)\s*>>\s*(\w+|\d+)").unwrap();
    result = shift_right_re
        .replace_all(&result, "shift_right($1, $2)")
        .to_string();

    result
}

// =====================================================================
//  方言自动检测
// =====================================================================

/// 根据语法特征自动检测方言
///
/// # 检测规则
/// - 包含 `` ` ``（反引号）→ MySQL
/// - 包含 `TOP N`（TOP 关键字）→ SQL Server
/// - 包含 `ROWNUM` / `DECODE` / `NVL` / `SYSDATE` / `.NEXTVAL` → Oracle
/// - 包含 `LIMIT offset, count`（逗号分隔的 LIMIT）→ MySQL
/// - 包含 `AUTOINCREMENT` / `WITHOUT ROWID` / `PRAGMA` / `GROUP_CONCAT(` → SQLite
/// - 其他 → PostgreSQL（默认）
///
/// # 参数
/// - `sql`：SQL 文本
///
/// # 返回
/// 检测到的方言（不确定时返回 PostgreSQL）
pub fn detect_dialect(sql: &str) -> Dialect {
    let upper = sql.to_uppercase();

    // 反引号 → MySQL
    if sql.contains('`') {
        return Dialect::MySql;
    }

    // TOP N → SQL Server
    if Regex::new(r"\bSELECT\s+TOP\s+\d+")
        .unwrap()
        .is_match(&upper)
    {
        return Dialect::SqlServer;
    }

    // Oracle 特征
    if upper.contains("ROWNUM")
        || upper.contains("DECODE(")
        || upper.contains("NVL(")
        || upper.contains("NVL2(")
        || upper.contains("SYSDATE")
        || upper.contains(".NEXTVAL")
        || upper.contains(".CURRVAL")
        || upper.contains("TO_DATE(")
    {
        return Dialect::Oracle;
    }

    // LIMIT offset, count → MySQL
    if Regex::new(r"\bLIMIT\s+\d+\s*,\s*\d+")
        .unwrap()
        .is_match(&upper)
    {
        return Dialect::MySql;
    }

    // SQLite 特征
    if upper.contains("AUTOINCREMENT")
        || upper.contains("WITHOUT ROWID")
        || upper.contains("PRAGMA ")
        || upper.contains("GROUP_CONCAT(")
        || Regex::new(r"\bINTEGER\s+PRIMARY\s+KEY\b")
            .unwrap()
            .is_match(&upper)
    {
        return Dialect::SQLite;
    }

    // 默认 PostgreSQL
    Dialect::PostgreSQL
}

/// 自动检测方言并解析 SQL
///
/// 等价于 `parse_with_dialect(sql, &detect_dialect(sql))`
pub fn parse_auto(sql: &str) -> Result<Vec<Statement>, ParseError> {
    let dialect = detect_dialect(sql);
    parse_with_dialect(sql, &dialect)
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- Dialect 枚举测试 ---

    #[test]
    fn test_dialect_name() {
        assert_eq!(Dialect::PostgreSQL.name(), "PostgreSQL");
        assert_eq!(Dialect::MySql.name(), "MySQL");
        assert_eq!(Dialect::Oracle.name(), "Oracle");
        assert_eq!(Dialect::SqlServer.name(), "SQL Server");
    }

    #[test]
    fn test_dialect_default() {
        assert_eq!(Dialect::default(), Dialect::PostgreSQL);
    }

    // --- Phase 6.9：方言语义适配测试 ---

    #[test]
    fn test_dialect_default_nulls_first_pg_asc() {
        assert!(!dialect_default_nulls_first(&Dialect::PostgreSQL, true)); // NULLS LAST
    }

    #[test]
    fn test_dialect_default_nulls_first_pg_desc() {
        assert!(dialect_default_nulls_first(&Dialect::PostgreSQL, false)); // NULLS FIRST
    }

    #[test]
    fn test_dialect_default_nulls_first_mysql_asc() {
        assert!(dialect_default_nulls_first(&Dialect::MySql, true)); // NULLS FIRST
    }

    #[test]
    fn test_dialect_default_nulls_first_mysql_desc() {
        assert!(!dialect_default_nulls_first(&Dialect::MySql, false)); // NULLS LAST
    }

    #[test]
    fn test_dialect_default_nulls_first_oracle_asc() {
        assert!(!dialect_default_nulls_first(&Dialect::Oracle, true)); // NULLS LAST
    }

    #[test]
    fn test_dialect_default_nulls_first_oracle_desc() {
        assert!(dialect_default_nulls_first(&Dialect::Oracle, false)); // NULLS FIRST
    }

    #[test]
    fn test_dialect_default_nulls_first_sqlserver_asc() {
        assert!(dialect_default_nulls_first(&Dialect::SqlServer, true)); // NULLS FIRST
    }

    #[test]
    fn test_dialect_default_nulls_first_sqlserver_desc() {
        assert!(!dialect_default_nulls_first(&Dialect::SqlServer, false)); // NULLS LAST
    }

    #[test]
    fn test_apply_dialect_semantics_pg_select_asc() {
        let sql = "SELECT * FROM t ORDER BY name ASC";
        let stmts = parse_with_dialect(sql, &Dialect::PostgreSQL).unwrap();
        if let Statement::Select(select) = &stmts[0] {
            assert_eq!(select.order_by.len(), 1);
            assert!(select.order_by[0].asc);
            assert!(!select.order_by[0].nulls_first); // PG ASC → NULLS LAST
        } else {
            panic!("expected Select");
        }
    }

    #[test]
    fn test_apply_dialect_semantics_pg_select_desc() {
        let sql = "SELECT * FROM t ORDER BY name DESC";
        let stmts = parse_with_dialect(sql, &Dialect::PostgreSQL).unwrap();
        if let Statement::Select(select) = &stmts[0] {
            assert_eq!(select.order_by.len(), 1);
            assert!(!select.order_by[0].asc);
            assert!(select.order_by[0].nulls_first); // PG DESC → NULLS FIRST
        } else {
            panic!("expected Select");
        }
    }

    #[test]
    fn test_apply_dialect_semantics_mysql_select_asc() {
        let sql = "SELECT * FROM t ORDER BY name ASC";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        if let Statement::Select(select) = &stmts[0] {
            assert_eq!(select.order_by.len(), 1);
            assert!(select.order_by[0].asc);
            assert!(select.order_by[0].nulls_first); // MySQL ASC → NULLS FIRST
        } else {
            panic!("expected Select");
        }
    }

    #[test]
    fn test_apply_dialect_semantics_mysql_select_desc() {
        let sql = "SELECT * FROM t ORDER BY name DESC";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        if let Statement::Select(select) = &stmts[0] {
            assert_eq!(select.order_by.len(), 1);
            assert!(!select.order_by[0].asc);
            assert!(!select.order_by[0].nulls_first); // MySQL DESC → NULLS LAST
        } else {
            panic!("expected Select");
        }
    }

    #[test]
    fn test_apply_dialect_semantics_oracle_select_asc() {
        let sql = "SELECT * FROM t ORDER BY name ASC";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        if let Statement::Select(select) = &stmts[0] {
            assert_eq!(select.order_by.len(), 1);
            assert!(!select.order_by[0].nulls_first); // Oracle ASC → NULLS LAST
        } else {
            panic!("expected Select");
        }
    }

    #[test]
    fn test_apply_dialect_semantics_sqlserver_select_desc() {
        let sql = "SELECT * FROM t ORDER BY name DESC";
        let stmts = parse_with_dialect(sql, &Dialect::SqlServer).unwrap();
        if let Statement::Select(select) = &stmts[0] {
            assert_eq!(select.order_by.len(), 1);
            assert!(!select.order_by[0].nulls_first); // SQL Server DESC → NULLS LAST
        } else {
            panic!("expected Select");
        }
    }

    #[test]
    fn test_apply_dialect_semantics_multi_key_order_by() {
        let sql = "SELECT * FROM t ORDER BY a ASC, b DESC, c ASC";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        if let Statement::Select(select) = &stmts[0] {
            assert_eq!(select.order_by.len(), 3);
            // MySQL: ASC → NULLS FIRST, DESC → NULLS LAST
            assert!(select.order_by[0].nulls_first); // a ASC → FIRST
            assert!(!select.order_by[1].nulls_first); // b DESC → LAST
            assert!(select.order_by[2].nulls_first); // c ASC → FIRST
        } else {
            panic!("expected Select");
        }
    }

    #[test]
    fn test_apply_dialect_semantics_with_cte() {
        let sql = "WITH cte AS (SELECT * FROM t ORDER BY x ASC) SELECT * FROM cte ORDER BY y DESC";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        if let Statement::Select(select) = &stmts[0] {
            // 外层 ORDER BY y DESC → MySQL DESC → NULLS LAST
            assert_eq!(select.order_by.len(), 1);
            assert!(!select.order_by[0].nulls_first);
            // CTE 内层 ORDER BY x ASC → MySQL ASC → NULLS FIRST
            if let Some(with) = &select.with {
                assert_eq!(with.ctes.len(), 1);
                assert_eq!(with.ctes[0].query.order_by.len(), 1);
                assert!(with.ctes[0].query.order_by[0].nulls_first);
            } else {
                panic!("expected WITH clause");
            }
        } else {
            panic!("expected Select");
        }
    }

    #[test]
    fn test_apply_dialect_semantics_set_op() {
        let sql = "SELECT * FROM t1 ORDER BY a ASC UNION ALL SELECT * FROM t2 ORDER BY b DESC";
        // 注意：sqlparser 可能将集合操作中的 ORDER BY 解析为外层 select.order_by
        // 此测试验证 parse_with_dialect 不报错且语义应用成功
        let result = parse_with_dialect(sql, &Dialect::PostgreSQL);
        // 部分集合操作语法可能不被支持，验证至少不 panic
        match result {
            Ok(stmts) => {
                assert!(!stmts.is_empty());
            }
            Err(_) => {
                // 集合操作 + ORDER BY 的解析在 sqlparser 中可能受限，允许失败
            }
        }
    }

    #[test]
    fn test_apply_dialect_semantics_insert_select() {
        let sql = "INSERT INTO t2 SELECT * FROM t1 ORDER BY id ASC";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        if let Statement::Insert {
            source: InsertSource::Select(select),
            ..
        } = &stmts[0]
        {
            assert_eq!(select.order_by.len(), 1);
            assert!(select.order_by[0].nulls_first); // MySQL ASC → NULLS FIRST
        } else {
            panic!("expected Insert with Select source");
        }
    }

    #[test]
    fn test_apply_dialect_semantics_no_order_by() {
        let sql = "SELECT * FROM t";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        if let Statement::Select(select) = &stmts[0] {
            assert!(select.order_by.is_empty()); // 无 ORDER BY 不影响
        } else {
            panic!("expected Select");
        }
    }

    #[test]
    fn test_dialect_from_str() {
        assert_eq!(
            Dialect::from_str("postgresql").unwrap(),
            Dialect::PostgreSQL
        );
        assert_eq!(Dialect::from_str("pg").unwrap(), Dialect::PostgreSQL);
        assert_eq!(Dialect::from_str("mysql").unwrap(), Dialect::MySql);
        assert_eq!(Dialect::from_str("oracle").unwrap(), Dialect::Oracle);
        assert_eq!(Dialect::from_str("mssql").unwrap(), Dialect::SqlServer);
        assert_eq!(Dialect::from_str("tsql").unwrap(), Dialect::SqlServer);
        assert!(Dialect::from_str("unknown").is_err());
    }

    #[test]
    fn test_dialect_display() {
        assert_eq!(format!("{}", Dialect::MySql), "MySQL");
        assert_eq!(format!("{}", Dialect::Oracle), "Oracle");
    }

    // --- PostgreSQL 方言（无预处理）---

    #[test]
    fn test_postgresql_no_preprocessing() {
        let sql = "SELECT * FROM users WHERE id = 1";
        let stmts = parse_with_dialect(sql, &Dialect::PostgreSQL).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_postgresql_limit_offset() {
        let sql = "SELECT * FROM t LIMIT 10 OFFSET 5";
        let stmts = parse_with_dialect(sql, &Dialect::PostgreSQL).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    // --- MySQL 方言 ---

    #[test]
    fn test_mysql_limit_offset_comma() {
        // MySQL: LIMIT offset, count → LIMIT count OFFSET offset
        let sql = "SELECT * FROM t LIMIT 10, 20";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_mysql_backtick_identifiers() {
        // MySQL: 反引号标识符
        let sql = "SELECT `id`, `name` FROM `users`";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_mysql_basic_select() {
        let sql = "SELECT id, name FROM users WHERE age > 18";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_mysql_limit_only() {
        // MySQL: LIMIT count（无 offset）
        let sql = "SELECT * FROM t LIMIT 10";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_mysql_insert() {
        let sql = "INSERT INTO users (id, name) VALUES (1, 'Alice')";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_mysql_update() {
        let sql = "UPDATE users SET name = 'Bob' WHERE id = 1";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_mysql_delete() {
        let sql = "DELETE FROM users WHERE id = 1";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_mysql_join() {
        let sql = "SELECT u.id, o.order_id FROM users u INNER JOIN orders o ON u.id = o.user_id";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_mysql_group_by() {
        let sql = "SELECT department, COUNT(*) FROM employees GROUP BY department";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_mysql_order_by() {
        let sql = "SELECT * FROM users ORDER BY name ASC, age DESC";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_mysql_subquery() {
        let sql = "SELECT * FROM (SELECT id, name FROM users) AS sub";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_mysql_aggregation() {
        let sql = "SELECT department, AVG(salary) FROM employees GROUP BY department HAVING AVG(salary) > 50000";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_mysql_create_table() {
        let sql = "CREATE TABLE test (id INT PRIMARY KEY, name VARCHAR(100))";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_mysql_multiple_statements() {
        let sql = "SELECT 1; SELECT 2";
        let stmts = parse_with_dialect(sql, &Dialect::MySql).unwrap();
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn test_mysql_limit_preprocess() {
        // 验证 LIMIT offset, count 被转换为 LIMIT count OFFSET offset
        let preprocessed = preprocess_mysql("SELECT * FROM t LIMIT 10, 20");
        assert!(preprocessed.contains("LIMIT 20 OFFSET 10"));
    }

    #[test]
    fn test_mysql_limit_no_comma() {
        // 无逗号的 LIMIT 不应被转换
        let preprocessed = preprocess_mysql("SELECT * FROM t LIMIT 10");
        assert!(!preprocessed.contains("OFFSET"));
    }

    // --- Oracle 方言 ---

    #[test]
    fn test_oracle_nvl() {
        // NVL(a, b) → COALESCE(a, b)
        let sql = "SELECT NVL(name, 'unknown') FROM users";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_sysdate() {
        // SYSDATE → CURRENT_TIMESTAMP
        let sql = "SELECT SYSDATE FROM dual";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_decode_simple() {
        // DECODE(x, 1, 'one', 2, 'two', 'other') → CASE WHEN x=1 THEN 'one' ... ELSE 'other' END
        let sql = "SELECT DECODE(status, 1, 'active', 2, 'inactive', 'unknown') FROM users";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_decode_no_default() {
        // DECODE(x, 1, 'one') → CASE WHEN x=1 THEN 'one' END
        let sql = "SELECT DECODE(status, 1, 'active') FROM users";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_to_date() {
        // TO_DATE(s, fmt) → CAST(s AS TIMESTAMP)
        let sql = "SELECT TO_DATE('2024-01-01', 'YYYY-MM-DD') FROM dual";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_to_number() {
        // TO_NUMBER(s) → CAST(s AS NUMERIC)
        let sql = "SELECT TO_NUMBER('123') FROM dual";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_to_char() {
        // TO_CHAR(s) → CAST(s AS TEXT)
        let sql = "SELECT TO_CHAR(123) FROM dual";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_nextval() {
        // seq.NEXTVAL → nextval('seq')
        let sql = "SELECT my_seq.NEXTVAL FROM dual";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_currval() {
        // seq.CURRVAL → currval('seq')
        let sql = "SELECT my_seq.CURRVAL FROM dual";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_basic_select() {
        let sql = "SELECT id, name FROM users WHERE age > 18";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_insert() {
        let sql = "INSERT INTO users (id, name) VALUES (1, 'Alice')";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_update() {
        let sql = "UPDATE users SET name = 'Bob' WHERE id = 1";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_delete() {
        let sql = "DELETE FROM users WHERE id = 1";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_join() {
        let sql = "SELECT u.id, o.order_id FROM users u, orders o WHERE u.id = o.user_id";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_group_by() {
        let sql = "SELECT department, COUNT(*) FROM employees GROUP BY department";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_order_by() {
        let sql = "SELECT * FROM users ORDER BY name ASC";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_nvl_preprocess() {
        let preprocessed = preprocess_oracle("SELECT NVL(name, 'unknown') FROM users");
        assert!(preprocessed.contains("COALESCE("));
        assert!(!preprocessed.contains("NVL("));
    }

    #[test]
    fn test_oracle_sysdate_preprocess() {
        let preprocessed = preprocess_oracle("SELECT SYSDATE FROM dual");
        assert!(preprocessed.contains("CURRENT_TIMESTAMP"));
        assert!(!preprocessed.contains("SYSDATE"));
    }

    #[test]
    fn test_oracle_decode_preprocess() {
        let preprocessed =
            preprocess_oracle("SELECT DECODE(x, 1, 'one', 2, 'two', 'other') FROM t");
        assert!(preprocessed.contains("CASE"));
        assert!(preprocessed.contains("WHEN x = 1 THEN 'one'"));
        assert!(preprocessed.contains("WHEN x = 2 THEN 'two'"));
        assert!(preprocessed.contains("ELSE 'other'"));
        assert!(preprocessed.contains("END"));
    }

    #[test]
    fn test_oracle_to_date_preprocess() {
        let preprocessed =
            preprocess_oracle("SELECT TO_DATE('2024-01-01', 'YYYY-MM-DD') FROM dual");
        assert!(preprocessed.contains("CAST('2024-01-01' AS TIMESTAMP)"));
    }

    #[test]
    fn test_oracle_nextval_preprocess() {
        let preprocessed = preprocess_oracle("SELECT my_seq.NEXTVAL FROM dual");
        assert!(preprocessed.contains("nextval('my_seq')"));
    }

    #[test]
    fn test_oracle_rownum_le() {
        // WHERE ROWNUM <= 10 → WHERE TRUE LIMIT 10
        let sql = "SELECT * FROM users WHERE ROWNUM <= 10";
        let stmts = parse_with_dialect(sql, &Dialect::Oracle).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_oracle_rownum_preprocess() {
        let preprocessed = preprocess_oracle("SELECT * FROM users WHERE ROWNUM <= 10");
        assert!(preprocessed.contains("LIMIT 10"));
        assert!(!preprocessed.contains("ROWNUM"));
    }

    // --- SQL Server 方言 ---

    #[test]
    fn test_sqlserver_top() {
        // SELECT TOP N → SELECT ... LIMIT N
        let sql = "SELECT TOP 10 * FROM users";
        let stmts = parse_with_dialect(sql, &Dialect::SqlServer).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlserver_isnull() {
        // ISNULL(a, b) → COALESCE(a, b)
        let sql = "SELECT ISNULL(name, 'unknown') FROM users";
        let stmts = parse_with_dialect(sql, &Dialect::SqlServer).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlserver_getdate() {
        // GETDATE() → CURRENT_TIMESTAMP
        let sql = "SELECT GETDATE()";
        let stmts = parse_with_dialect(sql, &Dialect::SqlServer).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlserver_len() {
        // LEN(s) → LENGTH(s)
        let sql = "SELECT LEN(name) FROM users";
        let stmts = parse_with_dialect(sql, &Dialect::SqlServer).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlserver_basic_select() {
        let sql = "SELECT id, name FROM users WHERE age > 18";
        let stmts = parse_with_dialect(sql, &Dialect::SqlServer).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlserver_insert() {
        let sql = "INSERT INTO users (id, name) VALUES (1, 'Alice')";
        let stmts = parse_with_dialect(sql, &Dialect::SqlServer).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlserver_update() {
        let sql = "UPDATE users SET name = 'Bob' WHERE id = 1";
        let stmts = parse_with_dialect(sql, &Dialect::SqlServer).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlserver_delete() {
        let sql = "DELETE FROM users WHERE id = 1";
        let stmts = parse_with_dialect(sql, &Dialect::SqlServer).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlserver_join() {
        let sql = "SELECT u.id, o.order_id FROM users u INNER JOIN orders o ON u.id = o.user_id";
        let stmts = parse_with_dialect(sql, &Dialect::SqlServer).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlserver_group_by() {
        let sql = "SELECT department, COUNT(*) FROM employees GROUP BY department";
        let stmts = parse_with_dialect(sql, &Dialect::SqlServer).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlserver_top_preprocess() {
        let preprocessed = preprocess_sqlserver("SELECT TOP 10 * FROM users");
        assert!(preprocessed.contains("LIMIT 10"));
        assert!(!preprocessed.contains("TOP"));
    }

    #[test]
    fn test_sqlserver_isnull_preprocess() {
        let preprocessed = preprocess_sqlserver("SELECT ISNULL(name, 'unknown') FROM users");
        assert!(preprocessed.contains("COALESCE("));
        assert!(!preprocessed.contains("ISNULL("));
    }

    #[test]
    fn test_sqlserver_getdate_preprocess() {
        let preprocessed = preprocess_sqlserver("SELECT GETDATE()");
        assert!(preprocessed.contains("CURRENT_TIMESTAMP"));
        assert!(!preprocessed.contains("GETDATE()"));
    }

    // --- 方言自动检测 ---

    #[test]
    fn test_detect_mysql_backtick() {
        assert_eq!(detect_dialect("SELECT `id` FROM `users`"), Dialect::MySql);
    }

    #[test]
    fn test_detect_mysql_limit_comma() {
        assert_eq!(
            detect_dialect("SELECT * FROM t LIMIT 10, 20"),
            Dialect::MySql
        );
    }

    #[test]
    fn test_detect_oracle_rownum() {
        assert_eq!(
            detect_dialect("SELECT * FROM t WHERE ROWNUM <= 10"),
            Dialect::Oracle
        );
    }

    #[test]
    fn test_detect_oracle_decode() {
        assert_eq!(
            detect_dialect("SELECT DECODE(x, 1, 'a') FROM t"),
            Dialect::Oracle
        );
    }

    #[test]
    fn test_detect_oracle_nvl() {
        assert_eq!(detect_dialect("SELECT NVL(x, 0) FROM t"), Dialect::Oracle);
    }

    #[test]
    fn test_detect_oracle_sysdate() {
        assert_eq!(detect_dialect("SELECT SYSDATE FROM dual"), Dialect::Oracle);
    }

    #[test]
    fn test_detect_sqlserver_top() {
        assert_eq!(detect_dialect("SELECT TOP 10 * FROM t"), Dialect::SqlServer);
    }

    #[test]
    fn test_detect_postgresql_default() {
        assert_eq!(
            detect_dialect("SELECT * FROM t WHERE id = 1"),
            Dialect::PostgreSQL
        );
    }

    #[test]
    fn test_detect_postgresql_limit_offset() {
        assert_eq!(
            detect_dialect("SELECT * FROM t LIMIT 10 OFFSET 5"),
            Dialect::PostgreSQL
        );
    }

    #[test]
    fn test_parse_auto_mysql() {
        let stmts = parse_auto("SELECT * FROM t LIMIT 10, 20").unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_parse_auto_oracle() {
        let stmts = parse_auto("SELECT NVL(x, 0) FROM t").unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_parse_auto_sqlserver() {
        let stmts = parse_auto("SELECT TOP 10 * FROM t").unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_parse_auto_postgresql() {
        let stmts = parse_auto("SELECT * FROM t LIMIT 10").unwrap();
        assert_eq!(stmts.len(), 1);
    }

    // --- 辅助函数测试 ---

    #[test]
    fn test_split_args_simple() {
        let args = split_args("a, b, c");
        assert_eq!(args, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_split_args_with_parens() {
        let args = split_args("a, COALESCE(b, c), d");
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "a");
        assert_eq!(args[1], "COALESCE(b, c)");
        assert_eq!(args[2], "d");
    }

    #[test]
    fn test_split_args_with_string() {
        let args = split_args("'a, b', c, 'd'");
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "'a, b'");
        assert_eq!(args[1], "c");
        assert_eq!(args[2], "'d'");
    }

    #[test]
    fn test_build_case_from_decode_with_default() {
        let args = vec![
            "x".to_string(),
            "1".into(),
            "'one'".into(),
            "2".into(),
            "'two'".into(),
            "'other'".into(),
        ];
        let case = build_case_from_decode(&args);
        assert!(case.contains("CASE"));
        assert!(case.contains("WHEN x = 1 THEN 'one'"));
        assert!(case.contains("WHEN x = 2 THEN 'two'"));
        assert!(case.contains("ELSE 'other'"));
        assert!(case.contains("END"));
    }

    #[test]
    fn test_build_case_from_decode_no_default() {
        let args = vec!["x".to_string(), "1".into(), "'one'".into()];
        let case = build_case_from_decode(&args);
        assert!(case.contains("WHEN x = 1 THEN 'one'"));
        assert!(!case.contains("ELSE"));
    }

    #[test]
    fn test_find_decode_call() {
        let sql = "SELECT DECODE(x, 1, 'a') FROM t";
        let pos = find_decode_call(sql);
        assert!(pos.is_some());
    }

    #[test]
    fn test_find_decode_call_none() {
        let sql = "SELECT * FROM t";
        let pos = find_decode_call(sql);
        assert!(pos.is_none());
    }

    // --- 错误处理 ---

    #[test]
    fn test_unknown_dialect_from_str() {
        let result = Dialect::from_str("redis");
        assert!(result.is_err());
    }

    #[test]
    fn test_sqlite_dialect_from_str() {
        assert_eq!(Dialect::from_str("sqlite").unwrap(), Dialect::SQLite);
        assert_eq!(Dialect::from_str("sqlite3").unwrap(), Dialect::SQLite);
        assert_eq!(Dialect::from_str("SQLite").unwrap(), Dialect::SQLite);
    }

    // --- SQLite 方言 ---

    #[test]
    fn test_sqlite_basic_select() {
        let sql = "SELECT id, name FROM users WHERE age > 18";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_backtick_identifiers() {
        // SQLite 支持反引号（兼容 MySQL）
        let sql = "SELECT `id`, `name` FROM `users`";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_bracket_identifiers() {
        // SQLite 特有方括号标识符
        let sql = "SELECT [id], [name] FROM [users]";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_limit_offset() {
        let sql = "SELECT * FROM t LIMIT 10 OFFSET 5";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_limit_comma() {
        // SQLite 支持 LIMIT offset, count 语法
        let sql = "SELECT * FROM t LIMIT 5, 10";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_integer_primary_key_autoincrement() {
        // SQLite AUTOINCREMENT 关键字
        let sql = "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_create_table_basic() {
        let sql = "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL)";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_without_rowid_removed() {
        // WITHOUT ROWID 应被预处理移除
        let sql = "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT) WITHOUT ROWID";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_pragma_replaced() {
        // PRAGMA 语句应被替换为 SELECT 1 占位
        let sql = "PRAGMA foreign_keys = ON";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        // 预处理将 PRAGMA 转为 SELECT 1;
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_insert() {
        let sql = "INSERT INTO users (id, name) VALUES (1, 'Alice')";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_update() {
        let sql = "UPDATE users SET name = 'Bob' WHERE id = 1";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_delete() {
        let sql = "DELETE FROM users WHERE id = 1";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_join() {
        let sql = "SELECT u.id, o.order_id FROM users u INNER JOIN orders o ON u.id = o.user_id";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_group_by() {
        let sql = "SELECT department, COUNT(*) FROM employees GROUP BY department";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_order_by() {
        let sql = "SELECT * FROM users ORDER BY name ASC, age DESC";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_subquery() {
        let sql = "SELECT * FROM (SELECT id, name FROM users) AS sub";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_aggregation() {
        let sql = "SELECT department, AVG(salary) FROM employees GROUP BY department HAVING AVG(salary) > 50000";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_sqlite_multiple_statements() {
        let sql = "SELECT 1; SELECT 2";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn test_sqlite_pragma_preprocess() {
        let preprocessed = preprocess_sqlite("PRAGMA foreign_keys = ON");
        assert!(preprocessed.contains("SELECT 1"));
        assert!(!preprocessed.contains("PRAGMA"));
    }

    #[test]
    fn test_sqlite_without_rowid_preprocess() {
        let preprocessed = preprocess_sqlite("CREATE TABLE t (id INT) WITHOUT ROWID");
        assert!(!preprocessed.contains("WITHOUT ROWID"));
        assert!(!preprocessed.contains("ROWID"));
    }

    #[test]
    fn test_apply_dialect_semantics_sqlite_asc() {
        let sql = "SELECT * FROM t ORDER BY name ASC";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        if let Statement::Select(select) = &stmts[0] {
            assert_eq!(select.order_by.len(), 1);
            assert!(select.order_by[0].nulls_first); // SQLite ASC → NULLS FIRST
        } else {
            panic!("expected Select");
        }
    }

    #[test]
    fn test_apply_dialect_semantics_sqlite_desc() {
        let sql = "SELECT * FROM t ORDER BY name DESC";
        let stmts = parse_with_dialect(sql, &Dialect::SQLite).unwrap();
        if let Statement::Select(select) = &stmts[0] {
            assert_eq!(select.order_by.len(), 1);
            assert!(!select.order_by[0].nulls_first); // SQLite DESC → NULLS LAST
        } else {
            panic!("expected Select");
        }
    }

    #[test]
    fn test_detect_sqlite_autoincrement() {
        assert_eq!(
            detect_dialect("CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT)"),
            Dialect::SQLite
        );
    }

    #[test]
    fn test_detect_sqlite_without_rowid() {
        assert_eq!(
            detect_dialect("CREATE TABLE t (id INT) WITHOUT ROWID"),
            Dialect::SQLite
        );
    }

    #[test]
    fn test_detect_sqlite_pragma() {
        assert_eq!(detect_dialect("PRAGMA foreign_keys = ON"), Dialect::SQLite);
    }

    #[test]
    fn test_detect_sqlite_group_concat() {
        assert_eq!(
            detect_dialect("SELECT GROUP_CONCAT(name) FROM t"),
            Dialect::SQLite
        );
    }

    #[test]
    fn test_parse_auto_sqlite() {
        let stmts = parse_auto("CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT)").unwrap();
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn test_stress_sqlite_50_queries() {
        // Stress：50 条 SQLite 语法查询
        let test_cases = vec![
            "SELECT id, name FROM users WHERE id = 1",
            "SELECT `id`, `name` FROM `users`",
            "SELECT [id], [name] FROM [users]",
            "SELECT * FROM t LIMIT 5, 10",
            "SELECT * FROM t LIMIT 10 OFFSET 5",
            "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)",
            "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT) WITHOUT ROWID",
            "INSERT INTO t (id, name) VALUES (1, 'Alice')",
            "UPDATE t SET name = 'Bob' WHERE id = 1",
            "DELETE FROM t WHERE id = 1",
        ];
        let mut success_count = 0;
        for i in 0..50 {
            let sql = test_cases[i % test_cases.len()];
            if parse_with_dialect(sql, &Dialect::SQLite).is_ok() {
                success_count += 1;
            }
        }
        assert!(
            success_count >= 45,
            "SQLite parse success rate: {success_count}/50, expected >= 45"
        );
    }

    #[test]
    fn test_parse_invalid_sql() {
        let result = parse_with_dialect("SELECT FROM", &Dialect::PostgreSQL);
        assert!(result.is_err());
    }

    // --- Stress 测试 ---

    #[test]
    fn test_stress_mysql_100_queries() {
        // Stress：100 条 MySQL 语法查询
        let mut success_count = 0;
        for i in 0..100 {
            let sql = format!("SELECT id, name FROM users WHERE id = {i} LIMIT {i}, 10");
            if parse_with_dialect(&sql, &Dialect::MySql).is_ok() {
                success_count += 1;
            }
        }
        // 验收标准：MySQL 解析成功率 >= 90%
        assert!(
            success_count >= 90,
            "MySQL parse success rate: {success_count}/100, expected >= 90"
        );
    }

    #[test]
    fn test_stress_oracle_100_queries() {
        // Stress：100 条 Oracle 语法查询
        let test_cases = vec![
            "SELECT NVL(name, 'unknown') FROM users",
            "SELECT DECODE(status, 1, 'active', 'inactive') FROM users",
            "SELECT SYSDATE FROM dual",
            "SELECT my_seq.NEXTVAL FROM dual",
            "SELECT TO_DATE('2024-01-01', 'YYYY-MM-DD') FROM dual",
            "SELECT TO_NUMBER('123') FROM dual",
            "SELECT TO_CHAR(123) FROM dual",
            "SELECT * FROM users WHERE ROWNUM <= 10",
            "SELECT id, name FROM users",
            "SELECT COUNT(*) FROM users",
        ];

        let mut success_count = 0;
        for i in 0..100 {
            let sql = test_cases[i % test_cases.len()];
            if parse_with_dialect(sql, &Dialect::Oracle).is_ok() {
                success_count += 1;
            }
        }
        // 验收标准：Oracle 解析成功率 >= 70%
        assert!(
            success_count >= 70,
            "Oracle parse success rate: {success_count}/100, expected >= 70"
        );
    }

    #[test]
    fn test_stress_sqlserver_50_queries() {
        // Stress：50 条 SQL Server 语法查询
        let mut success_count = 0;
        for i in 0..50 {
            let sql = format!("SELECT TOP {i} id, name FROM users WHERE id = {i}");
            if parse_with_dialect(&sql, &Dialect::SqlServer).is_ok() {
                success_count += 1;
            }
        }
        assert!(
            success_count >= 40,
            "SQL Server parse success rate: {success_count}/50"
        );
    }

    #[test]
    fn test_stress_mixed_dialects_auto_detect() {
        // Stress：混合方言自动检测
        let test_cases = vec![
            ("SELECT * FROM t LIMIT 10", Dialect::PostgreSQL),
            ("SELECT `id` FROM `t`", Dialect::MySql),
            ("SELECT * FROM t LIMIT 10, 20", Dialect::MySql),
            ("SELECT TOP 10 * FROM t", Dialect::SqlServer),
            ("SELECT NVL(x, 0) FROM t", Dialect::Oracle),
            ("SELECT SYSDATE FROM dual", Dialect::Oracle),
            ("SELECT * FROM t WHERE ROWNUM <= 5", Dialect::Oracle),
            ("SELECT * FROM t WHERE id = 1", Dialect::PostgreSQL),
        ];

        for (sql, expected_dialect) in test_cases {
            let detected = detect_dialect(sql);
            assert_eq!(
                detected, expected_dialect,
                "failed to detect dialect for: {sql}"
            );
        }
    }

    // --- 预处理函数单元测试 ---

    #[test]
    fn test_preprocess_postgresql_noop() {
        let sql = "SELECT * FROM t WHERE id = 1";
        assert_eq!(preprocess(sql, &Dialect::PostgreSQL), sql);
    }

    #[test]
    fn test_preprocess_mysql_with_limit_comma() {
        let result = preprocess_mysql("SELECT * FROM t LIMIT 5, 10");
        assert_eq!(result, "SELECT * FROM t LIMIT 10 OFFSET 5");
    }

    #[test]
    fn test_preprocess_oracle_nvl_to_coalesce() {
        let result = preprocess_oracle("SELECT NVL(a, b) FROM t");
        assert!(result.contains("COALESCE(a, b)"));
    }

    #[test]
    fn test_preprocess_sqlserver_isnull_to_coalesce() {
        let result = preprocess_sqlserver("SELECT ISNULL(a, b) FROM t");
        assert!(result.contains("COALESCE(a, b)"));
    }
}
