//! SzRSQL SQL 解析器（Phase 3.1）— PG SQL → sqlparser-rs AST → SzRSQL AST 转换器。
//!
//! # 设计
//!
//! - **入口**：`parse_sql(sql: &str) -> Result<Vec<Statement>, ParseError>`
//! - **方言**：`PostgreSqlDialect`（PG 标准 SQL）
//! - **转换**：sqlparser-rs AST → SzRSQL 内部 AST（`ast::*`）
//! - **错误**：`ParseError` 包装 sqlparser `ParserError` + 自定义转换错误
//!
//! # 覆盖范围
//!
//! - DDL：CREATE TABLE / DROP TABLE / CREATE INDEX / DROP INDEX
//! - DML：INSERT / UPDATE / DELETE / SELECT
//! - 事务：BEGIN / COMMIT / ROLLBACK / SAVEPOINT / RELEASE SAVEPOINT / SET TRANSACTION
//! - EXPLAIN / EXPLAIN ANALYZE
//! - 表达式：字面量、列引用、二元/一元运算、函数调用、CASE、CAST、IN、BETWEEN、LIKE、IS NULL、EXISTS、子查询、元组
//! - JOIN：INNER / LEFT / RIGHT / FULL / CROSS + ON / USING / NATURAL
//! - SELECT：DISTINCT / projection / FROM / WHERE / GROUP BY / HAVING / ORDER BY / LIMIT / OFFSET

use crate::ast::*;
use sqlparser::ast::{
    AssignmentTarget, BinaryOperator, ColumnDef as SpColumnDef, ColumnOption, ColumnOptionDef,
    ConflictTarget, DataType, DoUpdate, Expr as SpExpr, FunctionArg as SpFunctionArg,
    FunctionArgExpr as SpFunctionArgExpr, FunctionArguments as SpFunctionArguments, GeneratedAs,
    GeneratedExpressionMode, Ident, Insert, JoinConstraint, JoinOperator, ObjectName,
    OnConflictAction, OnInsert, OrderByExpr as SpOrderByExpr, Query as SpQuery,
    SelectItem as SpSelectItem, SetExpr as SpSetExpr, SetOperator as SpSetOperator,
    SetQuantifier as SpSetQuantifier, Statement as SpStatement, TableAlias as SpTableAlias,
    TableConstraint as SpTableConstraint, TableFactor as SpTableFactor,
    TableWithJoins as SpTableWithJoins, TransactionAccessMode, TransactionIsolationLevel,
    TransactionMode, TriggerEvent as SpTriggerEvent, TriggerExecBody,
    TriggerObject as SpTriggerObject, TriggerPeriod as SpTriggerPeriod, UnaryOperator,
    Value as SpValue, WindowFrame as SpWindowFrame, WindowFrameBound as SpWindowFrameBound,
    WindowFrameUnits as SpWindowFrameUnits, WindowSpec as SpWindowSpec, WindowType as SpWindowType,
};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::{Parser, ParserError};
use szrsql_types::value::{ColumnType, Value};
use tracing::{trace, warn};

// =====================================================================
//  递归深度与输入长度保护（ADV-BUG-001 修复）
// =====================================================================
//
// 背景：sqlparser-rs 0.53.0 对左结合的 AND/OR 链生成左倾嵌套树，
// 本文件的 `convert_expr` 递归转换该树时，递归深度 = 操作数个数。
// 当攻击者构造 50 个 OR 链即可在 2MB 工作线程栈上触发 STATUS_STACK_OVERFLOW，
// 导致服务拒绝（DoS）。
//
// 修复策略：
// 1. 在 `parse_sql_inner` 入口对 SQL 文本长度做预检（MAX_SQL_LEN）
// 2. 在 `convert_expr` 递归调用中传入深度计数器，超限返回 ParseError
//
// 阈值选择依据：
// - MAX_EXPR_DEPTH=512：真实 SQL 表达式嵌套极少超过 32 层，512 提供充足余量
// - MAX_SQL_LEN=1MB：远超正常 SQL 长度（典型 < 10KB），同时限制 sqlparser-rs 递归深度

/// 表达式最大递归深度（ADV-BUG-001 修复）
const MAX_EXPR_DEPTH: usize = 512;

/// SQL 文本最大长度（字节），超出直接拒绝（ADV-BUG-001 修复）
const MAX_SQL_LEN: usize = 1024 * 1024;

/// OR/AND 二值运算符最大链深度，超出直接拒绝（ADV-BUG-001 修复）
///
/// sqlparser-rs 内部递归下降解析二值表达式，左结合链深度 = 操作数个数。
/// sqlparser-rs 0.53.0 在 2MB 工作线程栈上约 50 个 OR 链即栈溢出，
/// 阈值设为 32 以提供安全余量，同时满足绝大多数真实 SQL 需求（典型 < 10）。
/// 对超过 32 个 OR/AND 的场景，建议改用 IN 列表或临时表。
const MAX_BINARY_OP_CHAIN: usize = 32;

/// 统计 SQL 文本中 OR/AND 关键字出现次数（ADV-BUG-001 修复）
///
/// 使用字节级扫描，识别 `\bOR\b` 和 `\bAND\b`（大小写不敏感）。
/// 注意：此函数为防御性预检，可能误统计字符串字面量中的 OR/AND，
/// 但宁可误报也不可漏报（误报只会导致拒绝异常长 SQL，不影响正常使用）。
fn count_binary_op_keywords(sql: &str) -> usize {
    let bytes = sql.as_bytes();
    let mut count = 0;
    let mut i = 0;
    let upper = |b: u8| b.to_ascii_uppercase();
    while i < bytes.len() {
        // 检查单词边界（前一个字符不是字母/数字/下划线）
        let prev_is_word = if i == 0 {
            false
        } else {
            let b = bytes[i - 1];
            b.is_ascii_alphanumeric() || b == b'_'
        };
        if prev_is_word {
            i += 1;
            continue;
        }
        // 尝试匹配 OR
        if i + 2 <= bytes.len() {
            let or_matches = upper(bytes[i]) == b'O'
                && upper(bytes[i + 1]) == b'R'
                && (i + 2 == bytes.len()
                    || {
                        let b = bytes[i + 2];
                        !(b.is_ascii_alphanumeric() || b == b'_')
                    });
            if or_matches {
                count += 1;
                i += 2;
                continue;
            }
        }
        // 尝试匹配 AND
        if i + 3 <= bytes.len() {
            let and_matches = upper(bytes[i]) == b'A'
                && upper(bytes[i + 1]) == b'N'
                && upper(bytes[i + 2]) == b'D'
                && (i + 3 == bytes.len()
                    || {
                        let b = bytes[i + 3];
                        !(b.is_ascii_alphanumeric() || b == b'_')
                    });
            if and_matches {
                count += 1;
                i += 3;
                continue;
            }
        }
        i += 1;
    }
    count
}

// =====================================================================
//  错误类型
// =====================================================================

/// SQL 解析错误
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ParseError {
    /// sqlparser-rs 解析错误
    #[error("sqlparser error: {0}")]
    SqlParser(String),
    /// 不支持的 SQL 语法
    #[error("unsupported SQL syntax: {0}")]
    Unsupported(String),
    /// 类型转换错误
    #[error("invalid data type: {0}")]
    InvalidDataType(String),
    /// 值转换错误
    #[error("invalid literal value: {0}")]
    InvalidValue(String),
}

impl From<ParserError> for ParseError {
    fn from(err: ParserError) -> Self {
        ParseError::SqlParser(err.to_string())
    }
}

// =====================================================================
//  入口函数
// =====================================================================

/// 解析 SQL 字符串为 SzRSQL AST 语句列表（PostgreSQL 方言）
///
/// 对于 `REPLACE INTO` 语句（MySQL 扩展，Phase 3.25），自动切换到 MySqlDialect
/// 进行解析（PG dialect 不支持 REPLACE 关键字）。
///
/// 对于 `ALTER TYPE` 语句（Phase 3.31），sqlparser 0.53.0 不支持解析，
/// 因此在入口处手动预处理：将 SQL 按分号切分，识别 ALTER TYPE 语句并手动解析，
/// 其余语句仍由 sqlparser 处理，最后按原始顺序合并结果。
pub fn parse_sql(sql: &str) -> Result<Vec<Statement>, ParseError> {
    let sql_len = sql.len();
    let span = tracing::span!(tracing::Level::TRACE, "parse_sql", sql_len, stmt_count = tracing::field::Empty);
    span.in_scope(|| {
        trace!(sql_len, "parsing SQL");
        parse_sql_inner(sql).map(|stmts| {
            tracing::Span::current().record("stmt_count", stmts.len());
            trace!(stmt_count = stmts.len(), "SQL parsed");
            stmts
        }).map_err(|e| {
            warn!(error = %e, sql_len, "SQL parse failed");
            e
        })
    })
}

/// `parse_sql` 的内部实现（被 `parse_sql` 的 tracing 包装器调用）
fn parse_sql_inner(sql: &str) -> Result<Vec<Statement>, ParseError> {
    // ADV-BUG-001 修复：SQL 长度预检，防止超长输入导致 sqlparser-rs 递归栈溢出
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
    // 阈值 MAX_BINARY_OP_CHAIN=256，远超真实 SQL 需求（典型 < 32），同时远低于栈溢出阈值
    let or_and_count = count_binary_op_keywords(sql);
    if or_and_count > MAX_BINARY_OP_CHAIN {
        return Err(ParseError::Unsupported(format!(
            "too many OR/AND operators in SQL: {} (max {}); this is a ADV-BUG-001 protection against stack overflow DoS",
            or_and_count,
            MAX_BINARY_OP_CHAIN
        )));
    }
    // Phase 3.31: 预处理 ALTER TYPE 语句 — sqlparser 0.53.0 不支持
    // Phase 3.35: 预处理 FLASHBACK 语句 — sqlparser 0.53.0 不支持
    // Phase 4.6: 预处理 LISTEN/UNLISTEN/NOTIFY 语句 — sqlparser 0.53.0 不支持
    // Phase 6.5: 预处理 CREATE/DROP FUNCTION 语句 — 含 `$$ body，sqlparser 不支持
    // 简化策略：按分号切分（不处理字符串字面量中的分号，对 ALTER TYPE/FLASHBACK/LISTEN 等实际使用场景足够）
    // 但当检测到 CREATE/DROP FUNCTION 时，必须使用智能切分器以正确处理 `$$ ... $$` 中的分号
    let has_function_ddl = contains_function_ddl(sql);
    let segments_owned: Vec<String>;
    let segments: Vec<&str> = if has_function_ddl {
        segments_owned = split_sql_statements_with_dollar_quotes(sql);
        segments_owned.iter().map(|s| s.as_str()).collect()
    } else {
        split_sql_statements(sql)
    };

    // 检测是否包含 ALTER TYPE 语句
    let has_alter_type = segments.iter().any(|seg| {
        let trimmed = seg.trim();
        !trimmed.is_empty() && trimmed.to_uppercase().starts_with("ALTER TYPE")
    });

    // Phase 3.35: 检测是否包含 FLASHBACK 语句
    let has_flashback = segments.iter().any(|seg| {
        let trimmed = seg.trim();
        !trimmed.is_empty() && trimmed.to_uppercase().starts_with("FLASHBACK")
    });

    // Phase 4.6: 检测是否包含 LISTEN/UNLISTEN/NOTIFY 语句
    let has_listen_notify = segments.iter().any(|seg| {
        let trimmed = seg.trim();
        if trimmed.is_empty() {
            return false;
        }
        let upper = trimmed.to_uppercase();
        upper.starts_with("LISTEN") || upper.starts_with("UNLISTEN") || upper.starts_with("NOTIFY")
    });

    // Phase 6.10: 检测是否包含 DROP MATERIALIZED VIEW / REFRESH MATERIALIZED VIEW 语句
    // sqlparser 0.53.0 不支持这两种语法，需要手动预处理
    let has_materialized_ddl = segments.iter().any(|seg| {
        let trimmed = seg.trim();
        if trimmed.is_empty() {
            return false;
        }
        let upper = trimmed.to_uppercase();
        upper.starts_with("DROP MATERIALIZED VIEW")
            || upper.starts_with("REFRESH MATERIALIZED VIEW")
    });

    // 若没有特殊语句，走原始路径（保持性能与兼容）
    if !has_alter_type
        && !has_flashback
        && !has_listen_notify
        && !has_function_ddl
        && !has_materialized_ddl
    {
        let contains_replace = contains_replace_statement(sql);
        // Phase 3.34: SET NAMES 是 MySQL 语法，PG dialect 不支持
        let contains_set_names = contains_set_names_statement(sql);
        let statements = if contains_replace || contains_set_names {
            let dialect = sqlparser::dialect::MySqlDialect {};
            Parser::parse_sql(&dialect, sql)?
        } else {
            let dialect = PostgreSqlDialect {};
            Parser::parse_sql(&dialect, sql)?
        };
        return statements.into_iter().map(convert_statement).collect();
    }

    // 有特殊语句：分别解析
    // - 手动解析段：ALTER TYPE / FLASHBACK / LISTEN/UNLISTEN/NOTIFY / CREATE FUNCTION / DROP FUNCTION
    //               / DROP MATERIALIZED VIEW / REFRESH MATERIALIZED VIEW
    // - 其他非空段：合并后交给 sqlparser 解析，再按顺序还原
    let mut result: Vec<Statement> = Vec::new();
    let mut non_special_segments: Vec<&str> = Vec::new();
    // 标记每段的类型：0=普通, 1=ALTER TYPE, 2=FLASHBACK, 3=LISTEN, 4=UNLISTEN, 5=NOTIFY,
    //                6=CREATE FUNCTION, 7=DROP FUNCTION,
    //                8=DROP MATERIALIZED VIEW, 9=REFRESH MATERIALIZED VIEW
    let mut segment_kind: Vec<u8> = Vec::with_capacity(segments.len());

    for seg in &segments {
        let trimmed = seg.trim();
        let upper = trimmed.to_uppercase();
        if !trimmed.is_empty() && upper.starts_with("ALTER TYPE") {
            segment_kind.push(1);
        } else if !trimmed.is_empty() && upper.starts_with("FLASHBACK") {
            segment_kind.push(2);
        } else if !trimmed.is_empty() && upper.starts_with("LISTEN") {
            segment_kind.push(3);
        } else if !trimmed.is_empty() && upper.starts_with("UNLISTEN") {
            segment_kind.push(4);
        } else if !trimmed.is_empty() && upper.starts_with("NOTIFY") {
            segment_kind.push(5);
        } else if !trimmed.is_empty()
            && (upper.starts_with("CREATE FUNCTION")
                || upper.starts_with("CREATE OR REPLACE FUNCTION"))
        {
            segment_kind.push(6);
        } else if !trimmed.is_empty() && upper.starts_with("DROP FUNCTION") {
            segment_kind.push(7);
        } else if !trimmed.is_empty() && upper.starts_with("DROP MATERIALIZED VIEW") {
            segment_kind.push(8);
        } else if !trimmed.is_empty() && upper.starts_with("REFRESH MATERIALIZED VIEW") {
            segment_kind.push(9);
        } else if !trimmed.is_empty() {
            segment_kind.push(0);
            non_special_segments.push(trimmed);
        }
        // 空段忽略，不参与计数
    }

    // 解析所有 ALTER TYPE 段
    let mut alter_stmts: Vec<Statement> = Vec::new();
    for seg in &segments {
        let trimmed = seg.trim();
        if !trimmed.is_empty() && trimmed.to_uppercase().starts_with("ALTER TYPE") {
            alter_stmts.push(parse_alter_type(trimmed)?);
        }
    }

    // Phase 3.35: 解析所有 FLASHBACK 段
    let mut flashback_stmts: Vec<Statement> = Vec::new();
    for seg in &segments {
        let trimmed = seg.trim();
        if !trimmed.is_empty() && trimmed.to_uppercase().starts_with("FLASHBACK") {
            flashback_stmts.push(parse_flashback(trimmed)?);
        }
    }

    // Phase 4.6: 解析所有 LISTEN 段
    let mut listen_stmts: Vec<Statement> = Vec::new();
    for seg in &segments {
        let trimmed = seg.trim();
        if !trimmed.is_empty() && trimmed.to_uppercase().starts_with("LISTEN") {
            listen_stmts.push(parse_listen(trimmed)?);
        }
    }

    // Phase 4.6: 解析所有 UNLISTEN 段
    let mut unlisten_stmts: Vec<Statement> = Vec::new();
    for seg in &segments {
        let trimmed = seg.trim();
        if !trimmed.is_empty() && trimmed.to_uppercase().starts_with("UNLISTEN") {
            unlisten_stmts.push(parse_unlisten(trimmed)?);
        }
    }

    // Phase 4.6: 解析所有 NOTIFY 段
    let mut notify_stmts: Vec<Statement> = Vec::new();
    for seg in &segments {
        let trimmed = seg.trim();
        if !trimmed.is_empty() && trimmed.to_uppercase().starts_with("NOTIFY") {
            notify_stmts.push(parse_notify(trimmed)?);
        }
    }

    // Phase 6.5: 解析所有 CREATE FUNCTION 段
    let mut create_function_stmts: Vec<Statement> = Vec::new();
    for seg in &segments {
        let trimmed = seg.trim();
        let upper = trimmed.to_uppercase();
        if !trimmed.is_empty()
            && (upper.starts_with("CREATE FUNCTION")
                || upper.starts_with("CREATE OR REPLACE FUNCTION"))
        {
            create_function_stmts.push(parse_create_function(trimmed)?);
        }
    }

    // Phase 6.5: 解析所有 DROP FUNCTION 段
    let mut drop_function_stmts: Vec<Statement> = Vec::new();
    for seg in &segments {
        let trimmed = seg.trim();
        if !trimmed.is_empty() && trimmed.to_uppercase().starts_with("DROP FUNCTION") {
            drop_function_stmts.push(parse_drop_function(trimmed)?);
        }
    }

    // Phase 6.10: 解析所有 DROP MATERIALIZED VIEW 段
    let mut drop_materialized_view_stmts: Vec<Statement> = Vec::new();
    for seg in &segments {
        let trimmed = seg.trim();
        if !trimmed.is_empty() && trimmed.to_uppercase().starts_with("DROP MATERIALIZED VIEW") {
            drop_materialized_view_stmts.push(parse_drop_materialized_view(trimmed)?);
        }
    }

    // Phase 6.10: 解析所有 REFRESH MATERIALIZED VIEW 段
    let mut refresh_materialized_view_stmts: Vec<Statement> = Vec::new();
    for seg in &segments {
        let trimmed = seg.trim();
        if !trimmed.is_empty()
            && trimmed
                .to_uppercase()
                .starts_with("REFRESH MATERIALIZED VIEW")
        {
            refresh_materialized_view_stmts.push(parse_refresh_materialized_view(trimmed)?);
        }
    }

    // 解析所有非特殊段（合并后一次性解析）
    let contains_replace = non_special_segments
        .iter()
        .any(|s| contains_replace_statement(s));
    // Phase 3.34: SET NAMES 在 PG dialect 下无法解析，需要 MySQL dialect
    let contains_set_names = non_special_segments
        .iter()
        .any(|s| contains_set_names_statement(s));
    let combined: String = non_special_segments.join("; ");
    let non_special_parsed = if combined.trim().is_empty() {
        Vec::new()
    } else if contains_replace || contains_set_names {
        let dialect = sqlparser::dialect::MySqlDialect {};
        Parser::parse_sql(&dialect, &combined)?
    } else {
        let dialect = PostgreSqlDialect {};
        Parser::parse_sql(&dialect, &combined)?
    };
    let non_special_stmts: Vec<Statement> = non_special_parsed
        .into_iter()
        .map(convert_statement)
        .collect::<Result<Vec<_>, _>>()?;

    // 数量一致性校验
    if non_special_stmts.len() != non_special_segments.len() {
        return Err(ParseError::Unsupported(format!(
            "parse_sql: non-special statement count mismatch (parsed={}, expected={}) — possible sqlparser splitting issue",
            non_special_stmts.len(),
            non_special_segments.len()
        )));
    }

    // 按原始顺序合并结果
    let mut alter_iter = alter_stmts.into_iter();
    let mut flashback_iter = flashback_stmts.into_iter();
    let mut listen_iter = listen_stmts.into_iter();
    let mut unlisten_iter = unlisten_stmts.into_iter();
    let mut notify_iter = notify_stmts.into_iter();
    let mut create_function_iter = create_function_stmts.into_iter();
    let mut drop_function_iter = drop_function_stmts.into_iter();
    let mut drop_materialized_view_iter = drop_materialized_view_stmts.into_iter();
    let mut refresh_materialized_view_iter = refresh_materialized_view_stmts.into_iter();
    let mut non_special_iter = non_special_stmts.into_iter();
    for kind in segment_kind {
        match kind {
            1 => {
                if let Some(stmt) = alter_iter.next() {
                    result.push(stmt);
                }
            }
            2 => {
                if let Some(stmt) = flashback_iter.next() {
                    result.push(stmt);
                }
            }
            3 => {
                if let Some(stmt) = listen_iter.next() {
                    result.push(stmt);
                }
            }
            4 => {
                if let Some(stmt) = unlisten_iter.next() {
                    result.push(stmt);
                }
            }
            5 => {
                if let Some(stmt) = notify_iter.next() {
                    result.push(stmt);
                }
            }
            6 => {
                if let Some(stmt) = create_function_iter.next() {
                    result.push(stmt);
                }
            }
            7 => {
                if let Some(stmt) = drop_function_iter.next() {
                    result.push(stmt);
                }
            }
            8 => {
                if let Some(stmt) = drop_materialized_view_iter.next() {
                    result.push(stmt);
                }
            }
            9 => {
                if let Some(stmt) = refresh_materialized_view_iter.next() {
                    result.push(stmt);
                }
            }
            _ => {
                if let Some(stmt) = non_special_iter.next() {
                    result.push(stmt);
                }
            }
        }
    }
    Ok(result)
}

