//! Oracle SQL 方言转换（PL/SQL → PG SQL）。
//!
//! 本模块实现 Oracle PL/SQL 方言到 PostgreSQL 兼容 SQL 的文本级转换。
//!
//! # 转换策略
//!
//! 采用 **文本级正则替换**，不依赖 AST 重写。优点是简单、快速、无外部依赖；
//! 缺点是无法处理嵌套场景（如字符串内含关键字、复杂表达式中的 DECODE）。
//! 对于复杂场景，建议手动改写。
//!
//! # 转换规则
//!
//! | Oracle 语法 | PG 兼容语法 |
//! |-------------|-------------|
//! | `FROM dual` | `FROM (SELECT 1) AS dual` |
//! | `SYSDATE` | `CURRENT_TIMESTAMP` |
//! | `NVL(a, b)` | `COALESCE(a, b)` |
//! | `NVL2(a, b, c)` | `CASE WHEN a IS NOT NULL THEN b ELSE c END` |
//! | `DECODE(...)` | `CASE WHEN ... THEN ... END` |
//! | `TO_CHAR(s)` | `to_char(s)` |
//! | `TO_DATE(s)` | `to_date(s)` |
//! | `TO_NUMBER(s)` | `CAST(s AS NUMERIC)` |
//! | `seq.NEXTVAL` | `nextval('seq')` |
//! | `seq.CURRVAL` | `currval('seq')` |
//! | `MINUS` | `EXCEPT` |
//! | `ROWNUM` (SELECT 列表) | `ROW_NUMBER() OVER ()` |
//! | `ROWNUM <= N` | `LIMIT N` |
//! | `TRUNC(date)` | `date_trunc('day', date)` |
//! | `ADD_MONTHS(d, n)` | `(d + make_interval(months => n))` |
//! | `INSTR(s, sub)` | `strpos(s, sub)` |
//! | `\|\|` 字符串连接 | `\|\|`（PG 兼容，无需转换） |
//! | `CREATE SEQUENCE` | 兼容（无需转换） |
//! | 双引号标识符 | 兼容（无需转换） |
//! | `EXTRACT(YEAR FROM date)` | 兼容（无需转换） |
//! | `SUBSTR(s, m, n)` | 兼容（无需转换） |
//!
//! # 用法
//!
//! ```ignore
//! use szrsql_oracle_bridge::sql_dialect::OracleDialect;
//!
//! let dialect = OracleDialect::new();
//! let pg_sql = dialect.convert_sql("SELECT NVL(name, 'N/A') FROM dual").unwrap();
//! assert!(pg_sql.contains("COALESCE(name, 'N/A')"));
//! ```

use regex::Regex;
use szrsql_sql::dialect::{parse_with_dialect, Dialect};

// =====================================================================
//  错误类型
// =====================================================================

/// Oracle SQL 方言转换错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OracleDialectError {
    /// 转换后的 SQL 解析失败（语法错误或剩余不支持的方言特性）。
    #[error("oracle dialect parse failed: {0}")]
    ParseFailed(String),
}

// =====================================================================
//  OracleDialect 结构体
// =====================================================================

/// Oracle SQL 方言转换器。
///
/// 通过文本级正则替换将 Oracle PL/SQL 方言转换为 PG 兼容 SQL。
/// 转换完成后调用 [`parse_with_dialect`] 验证语法合法性。
#[derive(Debug, Clone, Default)]
pub struct OracleDialect {
    // 当前为无状态转换器；预留字段供未来扩展
}

impl OracleDialect {
    /// 构造一个新的 Oracle 方言转换器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 转换 Oracle 方言 SQL 为 PG 兼容 SQL。
    ///
    /// # 处理流程
    /// 1. 应用文本级正则替换（覆盖 Oracle 特有语法）
    /// 2. 调用 [`parse_with_dialect`] 以 Oracle 方言解析验证
    /// 3. 解析成功则返回转换后的 SQL；失败则返回错误
    ///
    /// # 参数
    /// - `sql`：Oracle 方言 SQL 文本
    ///
    /// # 返回
    /// - `Ok(String)`：转换并验证成功的 PG 兼容 SQL
    /// - `Err(OracleDialectError::ParseFailed)`：转换后 SQL 解析失败
    pub fn convert_sql(&self, sql: &str) -> Result<String, OracleDialectError> {
        // 1. 文本级转换
        let transformed = self.transform(sql);

        // 2. 用 Oracle 方言解析验证（dialect 内部还会做自己的预处理）
        let _statements = parse_with_dialect(&transformed, &Dialect::Oracle)
            .map_err(|e| OracleDialectError::ParseFailed(format!("{e:?}")))?;

        // 3. 返回转换后的 SQL
        Ok(transformed)
    }