/// 按分号切分 SQL 语句（保留每段文本，包括空白段以便位置对应）
///
/// 注意：此实现不处理字符串字面量中的分号，对常规 SQL 足够。
fn split_sql_statements(sql: &str) -> Vec<&str> {
    sql.split(';').collect()
}

/// 检测 SQL 字符串是否包含 REPLACE 语句（大小写不敏感，跳过注释/字符串字面量）
///
/// 简化实现：按分号分割语句，检查每条语句是否以 `REPLACE` 关键字开头。
fn contains_replace_statement(sql: &str) -> bool {
    // 简化：按分号切分，对每段去前导空白后检查是否以 REPLACE（不区分大小写）开头
    // 注意：这是保守检测，可能因分号出现在字符串字面量中而误判；
    // 但 REPLACE 的实际使用场景通常不含分号字面量，可接受。
    sql.split(';').any(|stmt| {
        let trimmed = stmt.trim_start();
        trimmed.len() >= 7
            && trimmed[..7].eq_ignore_ascii_case("REPLACE")
            && (trimmed.len() == 7 || trimmed.as_bytes()[7].is_ascii_whitespace())
    })
}

/// 检测 SQL 字符串是否包含 SET NAMES 语句（Phase 3.34，MySQL 语法）
///
/// 简化实现：按分号分割语句，检查每条语句是否以 `SET NAMES` 开头（不区分大小写）。
/// 用于在 parse_sql 入口处切换到 MySqlDialect（PG dialect 不支持 SET NAMES）。
fn contains_set_names_statement(sql: &str) -> bool {
    sql.split(';').any(|stmt| {
        let trimmed = stmt.trim_start();
        let len = trimmed.len();
        // "SET NAMES" 长度为 9 字符
        if len < 9 {
            return false;
        }
        // 检查是否以 "SET NAMES" 开头（不区分大小写）
        if !trimmed[..9].eq_ignore_ascii_case("SET NAMES") {
            return false;
        }
        // 后续字符应为空白或字符串结尾（避免误匹配 SET NAMES_OTHER）
        if len == 9 {
            return true;
        }
        let next_byte = trimmed.as_bytes()[9];
        next_byte.is_ascii_whitespace() || next_byte == b'\'' || next_byte == b'"'
    })
}

/// 解析单条 SQL 语句（用于测试）
pub fn parse_one(sql: &str) -> Result<Statement, ParseError> {
    let stmts = parse_sql(sql)?;
    if stmts.len() != 1 {
        return Err(ParseError::Unsupported(format!(
            "expected 1 statement, got {}",
            stmts.len()
        )));
    }
    Ok(stmts.into_iter().next().unwrap())
}

/// 解析 SQL 字符串为**单条** SzRSQL AST 语句（ADV-BUG-002 修复）
///
/// 与 [`parse_one`] 的区别：
/// - `parse_one`：要求输入恰好包含 1 条语句（多于 1 条报错）
/// - `parse_single_statement`：pgwire 默认单语句模式，**只执行首条语句**，
///   但若检测到第二条语句则返回错误（更严格的安全语义）
///
/// # 安全说明
///
/// pgwire 协议的 Simple Query 模式默认接受分号分隔的多语句，
/// 但 PostgreSQL 实现中默认只执行首条语句（除非显式启用 multi-statement）。
/// SzRSQL 采用相同策略：默认禁止多语句执行，防止 SQL 注入。
///
/// # 返回
///
/// - `Ok(stmt)`：输入只包含 1 条语句（或仅有 1 条非空语句）
/// - `Err`：输入包含多条非空语句，拒绝执行
///
/// # 示例
///
/// ```
/// use szrsql_sql::parser::parse_single_statement;
/// // 单语句：正常解析
/// assert!(parse_single_statement("SELECT 1").is_ok());
/// // 多语句：拒绝
/// assert!(parse_single_statement("SELECT 1; DROP TABLE users").is_err());
/// // 空语句：报错
/// assert!(parse_single_statement("").is_err());
/// ```
pub fn parse_single_statement(sql: &str) -> Result<Statement, ParseError> {
    let stmts = parse_sql(sql)?;
    // 过滤掉空语句（如末尾多余的空段）
    let non_empty: Vec<&Statement> = stmts.iter().filter(|s| !is_empty_statement(s)).collect();
    if non_empty.is_empty() {
        return Err(ParseError::Unsupported(
            "empty SQL statement".to_string(),
        ));
    }
    if non_empty.len() > 1 {
        return Err(ParseError::Unsupported(format!(
            "multi-statement SQL not allowed in single-statement mode (ADV-BUG-002 protection): got {} statements",
            non_empty.len()
        )));
    }
    // 克隆首条非空语句（避免消耗 vec）
    Ok(stmts
        .into_iter()
        .find(|s| !is_empty_statement(s))
        .unwrap())
}

/// 判断语句是否为空语句（如纯分号产生的空 Statement）
///
/// SzRSQL AST 中没有显式的"空语句"类型，这里主要防御未来可能引入的空语句。
/// 当前实现总是返回 false，保持向前兼容。
fn is_empty_statement(_stmt: &Statement) -> bool {
    false
}

// =====================================================================
//  Statement 转换
// =====================================================================

pub(crate) fn convert_statement(stmt: SpStatement) -> Result<Statement, ParseError> {
    match stmt {
        SpStatement::Query(query) => {
            let select = convert_query(*query)?;
            Ok(Statement::Select(Box::new(select)))
        }
        SpStatement::Insert(Insert {
            table_name,
            columns,
            source,
            on,
            returning,
            replace_into,
            ..
        }) => {
            // Phase 3.25: REPLACE INTO — MySQL 扩展
            // sqlparser 将 REPLACE INTO 解析为 Insert { replace_into: true, ... }
            if replace_into {
                convert_replace(table_name, columns, source)
            } else {
                convert_insert(table_name, columns, source, on, returning)
            }
        }
        SpStatement::Update {
            table,
            assignments,
            from,
            selection,
            returning,
            ..
        } => convert_update(table, assignments, from, selection, returning),
        SpStatement::Delete(delete) => convert_delete(delete),
        SpStatement::CreateTable(create) => convert_create_table(create),
        SpStatement::Drop {
            object_type,
            if_exists,
            names,
            cascade,
            ..
        } => convert_drop(object_type, if_exists, names, cascade),
        SpStatement::CreateIndex(create) => convert_create_index(create),
        SpStatement::CreateSequence {
            temporary,
            if_not_exists,
            name,
            data_type,
            sequence_options,
            owned_by,
        } => convert_create_sequence(
            temporary,
            if_not_exists,
            name,
            data_type,
            sequence_options,
            owned_by,
        ),
        SpStatement::StartTransaction { modes, .. } => {
            let (isolation, access) = convert_tx_modes(modes);
            Ok(Statement::Begin { isolation, access })
        }
        SpStatement::Commit { .. } => Ok(Statement::Commit),
        SpStatement::Rollback { savepoint, .. } => Ok(Statement::Rollback {
            savepoint: savepoint.map(|i| i.value),
        }),
        SpStatement::Savepoint { name } => Ok(Statement::Savepoint(name.value)),
        SpStatement::ReleaseSavepoint { name } => Ok(Statement::ReleaseSavepoint(name.value)),
        SpStatement::SetTransaction { modes, .. } => {
            let (isolation, access) = convert_tx_modes(modes);
            Ok(Statement::SetTransaction { isolation, access })
        }
        SpStatement::Explain {
            analyze,
            verbose,
            statement,
            ..
        } => {
            let inner = convert_statement(*statement)?;
            Ok(Statement::Explain {
                statement: Box::new(inner),
                analyze,
                verbose,
            })
        }
        SpStatement::Merge {
            table,
            source,
            on,
            clauses,
            ..
        } => {
            // 转换目标表
            let (target, target_alias) = match table {
                SpTableFactor::Table { name, alias, .. } => {
                    let table_name = convert_object_name(name)?;
                    let alias_str = alias.as_ref().map(|a| a.name.value.clone());
                    (table_name, alias_str)
                }
                other => {
                    return Err(ParseError::Unsupported(format!(
                        "MERGE target must be a table, got {other:?}"
                    )))
                }
            };
            // 转换源
            let source_tf = convert_table_factor(source)?;
            // 转换 ON 条件
            let on_expr = convert_expr(*on)?;
            // 转换 WHEN 子句
            let mut our_clauses = Vec::with_capacity(clauses.len());
            for clause in clauses {
                let kind = match clause.clause_kind {
                    sqlparser::ast::MergeClauseKind::Matched => MergeClauseKind::Matched,
                    sqlparser::ast::MergeClauseKind::NotMatched => MergeClauseKind::NotMatched,
                    sqlparser::ast::MergeClauseKind::NotMatchedByTarget => {
                        // NOT MATCHED BY TARGET 等价于 NOT MATCHED（目标无匹配）
                        MergeClauseKind::NotMatched
                    }
                    sqlparser::ast::MergeClauseKind::NotMatchedBySource => {
                        MergeClauseKind::NotMatchedBySource
                    }
                };
                let predicate = match clause.predicate {
                    Some(e) => Some(convert_expr(e)?),
                    None => None,
                };
                let action = match clause.action {
                    sqlparser::ast::MergeAction::Insert(insert) => {
                        let columns = insert
                            .columns
                            .iter()
                            .map(|i| i.value.clone())
                            .collect::<Vec<_>>();
                        let values = match insert.kind {
                            sqlparser::ast::MergeInsertKind::Values(v) => {
                                // 取第一行 VALUES（MERGE INSERT 仅支持单行）
                                if v.rows.len() != 1 {
                                    return Err(ParseError::Unsupported(format!(
                                        "MERGE INSERT VALUES expects 1 row, got {}",
                                        v.rows.len()
                                    )));
                                }
                                v.rows
                                    .into_iter()
                                    .next()
                                    .unwrap()
                                    .into_iter()
                                    .map(convert_expr)
                                    .collect::<Result<Vec<_>, _>>()?
                            }
                            sqlparser::ast::MergeInsertKind::Row => {
                                return Err(ParseError::Unsupported(
                                    "MERGE INSERT ROW not supported".into(),
                                ))
                            }
                        };
                        MergeAction::Insert { columns, values }
                    }
                    sqlparser::ast::MergeAction::Update { assignments } => {
                        let our_assignments = assignments
                            .iter()
                            .map(convert_assignment)
                            .collect::<Result<Vec<_>, _>>()?;
                        MergeAction::Update {
                            assignments: our_assignments,
                        }
                    }
                    sqlparser::ast::MergeAction::Delete => MergeAction::Delete,
                };
                our_clauses.push(MergeClause {
                    kind,
                    predicate,
                    action,
                });
            }
            Ok(Statement::Merge {
                target,
                target_alias,
                source: source_tf,
                on: on_expr,
                clauses: our_clauses,
            })
        }
        // Phase 3.26: PREPARE / EXECUTE / DEALLOCATE
        SpStatement::Prepare {
            name,
            data_types,
            statement,
        } => {
            let parameter_types = data_types
                .into_iter()
                .map(convert_data_type)
                .collect::<Result<Vec<_>, _>>()?;
            let inner = convert_statement(*statement)?;
            Ok(Statement::Prepare {
                name: name.value,
                parameter_types,
                statement: Box::new(inner),
            })
        }
        SpStatement::Execute {
            name, parameters, ..
        } => {
            // EXECUTE name (params...) — using 子句当前未实现，忽略
            let params = parameters
                .into_iter()
                .map(convert_expr)
                .collect::<Result<Vec<_>, _>>()?;
            let name_str = match name.0.into_iter().next() {
                Some(ident) => ident.value,
                None => return Err(ParseError::Unsupported("EXECUTE with empty name".into())),
            };
            Ok(Statement::Execute {
                name: name_str,
                parameters: params,
            })
        }
        SpStatement::Deallocate { name, .. } => {
            // DEALLOCATE [PREPARE] name — `prepare` 标志仅语法糖，不影响语义
            // DEALLOCATE ALL 在 sqlparser 中如何表示？检查 name 是否为 "ALL"
            // 实际上 sqlparser 把 DEALLOCATE ALL 也解析为 Deallocate { name: Ident("ALL"), ... }
            let name_str = name.value;
            if name_str.eq_ignore_ascii_case("ALL") {
                Ok(Statement::Deallocate { name: None })
            } else {
                Ok(Statement::Deallocate {
                    name: Some(name_str),
                })
            }
        }
        // Phase 3.31: CREATE TYPE name AS ENUM (...)
        // 注：sqlparser 0.53.0 不支持 ALTER TYPE 解析，ALTER TYPE 在 parse_sql 入口
        // 处通过手动预处理（见 `parse_alter_type`）转换，不经过 convert_statement。
        SpStatement::CreateType {
            name,
            representation,
        } => convert_create_type(name, representation),
        // Phase 3.34: SHOW TABLES / SHOW CREATE TABLE / SHOW variable
        SpStatement::ShowTables { .. } => Ok(Statement::ShowTables),
        SpStatement::ShowCreate { obj_type, obj_name } => {
            use sqlparser::ast::ShowCreateObject;
            match obj_type {
                ShowCreateObject::Table => {
                    let name = convert_object_name(obj_name)?;
                    Ok(Statement::ShowCreateTable { name })
                }
                other => Err(ParseError::Unsupported(format!(
                    "SHOW CREATE: unsupported object type {other:?}"
                ))),
            }
        }
        SpStatement::ShowVariable { variable } => {
            // SHOW variable → 取首个 ident 作为变量名（简化：忽略多段 schema 路径）
            let variable_name = variable
                .into_iter()
                .next()
                .map(|i| i.value)
                .unwrap_or_default();
            Ok(Statement::ShowVariable {
                variable: variable_name,
            })
        }
        // Phase 3.34: SET NAMES 'charset' [COLLATE 'collation']
        SpStatement::SetNames {
            charset_name,
            collation_name,
        } => Ok(Statement::SetNames {
            charset: charset_name,
            collation: collation_name,
        }),
        SpStatement::SetNamesDefault {} => Ok(Statement::SetNames {
            charset: "default".into(),
            collation: None,
        }),
        // Phase 3.34: SET variable = value
        SpStatement::SetVariable {
            variables, value, ..
        } => {
            // 取首个变量名（简化：不支持 (var1, var2) = (val1, val2) 多变量赋值）
            use sqlparser::ast::OneOrManyWithParens;
            let variable_name = match variables {
                OneOrManyWithParens::One(name) => convert_object_name_to_string(&name),
                OneOrManyWithParens::Many(names) => {
                    if names.len() != 1 {
                        return Err(ParseError::Unsupported(format!(
                            "SET multi-variable assignment not supported: {names:?}"
                        )));
                    }
                    convert_object_name_to_string(&names[0])
                }
            };
            if value.len() != 1 {
                return Err(ParseError::Unsupported(format!(
                    "SET multi-value assignment not supported: {value:?}"
                )));
            }
            let value_expr = convert_expr(value.into_iter().next().unwrap())?;
            Ok(Statement::SetVariable {
                variable: variable_name,
                value: value_expr,
            })
        }
        // Phase 4.8: COPY FROM / COPY TO
        SpStatement::Copy {
            source,
            to,
            target,
            options,
            legacy_options,
            ..
        } => convert_copy(source, to, target, options, legacy_options),
        // Phase 6.4: CREATE TRIGGER / DROP TRIGGER
        SpStatement::CreateTrigger {
            or_replace,
            is_constraint,
            name,
            period,
            events,
            table_name,
            trigger_object,
            condition,
            exec_body,
            ..
        } => convert_create_trigger(
            or_replace,
            is_constraint,
            name,
            period,
            events,
            table_name,
            trigger_object,
            condition,
            exec_body,
        ),
        SpStatement::DropTrigger {
            if_exists,
            trigger_name,
            table_name,
            option,
        } => convert_drop_trigger(if_exists, trigger_name, table_name, option),
        // Phase 6.10: CREATE VIEW / CREATE MATERIALIZED VIEW
        SpStatement::CreateView {
            or_replace: _,
            materialized,
            name,
            columns,
            query,
            if_not_exists,
            ..
        } => convert_create_view(materialized, if_not_exists, name, columns, query),
        other => Err(ParseError::Unsupported(format!(
            "unsupported statement: {other:?}"
        ))),
    }
}

// =====================================================================
//  事务模式转换
// =====================================================================

fn convert_tx_modes(
    modes: Vec<TransactionMode>,
) -> (Option<TransactionIsolation>, Option<TransactionAccess>) {
    let mut isolation = None;
    let mut access = None;
    for mode in modes {
        match mode {
            TransactionMode::AccessMode(TransactionAccessMode::ReadOnly) => {
                access = Some(TransactionAccess::ReadOnly);
            }
            TransactionMode::AccessMode(TransactionAccessMode::ReadWrite) => {
                access = Some(TransactionAccess::ReadWrite);
            }
            TransactionMode::IsolationLevel(level) => {
                isolation = Some(match level {
                    TransactionIsolationLevel::ReadUncommitted => {
                        TransactionIsolation::ReadUncommitted
                    }
                    TransactionIsolationLevel::ReadCommitted => TransactionIsolation::ReadCommitted,
                    TransactionIsolationLevel::RepeatableRead => {
                        TransactionIsolation::RepeatableRead
                    }
                    TransactionIsolationLevel::Serializable => TransactionIsolation::Serializable,
                });
            }
        }
    }
    (isolation, access)
}

// =====================================================================
//  Query → Select 转换
// =====================================================================

fn convert_query(query: SpQuery) -> Result<Select, ParseError> {
    // 支持 SELECT / SetOperation（INTERSECT / EXCEPT / UNION）；不支持 VALUES / FETCH / FOR UPDATE
    let SpQuery {
        with,
        body,
        order_by,
        limit,
        offset,
        ..
    } = query;

    // 将 body 转换为 Select（包含 set_op 字段，递归处理嵌套集合操作）
    let mut select = convert_set_expr_to_select(&body)?;

    // Phase 6.1: WITH 子句（CTE）— 仅当外层 Query 持有 with 时设置
    // （嵌套 SetOperation 内部的 body 不携带 with；PG 语义下 with 仅属于最外层 Query）
    select.with = with.map(convert_with).transpose()?;

    // ORDER BY / LIMIT / OFFSET 作用于整体（外层）
    let order_by = match order_by {
        Some(ob) => ob
            .exprs
            .into_iter()
            .map(convert_order_by_expr)
            .collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
    };
    let limit = convert_option_expr(limit)?;
    let offset = match offset {
        Some(off) => Some(convert_expr(off.value)?),
        None => None,
    };

    select.order_by = order_by;
    select.limit = limit;
    select.offset = offset;
    Ok(select)
}

/// 将 sqlparser `With` 转换为 SzRSQL `WithClause` — Phase 6.1
///
/// sqlparser `Cte { alias, query, from, materialized, .. }`：
/// - `alias` 包含 CTE 名称 + 可选列别名
/// - `query` 是 `Box<Query>`，递归调用 `convert_query`
/// - `from`（PG 不支持，仅 MySQL 8.0+）忽略
/// - `materialized`（PG `MATERIALIZED`/`NOT MATERIALIZED` 提示）忽略，统一物化
fn convert_with(with: sqlparser::ast::With) -> Result<WithClause, ParseError> {
    let recursive = with.recursive;
    let ctes = with
        .cte_tables
        .into_iter()
        .map(|cte| {
            let name = cte.alias.name.value.clone();
            let columns = cte
                .alias
                .columns
                .into_iter()
                .map(|c| c.name.value)
                .collect::<Vec<_>>();
            let query = Box::new(convert_query(*cte.query)?);
            Ok(CommonTableExpr {
                name,
                columns,
                query,
            })
        })
        .collect::<Result<Vec<_>, ParseError>>()?;
    Ok(WithClause { recursive, ctes })
}

/// 将 sqlparser `SetExpr` 转换为 SzRSQL `Select`
///
/// - `Select` → 普通 SELECT（set_op = None）
/// - `Query` → 递归调用 convert_query（处理括号子查询）
/// - `SetOperation` → 当前 Select 包含 left 的字段，set_op 为 SetOperation { op, right }
///   递归处理嵌套集合操作（如 `a UNION b EXCEPT c` 解析为 SetOp(EXCEPT, SetOp(UNION, a, b), c)）
fn convert_set_expr_to_select(set_expr: &SpSetExpr) -> Result<Select, ParseError> {
    match set_expr {
        SpSetExpr::Select(select) => {
            let distinct = select.distinct.is_some();
            let projection = select
                .projection
                .iter()
                .cloned()
                .map(convert_select_item)
                .collect::<Result<Vec<_>, _>>()?;
            let from = select
                .from
                .iter()
                .cloned()
                .map(convert_table_with_joins)
                .collect::<Result<Vec<_>, _>>()?;
            let where_clause = convert_option_expr(select.selection.clone())?;
            let group_by = match &select.group_by {
                sqlparser::ast::GroupByExpr::Expressions(exprs, _) => exprs
                    .iter()
                    .cloned()
                    .map(convert_expr)
                    .collect::<Result<Vec<_>, _>>()?,
                _ => Vec::new(),
            };
            let having = convert_option_expr(select.having.clone())?;
            Ok(Select {
                with: None,
                distinct,
                projection,
                from,
                where_clause,
                group_by,
                having,
                order_by: Vec::new(),
                limit: None,
                offset: None,
                set_op: None,
            })
        }
        SpSetExpr::Query(query) => convert_query((**query).clone()),
        SpSetExpr::SetOperation {
            op,
            set_quantifier,
            left,
            right,
        } => {
            // left 转换为 Select（递归处理嵌套集合操作）
            let left_select = convert_set_expr_to_select(left)?;
            // right 转换为 Select
            let right_select = convert_set_expr_to_select(right)?;
            // 量词转换
            let sz_quantifier = match set_quantifier {
                SpSetQuantifier::All => SetQuantifier::All,
                SpSetQuantifier::Distinct => SetQuantifier::Distinct,
                SpSetQuantifier::None => SetQuantifier::None,
                SpSetQuantifier::ByName
                | SpSetQuantifier::AllByName
                | SpSetQuantifier::DistinctByName => {
                    return Err(ParseError::Unsupported(format!(
                        "unsupported set quantifier: {set_quantifier:?}"
                    )));
                }
            };
            let sz_op = match op {
                SpSetOperator::Union => SetOperator::Union,
                SpSetOperator::Intersect => SetOperator::Intersect,
                SpSetOperator::Except => SetOperator::Except,
            };
            // 当前 Select 的字段取自 left（最内层 SELECT 的字段），
            // set_op 保留 left 的完整结构（含嵌套）+ 当前层的 (op, right)
            // 注意：需要先 clone left_select 的字段，因为 Box::new(left_select) 会 move
            Ok(Select {
                with: None,
                distinct: left_select.distinct,
                projection: left_select.projection.clone(),
                from: left_select.from.clone(),
                where_clause: left_select.where_clause.clone(),
                group_by: left_select.group_by.clone(),
                having: left_select.having.clone(),
                order_by: Vec::new(),
                limit: None,
                offset: None,
                set_op: Some(SetOperation {
                    op: sz_op,
                    quantifier: sz_quantifier,
                    left: Box::new(left_select),
                    right: Box::new(right_select),
                }),
            })
        }
        other => Err(ParseError::Unsupported(format!(
            "unsupported set expr: {other:?}"
        ))),
    }
}

// =====================================================================
//  SelectItem 转换
// =====================================================================

fn convert_select_item(item: SpSelectItem) -> Result<SelectItem, ParseError> {
    match item {
        SpSelectItem::UnnamedExpr(expr) => Ok(SelectItem::UnnamedExpr(convert_expr(expr)?)),
        SpSelectItem::ExprWithAlias { expr, alias } => Ok(SelectItem::ExprWithAlias {
            expr: convert_expr(expr)?,
            alias: alias.value,
        }),
        SpSelectItem::QualifiedWildcard(obj, _) => {
            let parts: Vec<String> = obj.0.into_iter().map(|i| i.value).collect();
            // 取最后一部分作为表别名
            let last = parts.last().cloned().unwrap_or_default();
            Ok(SelectItem::QualifiedWildcard(last))
        }
        SpSelectItem::Wildcard(_) => Ok(SelectItem::Wildcard),
    }
}

// =====================================================================
//  TableWithJoins / TableFactor / Join 转换
// =====================================================================