    /// 执行所有文本级转换规则。
    ///
    /// 该方法仅做文本替换，不做语法验证。转换顺序经过精心设计以避免冲突：
    /// 1. NVL2（必须在 NVL 之前，避免被 NVL 正则误匹配）
    /// 2. NVL
    /// 3. DECODE（参数内含逗号，单独处理）
    /// 4. ROWNUM <= N → LIMIT N（必须在通用 ROWNUM 替换之前）
    /// 5. 通用 ROWNUM → ROW_NUMBER() OVER ()
    /// 6. SYSDATE → CURRENT_TIMESTAMP
    /// 7. seq.NEXTVAL / seq.CURRVAL
    /// 8. TO_CHAR / TO_DATE / TO_NUMBER
    /// 9. TRUNC(date) → date_trunc('day', date)
    /// 10. ADD_MONTHS(d, n) → make_interval
    /// 11. INSTR(s, sub) → strpos(s, sub)
    /// 12. MINUS → EXCEPT
    /// 13. FROM dual → FROM (SELECT 1) AS dual
    fn transform(&self, sql: &str) -> String {
        let mut result = sql.to_string();

        // 1. NVL2(a, b, c) → CASE WHEN a IS NOT NULL THEN b ELSE c END
        // 必须在 NVL 之前处理，否则 NVL 正则会把 NVL2( 替换为 COALESCE(2(，
        // 这是因为 \bNVL\s*\( 会匹配 NVL2( 中的 "NVL(" 部分（\b 在 NVL 后是数字 2，不是词边界）
        // 实际上 \bNVL\s*\( 不会匹配 NVL2(，因为 2 是单词字符，N 与 2 之间无词边界
        // 但为安全起见仍先处理 NVL2
        result = convert_nvl2(&result);

        // 2. NVL(a, b) → COALESCE(a, b)
        result = convert_nvl(&result);

        // 3. DECODE(...) → CASE WHEN ... THEN ... END
        result = convert_decode_calls(&result);

        // 4. ROWNUM <= N → TRUE，并追加 LIMIT N
        result = convert_rownum_limit(&result);

        // 5. 通用 ROWNUM（SELECT 列表中）→ ROW_NUMBER() OVER ()
        result = convert_rownum_general(&result);

        // 6. SYSDATE → CURRENT_TIMESTAMP
        result = convert_sysdate(&result);

        // 7. seq.NEXTVAL → nextval('seq') / seq.CURRVAL → currval('seq')
        result = convert_sequence_nextval(&result);
        result = convert_sequence_currval(&result);

        // 8. TO_CHAR / TO_DATE / TO_NUMBER 转换
        result = convert_to_char(&result);
        result = convert_to_date(&result);
        result = convert_to_number(&result);

        // 9. TRUNC(date) → date_trunc('day', date)
        result = convert_trunc_date(&result);

        // 10. ADD_MONTHS(d, n) → (d + make_interval(months => n))
        result = convert_add_months(&result);

        // 11. INSTR(s, sub) → strpos(s, sub)
        result = convert_instr(&result);

        // 12. MINUS → EXCEPT
        result = convert_minus(&result);

        // 13. FROM dual → FROM (SELECT 1) AS dual
        result = convert_from_dual(&result);

        result
    }
}

// =====================================================================
//  各转换规则的实现
// =====================================================================

/// NVL2(a, b, c) → CASE WHEN a IS NOT NULL THEN b ELSE c END
///
/// 使用正则匹配 3 个参数（不处理参数内嵌套逗号）。
fn convert_nvl2(sql: &str) -> String {
    let re = Regex::new(r"(?i)\bNVL2\s*\(([^,]+),\s*([^,]+),\s*([^)]+)\)").unwrap();
    re.replace_all(sql, "CASE WHEN $1 IS NOT NULL THEN $2 ELSE $3 END")
        .to_string()
}

/// NVL(a, b) → COALESCE(a, b)
fn convert_nvl(sql: &str) -> String {
    let re = Regex::new(r"(?i)\bNVL\s*\(").unwrap();
    re.replace_all(sql, "COALESCE(").to_string()
}