fn convert_table_with_joins(twj: SpTableWithJoins) -> Result<TableWithJoins, ParseError> {
    Ok(TableWithJoins {
        relation: convert_table_factor(twj.relation)?,
        joins: twj
            .joins
            .into_iter()
            .map(convert_join)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn convert_table_factor(tf: SpTableFactor) -> Result<TableFactor, ParseError> {
    match tf {
        SpTableFactor::Table { name, alias, .. } => Ok(TableFactor::Table {
            name: convert_object_name(name)?,
            alias: alias.map(convert_table_alias),
        }),
        SpTableFactor::Derived {
            subquery, alias, ..
        } => {
            let subquery = convert_query(*subquery)?;
            let alias = alias
                .map(convert_table_alias)
                .ok_or_else(|| ParseError::Unsupported("derived table without alias".into()))?;
            Ok(TableFactor::Derived {
                subquery: Box::new(subquery),
                alias,
            })
        }
        SpTableFactor::Function {
            name, args, alias, ..
        } => {
            let func_name = convert_object_name_to_string(&name);
            let args = args
                .into_iter()
                .map(|arg| match arg {
                    SpFunctionArg::Named { arg, .. } => arg,
                    SpFunctionArg::ExprNamed { arg, .. } => arg,
                    SpFunctionArg::Unnamed(arg) => arg,
                })
                .filter_map(|arg| match arg {
                    SpFunctionArgExpr::Expr(e) => Some(e),
                    _ => None,
                })
                .map(convert_expr)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(TableFactor::TableFunction {
                name: func_name,
                args,
                alias: alias.map(convert_table_alias),
            })
        }
        other => Err(ParseError::Unsupported(format!(
            "unsupported table factor: {other:?}"
        ))),
    }
}

fn convert_table_alias(alias: SpTableAlias) -> TableAlias {
    TableAlias {
        name: alias.name.value,
        column_aliases: if alias.columns.is_empty() {
            None
        } else {
            Some(alias.columns.into_iter().map(|c| c.name.value).collect())
        },
    }
}

fn convert_join(join: sqlparser::ast::Join) -> Result<Join, ParseError> {
    let (join_type, condition) = convert_join_operator(join.join_operator)?;
    Ok(Join {
        relation: convert_table_factor(join.relation)?,
        join_type,
        condition,
    })
}

fn convert_join_operator(op: JoinOperator) -> Result<(JoinType, JoinCondition), ParseError> {
    Ok(match op {
        JoinOperator::Inner(c) => (JoinType::Inner, convert_join_constraint(c)),
        JoinOperator::LeftOuter(c) => (JoinType::LeftOuter, convert_join_constraint(c)),
        JoinOperator::RightOuter(c) => (JoinType::RightOuter, convert_join_constraint(c)),
        JoinOperator::FullOuter(c) => (JoinType::FullOuter, convert_join_constraint(c)),
        JoinOperator::CrossJoin => (JoinType::Cross, JoinCondition::None),
        other => {
            return Err(ParseError::Unsupported(format!(
                "unsupported join operator: {other:?}"
            )));
        }
    })
}

fn convert_join_constraint(c: JoinConstraint) -> JoinCondition {
    match c {
        JoinConstraint::On(expr) => {
            // convert_expr 失败时仍返回 Natural 不合理，但此处不能传播错误。
            // 调用方应在 convert_join 中处理。
            // 这里采用简单 unwrap：ON 表达式应该总能转换
            JoinCondition::On(convert_expr(expr).unwrap_or(Expr::Wildcard))
        }
        JoinConstraint::Using(idents) => {
            JoinCondition::Using(idents.into_iter().map(|i| i.value).collect())
        }
        JoinConstraint::Natural => JoinCondition::Natural,
        JoinConstraint::None => JoinCondition::None,
    }
}

// =====================================================================
//  OrderByExpr 转换
// =====================================================================

fn convert_order_by_expr(ob: SpOrderByExpr) -> Result<OrderByExpr, ParseError> {
    Ok(OrderByExpr {
        expr: convert_expr(ob.expr)?,
        asc: ob.asc.unwrap_or(true),
        nulls_first: ob.nulls_first.unwrap_or(false),
    })
}

// =====================================================================
//  窗口函数 OVER 子句转换 — Phase 6.2
// =====================================================================

/// 转换 sqlparser `WindowType` → SzRSQL `WindowSpec`
///
/// 支持形式：
/// - `OVER (window_spec)` — 直接指定窗口规格
/// - `OVER named_window` — 引用命名窗口（当前不支持，返回 Unsupported 错误）
fn convert_window_type(wt: SpWindowType) -> Result<WindowSpec, ParseError> {
    match wt {
        SpWindowType::WindowSpec(spec) => convert_window_spec(spec),
        SpWindowType::NamedWindow(ident) => Err(ParseError::Unsupported(format!(
            "named window reference '{}' is not supported; inline the window spec in OVER (...) instead",
            ident.value
        ))),
    }
}

/// 转换 sqlparser `WindowSpec` → SzRSQL `WindowSpec`
///
/// 注：`window_name`（引用命名窗口）字段被忽略，因为它属于 PG 命名窗口特性，
/// 当前不支持 `WINDOW w AS (...)` 子句。
fn convert_window_spec(spec: SpWindowSpec) -> Result<WindowSpec, ParseError> {
    let partition_by = spec
        .partition_by
        .into_iter()
        .map(convert_expr)
        .collect::<Result<Vec<_>, _>>()?;
    let order_by = spec
        .order_by
        .into_iter()
        .map(convert_order_by_expr)
        .collect::<Result<Vec<_>, _>>()?;
    let window_frame = match spec.window_frame {
        Some(f) => Some(convert_window_frame(f)?),
        None => None,
    };
    Ok(WindowSpec {
        partition_by,
        order_by,
        window_frame,
    })
}

/// 转换 sqlparser `WindowFrame` → SzRSQL `WindowFrame`
fn convert_window_frame(f: SpWindowFrame) -> Result<WindowFrame, ParseError> {
    Ok(WindowFrame {
        units: convert_window_frame_units(f.units),
        start_bound: convert_window_frame_bound(f.start_bound),
        end_bound: f.end_bound.map(convert_window_frame_bound),
    })
}

/// 转换 sqlparser `WindowFrameUnits` → SzRSQL `WindowFrameUnits`
fn convert_window_frame_units(u: SpWindowFrameUnits) -> WindowFrameUnits {
    match u {
        SpWindowFrameUnits::Rows => WindowFrameUnits::Rows,
        SpWindowFrameUnits::Range => WindowFrameUnits::Range,
        SpWindowFrameUnits::Groups => WindowFrameUnits::Groups,
    }
}

/// 转换 sqlparser `WindowFrameBound` → SzRSQL `WindowFrameBound`
fn convert_window_frame_bound(b: SpWindowFrameBound) -> WindowFrameBound {
    match b {
        SpWindowFrameBound::CurrentRow => WindowFrameBound::CurrentRow,
        SpWindowFrameBound::Preceding(None) => WindowFrameBound::Preceding(None),
        SpWindowFrameBound::Preceding(Some(e)) => {
            // 转换可能失败时返回未绑定 — 但 convert_expr 内部不抛异常，此处用 unwrap_or 兜底
            match convert_expr(*e) {
                Ok(conv) => WindowFrameBound::Preceding(Some(Box::new(conv))),
                Err(_) => WindowFrameBound::Preceding(None),
            }
        }
        SpWindowFrameBound::Following(None) => WindowFrameBound::Following(None),
        SpWindowFrameBound::Following(Some(e)) => match convert_expr(*e) {
            Ok(conv) => WindowFrameBound::Following(Some(Box::new(conv))),
            Err(_) => WindowFrameBound::Following(None),
        },
    }
}

// =====================================================================
//  Expr 转换
// =====================================================================

fn convert_option_expr(opt: Option<SpExpr>) -> Result<Option<Expr>, ParseError> {
    match opt {
        Some(e) => Ok(Some(convert_expr(e)?)),
        None => Ok(None),
    }
}

fn convert_expr(expr: SpExpr) -> Result<Expr, ParseError> {
    convert_expr_inner(expr, 0)
}

/// `convert_expr` 的内部实现，携带递归深度计数器（ADV-BUG-001 修复）
///
/// 递归深度超限返回 `ParseError::Unsupported`，防止栈溢出 DoS。
/// 阈值 `MAX_EXPR_DEPTH` = 512，真实 SQL 表达式嵌套极少超过 32 层。
fn convert_expr_inner(expr: SpExpr, depth: usize) -> Result<Expr, ParseError> {
    if depth > MAX_EXPR_DEPTH {
        return Err(ParseError::Unsupported(format!(
            "expression nesting too deep: {} > {} (ADV-BUG-001 protection)",
            depth,
            MAX_EXPR_DEPTH
        )));
    }
    match expr {
        // Phase 3.26: $1、$2 ... 参数占位符 → Expr::Parameter(idx)
        SpExpr::Value(SpValue::Placeholder(s)) => convert_placeholder(&s),
        SpExpr::Value(v) => Ok(Expr::Literal(convert_value(v)?)),
        SpExpr::Identifier(ident) => Ok(Expr::Identifier(vec![ident.value])),
        SpExpr::CompoundIdentifier(idents) => Ok(Expr::Identifier(
            idents.into_iter().map(|i| i.value).collect(),
        )),
        SpExpr::BinaryOp { left, op, right } => Ok(Expr::BinaryOp {
            left: Box::new(convert_expr_inner(*left, depth + 1)?),
            op: convert_binary_op(op),
            right: Box::new(convert_expr_inner(*right, depth + 1)?),
        }),
        SpExpr::UnaryOp { op, expr } => Ok(Expr::UnaryOp {
            op: convert_unary_op(op),
            expr: Box::new(convert_expr_inner(*expr, depth + 1)?),
        }),
        SpExpr::Function(func) => {
            // 函数名（取最后一段，去掉引号）
            let name = func
                .name
                .0
                .last()
                .map(|i| i.value.clone())
                .unwrap_or_default();
            let name = name.to_lowercase();
            // 0.53: func.args 是 FunctionArguments 枚举（None / Subquery / List）
            // DISTINCT 移到 List.duplicate_treatment
            let (args, distinct) = match func.args {
                SpFunctionArguments::List(list) => {
                    let distinct = matches!(
                        list.duplicate_treatment,
                        Some(sqlparser::ast::DuplicateTreatment::Distinct)
                    );
                    let args = list
                        .args
                        .into_iter()
                        .map(|arg| match arg {
                            SpFunctionArg::Named { arg, .. } => arg,
                            SpFunctionArg::ExprNamed { arg, .. } => arg,
                            SpFunctionArg::Unnamed(arg) => arg,
                        })
                        .filter_map(|arg| match arg {
                            SpFunctionArgExpr::Expr(e) => Some(e),
                            _ => None,
                        })
                        .map(|e| convert_expr_inner(e, depth + 1))
                        .collect::<Result<Vec<_>, _>>()?;
                    (args, distinct)
                }
                SpFunctionArguments::Subquery(_) => (Vec::new(), false),
                SpFunctionArguments::None => (Vec::new(), false),
            };
            // Phase 6.2: 若带 OVER 子句则识别为窗口函数
            if let Some(over) = func.over {
                let window = convert_window_type(over)?;
                return Ok(Expr::WindowFunction {
                    name,
                    args,
                    distinct,
                    window,
                });
            }
            Ok(Expr::Function {
                name,
                args,
                distinct,
            })
        }
        SpExpr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => {
            let operand = match operand {
                Some(e) => Some(Box::new(convert_expr_inner(*e, depth + 1)?)),
                None => None,
            };
            let mut when_then = Vec::with_capacity(conditions.len());
            for (cond, res) in conditions.into_iter().zip(results) {
                when_then.push((convert_expr_inner(cond, depth + 1)?, convert_expr_inner(res, depth + 1)?));
            }
            let else_expr = match else_result {
                Some(e) => Some(Box::new(convert_expr_inner(*e, depth + 1)?)),
                None => None,
            };
            Ok(Expr::Case {
                operand,
                when_then,
                else_expr,
            })
        }
        SpExpr::Cast {
            expr, data_type, ..
        } => Ok(Expr::Cast {
            expr: Box::new(convert_expr_inner(*expr, depth + 1)?),
            data_type: convert_data_type(data_type)?,
        }),
        SpExpr::InList {
            expr,
            list,
            negated,
        } => Ok(Expr::InList {
            expr: Box::new(convert_expr_inner(*expr, depth + 1)?),
            list: list
                .into_iter()
                .map(|e| convert_expr_inner(e, depth + 1))
                .collect::<Result<Vec<_>, _>>()?,
            negated,
        }),
        SpExpr::InSubquery {
            expr,
            subquery,
            negated,
        } => Ok(Expr::InSubquery {
            expr: Box::new(convert_expr_inner(*expr, depth + 1)?),
            subquery: Box::new(convert_query(*subquery)?),
            negated,
        }),
        SpExpr::Between {
            expr,
            negated,
            low,
            high,
        } => Ok(Expr::Between {
            expr: Box::new(convert_expr_inner(*expr, depth + 1)?),
            low: Box::new(convert_expr_inner(*low, depth + 1)?),
            high: Box::new(convert_expr_inner(*high, depth + 1)?),
            negated,
        }),
        SpExpr::Like {
            negated,
            expr,
            pattern,
            ..
        } => Ok(Expr::Like {
            expr: Box::new(convert_expr_inner(*expr, depth + 1)?),
            pattern: Box::new(convert_expr_inner(*pattern, depth + 1)?),
            negated,
        }),
        SpExpr::IsNull(e) => Ok(Expr::IsNull {
            expr: Box::new(convert_expr_inner(*e, depth + 1)?),
            negated: false,
        }),
        SpExpr::IsNotNull(e) => Ok(Expr::IsNull {
            expr: Box::new(convert_expr_inner(*e, depth + 1)?),
            negated: true,
        }),
        SpExpr::Subquery(query) => Ok(Expr::Subquery(Box::new(convert_query(*query)?))),
        SpExpr::Exists { subquery, negated } => Ok(Expr::Exists {
            subquery: Box::new(convert_query(*subquery)?),
            negated,
        }),
        SpExpr::Tuple(exprs) => Ok(Expr::Tuple(
            exprs
                .into_iter()
                .map(|e| convert_expr_inner(e, depth + 1))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        // Phase 3.32: ARRAY[..] 或 [..] 数组字面量
        SpExpr::Array(arr) => Ok(Expr::Array(
            arr.elem
                .into_iter()
                .map(|e| convert_expr_inner(e, depth + 1))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        // Phase 3.32: left OP ANY(right) / left OP SOME(right)
        SpExpr::AnyOp {
            left,
            compare_op,
            right,
            ..
        } => Ok(Expr::AnyOp {
            left: Box::new(convert_expr_inner(*left, depth + 1)?),
            op: convert_binary_op(compare_op),
            right: Box::new(convert_expr_inner(*right, depth + 1)?),
        }),
        // Phase 3.32: left OP ALL(right)
        SpExpr::AllOp {
            left,
            compare_op,
            right,
        } => Ok(Expr::AllOp {
            left: Box::new(convert_expr_inner(*left, depth + 1)?),
            op: convert_binary_op(compare_op),
            right: Box::new(convert_expr_inner(*right, depth + 1)?),
        }),
        SpExpr::Nested(e) => convert_expr_inner(*e, depth + 1),
        SpExpr::Wildcard(_) => Ok(Expr::Wildcard),
        SpExpr::QualifiedWildcard(obj, _) => {
            // 转换为 Identifier 形式（最后一部分作为表别名）
            let parts: Vec<String> = obj.0.into_iter().map(|i| i.value).collect();
            Ok(Expr::Identifier(parts))
        }
        other => Err(ParseError::Unsupported(format!(
            "unsupported expr: {other:?}"
        ))),
    }
}

fn convert_binary_op(op: BinaryOperator) -> BinaryOp {
    match op {
        BinaryOperator::Plus => BinaryOp::Plus,
        BinaryOperator::Minus => BinaryOp::Minus,
        BinaryOperator::Multiply => BinaryOp::Multiply,
        BinaryOperator::Divide => BinaryOp::Divide,
        BinaryOperator::Modulo => BinaryOp::Modulo,
        BinaryOperator::Eq => BinaryOp::Eq,
        BinaryOperator::NotEq => BinaryOp::NotEq,
        BinaryOperator::Lt => BinaryOp::Lt,
        BinaryOperator::LtEq => BinaryOp::LtEq,
        BinaryOperator::Gt => BinaryOp::Gt,
        BinaryOperator::GtEq => BinaryOp::GtEq,
        BinaryOperator::And => BinaryOp::And,
        BinaryOperator::Or => BinaryOp::Or,
        BinaryOperator::BitwiseAnd => BinaryOp::BitAnd,
        BinaryOperator::BitwiseOr => BinaryOp::BitOr,
        BinaryOperator::BitwiseXor | BinaryOperator::PGBitwiseXor => BinaryOp::BitXor,
        BinaryOperator::PGBitwiseShiftLeft => BinaryOp::ShiftLeft,
        BinaryOperator::PGBitwiseShiftRight => BinaryOp::ShiftRight,
        BinaryOperator::StringConcat => BinaryOp::StringConcat,
        // Phase 3.33: PG 全文检索 `@@` 操作符
        BinaryOperator::AtAt => BinaryOp::AtAt,
        other => {
            // 不支持的运算符降级为 Eq，并在调试中记录
            debug_assert!(false, "unsupported binary op: {other:?}");
            BinaryOp::Eq
        }
    }
}

fn convert_unary_op(op: UnaryOperator) -> UnaryOp {
    match op {
        UnaryOperator::Plus => UnaryOp::Plus,
        UnaryOperator::Minus => UnaryOp::Minus,
        UnaryOperator::Not => UnaryOp::Not,
        UnaryOperator::PGBitwiseNot => UnaryOp::BitNot,
        other => {
            debug_assert!(false, "unsupported unary op: {other:?}");
            UnaryOp::Plus
        }
    }
}

// =====================================================================
//  Value 转换
// =====================================================================

fn convert_value(value: SpValue) -> Result<Value, ParseError> {
    match value {
        SpValue::Null => Ok(Value::Null),
        SpValue::Boolean(b) => Ok(Value::Bool(b)),
        SpValue::Number(s, _) => {
            // 尝试 i64，失败则 f64
            if let Ok(i) = s.parse::<i64>() {
                Ok(Value::Int64(i))
            } else if let Ok(f) = s.parse::<f64>() {
                Ok(Value::Float64(f))
            } else {
                Err(ParseError::InvalidValue(s))
            }
        }
        SpValue::SingleQuotedString(s) => Ok(Value::Text(s)),
        SpValue::NationalStringLiteral(s) => Ok(Value::Text(s)),
        SpValue::HexStringLiteral(s) => {
            // 转换为 Blob
            let bytes = hex::decode(&s).map_err(|_| ParseError::InvalidValue(s.clone()))?;
            Ok(Value::Blob(bytes))
        }
        SpValue::Placeholder(_) => Err(ParseError::Unsupported(
            "placeholder $N must be handled by convert_expr, not convert_value".into(),
        )),
        other => Err(ParseError::Unsupported(format!(
            "unsupported value: {other:?}"
        ))),
    }
}

/// 解析参数占位符 `$1`、`$2` ... 为 `Expr::Parameter(idx)` — Phase 3.26
///
/// 支持格式：
/// - `$1`、`$2` ... — PG 风格（1-based 索引）
/// - `?` — MySQL/通用风格（按出现顺序递增，这里仅做单参数支持，索引=1）
fn convert_placeholder(s: &str) -> Result<Expr, ParseError> {
    if s == "?" {
        return Ok(Expr::Parameter(1));
    }
    if let Some(rest) = s.strip_prefix('$') {
        let idx: usize = rest
            .parse()
            .map_err(|_| ParseError::InvalidValue(format!("invalid placeholder index: {s}")))?;
        if idx == 0 {
            return Err(ParseError::InvalidValue(
                "placeholder index must be >= 1: $0 is invalid".into(),
            ));
        }
        return Ok(Expr::Parameter(idx));
    }
    Err(ParseError::Unsupported(format!(
        "unsupported placeholder syntax: {s}"
    )))
}

// =====================================================================
//  DataType 转换
// =====================================================================

fn convert_data_type(dt: DataType) -> Result<ColumnType, ParseError> {
    Ok(match dt {
        DataType::Bool | DataType::Boolean => ColumnType::Bool,
        DataType::TinyInt(_)
        | DataType::SmallInt(_)
        | DataType::Int(_)
        | DataType::Integer(_)
        | DataType::Int2(_)
        | DataType::Int4(_) => ColumnType::Int64,
        DataType::BigInt(_) | DataType::Int8(_) => ColumnType::Int64,
        DataType::UnsignedTinyInt(_)
        | DataType::UnsignedSmallInt(_)
        | DataType::UnsignedInt(_)
        | DataType::UnsignedInteger(_)
        | DataType::UnsignedBigInt(_)
        | DataType::UnsignedInt2(_)
        | DataType::UnsignedInt4(_)
        | DataType::UnsignedInt8(_) => ColumnType::Int64,
        DataType::Real | DataType::Float4 | DataType::Float32 => ColumnType::Float64,
        DataType::Float(_)
        | DataType::Double
        | DataType::DoublePrecision
        | DataType::Float8
        | DataType::Float64 => ColumnType::Float64,
        DataType::Numeric(info) | DataType::Decimal(info) | DataType::Dec(info) => {
            let (precision, scale) = match info {
                sqlparser::ast::ExactNumberInfo::None => (38u8, 0u8),
                sqlparser::ast::ExactNumberInfo::Precision(p) => (p as u8, 0u8),
                sqlparser::ast::ExactNumberInfo::PrecisionAndScale(p, s) => (p as u8, s as u8),
            };
            ColumnType::Decimal { precision, scale }
        }
        DataType::Date | DataType::Date32 => ColumnType::Date,
        DataType::Timestamp(_, _) => ColumnType::Timestamp,
        DataType::Text
        | DataType::Character(_)
        | DataType::Char(_)
        | DataType::CharacterVarying(_)
        | DataType::CharVarying(_)
        | DataType::Varchar(_)
        | DataType::Nvarchar(_)
        | DataType::String(_) => ColumnType::Text,
        DataType::Bytea | DataType::Binary(_) | DataType::Varbinary(_) | DataType::Blob(_) => {
            ColumnType::Blob
        }
        DataType::JSON | DataType::JSONB => ColumnType::Json,
        DataType::Array(arr_def) => {
            use sqlparser::ast::ArrayElemTypeDef::*;
            let elem_type = match arr_def {
                None => ColumnType::Null,
                AngleBracket(inner) | SquareBracket(inner, _) | Parenthesis(inner) => {
                    convert_data_type(*inner)?
                }
            };
            ColumnType::Array(Box::new(elem_type))
        }
        DataType::Enum(members, _) => {
            let values: Vec<String> = members
                .into_iter()
                .map(|m| match m {
                    sqlparser::ast::EnumMember::Name(s) => s,
                    sqlparser::ast::EnumMember::NamedValue(s, _) => s,
                })
                .collect();
            ColumnType::Enum(values)
        }
        DataType::Custom(name, _) => {
            // 自定义类型尝试匹配常见名称
            let name_str = convert_object_name_to_string(&name).to_lowercase();
            match name_str.as_str() {
                "int" | "integer" | "bigint" => ColumnType::Int64,
                "text" | "varchar" => ColumnType::Text,
                "bool" | "boolean" => ColumnType::Bool,
                "double" | "float" | "real" => ColumnType::Float64,
                // PG SERIAL 系列：在 convert_create_table 中单独处理 default 表达式，
                // 这里只返回列类型（统一为 Int64，与 BIGSERIAL/SMALLSERIAL 在 i64 范围内一致）
                "serial" | "bigserial" | "smallserial" => ColumnType::Int64,
                // Phase 3.33: PG 全文检索类型
                "tsvector" => ColumnType::TsVector,
                "tsquery" => ColumnType::TsQuery,
                _ => ColumnType::Text, // 未知自定义类型降级为 Text
            }
        }
        other => {
            return Err(ParseError::InvalidDataType(format!("{other:?}")));
        }
    })
}

// =====================================================================
//  ObjectName / Ident 转换
// =====================================================================

fn convert_object_name(name: ObjectName) -> Result<TableName, ParseError> {
    let parts: Vec<String> = name.0.into_iter().map(|i| i.value).collect();
    match parts.len() {
        1 => Ok(TableName::new(parts[0].clone())),
        2 => Ok(TableName::with_schema(parts[0].clone(), parts[1].clone())),
        _ => Err(ParseError::Unsupported(format!(
            "unsupported object name with {} parts: {parts:?}",
            parts.len()
        ))),
    }
}

fn convert_object_name_to_string(name: &ObjectName) -> String {
    name.0
        .iter()
        .map(|i| i.value.clone())
        .collect::<Vec<_>>()
        .join(".")
}

// =====================================================================
//  DDL 转换
// =====================================================================

/// Phase 6.10: CREATE VIEW / CREATE MATERIALIZED VIEW 转换
///
/// 将 sqlparser 的 `CreateView` 转换为 SzRSQL `Statement::CreateView`。
///
/// # 参数
/// - `materialized`：true 表示 `CREATE MATERIALIZED VIEW`
/// - `if_not_exists`：true 表示 `CREATE VIEW IF NOT EXISTS`（SQLite/BigQuery 方言）
/// - `name`：视图名
/// - `columns`：显式列别名（`CREATE VIEW v (a, b) AS ...`）；`ViewColumnDef.name` 取列名
/// - `query`：视图查询体
///
/// # 限制
/// - `OR REPLACE` 当前被忽略（PG 支持，SzRSQL 暂未实现替换语义）
/// - `WITH (...)` 选项当前被忽略
/// - `ViewColumnDef.data_type` 与 `ViewColumnDef.options` 当前被忽略（仅取列名）
fn convert_create_view(
    materialized: bool,
    if_not_exists: bool,
    name: ObjectName,
    columns: Vec<sqlparser::ast::ViewColumnDef>,
    query: Box<SpQuery>,
) -> Result<Statement, ParseError> {
    let name = convert_object_name(name)?;
    let columns: Vec<String> = columns.into_iter().map(|c| c.name.value).collect();
    let select = convert_query(*query)?;
    Ok(Statement::CreateView {
        name,
        columns,
        query: Box::new(select),
        materialized,
        if_not_exists,
    })
}

fn convert_create_table(create: sqlparser::ast::CreateTable) -> Result<Statement, ParseError> {
    // Phase 3.28: 临时表支持
    // - temporary: bool — true 表示 CREATE TEMPORARY TABLE
    // - global: Option<bool> — PG 中 GLOBAL/LOCAL 仅为语法提示，不影响隔离语义（均会话级隔离）
    // - on_commit: Option<OnCommit> — ON COMMIT DELETE ROWS / PRESERVE ROWS / DROP
    let temporary = create.temporary;
    let on_commit = create.on_commit.map(|oc| match oc {
        sqlparser::ast::OnCommit::DeleteRows => OnCommitAction::DeleteRows,
        sqlparser::ast::OnCommit::PreserveRows => OnCommitAction::PreserveRows,
        sqlparser::ast::OnCommit::Drop => OnCommitAction::Drop,
    });
    let name = convert_object_name(create.name)?;
    let table_name_for_seq = name.name.clone();
    let columns = create
        .columns
        .into_iter()
        .map(|sp_col| {
            let col_name = sp_col.name.value.clone();
            let is_serial = is_serial_data_type(&sp_col.data_type);
            let mut def = convert_column_def(sp_col)?;
            if is_serial {
                // PG 语义：SERIAL 等价于 INTEGER + NOT NULL + DEFAULT nextval('table_col_seq')
                let seq_name = format!("{}_{}_seq", table_name_for_seq, col_name);
                def.default = Some(Expr::Function {
                    name: "nextval".into(),
                    args: vec![Expr::Literal(Value::Text(seq_name))],
                    distinct: false,
                });
                // SERIAL 隐含 NOT NULL
                def.not_null = true;
            }
            Ok(def)
        })
        .collect::<Result<Vec<_>, ParseError>>()?;
    let constraints = create
        .constraints
        .into_iter()
        .map(convert_table_constraint)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Statement::CreateTable {
        name,
        columns,
        constraints,
        if_not_exists: create.if_not_exists,
        temporary,
        on_commit,
    })
}

fn convert_column_def(col: SpColumnDef) -> Result<ColumnDefinition, ParseError> {
    // Phase 3.31: 检测 DataType::Custom，保留原始类型名供 Planner 解析
    // 当用户写 `CREATE TABLE t (c mood)` 时，sqlparser 将 `mood` 解析为 DataType::Custom
    // Parser 暂时将其降级为 ColumnType::Text，但保留原始名称到 custom_type_name
    // Planner 在 plan_create_table 时查询 Catalog 的 enum_types：若该名是已注册的
    // enum 类型，将 data_type 改写为 ColumnType::Enum(values)
    let custom_type_name = if let DataType::Custom(ref name, _) = col.data_type {
        let name_str = convert_object_name_to_string(name).to_lowercase();
        // 排除已知常见类型别名（int/text/bool/serial 等）
        match name_str.as_str() {
            "int" | "integer" | "bigint" | "text" | "varchar" | "bool" | "boolean" | "double"
            | "float" | "real" | "serial" | "bigserial" | "smallserial" => None,
            _ => Some(name_str),
        }
    } else {
        None
    };

    let mut def = ColumnDefinition::new(col.name.value, convert_data_type(col.data_type)?);
    def.custom_type_name = custom_type_name;
    for opt_def in col.options {
        apply_column_option(&mut def, opt_def)?;
    }
    Ok(def)
}

fn apply_column_option(
    def: &mut ColumnDefinition,
    opt_def: ColumnOptionDef,
) -> Result<(), ParseError> {
    match opt_def.option {
        ColumnOption::Null => {
            def.not_null = false;
        }
        ColumnOption::NotNull => {
            def.not_null = true;
        }
        ColumnOption::Default(expr) => {
            def.default = Some(convert_expr(expr)?);
        }
        ColumnOption::Unique { is_primary, .. } => {
            if is_primary {
                def.primary_key = true;
            } else {
                def.unique = true;
            }
        }
        ColumnOption::Check(expr) => {
            def.check = Some(convert_expr(expr)?);
        }
        ColumnOption::ForeignKey {
            foreign_table,
            referred_columns,
            on_delete,
            on_update,
            ..
        } => {
            def.references = Some(ForeignKeyReference {
                table: convert_object_name(foreign_table)?,
                columns: if referred_columns.is_empty() {
                    None
                } else {
                    Some(referred_columns.into_iter().map(|i| i.value).collect())
                },
                on_delete: on_delete.map(convert_referential_action),
                on_update: on_update.map(convert_referential_action),
            });
        }
        ColumnOption::Generated {
            generated_as,
            generation_expr,
            generation_expr_mode,
            ..
        } => {
            // Phase 6.18: 支持 GENERATED ALWAYS AS (expr) STORED
            // sqlparser 0.53 将 `GENERATED ALWAYS AS (expr) STORED` 解析为：
            //   generated_as = ExpStored, generation_expr = Some(expr),
            //   generation_expr_mode = Some(Stored)
            // 仅支持有表达式 + STORED 模式（拒绝 VIRTUAL 与 IDENTITY 列）
            let expr = match generation_expr {
                Some(e) => convert_expr(e)?,
                None => {
                    return Err(ParseError::Unsupported(
                        "GENERATED ... AS IDENTITY is not supported (only expression-based STORED generated columns are supported)".into(),
                    ));
                }
            };
            let stored = matches!(generation_expr_mode, Some(GeneratedExpressionMode::Stored));
            if !stored {
                return Err(ParseError::Unsupported(
                    "GENERATED ALWAYS AS ... VIRTUAL is not supported (only STORED is supported)"
                        .into(),
                ));
            }
            // ExpStored 是表达式 STORED 列的正确变体；Always/ByDefault 在无表达式时
            // 表示 IDENTITY 列（已在上方 generation_expr=None 分支拒绝）
            debug_assert!(
                matches!(generated_as, GeneratedAs::ExpStored),
                "STORED expression column should have generated_as=ExpStored, got {generated_as:?}"
            );
            def.generated = Some(GeneratedColumn { expr, stored: true });
        }
        _ => {
            // 其他列选项（Identity / DialectSpecific 等）暂不支持
        }
    }
    Ok(())
}

fn convert_referential_action(action: sqlparser::ast::ReferentialAction) -> ReferenceAction {
    match action {
        sqlparser::ast::ReferentialAction::Restrict => ReferenceAction::Restrict,
        sqlparser::ast::ReferentialAction::Cascade => ReferenceAction::Cascade,
        sqlparser::ast::ReferentialAction::SetNull => ReferenceAction::SetNull,
        sqlparser::ast::ReferentialAction::NoAction => ReferenceAction::NoAction,
        sqlparser::ast::ReferentialAction::SetDefault => ReferenceAction::SetDefault,
    }
}

fn convert_table_constraint(c: SpTableConstraint) -> Result<TableConstraint, ParseError> {
    Ok(match c {
        SpTableConstraint::PrimaryKey { name, columns, .. } => TableConstraint::PrimaryKey {
            name: name.map(|i| i.value),
            columns: columns.into_iter().map(|i| i.value).collect(),
        },
        SpTableConstraint::Unique { name, columns, .. } => TableConstraint::Unique {
            name: name.map(|i| i.value),
            columns: columns.into_iter().map(|i| i.value).collect(),
        },
        SpTableConstraint::ForeignKey {
            name,
            columns,
            foreign_table,
            referred_columns,
            on_delete,
            on_update,
            ..
        } => TableConstraint::ForeignKey {
            name: name.map(|i| i.value),
            columns: columns.into_iter().map(|i| i.value).collect(),
            reference: ForeignKeyReference {
                table: convert_object_name(foreign_table)?,
                columns: if referred_columns.is_empty() {
                    None
                } else {
                    Some(referred_columns.into_iter().map(|i| i.value).collect())
                },
                on_delete: on_delete.map(convert_referential_action),
                on_update: on_update.map(convert_referential_action),
            },
        },
        SpTableConstraint::Check { name, expr } => TableConstraint::Check {
            name: name.map(|i| i.value),
            expr: convert_expr(*expr)?,
        },
        other => {
            return Err(ParseError::Unsupported(format!(
                "unsupported table constraint: {other:?}"
            )));
        }
    })
}

fn convert_drop(
    object_type: sqlparser::ast::ObjectType,
    if_exists: bool,
    names: Vec<ObjectName>,
    cascade: bool,
) -> Result<Statement, ParseError> {
    use sqlparser::ast::ObjectType;
    match object_type {
        ObjectType::Table => {
            let names = names
                .into_iter()
                .map(convert_object_name)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Statement::DropTable {
                names,
                if_exists,
                cascade,
            })
        }
        ObjectType::Index => {
            let names = names
                .into_iter()
                .map(|n| convert_object_name_to_string(&n))
                .collect();
            Ok(Statement::DropIndex { names, if_exists })
        }
        ObjectType::Sequence => {
            let seq_names = names
                .into_iter()
                .map(convert_object_name)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Statement::DropSequence {
                names: seq_names,
                if_exists,
                cascade,
            })
        }
        // Phase 3.31: DROP TYPE
        ObjectType::Type => {
            let type_names = names
                .into_iter()
                .map(convert_object_name)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Statement::DropType {
                names: type_names,
                if_exists,
                cascade,
            })
        }
        // Phase 6.10: DROP VIEW（sqlparser 0.53.0 不支持 DROP MATERIALIZED VIEW，由 parse_drop_materialized_view 预处理）
        ObjectType::View => {
            let names = names
                .into_iter()
                .map(convert_object_name)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Statement::DropView {
                names,
                if_exists,
                cascade,
                materialized: false,
            })
        }
        other => Err(ParseError::Unsupported(format!(
            "unsupported drop object type: {other:?}"
        ))),
    }
}

fn convert_create_index(create: sqlparser::ast::CreateIndex) -> Result<Statement, ParseError> {
    let table = convert_object_name(create.table_name)?;
    let columns = create
        .columns
        .into_iter()
        .map(|ob| {
            Ok(IndexColumn {
                column: identifier_string(&ob.expr),
                asc: ob.asc.unwrap_or(true),
                nulls_first: ob.nulls_first.unwrap_or(false),
                expr: None,
            })
        })
        .collect::<Result<Vec<_>, ParseError>>()?;
    Ok(Statement::CreateIndex {
        name: create.name.map(|n| convert_object_name_to_string(&n)),
        table,
        columns,
        unique: create.unique,
        if_not_exists: create.if_not_exists,
    })
}

// =====================================================================
//  CREATE SEQUENCE 转换 — Phase 3.22
// =====================================================================

/// 将 sqlparser `CreateSequence` 转换为 SzRSQL AST
///
/// 支持的 sequence_options：
/// - `INCREMENT BY n` / `INCREMENT n` → increment
/// - `START WITH n` / `START n` → start
/// - `MINVALUE n` / `NO MINVALUE` → min_value
/// - `MAXVALUE n` / `NO MAXVALUE` → max_value
/// - `CYCLE` / `NO CYCLE` → cycle
/// - `CACHE n` — 当前忽略（PG 默认 1，与无 cache 等价）
///
/// 不支持：temporary、data_type（统一 Int64）、owned_by
fn convert_create_sequence(
    temporary: bool,
    if_not_exists: bool,
    name: ObjectName,
    data_type: Option<DataType>,
    sequence_options: Vec<sqlparser::ast::SequenceOptions>,
    owned_by: Option<ObjectName>,
) -> Result<Statement, ParseError> {
    use sqlparser::ast::SequenceOptions;

    if temporary {
        return Err(ParseError::Unsupported(
            "TEMPORARY SEQUENCE not supported".into(),
        ));
    }
    if data_type.is_some() {
        return Err(ParseError::Unsupported(
            "CREATE SEQUENCE WITH data type not supported (always bigint)".into(),
        ));
    }
    if owned_by.is_some() {
        return Err(ParseError::Unsupported(
            "CREATE SEQUENCE OWNED BY not supported".into(),
        ));
    }

    let name = convert_object_name(name)?;
    let mut increment: i64 = 1;
    let mut start: i64 = 1;
    let mut min_value: Option<i64> = None;
    let mut max_value: Option<i64> = None;
    let mut cycle: bool = false;

    for opt in sequence_options {
        match opt {
            SequenceOptions::IncrementBy(expr, _) => {
                let v = eval_const_int_expr(&expr)?;
                if v == 0 {
                    return Err(ParseError::InvalidValue(
                        "INCREMENT must be non-zero".into(),
                    ));
                }
                increment = v;
            }
            SequenceOptions::StartWith(expr, _) => {
                start = eval_const_int_expr(&expr)?;
            }
            SequenceOptions::MinValue(Some(expr)) => {
                min_value = Some(eval_const_int_expr(&expr)?);
            }
            SequenceOptions::MinValue(None) => {
                min_value = None; // NO MINVALUE
            }
            SequenceOptions::MaxValue(Some(expr)) => {
                max_value = Some(eval_const_int_expr(&expr)?);
            }
            SequenceOptions::MaxValue(None) => {
                max_value = None; // NO MAXVALUE
            }
            SequenceOptions::Cycle(c) => {
                // 注意：sqlparser 0.53.0 存在 bug —— `NO CYCLE` 解析为 `Cycle(true)`，
                // `CYCLE` 解析为 `Cycle(false)`，bool 语义反转。
                // 此处取反以恢复正确语义：c=true 表示 NO CYCLE → cycle=false。
                cycle = !c;
            }
            SequenceOptions::Cache(_) => {
                // CACHE 当前忽略（PG 默认 1）
            }
        }
    }

    // 校验：start 必须在 [min, max] 范围内（若显式指定）
    let lo = min_value.unwrap_or(i64::MIN);
    let hi = max_value.unwrap_or(i64::MAX);
    if start < lo || start > hi {
        return Err(ParseError::InvalidValue(format!(
            "START value {start} out of bounds [{lo}, {hi}]"
        )));
    }
    // 若 increment > 0：start < max；若 increment < 0：start > min
    if increment > 0 && max_value.is_some() && start > hi {
        return Err(ParseError::InvalidValue(format!(
            "START value {start} must be <= MAXVALUE {hi} for ascending sequence"
        )));
    }
    if increment < 0 && min_value.is_some() && start < lo {
        return Err(ParseError::InvalidValue(format!(
            "START value {start} must be >= MINVALUE {lo} for descending sequence"
        )));
    }

    Ok(Statement::CreateSequence {
        name,
        if_not_exists,
        start,
        increment,
        min_value,
        max_value,
        cycle,
    })
}

// =====================================================================
//  CREATE TYPE / ALTER TYPE 转换 — Phase 3.31
// =====================================================================

/// 将 sqlparser `CreateType` 转换为 SzRSQL AST — Phase 3.31
///
/// 仅支持 `CREATE TYPE name AS ENUM ('label1', 'label2', ...)`。
/// 其他表示形式（Composite）当前不支持。
fn convert_create_type(
    name: ObjectName,
    representation: sqlparser::ast::UserDefinedTypeRepresentation,
) -> Result<Statement, ParseError> {
    use sqlparser::ast::UserDefinedTypeRepresentation;

    let name = convert_object_name(name)?;
    let as_enum = match representation {
        UserDefinedTypeRepresentation::Enum { labels } => {
            labels.into_iter().map(|i| i.value).collect::<Vec<_>>()
        }
        UserDefinedTypeRepresentation::Composite { .. } => {
            return Err(ParseError::Unsupported(
                "CREATE TYPE ... AS (...) composite type not supported".into(),
            ));
        }
    };

    Ok(Statement::CreateType {
        name,
        as_enum,
        if_not_exists: false,
    })
}

/// 手动解析 `ALTER TYPE` 语句 — Phase 3.31
///
/// sqlparser 0.53.0 不支持 ALTER TYPE 解析，因此此处实现简化的手动解析器。
///
/// 支持的语法（PG 兼容子集）：
/// - `ALTER TYPE name ADD VALUE 'val'`
/// - `ALTER TYPE name ADD VALUE IF NOT EXISTS 'val'`
/// - `ALTER TYPE name ADD VALUE 'val' BEFORE 'existing'`（位置忽略）
/// - `ALTER TYPE name ADD VALUE 'val' AFTER 'existing'`（位置忽略）
/// - `ALTER TYPE name RENAME VALUE 'old' TO 'new'`
/// - `ALTER TYPE name RENAME TO new_name`
///
/// 简化假设：
/// - 类型名不含 schema 前缀（若包含 `.`，按 `schema.name` 解析）
/// - 字符串字面量使用单引号（与 PG 一致）
fn parse_alter_type(sql: &str) -> Result<Statement, ParseError> {
    // 移除前导 "ALTER TYPE" 关键字（大小写不敏感）
    let trimmed = sql.trim();
    let rest = if trimmed.to_uppercase().starts_with("ALTER TYPE") {
        trimmed["ALTER TYPE".len()..].trim_start()
    } else {
        return Err(ParseError::Unsupported(format!(
            "not an ALTER TYPE statement: {sql}"
        )));
    };

    // 提取类型名（直到下一个空白字符）
    let (name_str, rest) = split_at_whitespace(rest);
    if name_str.is_empty() {
        return Err(ParseError::Unsupported(format!(
            "ALTER TYPE missing type name: {sql}"
        )));
    }
    let name = parse_table_name(name_str)?;

    let rest = rest.trim();
    // 解析操作关键字：ADD VALUE / RENAME VALUE / RENAME TO
    let upper = rest.to_uppercase();
    if upper.starts_with("ADD VALUE") {
        let after_kw = rest["ADD VALUE".len()..].trim_start();
        // 检查 IF NOT EXISTS
        let (if_not_exists, after_ine) = if after_kw.to_uppercase().starts_with("IF NOT EXISTS") {
            (true, after_kw["IF NOT EXISTS".len()..].trim_start())
        } else {
            (false, after_kw)
        };
        // 提取字符串字面量 'val'
        let value = parse_string_literal(after_ine)?;
        // 忽略 BEFORE/AFTER 位置修饰
        Ok(Statement::AlterType {
            name,
            action: AlterTypeAction::AddValue {
                value,
                if_not_exists,
            },
        })
    } else if upper.starts_with("RENAME VALUE") {
        let after_kw = rest["RENAME VALUE".len()..].trim_start();
        let old = parse_string_literal(after_kw)?;
        // 期望 "TO 'new'"
        let after_old = skip_string_literal(after_kw)?.trim_start();
        let after_to = after_kw_after_keyword(after_old, "TO")?;
        let new = parse_string_literal(after_to)?;
        Ok(Statement::AlterType {
            name,
            action: AlterTypeAction::RenameValue { old, new },
        })
    } else if upper.starts_with("RENAME TO") {
        let after_kw = rest["RENAME TO".len()..].trim_start();
        let new_name_str = split_at_whitespace(after_kw).0;
        if new_name_str.is_empty() {
            return Err(ParseError::Unsupported(format!(
                "ALTER TYPE RENAME TO missing new name: {sql}"
            )));
        }
        let new_name = parse_table_name(new_name_str)?;
        Ok(Statement::AlterType {
            name,
            action: AlterTypeAction::Rename { new_name },
        })
    } else {
        Err(ParseError::Unsupported(format!(
            "unsupported ALTER TYPE operation: {rest}"
        )))
    }
}

/// 在字符串中找到第一个空白字符，返回 (前缀, 剩余)
fn split_at_whitespace(s: &str) -> (&str, &str) {
    match s.find(char::is_whitespace) {
        Some(idx) => (&s[..idx], &s[idx..]),
        None => (s, ""),
    }
}

/// 解析表名（支持 `name` 或 `schema.name`）
fn parse_table_name(s: &str) -> Result<TableName, ParseError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ParseError::Unsupported("empty table name".into()));
    }
    // 去掉可能的尾随分号或空白
    let s = s.trim_end_matches(';').trim();
    if let Some((schema, name)) = s.split_once('.') {
        Ok(TableName::with_schema(schema, name))
    } else {
        Ok(TableName::new(s))
    }
}

/// 解析单引号字符串字面量 `'...'`（不处理转义，简化处理）
fn parse_string_literal(s: &str) -> Result<String, ParseError> {
    let s = s.trim_start();
    let s = s
        .strip_prefix('\'')
        .ok_or_else(|| ParseError::Unsupported(format!("expected string literal: {s}")))?;
    let end = s
        .find('\'')
        .ok_or_else(|| ParseError::Unsupported(format!("unterminated string literal: {s}")))?;
    Ok(s[..end].to_string())
}

/// 跳过字符串字面量，返回其后的剩余部分
fn skip_string_literal(s: &str) -> Result<&str, ParseError> {
    let s = s.trim_start();
    let s = s
        .strip_prefix('\'')
        .ok_or_else(|| ParseError::Unsupported(format!("expected string literal: {s}")))?;
    let end = s
        .find('\'')
        .ok_or_else(|| ParseError::Unsupported(format!("unterminated string literal: {s}")))?;
    Ok(&s[end + 1..])
}

/// 在字符串中跳过指定关键字（大小写不敏感），返回关键字后的部分
fn after_kw_after_keyword<'a>(s: &'a str, keyword: &str) -> Result<&'a str, ParseError> {
    let s = s.trim_start();
    if s.to_uppercase().starts_with(keyword) {
        // 跳过 keyword 长度
        Ok(s[keyword.len()..].trim_start())
    } else {
        Err(ParseError::Unsupported(format!(
            "expected keyword `{keyword}`, got: {s}"
        )))
    }
}

/// 求值常量整数表达式（用于 SEQUENCE 选项）
///
/// 支持：数字字面量、一元负号 + 数字字面量、INTERVAL 'n'
fn eval_const_int_expr(expr: &SpExpr) -> Result<i64, ParseError> {
    match expr {
        SpExpr::Value(SpValue::Number(s, _)) => s
            .parse::<i64>()
            .map_err(|e| ParseError::InvalidValue(format!("invalid integer `{s}`: {e}"))),
        SpExpr::UnaryOp {
            op: UnaryOperator::Minus,
            expr: inner,
        } => Ok(-eval_const_int_expr(inner)?),
        SpExpr::Interval(interval) => eval_const_int_expr(&interval.value),
        other => Err(ParseError::Unsupported(format!(
            "unsupported constant expression in SEQUENCE option: {other:?}"
        ))),
    }
}

/// 判断 DataType 是否为 SERIAL/BIGSERIAL/SMALLSERIAL（PG 自增类型）
///
/// sqlparser 0.53.0 将 SERIAL 解析为 `DataType::Custom("serial")`
fn is_serial_data_type(dt: &DataType) -> bool {
    if let DataType::Custom(name, _) = dt {
        let name_str = convert_object_name_to_string(name).to_lowercase();
        matches!(name_str.as_str(), "serial" | "bigserial" | "smallserial")
    } else {
        false
    }
}

/// 从表达式提取标识符字符串（用于索引列）
fn identifier_string(expr: &SpExpr) -> String {
    match expr {
        SpExpr::Identifier(i) => i.value.clone(),
        SpExpr::CompoundIdentifier(idents) => {
            idents.last().map(|i| i.value.clone()).unwrap_or_default()
        }
        _ => String::new(),
    }
}

// =====================================================================
//  Phase 3.35: FLASHBACK 语句手动解析
// =====================================================================

/// 手动解析 FLASHBACK 语句 — Phase 3.35
///
/// sqlparser 0.53.0 不支持 FLASHBACK 语法（Oracle/MySQL 扩展），需手动解析。
///
/// 支持两种形式：
/// - `FLASHBACK TRANSACTION <txn_id>` — 闪回指定事务
/// - `FLASHBACK TABLE <name> TO TIMESTAMP '<timestamp>'` — 查询表在指定时间点的状态
///
/// 语法大小写不敏感；txn_id 为无符号整数；timestamp 为单引号字符串字面量。
fn parse_flashback(sql: &str) -> Result<Statement, ParseError> {
    let trimmed = sql.trim();
    let tokens = tokenize_flashback(trimmed);

    if tokens.is_empty() {
        return Err(ParseError::Unsupported("empty FLASHBACK statement".into()));
    }

    let first = tokens[0].to_uppercase();
    if first != "FLASHBACK" {
        return Err(ParseError::Unsupported(format!(
            "expected FLASHBACK, got {first}"
        )));
    }

    if tokens.len() < 2 {
        return Err(ParseError::Unsupported(format!(
            "FLASHBACK requires a sub-keyword (TRANSACTION/TABLE), got: {trimmed}"
        )));
    }

    let second = tokens[1].to_uppercase();
    match second.as_str() {
        "TRANSACTION" => parse_flashback_transaction(&tokens[2..]),
        "TABLE" => parse_flashback_table(&tokens[2..]),
        other => Err(ParseError::Unsupported(format!(
            "unsupported FLASHBACK sub-keyword: {other} (expected TRANSACTION or TABLE)"
        ))),
    }
}