/// DECODE(...) → CASE WHEN ... THEN ... END
///
/// 算法：
/// 1. 查找 DECODE( 调用位置
/// 2. 提取括号内参数（处理嵌套括号）
/// 3. 按逗号分割参数（处理嵌套括号与字符串）
/// 4. 构建 CASE 表达式：args[0]=expr, args[1,2]=val,ret 对, args[奇数]=default
fn convert_decode_calls(sql: &str) -> String {
    let decode_re = Regex::new(r"(?i)\bDECODE\s*\(").unwrap();
    if !decode_re.is_match(sql) {
        return sql.to_string();
    }

    let mut result = sql.to_string();
    while let Some(pos) = find_decode_call_position(&result) {
        let (call_str, args) = extract_parenthesized(&result, pos);
        if args.is_empty() {
            break;
        }
        let case_expr = build_case_from_decode(&args);
        result = result.replacen(&call_str, &case_expr, 1);
    }
    result
}

/// 查找 SQL 中第一个 DECODE( 调用的左括号位置。
fn find_decode_call_position(sql: &str) -> Option<usize> {
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

/// 从左括号位置提取完整括号内容（处理嵌套）。
///
/// 返回 (完整调用字符串 "DECODE(...)", 参数列表)
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

/// 按逗号分割参数列表（处理嵌套括号与字符串字面量）。
pub(crate) fn split_args(s: &str) -> Vec<String> {
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

/// 根据 DECODE 参数构建 CASE 表达式。
///
/// 参数格式：`[expr, val1, ret1, val2, ret2, ..., (default)]`
fn build_case_from_decode(args: &[String]) -> String {
    if args.is_empty() {
        return String::new();
    }
    let expr = &args[0];
    let mut case = "CASE".to_string();

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

    // 剩余参数为 default（ELSE）
    if i < args.len() {
        case.push_str(&format!(" ELSE {}", args[i]));
    }

    case.push_str(" END");
    case
}

/// `WHERE ROWNUM <= N` 或 `WHERE ROWNUM < N+1` → 移除条件，追加 `LIMIT N`
///
/// 必须在通用 ROWNUM 替换之前处理，否则会破坏 WHERE 子句。
fn convert_rownum_limit(sql: &str) -> String {
    let mut result = sql.to_string();

    // ROWNUM <= N → TRUE，追加 LIMIT N
    let rownum_le_re = Regex::new(r"(?i)\bROWNUM\s*<=\s*(\d+)").unwrap();
    if let Some(caps) = rownum_le_re.captures(&result) {
        let n = caps.get(1).unwrap().as_str().to_string();
        let replaced = rownum_le_re.replace_all(&result, "TRUE").to_string();
        if !Regex::new(r"(?i)\bLIMIT\b").unwrap().is_match(&replaced) {
            result = format!("{replaced} LIMIT {n}");
        } else {
            result = replaced;
        }
    }

    // ROWNUM < N → TRUE，追加 LIMIT N-1
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

    result
}

/// 通用 ROWNUM（SELECT 列表中）→ ROW_NUMBER() OVER ()
///
/// 处理 `SELECT ROWNUM, ... FROM t` 这类用法。
fn convert_rownum_general(sql: &str) -> String {
    let re = Regex::new(r"(?i)\bROWNUM\b").unwrap();
    re.replace_all(sql, "ROW_NUMBER() OVER ()")
        .to_string()
}

/// SYSDATE → CURRENT_TIMESTAMP
fn convert_sysdate(sql: &str) -> String {
    let re = Regex::new(r"(?i)\bSYSDATE\b").unwrap();
    re.replace_all(sql, "CURRENT_TIMESTAMP").to_string()
}

/// seq.NEXTVAL → nextval('seq')
fn convert_sequence_nextval(sql: &str) -> String {
    let re = Regex::new(r"(?i)\b(\w+)\.NEXTVAL\b").unwrap();
    re.replace_all(sql, "nextval('$1')").to_string()
}

/// seq.CURRVAL → currval('seq')
fn convert_sequence_currval(sql: &str) -> String {
    let re = Regex::new(r"(?i)\b(\w+)\.CURRVAL\b").unwrap();
    re.replace_all(sql, "currval('$1')").to_string()
}

/// TO_CHAR(s) → to_char(s)
///
/// 仅处理单参数形式 TO_CHAR(s)；多参数形式（带格式化字符串）保留原样，
/// 由 parse_with_dialect 进一步处理。
fn convert_to_char(sql: &str) -> String {
    // 单参数：TO_CHAR(s)
    let single_re = Regex::new(r"(?i)\bTO_CHAR\s*\(([^,)]+)\)").unwrap();
    let result = single_re.replace_all(sql, "to_char($1)").to_string();

    // 多参数：TO_CHAR(s, fmt) → to_char(s, fmt)（仅替换函数名）
    let multi_re = Regex::new(r"(?i)\bTO_CHAR\s*\(").unwrap();
    multi_re.replace_all(&result, "to_char(").to_string()
}

/// TO_DATE(s[, fmt]) → to_date(s[, fmt])
fn convert_to_date(sql: &str) -> String {
    let re = Regex::new(r"(?i)\bTO_DATE\s*\(").unwrap();
    re.replace_all(sql, "to_date(").to_string()
}

/// TO_NUMBER(s) → CAST(s AS NUMERIC)
fn convert_to_number(sql: &str) -> String {
    let re = Regex::new(r"(?i)\bTO_NUMBER\s*\(([^)]+)\)").unwrap();
    re.replace_all(sql, "CAST($1 AS NUMERIC)").to_string()
}

/// TRUNC(date) → date_trunc('day', date)
///
/// 仅处理单参数形式（日期截断到天）。
/// 双参数形式 TRUNC(date, fmt) 不处理（保留原样）。
fn convert_trunc_date(sql: &str) -> String {
    let re = Regex::new(r"(?i)\bTRUNC\s*\(([^,)]+)\)").unwrap();
    re.replace_all(sql, "date_trunc('day', $1)").to_string()
}

/// ADD_MONTHS(d, n) → (d + make_interval(months => n))
///
/// 使用 PG 的 make_interval 函数生成月数间隔。
fn convert_add_months(sql: &str) -> String {
    let re = Regex::new(r"(?i)\bADD_MONTHS\s*\(([^,]+),\s*([^)]+)\)").unwrap();
    re.replace_all(sql, "($1 + make_interval(months => $2))")
        .to_string()
}

/// INSTR(s, sub) → strpos(s, sub)
///
/// 仅处理两参数形式（PG strpos 不支持三参数的 INSTR(s, sub, start, nth)）。
fn convert_instr(sql: &str) -> String {
    let re = Regex::new(r"(?i)\bINSTR\s*\(([^,]+),\s*([^,)]+)\)").unwrap();
    re.replace_all(sql, "strpos($1, $2)").to_string()
}

/// MINUS → EXCEPT（Oracle MINUS 等价于 SQL 标准 EXCEPT）
fn convert_minus(sql: &str) -> String {
    let re = Regex::new(r"(?i)\bMINUS\b").unwrap();
    re.replace_all(sql, "EXCEPT").to_string()
}

/// `FROM dual` → `FROM (SELECT 1) AS dual`
///
/// Oracle 的 dual 表是单行虚拟表，PG 无此表。
/// 使用派生表 `(SELECT 1) AS dual` 模拟，保留所有后续子句
/// （WHERE / GROUP BY / HAVING / ORDER BY）的兼容性。
fn convert_from_dual(sql: &str) -> String {
    let re = Regex::new(r"(?i)\bFROM\s+dual\b").unwrap();
    re.replace_all(sql, "FROM (SELECT 1) AS dual")
        .to_string()
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  构造测试
    // -----------------------------------------------------------------

    #[test]
    fn new_returns_default_dialect() {
        let dialect = OracleDialect::new();
        // 无状态转换器，仅验证可构造
        let _ = format!("{dialect:?}");
    }

    #[test]
    fn default_equals_new() {
        let from_new = OracleDialect::new();
        let from_default = OracleDialect::default();
        assert_eq!(format!("{from_new:?}"), format!("{from_default:?}"));
    }

    // -----------------------------------------------------------------
    //  convert_sql 端到端测试
    // -----------------------------------------------------------------

    #[test]
    fn convert_sql_sysdate_to_current_timestamp() {
        let dialect = OracleDialect::new();
        let result = dialect.convert_sql("SELECT SYSDATE FROM dual").unwrap();
        assert!(result.contains("CURRENT_TIMESTAMP"));
        assert!(!result.contains("SYSDATE"));
    }

    #[test]
    fn convert_sql_nvl_to_coalesce() {
        let dialect = OracleDialect::new();
        let result = dialect
            .convert_sql("SELECT NVL(name, 'unknown') FROM users")
            .unwrap();
        assert!(result.contains("COALESCE(name, 'unknown')"));
        assert!(!result.contains("NVL("));
    }

    #[test]
    fn convert_sql_decode_to_case() {
        let dialect = OracleDialect::new();
        let sql = "SELECT DECODE(status, 1, 'active', 2, 'inactive', 'unknown') FROM users";
        let result = dialect.convert_sql(sql).unwrap();
        assert!(result.contains("CASE"));
        assert!(result.contains("WHEN status = 1 THEN 'active'"));
        assert!(result.contains("WHEN status = 2 THEN 'inactive'"));
        assert!(result.contains("ELSE 'unknown'"));
        assert!(result.contains("END"));
    }

    #[test]
    fn convert_sql_from_dual_replaced() {
        let dialect = OracleDialect::new();
        let result = dialect.convert_sql("SELECT 1 FROM dual").unwrap();
        assert!(result.contains("FROM (SELECT 1) AS dual"));
        // 原 "FROM dual" 不应再存在（除非出现在字符串字面量中）
        assert!(!result.contains("FROM dual"));
    }

    #[test]
    fn convert_sql_sequence_nextval() {
        let dialect = OracleDialect::new();
        let result = dialect
            .convert_sql("SELECT my_seq.NEXTVAL FROM dual")
            .unwrap();
        assert!(result.contains("nextval('my_seq')"));
        assert!(!result.contains(".NEXTVAL"));
    }

    #[test]
    fn convert_sql_sequence_currval() {
        let dialect = OracleDialect::new();
        let result = dialect
            .convert_sql("SELECT my_seq.CURRVAL FROM dual")
            .unwrap();
        assert!(result.contains("currval('my_seq')"));
        assert!(!result.contains(".CURRVAL"));
    }

    #[test]
    fn convert_sql_minus_to_except() {
        let dialect = OracleDialect::new();
        let result = dialect
            .convert_sql("SELECT id FROM a MINUS SELECT id FROM b")
            .unwrap();
        assert!(result.contains("EXCEPT"));
        assert!(!result.contains("MINUS"));
    }

    #[test]
    fn convert_sql_rownum_le_to_limit() {
        let dialect = OracleDialect::new();
        let result = dialect
            .convert_sql("SELECT * FROM users WHERE ROWNUM <= 10")
            .unwrap();
        assert!(result.contains("LIMIT 10"));
    }

    #[test]
    fn convert_sql_to_char_renamed() {
        let dialect = OracleDialect::new();
        let result = dialect.convert_sql("SELECT TO_CHAR(123) FROM dual").unwrap();
        assert!(result.contains("to_char(123)"));
    }

    #[test]
    fn convert_sql_to_date_renamed() {
        let dialect = OracleDialect::new();
        let result = dialect
            .convert_sql("SELECT TO_DATE('2024-01-01', 'YYYY-MM-DD') FROM dual")
            .unwrap();
        assert!(result.contains("to_date("));
    }

    #[test]
    fn convert_sql_to_number_to_cast() {
        let dialect = OracleDialect::new();
        let result = dialect.convert_sql("SELECT TO_NUMBER('123') FROM dual").unwrap();
        assert!(result.contains("CAST('123' AS NUMERIC)"));
    }

    #[test]
    fn convert_sql_instr_to_strpos() {
        let dialect = OracleDialect::new();
        let result = dialect
            .convert_sql("SELECT INSTR('hello world', 'world') FROM dual")
            .unwrap();
        assert!(result.contains("strpos('hello world', 'world')"));
    }

    #[test]
    fn convert_sql_trunc_date_to_date_trunc() {
        let dialect = OracleDialect::new();
        let result = dialect
            .convert_sql("SELECT TRUNC(created_at) FROM events")
            .unwrap();
        assert!(result.contains("date_trunc('day', created_at)"));
    }

    #[test]
    fn convert_sql_add_months_to_make_interval() {
        let dialect = OracleDialect::new();
        let result = dialect
            .convert_sql("SELECT ADD_MONTHS(hire_date, 6) FROM employees")
            .unwrap();
        assert!(result.contains("make_interval(months => 6)"));
    }

    #[test]
    fn convert_sql_nvl2_to_case() {
        let dialect = OracleDialect::new();
        let result = dialect
            .convert_sql("SELECT NVL2(name, 'yes', 'no') FROM users")
            .unwrap();
        assert!(result.contains("CASE WHEN name IS NOT NULL THEN 'yes' ELSE 'no' END"));
    }

    // -----------------------------------------------------------------
    //  错误路径测试
    // -----------------------------------------------------------------

    #[test]
    fn convert_sql_invalid_syntax_returns_error() {
        let dialect = OracleDialect::new();
        // 语法错误的 SQL 应返回 ParseFailed 错误
        let result = dialect.convert_sql("SELECT FROM WHERE");
        assert!(matches!(result, Err(OracleDialectError::ParseFailed(_))));
    }

    // -----------------------------------------------------------------
    //  私有转换函数的单元测试
    // -----------------------------------------------------------------

    #[test]
    fn convert_nvl_replaces_nvl_only() {
        let result = convert_nvl("SELECT NVL(a, b) FROM t");
        assert_eq!(result, "SELECT COALESCE(a, b) FROM t");
    }

    #[test]
    fn convert_nvl_does_not_touch_nvl2() {
        // NVL( 不会匹配 NVL2(，因为 2 是单词字符
        let result = convert_nvl("SELECT NVL2(a, b, c) FROM t");
        assert_eq!(result, "SELECT NVL2(a, b, c) FROM t");
    }

    #[test]
    fn convert_decode_with_default() {
        let result = convert_decode_calls("SELECT DECODE(x, 1, 'one', 'other') FROM t");
        assert!(result.contains("WHEN x = 1 THEN 'one'"));
        assert!(result.contains("ELSE 'other'"));
    }

    #[test]
    fn convert_decode_no_default() {
        let result = convert_decode_calls("SELECT DECODE(x, 1, 'one') FROM t");
        assert!(result.contains("WHEN x = 1 THEN 'one'"));
        assert!(!result.contains("ELSE"));
    }

    #[test]
    fn convert_from_dual_case_insensitive() {
        // 替换文本中的 FROM 始终大写（替换字符串固定），与原 SQL 中 FROM 大小写无关
        assert_eq!(
            convert_from_dual("SELECT 1 FROM DUAL"),
            "SELECT 1 FROM (SELECT 1) AS dual"
        );
        assert_eq!(
            convert_from_dual("select 1 from Dual"),
            "select 1 FROM (SELECT 1) AS dual"
        );
    }

    #[test]
    fn convert_minus_does_not_match_substring() {
        // "MINUS" 作为独立单词才替换，"MINUSTAKE" 不应被替换
        let result = convert_minus("SELECT MINUS_ID FROM t MINUS SELECT id FROM s");
        // MINUS_ID 保留，独立的 MINUS 被替换
        assert!(result.contains("MINUS_ID"));
        assert!(result.contains("EXCEPT"));
    }

    #[test]
    fn split_args_handles_nested_parens() {
        let args = split_args("a, COALESCE(b, c), d");
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "a");
        assert_eq!(args[1], "COALESCE(b, c)");
        assert_eq!(args[2], "d");
    }

    #[test]
    fn split_args_handles_string_literals() {
        let args = split_args("'a, b', c, 'd'");
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "'a, b'");
        assert_eq!(args[1], "c");
        assert_eq!(args[2], "'d'");
    }

    #[test]
    fn convert_rownum_limit_handles_le() {
        let result = convert_rownum_limit("SELECT * FROM t WHERE ROWNUM <= 5");
        assert!(result.contains("LIMIT 5"));
        assert!(!result.contains("ROWNUM"));
    }

    #[test]
    fn convert_rownum_limit_handles_lt() {
        let result = convert_rownum_limit("SELECT * FROM t WHERE ROWNUM < 5");
        // ROWNUM < 5 → LIMIT 4
        assert!(result.contains("LIMIT 4"));
    }

    #[test]
    fn convert_rownum_general_for_select_list() {
        let result = convert_rownum_general("SELECT ROWNUM, name FROM t");
        assert!(result.contains("ROW_NUMBER() OVER ()"));
    }

    #[test]
    fn convert_to_char_single_and_multi_arg() {
        // 单参数 → to_char(s)
        assert_eq!(convert_to_char("TO_CHAR(123)"), "to_char(123)");
        // 多参数 → 函数名替换为 to_char
        let result = convert_to_char("TO_CHAR(123, '999')");
        assert!(result.contains("to_char("));
    }
}