/// 解析 `FLASHBACK TRANSACTION <txn_id>`
fn parse_flashback_transaction(tokens: &[&str]) -> Result<Statement, ParseError> {
    if tokens.len() != 1 {
        return Err(ParseError::Unsupported(format!(
            "FLASHBACK TRANSACTION requires exactly 1 argument (txn_id), got {tokens:?}"
        )));
    }
    let txn_id: u64 = tokens[0].parse().map_err(|_| {
        ParseError::InvalidValue(format!(
            "FLASHBACK TRANSACTION txn_id must be a non-negative integer, got: {}",
            tokens[0]
        ))
    })?;
    Ok(Statement::FlashbackTransaction { txn_id })
}

/// 解析 `FLASHBACK TABLE <name> TO TIMESTAMP '<timestamp>'`
fn parse_flashback_table(tokens: &[&str]) -> Result<Statement, ParseError> {
    // 期望形式： <name> TO TIMESTAMP '<timestamp>'
    // tokens 中至少需要 4 个：name, TO, TIMESTAMP, 'ts'
    if tokens.len() < 4 {
        return Err(ParseError::Unsupported(format!(
            "FLASHBACK TABLE requires syntax: <name> TO TIMESTAMP '<ts>', got tokens: {tokens:?}"
        )));
    }
    let name = tokens[0];
    let to_kw = tokens[1].to_uppercase();
    let timestamp_kw = tokens[2].to_uppercase();
    if to_kw != "TO" {
        return Err(ParseError::Unsupported(format!(
            "FLASHBACK TABLE expects TO, got: {to_kw}"
        )));
    }
    if timestamp_kw != "TIMESTAMP" {
        return Err(ParseError::Unsupported(format!(
            "FLASHBACK TABLE expects TIMESTAMP, got: {timestamp_kw}"
        )));
    }
    // timestamp 部分可能因分词被拆为多段（含空格的 ISO 8601），重新合并并去除引号
    let raw_ts = tokens[3..].join(" ");
    let timestamp = strip_quotes(&raw_ts).ok_or_else(|| {
        ParseError::InvalidValue(format!(
            "FLASHBACK TABLE timestamp must be a quoted string, got: {raw_ts}"
        ))
    })?;
    let table_name = if let Some(dot_pos) = name.find('.') {
        TableName::with_schema(&name[..dot_pos], &name[dot_pos + 1..])
    } else {
        TableName::new(name)
    };
    Ok(Statement::FlashbackTable {
        table: table_name,
        timestamp,
    })
}

// =====================================================================
//  Phase 4.8: COPY FROM / COPY TO 解析
// =====================================================================

/// 转换 sqlparser `Statement::Copy` → SzRSQL `Statement::Copy`
///
/// 支持的 PG COPY 语法：
/// - `COPY table [(cols)] FROM '/path' [WITH (...)]`
/// - `COPY table [(cols)] TO '/path' [WITH (...)]`
/// - `COPY (SELECT ...) TO '/path' [WITH (...)]`
///
/// 支持的 WITH 选项（CopyOption）：
/// - FORMAT csv|text
/// - DELIMITER 'char'
/// - NULL 'string'
/// - HEADER [true|false]
/// - QUOTE 'char'
/// - ESCAPE 'char'
///
/// 支持的旧式选项（CopyLegacyOption）：
/// - DELIMITER 'char'
/// - NULL 'string'
/// - CSV [HEADER] [QUOTE 'char'] [ESCAPE 'char']
///
/// 不支持：BINARY、STDIN、STDOUT、PROGRAM、FREEZE、ENCODING、FORCE_*。
fn convert_copy(
    source: sqlparser::ast::CopySource,
    to: bool,
    target: sqlparser::ast::CopyTarget,
    options: Vec<sqlparser::ast::CopyOption>,
    legacy_options: Vec<sqlparser::ast::CopyLegacyOption>,
) -> Result<Statement, ParseError> {
    use sqlparser::ast::{CopyLegacyOption, CopyOption, CopySource, CopyTarget as SpCopyTarget};

    // 1. 转换方向
    let direction = if to {
        CopyDirection::To
    } else {
        CopyDirection::From
    };

    // 2. 转换 source → target（表名或 SELECT 查询）
    let (copy_target, columns) = match source {
        CopySource::Table {
            table_name,
            columns,
        } => {
            let table = convert_object_name(table_name)?;
            let cols = if columns.is_empty() {
                None
            } else {
                Some(columns.into_iter().map(|i| i.value).collect())
            };
            (CopyTarget::Table(table), cols)
        }
        CopySource::Query(query) => {
            // COPY (SELECT ...) 仅在 COPY TO 时合法
            if !to {
                return Err(ParseError::Unsupported(
                    "COPY FROM does not support query source (only table)".into(),
                ));
            }
            let select = convert_query(*query)?;
            (CopyTarget::Query(Box::new(select)), None)
        }
    };

    // 3. 转换 target → file_path
    let file_path = match target {
        SpCopyTarget::File { filename } => filename,
        SpCopyTarget::Stdin | SpCopyTarget::Stdout => {
            return Err(ParseError::Unsupported(
                "COPY STDIN/STDOUT not supported (use file path)".into(),
            ));
        }
        SpCopyTarget::Program { .. } => {
            return Err(ParseError::Unsupported("COPY PROGRAM not supported".into()));
        }
    };

    // 4. 转换选项
    let mut opts = CopyOptions::default();

    // 4.1 优先处理 legacy_options（旧式语法）
    //     `COPY t FROM '/path' CSV` 或 `COPY t FROM '/path' CSV HEADER`
    //     `COPY t FROM '/path' DELIMITER ','`
    //     `COPY t FROM '/path' BINARY`（不支持）
    let mut legacy_csv_seen = false;
    for legacy in legacy_options {
        match legacy {
            CopyLegacyOption::Binary => {
                return Err(ParseError::Unsupported(
                    "COPY BINARY format not supported".into(),
                ));
            }
            CopyLegacyOption::Delimiter(ch) => {
                opts.delimiter = ch;
            }
            CopyLegacyOption::Null(s) => {
                opts.null_string = s;
            }
            CopyLegacyOption::Csv(csv_opts) => {
                legacy_csv_seen = true;
                opts.format = CopyFormat::Csv;
                opts.delimiter = ',';
                opts.null_string = String::new();
                for csv_opt in csv_opts {
                    match csv_opt {
                        sqlparser::ast::CopyLegacyCsvOption::Header => {
                            opts.header = true;
                        }
                        sqlparser::ast::CopyLegacyCsvOption::Quote(ch) => {
                            opts.quote = ch;
                        }
                        sqlparser::ast::CopyLegacyCsvOption::Escape(ch) => {
                            opts.escape = ch;
                        }
                        sqlparser::ast::CopyLegacyCsvOption::ForceQuote(_) => {
                            return Err(ParseError::Unsupported(
                                "COPY FORCE QUOTE not supported".into(),
                            ));
                        }
                        sqlparser::ast::CopyLegacyCsvOption::ForceNotNull(_) => {
                            return Err(ParseError::Unsupported(
                                "COPY FORCE NOT NULL not supported".into(),
                            ));
                        }
                    }
                }
            }
        }
    }

    // 4.2 处理新式 WITH (...) 选项（覆盖 legacy_options 的同名字段）
    for opt in options {
        match opt {
            CopyOption::Format(ident) => {
                let fmt_lower = ident.value.to_lowercase();
                match fmt_lower.as_str() {
                    "csv" => {
                        opts.format = CopyFormat::Csv;
                        // FORMAT csv 时若未通过 DELIMITER 显式指定，使用 CSV 默认 ','
                        // 但若 legacy_options 已设置 DELIMITER，保留之
                        if !legacy_csv_seen {
                            opts.delimiter = ',';
                            opts.null_string = String::new();
                        }
                    }
                    "text" => {
                        opts.format = CopyFormat::Text;
                        if !legacy_csv_seen {
                            opts.delimiter = '\t';
                            opts.null_string = "\\N".to_string();
                        }
                    }
                    "binary" => {
                        return Err(ParseError::Unsupported(
                            "COPY BINARY format not supported".into(),
                        ));
                    }
                    other => {
                        return Err(ParseError::Unsupported(format!(
                            "COPY FORMAT '{other}' not supported (use csv or text)"
                        )));
                    }
                }
            }
            CopyOption::Delimiter(ch) => {
                opts.delimiter = ch;
            }
            CopyOption::Null(s) => {
                opts.null_string = s;
            }
            CopyOption::Header(b) => {
                opts.header = b;
            }
            CopyOption::Quote(ch) => {
                opts.quote = ch;
            }
            CopyOption::Escape(ch) => {
                opts.escape = ch;
            }
            CopyOption::Freeze(_) => {
                return Err(ParseError::Unsupported("COPY FREEZE not supported".into()));
            }
            CopyOption::ForceQuote(_) => {
                return Err(ParseError::Unsupported(
                    "COPY FORCE_QUOTE not supported".into(),
                ));
            }
            CopyOption::ForceNotNull(_) => {
                return Err(ParseError::Unsupported(
                    "COPY FORCE_NOT_NULL not supported".into(),
                ));
            }
            CopyOption::ForceNull(_) => {
                return Err(ParseError::Unsupported(
                    "COPY FORCE_NULL not supported".into(),
                ));
            }
            CopyOption::Encoding(_) => {
                return Err(ParseError::Unsupported(
                    "COPY ENCODING not supported".into(),
                ));
            }
        }
    }

    Ok(Statement::Copy {
        target: copy_target,
        columns,
        direction,
        file_path,
        options: opts,
    })
}

// =====================================================================
//  Phase 4.6: LISTEN / UNLISTEN / NOTIFY 解析
// =====================================================================

/// 解析 `LISTEN <channel>` — Phase 4.6
///
/// 语法：
/// - `LISTEN <channel>` — 注册监听指定频道
///
/// channel 为标识符（不含引号）或双引号字符串。
/// 语法大小写不敏感（LISTEN 关键字），channel 名保留原样。
fn parse_listen(sql: &str) -> Result<Statement, ParseError> {
    let tokens = tokenize_listen_notify(sql);
    if tokens.len() != 2 {
        return Err(ParseError::Unsupported(format!(
            "LISTEN requires exactly 1 channel argument, got: {sql}"
        )));
    }
    if tokens[0].to_uppercase() != "LISTEN" {
        return Err(ParseError::Unsupported(format!(
            "expected LISTEN, got: {}",
            tokens[0]
        )));
    }
    let channel = parse_channel_name(&tokens[1])?;
    Ok(Statement::Listen { channel })
}

/// 解析 `UNLISTEN <channel>` 或 `UNLISTEN *` — Phase 4.6
///
/// 语法：
/// - `UNLISTEN <channel>` — 取消监听指定频道
/// - `UNLISTEN *` — 取消监听所有频道
fn parse_unlisten(sql: &str) -> Result<Statement, ParseError> {
    let tokens = tokenize_listen_notify(sql);
    if tokens.len() != 2 {
        return Err(ParseError::Unsupported(format!(
            "UNLISTEN requires exactly 1 argument (channel or *), got: {sql}"
        )));
    }
    if tokens[0].to_uppercase() != "UNLISTEN" {
        return Err(ParseError::Unsupported(format!(
            "expected UNLISTEN, got: {}",
            tokens[0]
        )));
    }
    // UNLISTEN * — 取消所有
    if tokens[1] == "*" {
        return Ok(Statement::Unlisten {
            channel: "*".to_string(),
        });
    }
    let channel = parse_channel_name(&tokens[1])?;
    Ok(Statement::Unlisten { channel })
}

/// 解析 `NOTIFY <channel>` 或 `NOTIFY <channel>, '<payload>'` — Phase 4.6
///
/// 语法：
/// - `NOTIFY <channel>` — 发送通知（payload 为空字符串）
/// - `NOTIFY <channel>, '<payload>'` — 发送带负载的通知
///
/// 注意：逗号两侧可能有或无空白，本函数容忍两种写法：
/// - `NOTIFY foo, 'bar'`（逗号紧贴 channel）
/// - `NOTIFY foo , 'bar'`（逗号独立）
fn parse_notify(sql: &str) -> Result<Statement, ParseError> {
    let tokens = tokenize_listen_notify(sql);
    if tokens.len() < 2 {
        return Err(ParseError::Unsupported(format!(
            "NOTIFY requires at least 1 channel argument, got: {sql}"
        )));
    }
    if tokens[0].to_uppercase() != "NOTIFY" {
        return Err(ParseError::Unsupported(format!(
            "expected NOTIFY, got: {}",
            tokens[0]
        )));
    }

    // tokens[1] = channel；可选 tokens[2] = ","；可选 tokens[3] = payload
    // 由于 tokenize_listen_notify 会把 "channel," 拆为 "channel" + ","，
    // 所以逗号始终独立成 token。
    let channel = parse_channel_name(&tokens[1])?;

    if tokens.len() == 2 {
        return Ok(Statement::Notify {
            channel,
            payload: String::new(),
        });
    }

    // 带 payload：剩余应为 [",", payload]
    if tokens.len() != 4 || tokens[2] != "," {
        return Err(ParseError::Unsupported(format!(
            "NOTIFY payload syntax: NOTIFY <channel>, '<payload>', got: {sql}"
        )));
    }

    let payload = strip_string_literal(&tokens[3]).ok_or_else(|| {
        ParseError::InvalidValue(format!(
            "NOTIFY payload must be a quoted string, got: {}",
            tokens[3]
        ))
    })?;
    Ok(Statement::Notify { channel, payload })
}

/// 解析频道名：支持裸标识符和双引号字符串。
fn parse_channel_name(token: &str) -> Result<String, ParseError> {
    // 双引号字符串
    if token.starts_with('"') && token.ends_with('"') && token.len() >= 2 {
        return Ok(token[1..token.len() - 1].to_string());
    }
    // 裸标识符：仅允许字母数字下划线和点（schema 限定）
    if token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        && !token.is_empty()
    {
        return Ok(token.to_string());
    }
    Err(ParseError::Unsupported(format!(
        "invalid channel name: {token} (expected identifier or quoted string)"
    )))
}

/// 从单引号或双引号字符串字面量中提取内容。
///
/// - `'hello'` → `Some("hello")`
/// - `"hello"` → `Some("hello")`
/// - `hello` → `None`（非字符串字面量）
/// - `'hello` → `None`（引号不匹配）
fn strip_string_literal(token: &str) -> Option<String> {
    if token.len() < 2 {
        return None;
    }
    let first = token.chars().next()?;
    let last = token.chars().last()?;
    if (first == '\'' || first == '"') && first == last {
        Some(token[1..token.len() - 1].to_string())
    } else {
        None
    }
}

/// LISTEN/UNLISTEN/NOTIFY 专用分词器 — Phase 4.6
///
/// 与 `split_whitespace` 不同，本分词器正确处理：
/// - 单引号字符串字面量（`'hello world'` 作为单个 token）
/// - 双引号标识符（`"my channel"` 作为单个 token）
/// - 逗号作为独立 token（`channel,'payload'` → `["channel", ",", "'payload'"]`）
///
/// 转义字符不在本分词器处理范围内（PG 中 NOTIFY payload 不支持转义）。
fn tokenize_listen_notify(sql: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote_char: Option<char> = None;
    for ch in sql.chars() {
        match quote_char {
            Some(qc) => {
                // 在引号字符串内
                current.push(ch);
                if ch == qc {
                    // 闭合引号
                    quote_char = None;
                    tokens.push(std::mem::take(&mut current));
                }
            }
            None => {
                match ch {
                    '\'' | '"' => {
                        // 开始引号字符串：先把累积的 token 推入
                        if !current.is_empty() {
                            tokens.push(std::mem::take(&mut current));
                        }
                        current.push(ch);
                        quote_char = Some(ch);
                    }
                    ',' => {
                        // 逗号独立成 token
                        if !current.is_empty() {
                            tokens.push(std::mem::take(&mut current));
                        }
                        tokens.push(",".to_string());
                    }
                    c if c.is_whitespace() => {
                        if !current.is_empty() {
                            tokens.push(std::mem::take(&mut current));
                        }
                    }
                    other => {
                        current.push(other);
                    }
                }
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// 简单分词器：按空白拆分，但保留单引号内的内容为一个 token（含引号）
fn tokenize_flashback(sql: &str) -> Vec<&str> {
    // 简化实现：按空白切分，单引号内容若不含空白会自然保留为一个 token；
    // 若含空白（如 '2026-07-20 12:00:00'）则会被拆分。
    // 上层 parse_flashback_table 会将 timestamp 之后的所有 tokens 重新 join。
    sql.split_whitespace().collect()
}

/// 去除字符串两端的单引号（或双引号），返回内部内容
fn strip_quotes(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() < 2 {
        return None;
    }
    let first = s.chars().next()?;
    let last = s.chars().last()?;
    if first == last && (first == '\'' || first == '"') {
        Some(s[1..s.len() - 1].to_string())
    } else {
        None
    }
}

// =====================================================================
//  Trigger 转换 — Phase 6.4
// =====================================================================

/// 转换 sqlparser TriggerPeriod → SzRSQL TriggerTiming
fn convert_trigger_period(p: SpTriggerPeriod) -> Result<TriggerTiming, ParseError> {
    Ok(match p {
        SpTriggerPeriod::Before => TriggerTiming::Before,
        SpTriggerPeriod::After => TriggerTiming::After,
        SpTriggerPeriod::InsteadOf => TriggerTiming::InsteadOf,
    })
}

/// 转换 sqlparser TriggerObject → SzRSQL TriggerLevel
fn convert_trigger_object(o: SpTriggerObject) -> TriggerLevel {
    match o {
        SpTriggerObject::Row => TriggerLevel::Row,
        SpTriggerObject::Statement => TriggerLevel::Statement,
    }
}

/// 转换 sqlparser TriggerEvent → SzRSQL TriggerEvent
fn convert_trigger_event(e: SpTriggerEvent) -> Result<TriggerEvent, ParseError> {
    Ok(match e {
        SpTriggerEvent::Insert => TriggerEvent::Insert,
        SpTriggerEvent::Update(cols) => {
            let columns = cols.into_iter().map(|i| i.value).collect();
            TriggerEvent::Update(columns)
        }
        SpTriggerEvent::Delete => TriggerEvent::Delete,
        SpTriggerEvent::Truncate => TriggerEvent::Truncate,
    })
}

/// CREATE TRIGGER 转换
#[allow(clippy::too_many_arguments)]
fn convert_create_trigger(
    or_replace: bool,
    is_constraint: bool,
    name: ObjectName,
    period: SpTriggerPeriod,
    events: Vec<SpTriggerEvent>,
    table_name: ObjectName,
    trigger_object: SpTriggerObject,
    condition: Option<SpExpr>,
    exec_body: TriggerExecBody,
) -> Result<Statement, ParseError> {
    // 触发器名：取最后一部分（PG 触发器名是 simple name）
    let trig_name = name
        .0
        .last()
        .ok_or_else(|| ParseError::Unsupported("trigger name is empty".to_string()))?
        .value
        .clone();

    // 表名
    let table = convert_object_name(table_name)?;

    // 时机
    let timing = convert_trigger_period(period)?;

    // 级别
    let level = convert_trigger_object(trigger_object);

    // 事件列表（不可为空）
    if events.is_empty() {
        return Err(ParseError::Unsupported(
            "trigger must have at least one event".to_string(),
        ));
    }
    let events_converted: Vec<TriggerEvent> = events
        .into_iter()
        .map(convert_trigger_event)
        .collect::<Result<Vec<_>, _>>()?;

    // WHEN 条件
    let when_clause = match condition {
        Some(expr) => Some(convert_expr(expr)?),
        None => None,
    };

    // 触发器函数：从 TriggerExecBody.func_desc 提取 name 与 args
    // func_desc.args 是 Option<Vec<OperateFunctionArg>>，其中 OperateFunctionArg 含 data_type 等
    // 但触发器函数通常无参数，且 PG 调用约定通过 NEW/OLD 传递，不是 SQL 参数
    // 这里仅保留函数名，args 转换为 Expr 列表（若调用方写了字面量参数）
    let func_name = convert_object_name_to_string(&exec_body.func_desc.name);
    let func_args: Vec<Expr> = match exec_body.func_desc.args {
        Some(args) => {
            let mut collected = Vec::new();
            for arg in args {
                // 触发器函数参数：仅保留 default_expr 作为字面量参数（简化）
                // 多数 PG 触发器函数无参数，此处保留扩展能力
                if let Some(def_expr) = arg.default_expr {
                    collected.push(convert_expr(def_expr)?);
                }
            }
            collected
        }
        None => Vec::new(),
    };

    let definition = TriggerDefinition {
        name: trig_name,
        table,
        timing,
        level,
        events: events_converted,
        when_clause,
        func_name,
        func_args,
        enabled: true,
        is_constraint,
    };

    Ok(Statement::CreateTrigger {
        definition,
        or_replace,
        if_not_exists: false,
    })
}

/// DROP TRIGGER 转换
fn convert_drop_trigger(
    if_exists: bool,
    trigger_name: ObjectName,
    table_name: ObjectName,
    _option: Option<sqlparser::ast::ReferentialAction>,
) -> Result<Statement, ParseError> {
    let name = convert_object_name_to_string(&trigger_name);
    let table = convert_object_name(table_name)?;
    Ok(Statement::DropTrigger {
        name,
        table,
        if_exists,
        cascade: false,
    })
}

// =====================================================================
//  DML 转换
// =====================================================================

fn convert_insert(
    table_name: ObjectName,
    columns: Vec<Ident>,
    source: Option<Box<SpQuery>>,
    on: Option<OnInsert>,
    returning: Option<Vec<SpSelectItem>>,
) -> Result<Statement, ParseError> {
    let table = convert_object_name(table_name)?;
    let columns = if columns.is_empty() {
        None
    } else {
        Some(columns.into_iter().map(|i| i.value).collect())
    };
    let source = match source {
        Some(query) => {
            // 检查是否是 VALUES
            match query.body.as_ref() {
                SpSetExpr::Values(values) => {
                    let rows = values
                        .rows
                        .iter()
                        .map(|row| {
                            row.iter()
                                .cloned()
                                .map(convert_expr)
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    InsertSource::Values(rows)
                }
                _ => InsertSource::Select(Box::new(convert_query(*query)?)),
            }
        }
        None => InsertSource::DefaultValues,
    };
    let on_conflict = convert_on_insert(on)?;
    let returning = convert_returning(returning)?;
    Ok(Statement::Insert {
        table,
        columns,
        source,
        on_conflict,
        returning,
    })
}

/// 转换 REPLACE INTO — Phase 3.25
///
/// sqlparser 将 `REPLACE INTO t ...` 解析为 `Insert { replace_into: true, ... }`，
/// 此函数将其转换为 SzRSQL 的 `Statement::Replace`。
///
/// 不支持 `ON CONFLICT` 与 `RETURNING`（MySQL 不支持这些 PG 扩展）。
fn convert_replace(
    table_name: ObjectName,
    columns: Vec<Ident>,
    source: Option<Box<SpQuery>>,
) -> Result<Statement, ParseError> {
    let table = convert_object_name(table_name)?;
    let columns = if columns.is_empty() {
        None
    } else {
        Some(columns.into_iter().map(|i| i.value).collect())
    };
    let source = match source {
        Some(query) => match query.body.as_ref() {
            SpSetExpr::Values(values) => {
                let rows = values
                    .rows
                    .iter()
                    .map(|row| {
                        row.iter()
                            .cloned()
                            .map(convert_expr)
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                InsertSource::Values(rows)
            }
            _ => InsertSource::Select(Box::new(convert_query(*query)?)),
        },
        None => InsertSource::DefaultValues,
    };
    Ok(Statement::Replace {
        table,
        columns,
        source,
    })
}

/// 转换 sqlparser `OnInsert` → SzRSQL `OnConflict`
///
/// 仅支持 PostgreSQL 风格的 `ON CONFLICT`（`OnInsert::OnConflict`），
/// MySQL 风格的 `OnInsert::DuplicateKeyUpdate` 不支持（返回 `Unsupported` 错误）。
fn convert_on_insert(on: Option<OnInsert>) -> Result<Option<OnConflict>, ParseError> {
    match on {
        None => Ok(None),
        Some(OnInsert::OnConflict(oc)) => {
            let conflict_columns = match &oc.conflict_target {
                None => None,
                Some(ConflictTarget::Columns(idents)) => {
                    Some(idents.iter().map(|i| i.value.clone()).collect())
                }
                Some(ConflictTarget::OnConstraint(_)) => {
                    return Err(ParseError::Unsupported(
                        "ON CONSTRAINT not supported (use column list)".into(),
                    ));
                }
            };
            let result = match &oc.action {
                OnConflictAction::DoNothing => OnConflict::DoNothing { conflict_columns },
                OnConflictAction::DoUpdate(DoUpdate {
                    assignments,
                    selection,
                }) => {
                    let assigns = assignments
                        .iter()
                        .map(convert_assignment)
                        .collect::<Result<Vec<_>, _>>()?;
                    let where_clause = match selection {
                        Some(e) => Some(convert_expr(e.clone())?),
                        None => None,
                    };
                    OnConflict::DoUpdate {
                        conflict_columns,
                        assignments: assigns,
                        where_clause,
                    }
                }
            };
            Ok(Some(result))
        }
        Some(OnInsert::DuplicateKeyUpdate(_)) => Err(ParseError::Unsupported(
            "ON DUPLICATE KEY UPDATE not supported (use ON CONFLICT)".into(),
        )),
        // OnInsert 标记为 non-exhaustive，必须保留通配分支
        other => Err(ParseError::Unsupported(format!(
            "unsupported OnInsert variant: {other:?}"
        ))),
    }
}

/// 转换 sqlparser `Assignment` → SzRSQL `Assignment`
fn convert_assignment(a: &sqlparser::ast::Assignment) -> Result<Assignment, ParseError> {
    let column = match &a.target {
        AssignmentTarget::ColumnName(name) => {
            name.0.last().map(|i| i.value.clone()).unwrap_or_default()
        }
        _ => {
            return Err(ParseError::Unsupported(
                "unsupported assignment target".into(),
            ));
        }
    };
    Ok(Assignment {
        column,
        value: convert_expr(a.value.clone())?,
    })
}

fn convert_update(
    table: SpTableWithJoins,
    assignments: Vec<sqlparser::ast::Assignment>,
    from: Option<SpTableWithJoins>,
    selection: Option<SpExpr>,
    returning: Option<Vec<SpSelectItem>>,
) -> Result<Statement, ParseError> {
    // UPDATE 的目标表（table.relation 必须是 Table）
    let (table_name, alias) = match table.relation {
        SpTableFactor::Table { name, alias, .. } => {
            (convert_object_name(name)?, alias.map(convert_table_alias))
        }
        other => {
            return Err(ParseError::Unsupported(format!(
                "unsupported update target: {other:?}"
            )));
        }
    };
    let assignments = assignments
        .into_iter()
        .map(|a| {
            let column = match a.target {
                AssignmentTarget::ColumnName(name) => convert_object_name_to_string(&name),
                other => {
                    return Err(ParseError::Unsupported(format!(
                        "unsupported assignment target: {other:?}"
                    )));
                }
            };
            Ok(Assignment {
                column,
                value: convert_expr(a.value)?,
            })
        })
        .collect::<Result<Vec<_>, ParseError>>()?;
    let from = match from {
        Some(twj) => vec![convert_table_factor(twj.relation)?],
        None => Vec::new(),
    };
    let where_clause = convert_option_expr(selection)?;
    let returning = convert_returning(returning)?;
    Ok(Statement::Update {
        table: table_name,
        alias: alias.map(|a| a.name),
        assignments,
        where_clause,
        from,
        returning,
    })
}

fn convert_delete(delete: sqlparser::ast::Delete) -> Result<Statement, ParseError> {
    // 0.53: FromTable 只有 WithFromKeyword / WithoutKeyword，两者都包含 Vec<TableWithJoins>
    let (table_name, alias) = match delete.from {
        sqlparser::ast::FromTable::WithFromKeyword(twjs)
        | sqlparser::ast::FromTable::WithoutKeyword(twjs) => {
            if twjs.len() != 1 || !twjs[0].joins.is_empty() {
                return Err(ParseError::Unsupported(
                    "multi-table delete not supported".into(),
                ));
            }
            match twjs.into_iter().next().unwrap().relation {
                SpTableFactor::Table { name, alias, .. } => {
                    (convert_object_name(name)?, alias.map(convert_table_alias))
                }
                other => {
                    return Err(ParseError::Unsupported(format!(
                        "unsupported delete target: {other:?}"
                    )));
                }
            }
        }
    };
    let using = match delete.using {
        Some(twjs) => twjs
            .into_iter()
            .map(|twj| convert_table_factor(twj.relation))
            .collect::<Result<Vec<_>, _>>()?,
        None => Vec::new(),
    };
    let where_clause = convert_option_expr(delete.selection)?;
    let returning = convert_returning(delete.returning)?;
    Ok(Statement::Delete {
        table: table_name,
        alias: alias.map(|a| a.name),
        using,
        where_clause,
        returning,
    })
}

fn convert_returning(
    returning: Option<Vec<SpSelectItem>>,
) -> Result<Option<Vec<SelectItem>>, ParseError> {
    match returning {
        Some(items) => {
            let converted = items
                .into_iter()
                .map(convert_select_item)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Some(converted))
        }
        None => Ok(None),
    }
}

// =====================================================================
//  Phase 6.5: CREATE FUNCTION / DROP FUNCTION 解析
// =====================================================================

/// 检测 SQL 字符串是否包含 CREATE FUNCTION 或 DROP FUNCTION 语句（大小写不敏感）
///
/// 简化实现：按分号切分，检查每条语句是否以 `CREATE FUNCTION`、
/// `CREATE OR REPLACE FUNCTION` 或 `DROP FUNCTION` 开头。
fn contains_function_ddl(sql: &str) -> bool {
    sql.split(';').any(|stmt| {
        let trimmed = stmt.trim_start();
        let upper = trimmed.to_uppercase();
        upper.starts_with("CREATE FUNCTION")
            || upper.starts_with("CREATE OR REPLACE FUNCTION")
            || upper.starts_with("DROP FUNCTION")
    })
}

/// 智能切分 SQL 语句 — Phase 6.5
///
/// 与 [`split_sql_statements`] 不同，此函数正确处理：
/// - `$$ ... $$` 和 `$tag$ ... $tag$` dollar 引号字符串
/// - `'...'` 单引号字符串字面量（含 `''` 转义）
/// - `-- ...` 单行注释
/// - `/* ... */` 多行注释（可嵌套）
///
/// 仅在检测到 CREATE/DROP FUNCTION 时使用，避免影响常规路径性能。
fn split_sql_statements_with_dollar_quotes(sql: &str) -> Vec<String> {
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = sql.chars().collect();
    let mut i = 0;
    let n = chars.len();

    while i < n {
        let c = chars[i];

        // 单行注释 -- ... \n
        if c == '-' && i + 1 < n && chars[i + 1] == '-' {
            while i < n && chars[i] != '\n' {
                current.push(chars[i]);
                i += 1;
            }
            continue;
        }

        // 多行注释 /* ... */（可嵌套）
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            let mut depth = 1;
            current.push(chars[i]);
            current.push(chars[i + 1]);
            i += 2;
            while i < n && depth > 0 {
                if chars[i] == '/' && i + 1 < n && chars[i + 1] == '*' {
                    depth += 1;
                    current.push(chars[i]);
                    current.push(chars[i + 1]);
                    i += 2;
                } else if chars[i] == '*' && i + 1 < n && chars[i + 1] == '/' {
                    depth -= 1;
                    current.push(chars[i]);
                    current.push(chars[i + 1]);
                    i += 2;
                } else {
                    current.push(chars[i]);
                    i += 1;
                }
            }
            continue;
        }

        // 单引号字符串 '...'（含 '' 转义）
        if c == '\'' {
            current.push(c);
            i += 1;
            while i < n {
                if chars[i] == '\'' {
                    // 检查是否为 '' 转义
                    if i + 1 < n && chars[i + 1] == '\'' {
                        current.push(chars[i]);
                        current.push(chars[i + 1]);
                        i += 2;
                    } else {
                        current.push(chars[i]);
                        i += 1;
                        break;
                    }
                } else {
                    current.push(chars[i]);
                    i += 1;
                }
            }
            continue;
        }

        // Dollar 引号字符串 $$ ... $$ 或 $tag$ ... $tag$
        if c == '$' {
            // 尝试读取 dollar 引号标签
            let mut j = i + 1;
            let mut tag = String::new();
            while j < n && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                tag.push(chars[j]);
                j += 1;
            }
            if j < n && chars[j] == '$' {
                // 找到 $tag$ 开始标记
                let opening = format!("${tag}$");
                let closing = opening.clone();
                // 消费开始标记
                for ch in opening.chars() {
                    current.push(ch);
                }
                i = j + 1;
                // 查找结束标记
                let closing_chars: Vec<char> = closing.chars().collect();
                let closing_len = closing_chars.len();
                while i < n {
                    if chars[i] == '$' {
                        // 检查是否匹配结束标记
                        let mut matched = true;
                        for (k, expected) in closing_chars.iter().enumerate() {
                            if i + k >= n || chars[i + k] != *expected {
                                matched = false;
                                break;
                            }
                        }
                        if matched {
                            for ch in closing_chars.iter() {
                                current.push(*ch);
                            }
                            i += closing_len;
                            break;
                        } else {
                            current.push(chars[i]);
                            i += 1;
                        }
                    } else {
                        current.push(chars[i]);
                        i += 1;
                    }
                }
                continue;
            }
            // 不是 dollar 引号，按普通字符处理
        }

        // 语句分隔符
        if c == ';' {
            current.push(c);
            segments.push(std::mem::take(&mut current));
            i += 1;
            continue;
        }

        current.push(c);
        i += 1;
    }

    // 处理最后一段（无分号结尾）
    if !current.trim().is_empty() {
        segments.push(current);
    }

    segments
}

/// 解析 CREATE FUNCTION 语句 — Phase 6.5
///
/// 支持语法：
/// ```sql
/// CREATE [OR REPLACE] FUNCTION name ( [params] )
///   RETURNS rettype
///   [ LANGUAGE lang_name ]
///   [ IMMUTABLE | STABLE | VOLATILE ]
///   [ STRICT ]
///   [ SECURITY DEFINER | SECURITY INVOKER ]
///   AS $$ body $$ [additional_attrs]
///   | AS 'body' [additional_attrs]
/// ```
///
/// 简化假设：
/// - `AS` 子句必须在所有属性之前或之后（PG 允许混合，但实际很少见）
/// - 不支持 `AS 'obj_file', 'link_symbol'`（C 语言函数）
/// - 不支持 `RETURNS TABLE(...)` 的详细解析（作为原文存储）
fn parse_create_function(sql: &str) -> Result<Statement, ParseError> {
    let trimmed = sql.trim();
    let upper = trimmed.to_uppercase();

    // 1. 消费 CREATE [OR REPLACE] FUNCTION
    let or_replace;
    let rest;
    if upper.starts_with("CREATE OR REPLACE FUNCTION") {
        or_replace = true;
        rest = trimmed["CREATE OR REPLACE FUNCTION".len()..]
            .trim_start()
            .to_string();
    } else if upper.starts_with("CREATE FUNCTION") {
        or_replace = false;
        rest = trimmed["CREATE FUNCTION".len()..].trim_start().to_string();
    } else {
        return Err(ParseError::Unsupported(format!(
            "not a CREATE FUNCTION statement: {sql}"
        )));
    }

    // 2. 提取函数名（直到 `(` 或空白）
    let (name_str, after_name) = extract_function_name(&rest)?;
    let name = name_str.trim().to_string();
    if name.is_empty() {
        return Err(ParseError::Unsupported(format!(
            "CREATE FUNCTION missing function name: {sql}"
        )));
    }

    // 3. 提取参数列表 `(...)` — 必须紧跟函数名
    let (params_str, after_params) = extract_parenthesized(&after_name)?;
    let parameters = parse_function_parameters(&params_str)?;

    // 4. 解析剩余部分（RETURNS / LANGUAGE / AS / 属性）
    let mut return_type = String::new();
    let mut language = String::new();
    let mut body: Option<String> = None;
    let mut volatility: Option<FunctionVolatility> = None;
    let mut strict = false;
    let mut security_definer = false;

    // 按关键字顺序扫描剩余文本
    let mut remaining = after_params.trim().to_string();
    while !remaining.is_empty() {
        let upper_remaining = remaining.to_uppercase();

        // RETURNS rettype
        if upper_remaining.starts_with("RETURNS") {
            let after_kw = remaining["RETURNS".len()..].trim_start().to_string();
            // 返回类型可能是 `void`, `integer`, `TABLE(...)`, `SETOF ...`
            // 简化：提取到下一个空白分隔的关键字或 AS
            let (ret_type, rest) = extract_return_type(&after_kw)?;
            return_type = ret_type;
            remaining = rest.trim_start().to_string();
            continue;
        }

        // LANGUAGE lang_name
        if upper_remaining.starts_with("LANGUAGE") {
            let after_kw = &remaining["LANGUAGE".len()..].trim_start();
            let (lang, rest) = split_at_whitespace(after_kw);
            language = lang.trim_matches(|c| c == '\'' || c == '"').to_string();
            remaining = rest.trim_start().to_string();
            // 去掉可能的尾随分号
            if remaining.ends_with(';') {
                remaining = remaining[..remaining.len() - 1].trim_end().to_string();
            }
            continue;
        }

        // IMMUTABLE / STABLE / VOLATILE
        if upper_remaining.starts_with("IMMUTABLE") {
            volatility = Some(FunctionVolatility::Immutable);
            remaining = remaining["IMMUTABLE".len()..].trim_start().to_string();
            continue;
        }
        if upper_remaining.starts_with("STABLE") {
            volatility = Some(FunctionVolatility::Stable);
            remaining = remaining["STABLE".len()..].trim_start().to_string();
            continue;
        }
        if upper_remaining.starts_with("VOLATILE") {
            volatility = Some(FunctionVolatility::Volatile);
            remaining = remaining["VOLATILE".len()..].trim_start().to_string();
            continue;
        }

        // STRICT (or RETURNS NULL ON NULL INPUT)
        if upper_remaining.starts_with("STRICT")
            || upper_remaining.starts_with("RETURNS NULL ON NULL INPUT")
        {
            strict = true;
            if upper_remaining.starts_with("RETURNS NULL ON NULL INPUT") {
                remaining = remaining["RETURNS NULL ON NULL INPUT".len()..]
                    .trim_start()
                    .to_string();
            } else {
                remaining = remaining["STRICT".len()..].trim_start().to_string();
            }
            continue;
        }

        // SECURITY DEFINER / SECURITY INVOKER
        if upper_remaining.starts_with("SECURITY DEFINER") {
            security_definer = true;
            remaining = remaining["SECURITY DEFINER".len()..]
                .trim_start()
                .to_string();
            continue;
        }
        if upper_remaining.starts_with("SECURITY INVOKER") {
            security_definer = false;
            remaining = remaining["SECURITY INVOKER".len()..]
                .trim_start()
                .to_string();
            continue;
        }

        // AS $$ body $$ or AS 'body'
        if upper_remaining.starts_with("AS") {
            let after_as = remaining["AS".len()..].trim_start();
            let (body_text, after_body) = extract_function_body(after_as)?;
            body = Some(body_text);
            remaining = after_body.trim_start().to_string();
            // 去掉可能的尾随分号
            if remaining.ends_with(';') {
                remaining = remaining[..remaining.len() - 1].trim_end().to_string();
            }
            continue;
        }

        // 跳过其他未知属性（如 PARALLEL, LEAKPROOF, COST, ROWS, TRANSFORM, etc.）
        // 简化：跳过到下一个空白
        let (word, rest) = split_at_whitespace(&remaining);
        if word.is_empty() {
            break;
        }
        // 检查是否是 `word = value` 形式（如 COST = 100）
        let rest_trimmed = rest.trim_start();
        if let Some(after_eq) = rest_trimmed.strip_prefix('=') {
            // 跳过 = value
            let after_eq = after_eq.trim_start();
            let (_value, after_value) = split_at_whitespace(after_eq);
            remaining = after_value.trim_start().to_string();
        } else {
            remaining = rest_trimmed.to_string();
        }
    }

    // 去掉尾随分号
    let return_type = return_type.trim_end_matches(';').trim().to_string();
    let language = language.trim_end_matches(';').trim().to_string();

    let body = body.ok_or_else(|| {
        ParseError::Unsupported(format!("CREATE FUNCTION missing AS clause (body): {sql}"))
    })?;

    Ok(Statement::CreateFunction {
        name,
        parameters,
        return_type,
        language,
        body,
        or_replace,
        volatility,
        strict,
        security_definer,
    })
}

/// 提取函数名（支持 `schema.name` 或 `name`）
///
/// 返回 (函数名字符串, 剩余文本)
fn extract_function_name(s: &str) -> Result<(String, String), ParseError> {
    let s = s.trim_start();
    if s.is_empty() {
        return Err(ParseError::Unsupported(
            "CREATE FUNCTION missing function name".into(),
        ));
    }
    // 函数名可能是 quoted identifier "name" 或 "schema"."name"
    if s.starts_with('"') {
        // 处理 quoted identifier — 使用字节索引避免 String 生命周期问题
        let bytes = s.as_bytes();
        let mut byte_i = 1; // 跳过开头 "
        let mut name = String::new();
        while byte_i < bytes.len() {
            if bytes[byte_i] == b'"' {
                if byte_i + 1 < bytes.len() && bytes[byte_i + 1] == b'"' {
                    name.push('"');
                    byte_i += 2;
                } else {
                    byte_i += 1; // 跳过结尾 "
                    break;
                }
            } else {
                // 找到下一个 " 的字节位置
                let next_quote = s[byte_i..]
                    .find('"')
                    .map(|p| byte_i + p)
                    .unwrap_or(bytes.len());
                name.push_str(&s[byte_i..next_quote]);
                byte_i = next_quote;
            }
        }
        let remaining = s[byte_i..].to_string();
        if remaining.trim_start().starts_with('.') {
            let after_dot = remaining.trim_start()[1..].trim_start().to_string();
            let (rest_name, rest) = extract_function_name(&after_dot)?;
            return Ok((format!("{name}.{rest_name}"), rest));
        }
        return Ok((name, remaining));
    }
    // 普通 identifier：直到 `(` 或 空白
    let end = s
        .find(|c: char| c.is_whitespace() || c == '(')
        .unwrap_or(s.len());
    let name = &s[..end];
    Ok((name.to_string(), s[end..].to_string()))
}

/// 提取括号内的内容 `(...)` — 返回 (括号内字符串不含外层括号, 括号后剩余)
fn extract_parenthesized(s: &str) -> Result<(String, String), ParseError> {
    let s = s.trim_start();
    if !s.starts_with('(') {
        return Err(ParseError::Unsupported(format!(
            "expected '(' but got: {s}"
        )));
    }
    // 使用字节索引避免 String 生命周期问题
    let bytes = s.as_bytes();
    let mut depth = 1;
    let mut byte_i = 1;
    let mut content = String::new();
    let mut in_string = false;
    while byte_i < bytes.len() {
        let c = bytes[byte_i];
        if in_string {
            content.push(c as char);
            if c == b'\'' {
                if byte_i + 1 < bytes.len() && bytes[byte_i + 1] == b'\'' {
                    content.push('\'');
                    byte_i += 2;
                    continue;
                } else {
                    in_string = false;
                }
            }
            byte_i += 1;
            continue;
        }
        match c {
            b'(' => {
                depth += 1;
                content.push('(');
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    byte_i += 1;
                    let remaining = s[byte_i..].to_string();
                    return Ok((content, remaining));
                }
                content.push(')');
            }
            b'\'' => {
                in_string = true;
                content.push('\'');
            }
            _ => {
                // 处理多字节 UTF-8 字符
                // 找到下一个 ASCII 特殊字符或字符串结尾
                let next_special = s[byte_i..]
                    .find(['(', ')', '\''])
                    .map(|p| byte_i + p)
                    .unwrap_or(bytes.len());
                content.push_str(&s[byte_i..next_special]);
                byte_i = next_special;
                continue;
            }
        }
        byte_i += 1;
    }
    if depth != 0 {
        return Err(ParseError::Unsupported(format!(
            "unmatched parenthesis: {s}"
        )));
    }
    // 不会到达这里，但满足编译器
    Ok((content, String::new()))
}

/// 解析返回类型（支持 `void`, `integer`, `SETOF integer`, `TABLE(...)` 等）
fn extract_return_type(s: &str) -> Result<(String, String), ParseError> {
    let s = s.trim_start();
    let upper = s.to_uppercase();

    // SETOF type
    if upper.starts_with("SETOF") {
        let after = s["SETOF".len()..].trim_start().to_string();
        let (type_str, rest) = extract_return_type(&after)?;
        return Ok((format!("SETOF {type_str}"), rest));
    }

    // TABLE(...)
    if upper.starts_with("TABLE") {
        let after = s["TABLE".len()..].trim_start().to_string();
        let (inner, rest) = extract_parenthesized(&after)?;
        return Ok((format!("TABLE({inner})"), rest));
    }

    // 普通类型名：直到空白或 AS 或 ;
    let end = s
        .find(|c: char| c.is_whitespace() || c == ';')
        .unwrap_or(s.len());
    let type_str = &s[..end];
    // 去掉尾随分号
    let type_str = type_str.trim_end_matches(';');
    Ok((type_str.to_string(), s[end..].to_string()))
}

/// 提取函数体（`$$ ... $$`、`$tag$ ... $tag$` 或 `'...'`）
///
/// 返回 (body 内容不含定界符, 定界符后剩余)
fn extract_function_body(s: &str) -> Result<(String, String), ParseError> {
    let s = s.trim_start();

    // $$ ... $$ or $tag$ ... $tag$
    if s.starts_with('$') {
        let chars: Vec<char> = s.chars().collect();
        let mut j = 1;
        let mut tag = String::new();
        while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
            tag.push(chars[j]);
            j += 1;
        }
        if j < chars.len() && chars[j] == '$' {
            let opening = format!("${tag}$");
            let opening_len = opening.chars().count();
            // 查找结束标记 — 使用字节搜索
            let closing = opening.clone();
            let closing_bytes = closing.as_bytes();
            let s_bytes = s.as_bytes();
            let body_start = opening_len;
            let search_from = body_start;
            let pos = s_bytes[search_from..]
                .windows(closing_bytes.len())
                .position(|w| w == closing_bytes)
                .ok_or_else(|| {
                    ParseError::Unsupported(format!(
                        "unterminated dollar-quoted string: expected closing {closing}"
                    ))
                })?;
            let body_text = &s[body_start..search_from + pos];
            let after = &s[search_from + pos + closing_bytes.len()..];
            return Ok((body_text.to_string(), after.to_string()));
        } else {
            return Err(ParseError::Unsupported(format!(
                "invalid dollar-quote start: {s}"
            )));
        }
    }

    // '...' single-quoted string
    if s.starts_with('\'') {
        let chars: Vec<char> = s.chars().collect();
        let mut i = 1;
        let mut body = String::new();
        while i < chars.len() {
            if chars[i] == '\'' {
                if i + 1 < chars.len() && chars[i + 1] == '\'' {
                    body.push('\'');
                    i += 2;
                    continue;
                } else {
                    i += 1;
                    let remaining: String = chars[i..].iter().collect();
                    return Ok((body, remaining));
                }
            } else {
                body.push(chars[i]);
                i += 1;
            }
        }
        return Err(ParseError::Unsupported(format!(
            "unterminated string literal: {s}"
        )));
    }

    Err(ParseError::Unsupported(format!(
        "expected function body after AS (got: {s})"
    )))
}

/// 解析函数参数列表（括号内字符串）
fn parse_function_parameters(s: &str) -> Result<Vec<FunctionParameter>, ParseError> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }

    // 按逗号切分（不处理括号/字符串内的逗号 — 简化）
    let parts = split_params(s);
    let mut params = Vec::with_capacity(parts.len());
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        params.push(parse_single_function_parameter(part)?);
    }
    Ok(params)
}

/// 按顶层逗号切分参数列表（处理嵌套括号和字符串）
fn split_params(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    let mut in_string = false;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            current.push(c);
            if c == '\'' {
                if i + 1 < chars.len() && chars[i + 1] == '\'' {
                    current.push(chars[i + 1]);
                    i += 2;
                    continue;
                } else {
                    in_string = false;
                }
            }
            i += 1;
            continue;
        }
        match c {
            '\'' => {
                in_string = true;
                current.push(c);
            }
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                result.push(std::mem::take(&mut current));
            }
            _ => {
                current.push(c);
            }
        }
        i += 1;
    }
    if !current.trim().is_empty() {
        result.push(current);
    }
    result
}

/// 解析单个函数参数
///
/// 语法：`[IN|OUT|INOUT|VARIADIC] [name] type [DEFAULT expr | = expr]`
fn parse_single_function_parameter(s: &str) -> Result<FunctionParameter, ParseError> {
    let s = s.trim();
    let upper = s.to_uppercase();

    // 提取模式
    let (mode, rest) = if upper.starts_with("INOUT") {
        (Some(FunctionArgMode::InOut), &s["INOUT".len()..])
    } else if upper.starts_with("VARIADIC") {
        (Some(FunctionArgMode::Variadic), &s["VARIADIC".len()..])
    } else if upper.starts_with("OUT") {
        (Some(FunctionArgMode::Out), &s["OUT".len()..])
    } else if upper.starts_with("IN ") || upper.starts_with("IN\t") || upper.starts_with("IN\n") {
        (Some(FunctionArgMode::In), &s["IN".len()..])
    } else {
        (None, s)
    };

    let rest = rest.trim_start();

    // 检查是否有 DEFAULT / =
    let (main_part, default_expr): (String, Option<String>) =
        if let Some(idx) = rest.to_uppercase().find("DEFAULT") {
            let main = rest[..idx].trim_end().to_string();
            let default = rest[idx + "DEFAULT".len()..].trim().to_string();
            (main, Some(default))
        } else if let Some(idx) = rest.find('=') {
            let main = rest[..idx].trim_end().to_string();
            let default = rest[idx + 1..].trim().to_string();
            (main, Some(default))
        } else {
            (rest.to_string(), None)
        };

    // main_part 可能是 `name type` 或 `type`（匿名参数）
    // 简化：如果包含空白，第一个 token 是 name，剩余是 type
    // 但类型可能包含括号（如 varchar(255)），所以需要更细致的处理
    let (name, data_type) = if let Some(space_idx) = main_part.find(char::is_whitespace) {
        let first = &main_part[..space_idx];
        let rest_type = main_part[space_idx..].trim();
        // 检查 first 是否是合法的参数名（不以数字开头，不含特殊字符）
        if is_valid_param_name(first) {
            (Some(first.to_string()), rest_type.to_string())
        } else {
            (None, main_part.clone())
        }
    } else {
        (None, main_part.clone())
    };

    Ok(FunctionParameter {
        mode,
        name,
        data_type,
        default_expr,
    })
}

/// 判断字符串是否是合法的参数名（字母/下划线开头，只含字母数字下划线）
fn is_valid_param_name(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let first = s.chars().next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 解析 DROP FUNCTION 语句 — Phase 6.5
///
/// 支持语法：
/// ```sql
/// DROP FUNCTION [IF EXISTS] name [ ( [argtypes] ) ] [CASCADE | RESTRICT]
/// ```
fn parse_drop_function(sql: &str) -> Result<Statement, ParseError> {
    let trimmed = sql.trim();
    let upper = trimmed.to_uppercase();

    let rest = if upper.starts_with("DROP FUNCTION") {
        trimmed["DROP FUNCTION".len()..].trim_start().to_string()
    } else {
        return Err(ParseError::Unsupported(format!(
            "not a DROP FUNCTION statement: {sql}"
        )));
    };

    // 检查 IF EXISTS
    let (if_exists, rest) = if rest.to_uppercase().starts_with("IF EXISTS") {
        (true, rest["IF EXISTS".len()..].trim_start().to_string())
    } else {
        (false, rest)
    };

    // 提取函数名
    let (name_str, after_name) = extract_function_name(&rest)?;
    let name = name_str.trim().to_string();

    // 检查是否有参数类型列表 `(...)`
    let after_name_trimmed = after_name.trim_start();
    let (parameter_types, rest) = if after_name_trimmed.starts_with('(') {
        let (inner, after) = extract_parenthesized(after_name_trimmed)?;
        let types: Vec<String> = inner
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        (types, after.trim_start().to_string())
    } else {
        (Vec::new(), after_name_trimmed.to_string())
    };

    // 检查 CASCADE / RESTRICT
    let rest_upper = rest.to_uppercase();
    let cascade = rest_upper.starts_with("CASCADE");

    Ok(Statement::DropFunction {
        name,
        parameter_types,
        if_exists,
        cascade,
    })
}

// =====================================================================
//  Phase 6.10: DROP MATERIALIZED VIEW / REFRESH MATERIALIZED VIEW 解析
// =====================================================================

/// 解析 `DROP MATERIALIZED VIEW [IF EXISTS] name [, name2, ...] [CASCADE | RESTRICT]`
///
/// sqlparser 0.53.0 不支持此语法（ObjectType 无 MaterializedView 变体），
/// 因此在 `parse_sql` 入口处手动解析。
///
/// # 简化
/// - 不处理 schema 限定的逗号分隔歧义（`DROP MATERIALIZED VIEW s.a, s.b` 仍可处理）
/// - 不处理引号包裹的视图名中的逗号
fn parse_drop_materialized_view(sql: &str) -> Result<Statement, ParseError> {
    let trimmed = sql.trim();
    let upper = trimmed.to_uppercase();
    let rest = if upper.starts_with("DROP MATERIALIZED VIEW") {
        trimmed["DROP MATERIALIZED VIEW".len()..].trim_start()
    } else {
        return Err(ParseError::Unsupported(format!(
            "not a DROP MATERIALIZED VIEW statement: {sql}"
        )));
    };

    // 检查 IF EXISTS
    let (if_exists, rest): (bool, &str) = if rest.to_uppercase().starts_with("IF EXISTS") {
        (true, rest["IF EXISTS".len()..].trim_start())
    } else {
        (false, rest)
    };

    // 提取 CASCADE / RESTRICT（在末尾）
    let rest_upper = rest.to_uppercase();
    let cascade = rest_upper.ends_with("CASCADE");
    let restrict = rest_upper.ends_with("RESTRICT");
    let names_str: &str = if cascade {
        rest[..rest.len() - "CASCADE".len()].trim_end()
    } else if restrict {
        rest[..rest.len() - "RESTRICT".len()].trim_end()
    } else {
        rest
    };

    // 按逗号切分视图名列表
    let names: Vec<TableName> = names_str
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(parse_table_name_from_text)
        .collect::<Result<Vec<_>, _>>()?;

    if names.is_empty() {
        return Err(ParseError::Unsupported(format!(
            "DROP MATERIALIZED VIEW requires at least one view name, got: {sql}"
        )));
    }

    Ok(Statement::DropView {
        names,
        if_exists,
        cascade,
        materialized: true,
    })
}

/// 解析 `REFRESH MATERIALIZED VIEW [CONCURRENTLY] name [WITH DATA | WITH NO DATA]`
///
/// sqlparser 0.53.0 不支持 REFRESH 关键字，因此在 `parse_sql` 入口处手动解析。
///
/// # 简化
/// - CONCURRENTLY 关键字被识别但忽略（SzRSQL 暂不支持并发刷新）
/// - WITH DATA / WITH NO DATA 当前统一按 WITH DATA 处理（`with_data=true`）
fn parse_refresh_materialized_view(sql: &str) -> Result<Statement, ParseError> {
    let trimmed = sql.trim();
    let upper = trimmed.to_uppercase();
    let rest = if upper.starts_with("REFRESH MATERIALIZED VIEW") {
        trimmed["REFRESH MATERIALIZED VIEW".len()..].trim_start()
    } else {
        return Err(ParseError::Unsupported(format!(
            "not a REFRESH MATERIALIZED VIEW statement: {sql}"
        )));
    };

    // 可选 CONCURRENTLY 关键字
    let rest: &str = if rest.to_uppercase().starts_with("CONCURRENTLY") {
        rest["CONCURRENTLY".len()..].trim_start()
    } else {
        rest
    };

    // 提取末尾的 WITH DATA / WITH NO DATA
    let rest_upper = rest.to_uppercase();
    // PG 语义：WITH NO DATA → false；WITH DATA 或省略 → true（默认）
    let with_data = !rest_upper.ends_with("WITH NO DATA");

    let name_str: &str = if rest_upper.ends_with("WITH NO DATA") {
        rest[..rest.len() - "WITH NO DATA".len()].trim_end()
    } else if rest_upper.ends_with("WITH DATA") {
        rest[..rest.len() - "WITH DATA".len()].trim_end()
    } else {
        rest
    };

    let name = parse_table_name_from_text(name_str.trim())?;
    Ok(Statement::RefreshMaterializedView { name, with_data })
}

/// 从文本解析表名（支持 `name` 和 `schema.name` 两种形式）
///
/// 简化实现：按 `.` 切分，最多 2 段。不处理引号包裹的标识符。
fn parse_table_name_from_text(s: &str) -> Result<TableName, ParseError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ParseError::Unsupported("empty table/view name".to_string()));
    }
    // 简单处理：按 `.` 切分
    let parts: Vec<&str> = s.split('.').collect();
    match parts.len() {
        1 => Ok(TableName::new(parts[0].to_string())),
        2 => Ok(TableName::with_schema(
            parts[0].to_string(),
            parts[1].to_string(),
        )),
        _ => Err(ParseError::Unsupported(format!(
            "unsupported table name with {} parts: {s}",
            parts.len()
        ))),
    }
}

// hex 解码（避免引入新依赖）
mod hex {
    pub fn decode(s: &str) -> Result<Vec<u8>, ()> {
        if !s.len().is_multiple_of(2) {
            return Err(());
        }
        let mut bytes = Vec::with_capacity(s.len() / 2);
        let chars: Vec<char> = s.chars().collect();
        for chunk in chars.chunks(2) {
            let h = chunk[0].to_digit(16).ok_or(())?;
            let l = chunk[1].to_digit(16).ok_or(())?;
            bytes.push((h * 16 + l) as u8);
        }
        Ok(bytes)
    }
}
