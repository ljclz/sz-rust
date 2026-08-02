//! Oracle 适配器主入口。
//!
//! 本模块提供 SzRSQL 与 Oracle 之间的双向适配能力（L2 逻辑级兼容）：
//!
//! - **导入**：解析 Oracle 导出的 SQL 脚本（DDL + DML），转换为内存中的
//!   [`OracleTable`] 结构（表名 + 列定义 + 行数据）
//! - **导出**：将 [`OracleTable`] 结构生成 Oracle 兼容的 SQL 脚本
//!   （`CREATE TABLE` + `INSERT INTO`）
//! - **SQL 转换**：委托 [`OracleDialect`] 将 Oracle PL/SQL 方言转换为
//!   PG 兼容 SQL
//!
//! # 实现说明
//!
//! - **零 Oracle 客户端依赖**：不依赖 libclntsh 等专有库，纯 Rust 实现
//! - **文本级解析**：通过正则与字符串匹配解析 SQL 脚本，不依赖 AST 反向解析
//! - **复用类型映射**：导入/导出值转换复用 [`OracleType`] 的 `to_value` /
//!   `value_to_oracle_literal` 方法，保证与类型模块语义一致
//!
//! # 用法
//!
//! ```ignore
//! use szrsql_oracle_bridge::{OracleAdapter, OracleTable, OracleColumn};
//! use szrsql_oracle_bridge::types::OracleType;
//! use szrsql_types::value::Value;
//!
//! let adapter = OracleAdapter::new();
//!
//! // 导入 Oracle SQL 脚本
//! let script = "CREATE TABLE users (id NUMBER, name VARCHAR2(100));
//!               INSERT INTO users VALUES (1, 'Alice');";
//! let tables = adapter.import_from_oracle(script).unwrap();
//! assert_eq!(tables.len(), 1);
//! assert_eq!(tables[0].name, "users");
//!
//! // 导出为 Oracle 兼容 SQL 脚本
//! let oracle_sql = adapter.export_to_oracle(&tables).unwrap();
//! assert!(oracle_sql.contains("CREATE TABLE users"));
//! assert!(oracle_sql.contains("INSERT INTO users"));
//!
//! // 转换 Oracle 方言 SQL
//! let pg_sql = adapter.convert_sql("SELECT NVL(name, 'N/A') FROM dual").unwrap();
//! assert!(pg_sql.contains("COALESCE"));
//! ```

use regex::Regex;
use szrsql_types::value::Value;
use tracing::debug;

use crate::sql_dialect::{split_args, OracleDialect, OracleDialectError};
use crate::types::OracleType;

// =====================================================================
//  错误类型
// =====================================================================

/// 适配器错误。
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// Oracle 方言转换错误（SQL 文本级转换或解析失败）。
    #[error("oracle dialect error: {0}")]
    Dialect(#[from] OracleDialectError),

    /// SQL 脚本解析错误（CREATE TABLE / INSERT 语法不合法）。
    #[error("sql parse error: {0}")]
    SqlParse(String),

    /// Oracle 类型解析错误（DDL 中的类型声明无法识别）。
    #[error("oracle type error: {0}")]
    Type(#[from] crate::types::OracleTypeError),

    /// 不支持的脚本特性（如 PL/SQL 块等）。
    #[error("unsupported feature: {0}")]
    Unsupported(String),
}

// =====================================================================
//  OracleColumn / OracleTable
// =====================================================================

/// Oracle 列定义。
///
/// 描述一张 Oracle 表中单列的元信息：列名、Oracle 类型、NOT NULL 约束。
/// 其他约束（PRIMARY KEY / UNIQUE / DEFAULT / CHECK）当前未建模，
/// 因为本桥接聚焦于数据互操作，约束语义由 SzRSQL catalog 自行管理。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OracleColumn {
    /// 列名
    pub name: String,
    /// Oracle 数据类型
    pub oracle_type: OracleType,
    /// NOT NULL 约束
    pub not_null: bool,
}

/// Oracle 表数据。
///
/// 包含表名、列定义与行数据（每行为 `Vec<Value>`，与列顺序对齐）。
/// `import_from_oracle` 返回此结构列表，`export_to_oracle` 接受此结构列表。
#[derive(Debug, Clone, PartialEq)]
pub struct OracleTable {
    /// 表名
    pub name: String,
    /// 列定义（按声明顺序）
    pub columns: Vec<OracleColumn>,
    /// 行数据（每行值的数量应与 `columns.len()` 一致）
    pub rows: Vec<Vec<Value>>,
}

impl OracleTable {
    /// 构造一个空表（仅有表名，无列无行）。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            columns: Vec::new(),
            rows: Vec::new(),
        }
    }

    /// 追加一列（builder 风格）。
    pub fn add_column(&mut self, col: OracleColumn) -> &mut Self {
        self.columns.push(col);
        self
    }

    /// 追加一行数据。
    pub fn add_row(&mut self, row: Vec<Value>) -> &mut Self {
        self.rows.push(row);
        self
    }
}

// =====================================================================
//  OracleAdapter
// =====================================================================

/// Oracle 适配器主入口。
///
/// 提供 Oracle SQL 脚本导入/导出与方言转换能力，实现 SzRSQL 与
/// Oracle 之间的 L2 逻辑级兼容（SQL 语义级互操作）。
///
/// # 设计要点
///
/// - **无状态**：适配器本身无内部可变状态，所有方法均为纯函数，
///   可安全地在多线程中共享 `&OracleAdapter`
/// - **复用方言转换器**：内部持有 [`OracleDialect`] 实例，`convert_sql`
///   直接委托给方言转换器，避免逻辑重复
/// - **文本级解析**：`import_from_oracle` 通过正则与字符串匹配解析脚本，
///   不依赖 sqlparser AST 反向遍历，保持模块独立性
#[derive(Debug, Clone, Default)]
pub struct OracleAdapter {
    /// 内部持有的方言转换器，用于 `convert_sql` 委托
    dialect: OracleDialect,
}

impl OracleAdapter {
    /// 构造一个新的 Oracle 适配器实例。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 Oracle SQL 脚本导入数据。
    ///
    /// # 处理流程
    /// 1. 按分号切分脚本为语句列表（处理字符串字面量内的分号）
    /// 2. 识别 `CREATE TABLE` 语句，解析表名与列定义
    /// 3. 识别 `INSERT INTO ... VALUES (...)` 语句，解析值列表并追加到对应表
    /// 4. 忽略其他语句（DDL 如 CREATE INDEX / DROP TABLE / 注释等）
    ///
    /// # 参数
    /// - `script`：Oracle 导出的 SQL 脚本文本
    ///
    /// # 返回
    /// - `Ok(Vec<OracleTable>)`：解析成功的表列表（按 CREATE TABLE 出现顺序）
    /// - `Err(AdapterError::SqlParse)`：CREATE TABLE / INSERT 语法错误
    /// - `Err(AdapterError::Type)`：DDL 中的 Oracle 类型声明无法识别
    ///
    /// # 限制
    /// - 不处理 PL/SQL 匿名块（BEGIN ... END）
    /// - INSERT INTO ... SELECT 支持 SELECT */列列表/字面量，不支持 WHERE/JOIN 等复杂子句
    /// - 不解析外键、索引、触发器等非数据对象
    pub fn import_from_oracle(&self, script: &str) -> Result<Vec<OracleTable>, AdapterError> {
        debug!(script_len = script.len(), "import_from_oracle: start");

        let statements = split_statements(script);
        debug!(
            stmt_count = statements.len(),
            "import_from_oracle: split into statements"
        );

        let mut tables: Vec<OracleTable> = Vec::new();

        for stmt in &statements {
            let trimmed = stmt.trim();
            if trimmed.is_empty() {
                continue;
            }

            // 跳过注释行（-- 单行注释）
            if trimmed.starts_with("--") {
                continue;
            }

            let upper = trimmed.to_uppercase();
            if upper.starts_with("CREATE TABLE")
                || upper.starts_with("CREATE GLOBAL TEMPORARY TABLE")
            {
                let table = parse_create_table(trimmed)?;
                debug!(table_name = %table.name, col_count = table.columns.len(), "parsed CREATE TABLE");
                tables.push(table);
            } else if upper.starts_with("INSERT INTO") {
                parse_insert(trimmed, &mut tables)?;
            }
            // 其他语句（CREATE INDEX / DROP / COMMIT 等）静默忽略
        }

        debug!(table_count = tables.len(), "import_from_oracle: done");
        Ok(tables)
    }

    /// 将 SzRSQL 表数据导出为 Oracle 兼容 SQL 脚本。
    ///
    /// # 生成内容
    /// 对每张表生成：
    /// 1. `CREATE TABLE name (col1 type1 [NOT NULL], col2 type2, ...);`
    /// 2. 对每行数据生成 `INSERT INTO name VALUES (val1, val2, ...);`
    ///
    /// 值的字面量格式由 [`OracleType::value_to_oracle_literal`] 决定，
    /// 保证与 Oracle 语法兼容（如日期使用 `TO_DATE(...)`，BLOB 使用 `HEXTORAW(...)`）。
    ///
    /// # 参数
    /// - `tables`：待导出的表数据列表
    ///
    /// # 返回
    /// - `Ok(String)`：生成的 Oracle 兼容 SQL 脚本
    pub fn export_to_oracle(&self, tables: &[OracleTable]) -> Result<String, AdapterError> {
        debug!(table_count = tables.len(), "export_to_oracle: start");

        let mut output = String::new();

        for table in tables {
            // 生成 CREATE TABLE 语句
            output.push_str(&generate_create_table(table));
            output.push('\n');

            // 生成 INSERT 语句
            for row in &table.rows {
                output.push_str(&generate_insert(table, row));
                output.push('\n');
            }
        }

        debug!(output_len = output.len(), "export_to_oracle: done");
        Ok(output)
    }

    /// 转换 Oracle 方言 SQL 为 PG 兼容 SQL。
    ///
    /// 委托给内部持有的 [`OracleDialect::convert_sql`]，应用文本级方言转换
    /// 并通过 `parse_with_dialect` 验证语法合法性。
    ///
    /// # 参数
    /// - `sql`：Oracle 方言 SQL 文本
    ///
    /// # 返回
    /// - `Ok(String)`：转换并验证成功的 PG 兼容 SQL
    /// - `Err(AdapterError::Dialect)`：转换后 SQL 解析失败
    pub fn convert_sql(&self, sql: &str) -> Result<String, AdapterError> {
        debug!(
            sql_len = sql.len(),
            "convert_sql: delegating to OracleDialect"
        );
        let result = self.dialect.convert_sql(sql)?;
        Ok(result)
    }
}

// =====================================================================
//  脚本切分与语句解析
// =====================================================================

/// 按分号切分 SQL 脚本为语句列表。
///
/// 处理字符串字面量内的分号（不切分），支持单引号字符串（Oracle 标准）。
/// 不处理 PL/SQL 块（BEGIN ... END;）的嵌套分号——当前版本仅做平面切分。
fn split_statements(script: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut in_string = false;

    for ch in script.chars() {
        if in_string {
            current.push(ch);
            if ch == '\'' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '\'' => {
                in_string = true;
                current.push(ch);
            }
            ';' => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    statements.push(trimmed);
                }
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    // 处理末尾未以分号结尾的语句
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        statements.push(trimmed);
    }
    statements
}

/// 解析 CREATE TABLE 语句。
///
/// 支持格式：
/// - `CREATE TABLE name (col1 type1, col2 type2, ...)`
/// - `CREATE GLOBAL TEMPORARY TABLE name (...)`（忽略 TEMPORARY 语义）
///
/// 列定义可含 NOT NULL 约束；其他约束（PRIMARY KEY / UNIQUE / DEFAULT / CHECK）
/// 当前忽略，仅保留类型与 NOT NULL 信息。
fn parse_create_table(stmt: &str) -> Result<OracleTable, AdapterError> {
    // 定位表名：跳过 CREATE [GLOBAL TEMPORARY] TABLE 关键字
    let upper = stmt.to_uppercase();
    let after_table = if upper.starts_with("CREATE GLOBAL TEMPORARY TABLE") {
        &stmt["CREATE GLOBAL TEMPORARY TABLE".len()..]
    } else if upper.starts_with("CREATE TABLE") {
        &stmt["CREATE TABLE".len()..]
    } else {
        return Err(AdapterError::SqlParse(format!(
            "not a CREATE TABLE statement: {stmt}"
        )));
    };

    // 跳过空白与可选的 IF NOT EXISTS
    let after_ifne = after_table.trim_start();
    let after_ifne = if after_ifne.to_uppercase().starts_with("IF NOT EXISTS") {
        after_ifne["IF NOT EXISTS".len()..].trim_start()
    } else {
        after_ifne
    };

    // 提取表名（支持双引号标识符与 schema.table 形式）
    let (table_name, rest) = extract_identifier(after_ifne)
        .ok_or_else(|| AdapterError::SqlParse(format!("missing table name: {stmt}")))?;
    let rest = rest.trim_start();

    // 期望剩余部分以 '(' 开头
    let rest = rest
        .strip_prefix('(')
        .ok_or_else(|| AdapterError::SqlParse(format!("missing '(' after table name: {stmt}")))?;

    // 找到匹配的右括号（处理嵌套括号，如 NUMBER(10,2)）
    let close_pos = find_matching_paren(rest)
        .ok_or_else(|| AdapterError::SqlParse(format!("unmatched '(' in CREATE TABLE: {stmt}")))?;
    let columns_str = &rest[..close_pos];

    // 切分列定义（按逗号，处理嵌套括号）
    let column_defs = split_args(columns_str);

    let mut table = OracleTable::new(table_name);
    for def in &column_defs {
        let def = def.trim();
        if def.is_empty() {
            continue;
        }
        // 跳过表级约束：PRIMARY KEY (...) / UNIQUE (...) / FOREIGN KEY (...) / CONSTRAINT ... / CHECK (...)
        let def_upper = def.to_uppercase();
        if def_upper.starts_with("PRIMARY KEY")
            || def_upper.starts_with("UNIQUE")
            || def_upper.starts_with("FOREIGN KEY")
            || def_upper.starts_with("CONSTRAINT")
            || def_upper.starts_with("CHECK")
        {
            continue;
        }
        let col = parse_column_def(def)?;
        table.add_column(col);
    }

    Ok(table)
}

/// 解析单个列定义字符串。
///
/// 格式：`col_name TYPE [NOT NULL] [其他约束...]`
///
/// 仅提取列名、类型、NOT NULL；其他约束（DEFAULT / PRIMARY KEY / UNIQUE / CHECK）
/// 静默忽略。
fn parse_column_def(def: &str) -> Result<OracleColumn, AdapterError> {
    let def = def.trim();
    // 提取列名（支持双引号标识符）
    let (name, rest) = extract_identifier(def)
        .ok_or_else(|| AdapterError::SqlParse(format!("missing column name: {def}")))?;
    let rest = rest.trim_start();
    if rest.is_empty() {
        return Err(AdapterError::SqlParse(format!(
            "missing column type: {def}"
        )));
    }

    // 提取类型声明：从 rest 开头读取直到下一个空白或约束关键字
    let (oracle_type, rest_after_type) = parse_oracle_type_decl(rest)?;
    let rest_after_type = rest_after_type.trim();

    // 检测 NOT NULL 约束（大小写不敏感，词边界匹配）
    let not_null = Regex::new(r"(?i)\bNOT\s+NULL\b")
        .unwrap()
        .is_match(rest_after_type);

    Ok(OracleColumn {
        name,
        oracle_type,
        not_null,
    })
}

/// 从字符串开头提取一个标识符（表名/列名）。
///
/// 支持：
/// - 双引号标识符：`"my column"` → `my column`
/// - 普通标识符：`my_col` / `schema.table` → 取完整路径
///
/// 返回 `(标识符, 剩余字符串)`。
fn extract_identifier(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }

    // 双引号标识符
    if s.starts_with('"') {
        let bytes = s.as_bytes();
        for (i, &b) in bytes.iter().enumerate().skip(1) {
            if b == b'"' {
                let name = s[1..i].to_string();
                let rest = &s[i + 1..];
                return Some((name, rest));
            }
        }
        return None;
    }

    // 普通标识符：字母/下划线/数字/点（支持 schema.table）
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    Some((s[..end].to_string(), &s[end..]))
}

/// 解析 Oracle 类型声明字符串。
///
/// 从字符串开头读取类型名与可选参数，返回 `(OracleType, 剩余字符串)`。
///
/// 支持的类型：
/// - `NUMBER` / `NUMBER(p)` / `NUMBER(p, s)`
/// - `VARCHAR2(n)` / `VARCHAR2(n CHAR)` / `VARCHAR2(n BYTE)`
/// - `CHAR(n)` / `CHAR(n CHAR)` / `CHAR(n BYTE)`
/// - `DATE`
/// - `TIMESTAMP` / `TIMESTAMP(p)`
/// - `CLOB` / `BLOB`
/// - `RAW(n)`
/// - `INTEGER` / `INT` → NUMBER(38, 0)
/// - `FLOAT` / `FLOAT(n)` → NUMBER(38, 127)
fn parse_oracle_type_decl(s: &str) -> Result<(OracleType, &str), AdapterError> {
    let s = s.trim_start();
    // 读取类型名（字母到非字母数字为止）
    let type_name_end = s
        .find(|c: char| !(c.is_ascii_alphanumeric()))
        .unwrap_or(s.len());
    if type_name_end == 0 {
        return Err(AdapterError::SqlParse(format!("missing type name in: {s}")));
    }
    let type_name = s[..type_name_end].to_uppercase();
    let rest = s[type_name_end..].trim_start();

    match type_name.as_str() {
        "NUMBER" | "NUMERIC" | "DECIMAL" | "DEC" => {
            // 可选 (precision, scale)
            if let Some((args, rest)) = try_consume_paren_args(rest) {
                let (precision, scale) = parse_number_args(&args)?;
                Ok((OracleType::number(precision, scale)?, rest))
            } else {
                Ok((OracleType::number_default(), rest))
            }
        }
        "INTEGER" | "INT" | "INT4" | "SMALLINT" => Ok((OracleType::number(38, 0)?, rest)),
        "FLOAT" | "FLOAT8" | "DOUBLE" | "REAL" | "BINARY_FLOAT" | "BINARY_DOUBLE" => Ok((
            OracleType::Number {
                precision: 38,
                scale: 127,
            },
            rest,
        )),
        "VARCHAR2" | "VARCHAR" => {
            let (args, rest) = try_consume_paren_args(rest)
                .ok_or_else(|| AdapterError::SqlParse(format!("VARCHAR2 requires length: {s}")))?;
            let (size, char_semantics) = parse_char_length_args(&args)?;
            Ok((OracleType::varchar2(size, char_semantics)?, rest))
        }
        "CHAR" | "CHARACTER" => {
            let (args, rest) = try_consume_paren_args(rest)
                .ok_or_else(|| AdapterError::SqlParse(format!("CHAR requires length: {s}")))?;
            let (size, char_semantics) = parse_char_length_args(&args)?;
            Ok((
                OracleType::Char {
                    size,
                    char_semantics,
                },
                rest,
            ))
        }
        "DATE" => Ok((OracleType::Date, rest)),
        "TIMESTAMP" => {
            if let Some((args, rest)) = try_consume_paren_args(rest) {
                let precision = args.trim().parse::<u8>().map_err(|_| {
                    AdapterError::SqlParse(format!("invalid TIMESTAMP precision: {args}"))
                })?;
                Ok((OracleType::timestamp(precision)?, rest))
            } else {
                Ok((OracleType::timestamp(6)?, rest))
            }
        }
        "CLOB" => Ok((OracleType::Clob, rest)),
        "BLOB" => Ok((OracleType::Blob, rest)),
        "RAW" => {
            let (args, rest) = try_consume_paren_args(rest)
                .ok_or_else(|| AdapterError::SqlParse(format!("RAW requires length: {s}")))?;
            let size = args
                .trim()
                .parse::<u32>()
                .map_err(|_| AdapterError::SqlParse(format!("invalid RAW length: {args}")))?;
            if size == 0 {
                return Err(crate::types::OracleTypeError::RawZeroLength { size }.into());
            }
            Ok((OracleType::Raw { size }, rest))
        }
        _ => Err(AdapterError::SqlParse(format!(
            "unsupported Oracle type: {type_name}"
        ))),
    }
}

/// 尝试消费开头的括号参数列表 `(...)`，返回 `(括号内文本, 剩余字符串)`。
///
/// 若字符串不以 `(` 开头，返回 `None`。
fn try_consume_paren_args(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    if !s.starts_with('(') {
        return None;
    }
    let close_pos = find_matching_paren(&s[1..])?;
    let args = s[1..1 + close_pos].to_string();
    let rest = &s[1 + close_pos + 1..];
    Some((args, rest))
}

/// 解析 NUMBER 类型的参数列表 `(precision[, scale])`。
///
/// - 空字符串 → (38, 0)
/// - "10" → (10, 0)
/// - "10, 2" → (10, 2)
fn parse_number_args(args: &str) -> Result<(u8, i8), AdapterError> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok((38, 0));
    }
    let parts: Vec<&str> = trimmed.split(',').collect();
    match parts.len() {
        1 => {
            let precision = parts[0]
                .trim()
                .parse::<u8>()
                .map_err(|_| AdapterError::SqlParse(format!("invalid NUMBER precision: {args}")))?;
            Ok((precision, 0))
        }
        2 => {
            let precision = parts[0]
                .trim()
                .parse::<u8>()
                .map_err(|_| AdapterError::SqlParse(format!("invalid NUMBER precision: {args}")))?;
            let scale = parts[1]
                .trim()
                .parse::<i8>()
                .map_err(|_| AdapterError::SqlParse(format!("invalid NUMBER scale: {args}")))?;
            Ok((precision, scale))
        }
        _ => Err(AdapterError::SqlParse(format!(
            "invalid NUMBER args (expected 1 or 2 parts): {args}"
        ))),
    }
}

/// 解析 VARCHAR2/CHAR 的长度参数 `(n [CHAR|BYTE])`。
///
/// 返回 `(size, char_semantics)`。
fn parse_char_length_args(args: &str) -> Result<(u32, bool), AdapterError> {
    let trimmed = args.trim().to_uppercase();
    if trimmed.ends_with("CHAR") {
        let num_part = trimmed[..trimmed.len() - 4].trim();
        let size = num_part
            .parse::<u32>()
            .map_err(|_| AdapterError::SqlParse(format!("invalid CHAR length: {args}")))?;
        Ok((size, true))
    } else if trimmed.ends_with("BYTE") {
        let num_part = trimmed[..trimmed.len() - 4].trim();
        let size = num_part
            .parse::<u32>()
            .map_err(|_| AdapterError::SqlParse(format!("invalid BYTE length: {args}")))?;
        Ok((size, false))
    } else {
        let size = trimmed
            .parse::<u32>()
            .map_err(|_| AdapterError::SqlParse(format!("invalid length: {args}")))?;
        // 无后缀默认 BYTE 语义（Oracle 默认由 NLS_LENGTH_SEMANTICS 决定，此处取 BYTE）
        Ok((size, false))
    }
}

/// 查找字符串中第一个未匹配的右括号位置（从开头左括号之后开始）。
///
/// 输入 `s` 应为左括号 *之后* 的子串。返回匹配右括号在 `s` 中的字节位置。
fn find_matching_paren(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

// =====================================================================
//  INSERT 语句解析
// =====================================================================

/// 解析 INSERT INTO 语句，将值追加到对应表。
///
/// 支持格式：
/// - `INSERT INTO table VALUES (v1, v2, ...)`
/// - `INSERT INTO table (col1, col2) VALUES (v1, v2)`
/// - `INSERT INTO table SELECT ... FROM source`（列列表/字面量/DUAL 伪表）
fn parse_insert(stmt: &str, tables: &mut [OracleTable]) -> Result<(), AdapterError> {
    // 提取表名
    let after_into = stmt
        .trim()
        .strip_prefix("INSERT INTO")
        .or_else(|| stmt.trim().strip_prefix("insert into"))
        .ok_or_else(|| AdapterError::SqlParse(format!("not an INSERT statement: {stmt}")))?
        .trim_start();

    let (table_name, rest) = extract_identifier(after_into)
        .ok_or_else(|| AdapterError::SqlParse(format!("missing table name in INSERT: {stmt}")))?;
    let rest = rest.trim_start();

    // 可选的列名列表 (col1, col2)
    let (_column_list, rest) = if let Some(rest_after_paren) = rest.strip_prefix('(') {
        let close_pos = find_matching_paren(rest_after_paren).ok_or_else(|| {
            AdapterError::SqlParse(format!("unmatched '(' in INSERT columns: {stmt}"))
        })?;
        let cols = rest_after_paren[..close_pos].to_string();
        (Some(cols), rest_after_paren[close_pos + 1..].trim_start())
    } else {
        (None, rest)
    };

    // 期望 VALUES 关键字或 SELECT 子句（大小写不敏感）
    let upper = rest.to_uppercase();
    let after_values = if let Some(after_values) = upper.strip_prefix("VALUES") {
        // upper 是 rest 的大写副本，长度一致；用 after_values 的长度定位 rest 中的剩余部分
        rest[rest.len() - after_values.len()..].trim_start()
    } else if upper.starts_with("SELECT") {
        // INSERT INTO ... SELECT：执行子查询并将结果插入目标表
        return execute_insert_select(&table_name, _column_list.as_deref(), rest, tables);
    } else {
        return Err(AdapterError::SqlParse(format!(
            "missing VALUES in INSERT: {stmt}"
        )));
    };

    // 解析值列表（可能多个 (...) 由逗号连接）
    let value_tuples = parse_value_tuples(after_values)?;
    let row_values: Vec<Vec<Value>> = value_tuples
        .into_iter()
        .map(|tuple| {
            tuple
                .into_iter()
                .map(|v| parse_oracle_value(v.trim()))
                .collect()
        })
        .collect();

    // 查找对应表并追加行
    let table = tables
        .iter_mut()
        .find(|t| t.name.eq_ignore_ascii_case(&table_name))
        .ok_or_else(|| {
            AdapterError::SqlParse(format!("INSERT targets unknown table '{table_name}'"))
        })?;

    for row in row_values {
        table.add_row(row);
    }

    Ok(())
}

// =====================================================================
//  INSERT INTO ... SELECT 解析与执行
// =====================================================================

/// 执行 `INSERT INTO target [(cols)] SELECT ... FROM source` 语句。
///
/// 支持的 SELECT 形式：
/// - `SELECT * FROM source` — 全列复制
/// - `SELECT col1, col2 FROM source` — 指定列
/// - `SELECT expr1, expr2 FROM source` — 列引用或字面量
/// - `SELECT ... FROM dual` — Oracle DUAL 伪表（单行）
///
/// 当 INSERT 指定了显式列列表时，按位置映射 SELECT 输出到目标列。
/// 不支持的子句（WHERE / ORDER BY / GROUP BY 等）被忽略（文本级解析限制）。
fn execute_insert_select(
    target_table_name: &str,
    target_columns: Option<&str>,
    select_stmt: &str,
    tables: &mut [OracleTable],
) -> Result<(), AdapterError> {
    // 解析 SELECT ... FROM ...
    let (select_cols, source_table_name) = parse_select_clause(select_stmt)?;

    // 查找目标表
    let target_idx = tables
        .iter()
        .position(|t| t.name.eq_ignore_ascii_case(target_table_name))
        .ok_or_else(|| {
            AdapterError::SqlParse(format!(
                "INSERT target table '{target_table_name}' not found"
            ))
        })?;

    // 解析目标列索引（若指定了显式列列表）
    let target_col_indices: Option<Vec<usize>> = target_columns.map(|cols_str| {
        cols_str
            .split(',')
            .map(|c| c.trim().to_lowercase())
            .filter_map(|col_name| {
                tables[target_idx]
                    .columns
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(&col_name))
            })
            .collect()
    });

    // 生成要插入的行
    let new_rows = if source_table_name.eq_ignore_ascii_case("dual") {
        // Oracle DUAL 伪表：返回单行
        // SELECT 列表中的表达式求值为字面量或 NULL
        let row: Vec<Value> = select_cols
            .iter()
            .map(|col| eval_select_expr(col, &[]))
            .collect();
        vec![row]
    } else {
        // 查找源表
        let source_idx = tables
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(&source_table_name))
            .ok_or_else(|| {
                AdapterError::SqlParse(format!(
                    "SELECT source table '{source_table_name}' not found"
                ))
            })?;
        // 克隆源表数据以避免借用冲突
        let source_rows = tables[source_idx].rows.clone();
        let source_columns = &tables[source_idx].columns;

        // 预解析 SELECT 列表达式 → 源表列索引或字面量
        // SELECT * — 全列复制
        if select_cols.len() == 1 && select_cols[0].trim() == "*" {
            source_rows
                .iter()
                .map(|src_row| {
                    // 若指定了目标列列表，按位置映射到目标表的列顺序
                    if let Some(ref indices) = target_col_indices {
                        let mut full_row = vec![Value::Null; tables[target_idx].columns.len()];
                        for (i, &tgt_idx) in indices.iter().enumerate() {
                            if i < src_row.len() {
                                full_row[tgt_idx] = src_row[i].clone();
                            }
                        }
                        full_row
                    } else {
                        // 无显式列列表：直接按位置插入
                        let mut row = src_row.clone();
                        let target_col_count = tables[target_idx].columns.len();
                        if row.len() < target_col_count {
                            row.resize(target_col_count, Value::Null);
                        } else if row.len() > target_col_count {
                            row.truncate(target_col_count);
                        }
                        row
                    }
                })
                .collect()
        } else {
            // 指定列/表达式列表 — 为每个 SELECT 表达式解析为列索引或字面量
            let col_resolvers: Vec<SelectColResolver> = select_cols
                .iter()
                .map(|col| resolve_select_col(col, source_columns))
                .collect();

            source_rows
                .iter()
                .map(|src_row| {
                    let row: Vec<Value> = col_resolvers
                        .iter()
                        .map(|resolver| resolver.eval(src_row))
                        .collect();
                    // 若指定了目标列列表，按位置映射到目标表的列顺序
                    if let Some(ref indices) = target_col_indices {
                        let mut full_row = vec![Value::Null; tables[target_idx].columns.len()];
                        for (i, &tgt_idx) in indices.iter().enumerate() {
                            if i < row.len() {
                                full_row[tgt_idx] = row[i].clone();
                            }
                        }
                        full_row
                    } else {
                        // 无显式列列表：直接按位置插入
                        let mut row = row;
                        let target_col_count = tables[target_idx].columns.len();
                        if row.len() < target_col_count {
                            row.resize(target_col_count, Value::Null);
                        } else if row.len() > target_col_count {
                            row.truncate(target_col_count);
                        }
                        row
                    }
                })
                .collect()
        }
    };

    // 追加行到目标表
    for row in new_rows {
        tables[target_idx].add_row(row);
    }

    Ok(())
}

/// 解析 SELECT 子句，提取列列表与 FROM 表名。
///
/// 支持格式：`SELECT col1, col2, ... FROM table_name [其他子句...]`
/// 或 `SELECT * FROM table_name [其他子句...]`
///
/// 返回 `(列表达式列表, 表名)`。
fn parse_select_clause(select_stmt: &str) -> Result<(Vec<String>, String), AdapterError> {
    let stmt = select_stmt.trim();
    // 去掉前导 SELECT（大小写不敏感）
    let after_select = if stmt.to_uppercase().starts_with("SELECT ") {
        &stmt[7..]
    } else if stmt.to_uppercase().starts_with("SELECT\t") {
        &stmt[7..]
    } else {
        return Err(AdapterError::SqlParse(format!(
            "not a SELECT statement: {stmt}"
        )));
    };

    // 查找 FROM 关键字（大小写不敏感，词边界匹配）
    let after_select_upper = after_select.to_uppercase();
    let from_pos = after_select_upper
        .find(" FROM ")
        .or_else(|| after_select_upper.find("\tFROM\t"))
        .or_else(|| after_select_upper.find("\tFROM "))
        .ok_or_else(|| AdapterError::SqlParse(format!("missing FROM in SELECT: {stmt}")))?;

    let cols_str = &after_select[..from_pos];
    let after_from = after_select[from_pos + 6..].trim(); // 6 = len(" FROM " or "\tFROM ")

    // 解析列列表（按逗号分割，处理括号内的逗号）
    let select_cols = split_top_level_commas(cols_str)
        .into_iter()
        .map(|s| s.trim().to_string())
        .collect::<Vec<_>>();

    // 解析表名（取 FROM 后的第一个标识符）
    let (table_name, _rest) = extract_identifier(after_from)
        .ok_or_else(|| AdapterError::SqlParse(format!("missing table name in FROM: {stmt}")))?;

    Ok((select_cols, table_name))
}

/// 按顶层逗号分割（忽略括号内的逗号）。
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
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
                string_char = '\'';
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
                parts.push(current.clone());
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

/// SELECT 列表达式解析结果。
///
/// 将 SELECT 列表中的每个表达式预解析为两种形式：
/// - `Column(usize)` — 列引用，记录在源表中的列索引
/// - `Literal(Value)` — 字面量（数字/字符串/NULL），直接返回固定值
enum SelectColResolver {
    /// 列引用：源表列索引
    Column(usize),
    /// 字面量值
    Literal(Value),
}

impl SelectColResolver {
    /// 对源行求值，返回对应的 Value
    fn eval(&self, src_row: &[Value]) -> Value {
        match self {
            SelectColResolver::Column(idx) => src_row.get(*idx).cloned().unwrap_or(Value::Null),
            SelectColResolver::Literal(val) => val.clone(),
        }
    }
}

/// 将 SELECT 列表达式解析为 `SelectColResolver`。
///
/// 优先尝试列名匹配（大小写不敏感），若不匹配则尝试字面量解析。
fn resolve_select_col(expr: &str, source_columns: &[OracleColumn]) -> SelectColResolver {
    let expr = expr.trim();

    // NULL 关键字
    if expr.eq_ignore_ascii_case("NULL") {
        return SelectColResolver::Literal(Value::Null);
    }

    // 字符串字面量
    if expr.starts_with('\'') && expr.ends_with('\'') && expr.len() >= 2 {
        let inner = &expr[1..expr.len() - 1];
        return SelectColResolver::Literal(Value::Text(inner.to_string()));
    }

    // 数字字面量（整数）
    if let Ok(n) = expr.parse::<i64>() {
        return SelectColResolver::Literal(Value::Int64(n));
    }
    // 数字字面量（浮点）
    if let Ok(f) = expr.parse::<f64>() {
        return SelectColResolver::Literal(Value::Float64(f));
    }

    // 列引用：处理 table.column 形式（取 . 后的部分）
    let col_name = if let Some(dot_pos) = expr.rfind('.') {
        &expr[dot_pos + 1..]
    } else {
        expr
    };

    // 在源表列中查找匹配的列名（大小写不敏感）
    for (i, col) in source_columns.iter().enumerate() {
        if col.name.eq_ignore_ascii_case(col_name) {
            return SelectColResolver::Column(i);
        }
    }

    // 无法识别的表达式，返回 NULL 字面量
    SelectColResolver::Literal(Value::Null)
}

/// 求值 SELECT 表达式为 Value（用于 DUAL 表等无源行场景）。
///
/// 仅解析字面量（数字/字符串/NULL），列引用返回 NULL。
fn eval_select_expr(expr: &str, _src_row: &[Value]) -> Value {
    let expr = expr.trim();

    // NULL 关键字
    if expr.eq_ignore_ascii_case("NULL") {
        return Value::Null;
    }

    // 字符串字面量
    if expr.starts_with('\'') && expr.ends_with('\'') && expr.len() >= 2 {
        let inner = &expr[1..expr.len() - 1];
        return Value::Text(inner.to_string());
    }

    // 数字字面量（整数）
    if let Ok(n) = expr.parse::<i64>() {
        return Value::Int64(n);
    }
    // 数字字面量（浮点）
    if let Ok(f) = expr.parse::<f64>() {
        return Value::Float64(f);
    }

    // 列引用（DUAL 表无列）— 返回 NULL
    Value::Null
}

/// 解析 VALUES 子句中的多个值元组。
///
/// 格式：`(v1, v2, ...), (v3, v4, ...), ...`
///
/// 返回每个元组的值字符串列表。
fn parse_value_tuples(s: &str) -> Result<Vec<Vec<String>>, AdapterError> {
    let s = s.trim();
    let mut tuples = Vec::new();

    let mut remaining = s;
    loop {
        let remaining_trimmed = remaining.trim_start();
        if remaining_trimmed.is_empty() {
            break;
        }
        if !remaining_trimmed.starts_with('(') {
            return Err(AdapterError::SqlParse(format!(
                "expected '(' in VALUES: {remaining}"
            )));
        }
        let close_pos = find_matching_paren(&remaining_trimmed[1..]).ok_or_else(|| {
            AdapterError::SqlParse(format!("unmatched '(' in VALUES: {remaining}"))
        })?;
        let inner = &remaining_trimmed[1..1 + close_pos];
        let values = split_args(inner);
        tuples.push(values);
        remaining = &remaining_trimmed[1 + close_pos + 1..];
        // 跳过可能的逗号分隔
        let after_comma = remaining.trim_start();
        if let Some(rest_after_comma) = after_comma.strip_prefix(',') {
            remaining = rest_after_comma;
        } else {
            remaining = after_comma;
        }
    }

    if tuples.is_empty() {
        return Err(AdapterError::SqlParse(format!("empty VALUES list: {s}")));
    }
    Ok(tuples)
}

/// 将单个 Oracle 值字符串转换为 SzRSQL [`Value`]。
///
/// 支持的字面量形式：
/// - `NULL` → [`Value::Null`]
/// - 整数 `123` / `-45` → [`Value::Int64`]
/// - 浮点数 `1.23` → [`Value::Float64`]
/// - 单引号字符串 `'hello'` / `'it''s'` → [`Value::Text`]（处理 `''` 转义）
/// - `TO_DATE('...', ...)` → [`Value::Timestamp`]（解析 YYYY-MM-DD[ HH:MM:SS]）
/// - `TO_TIMESTAMP('...', ...)` → [`Value::Timestamp`]
/// - `HEXTORAW('...')` → [`Value::Blob`]（十六进制解码）
/// - `SYSDATE` / `CURRENT_TIMESTAMP` → [`Value::Null`]（无运行时上下文）
/// - 其他无法识别的形式 → [`Value::Text`]（原样保留）
fn parse_oracle_value(s: &str) -> Value {
    let trimmed = s.trim();

    // NULL
    if trimmed.eq_ignore_ascii_case("NULL") {
        return Value::Null;
    }

    // SYSDATE / CURRENT_TIMESTAMP（无运行时上下文，返回 Null）
    if trimmed.eq_ignore_ascii_case("SYSDATE")
        || trimmed.eq_ignore_ascii_case("CURRENT_TIMESTAMP")
        || trimmed.eq_ignore_ascii_case("CURRENT_DATE")
    {
        return Value::Null;
    }

    // 单引号字符串字面量
    if trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2 {
        let inner = &trimmed[1..trimmed.len() - 1];
        // 处理 '' 转义为 '
        let unescaped = inner.replace("''", "'");
        return Value::Text(unescaped);
    }

    // TO_DATE('...', ...) / TO_TIMESTAMP('...', ...)
    let upper = trimmed.to_uppercase();
    if upper.starts_with("TO_DATE(") || upper.starts_with("TO_TIMESTAMP(") {
        // 提取第一个参数（单引号字符串）
        if let Some(value) = extract_first_string_arg(trimmed) {
            // 复用 types 模块的日期解析逻辑
            let oracle_type = if upper.starts_with("TO_DATE(") {
                OracleType::Date
            } else {
                OracleType::Timestamp { precision: 6 }
            };
            return oracle_type.to_value(&value).unwrap_or(Value::Null);
        }
        return Value::Null;
    }

    // HEXTORAW('...')
    if upper.starts_with("HEXTORAW(") {
        if let Some(hex) = extract_first_string_arg(trimmed) {
            return OracleType::Blob.to_value(&hex).unwrap_or(Value::Null);
        }
        return Value::Null;
    }

    // 数值字面量：整数
    if let Ok(n) = trimmed.parse::<i64>() {
        return Value::Int64(n);
    }

    // 数值字面量：浮点数（含小数点或科学计数法）
    if trimmed.contains('.') || trimmed.to_lowercase().contains('e') {
        if let Ok(f) = trimmed.parse::<f64>() {
            return Value::Float64(f);
        }
    }

    // 其他无法识别 → 原样作为 Text
    Value::Text(trimmed.to_string())
}

/// 从函数调用字符串中提取第一个单引号字符串参数。
///
/// 例如 `TO_DATE('2024-01-01', 'YYYY-MM-DD')` → `2024-01-01`
fn extract_first_string_arg(s: &str) -> Option<String> {
    let start = s.find('\'')?;
    let rest = &s[start + 1..];
    let mut result = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            // 检查是否为转义 ''
            if chars.peek() == Some(&'\'') {
                result.push('\'');
                chars.next();
                continue;
            }
            return Some(result);
        }
        result.push(c);
    }
    None
}

// =====================================================================
//  DDL/DML 生成
// =====================================================================

/// 生成 CREATE TABLE 语句。
fn generate_create_table(table: &OracleTable) -> String {
    let mut sql = format!("CREATE TABLE {} (\n", table.name);
    for (i, col) in table.columns.iter().enumerate() {
        sql.push_str("    ");
        sql.push_str(&col.name);
        sql.push(' ');
        sql.push_str(&col.oracle_type.to_ddl());
        if col.not_null {
            sql.push_str(" NOT NULL");
        }
        if i + 1 < table.columns.len() {
            sql.push(',');
        }
        sql.push('\n');
    }
    sql.push_str(");\n");
    sql
}

/// 生成单行 INSERT 语句。
fn generate_insert(table: &OracleTable, row: &[Value]) -> String {
    let mut sql = format!("INSERT INTO {} VALUES (", table.name);
    for (i, value) in row.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        sql.push_str(&OracleType::value_to_oracle_literal(value));
    }
    sql.push_str(");\n");
    sql
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OracleType;

    // -----------------------------------------------------------------
    //  构造测试
    // -----------------------------------------------------------------

    #[test]
    fn new_returns_default_adapter() {
        let adapter = OracleAdapter::new();
        let _ = format!("{adapter:?}");
    }

    #[test]
    fn default_equals_new() {
        let from_new = OracleAdapter::new();
        let from_default = OracleAdapter::default();
        assert_eq!(format!("{from_new:?}"), format!("{from_default:?}"));
    }

    // -----------------------------------------------------------------
    //  convert_sql 测试（委托给 OracleDialect）
    // -----------------------------------------------------------------

    #[test]
    fn convert_sql_delegates_to_dialect() {
        let adapter = OracleAdapter::new();
        let result = adapter
            .convert_sql("SELECT NVL(name, 'N/A') FROM dual")
            .unwrap();
        assert!(result.contains("COALESCE(name, 'N/A')"));
    }

    #[test]
    fn convert_sql_invalid_returns_error() {
        let adapter = OracleAdapter::new();
        let result = adapter.convert_sql("SELECT FROM WHERE");
        assert!(matches!(result, Err(AdapterError::Dialect(_))));
    }

    // -----------------------------------------------------------------
    //  import_from_oracle 测试
    // -----------------------------------------------------------------

    #[test]
    fn import_from_oracle_empty_script_returns_empty_vec() {
        let adapter = OracleAdapter::new();
        let tables = adapter.import_from_oracle("").unwrap();
        assert!(tables.is_empty());
    }

    #[test]
    fn import_from_oracle_skips_comments_and_blank() {
        let adapter = OracleAdapter::new();
        let script = "-- this is a comment\n\n   \n-- another comment";
        let tables = adapter.import_from_oracle(script).unwrap();
        assert!(tables.is_empty());
    }

    #[test]
    fn import_from_oracle_parses_create_table_basic() {
        let adapter = OracleAdapter::new();
        let script = "CREATE TABLE users (id NUMBER, name VARCHAR2(100))";
        let tables = adapter.import_from_oracle(script).unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].name, "users");
        assert_eq!(tables[0].columns.len(), 2);
        assert_eq!(tables[0].columns[0].name, "id");
        assert_eq!(tables[0].columns[1].name, "name");
        assert!(tables[0].rows.is_empty());
    }

    #[test]
    fn import_from_oracle_parses_create_table_with_types() {
        let adapter = OracleAdapter::new();
        let script = "CREATE TABLE products (
            id NUMBER(10) NOT NULL,
            price NUMBER(10, 2),
            name VARCHAR2(200 CHAR),
            code CHAR(10 BYTE),
            created DATE,
            ts TIMESTAMP(6),
            data BLOB,
            raw_data RAW(100),
            description CLOB
        )";
        let tables = adapter.import_from_oracle(script).unwrap();
        assert_eq!(tables.len(), 1);
        let cols = &tables[0].columns;
        assert_eq!(cols.len(), 9);

        assert_eq!(cols[0].name, "id");
        assert_eq!(cols[0].oracle_type, OracleType::number(10, 0).unwrap());
        assert!(cols[0].not_null);

        assert_eq!(cols[1].name, "price");
        assert_eq!(cols[1].oracle_type, OracleType::number(10, 2).unwrap());
        assert!(!cols[1].not_null);

        assert_eq!(
            cols[2].oracle_type,
            OracleType::Varchar2 {
                size: 200,
                char_semantics: true
            }
        );
        assert_eq!(
            cols[3].oracle_type,
            OracleType::Char {
                size: 10,
                char_semantics: false
            }
        );
        assert_eq!(cols[4].oracle_type, OracleType::Date);
        assert_eq!(cols[5].oracle_type, OracleType::timestamp(6).unwrap());
        assert_eq!(cols[6].oracle_type, OracleType::Blob);
        assert_eq!(cols[7].oracle_type, OracleType::Raw { size: 100 });
        assert_eq!(cols[8].oracle_type, OracleType::Clob);
    }

    #[test]
    fn import_from_oracle_parses_insert_values() {
        let adapter = OracleAdapter::new();
        let script = "CREATE TABLE users (id NUMBER, name VARCHAR2(100));
                      INSERT INTO users VALUES (1, 'Alice');
                      INSERT INTO users VALUES (2, 'Bob');";
        let tables = adapter.import_from_oracle(script).unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].rows.len(), 2);
        assert_eq!(tables[0].rows[0][0], Value::Int64(1));
        assert_eq!(tables[0].rows[0][1], Value::Text("Alice".to_string()));
        assert_eq!(tables[0].rows[1][0], Value::Int64(2));
        assert_eq!(tables[0].rows[1][1], Value::Text("Bob".to_string()));
    }

    #[test]
    fn import_from_oracle_handles_escaped_quotes_in_string() {
        let adapter = OracleAdapter::new();
        let script = "CREATE TABLE t (s VARCHAR2(100)); INSERT INTO t VALUES ('it''s a test');";
        let tables = adapter.import_from_oracle(script).unwrap();
        assert_eq!(tables[0].rows[0][0], Value::Text("it's a test".to_string()));
    }

    #[test]
    fn import_from_oracle_handles_null_value() {
        let adapter = OracleAdapter::new();
        let script = "CREATE TABLE t (a NUMBER, b VARCHAR2(50));
                      INSERT INTO t VALUES (NULL, 'x');";
        let tables = adapter.import_from_oracle(script).unwrap();
        assert_eq!(tables[0].rows[0][0], Value::Null);
        assert_eq!(tables[0].rows[0][1], Value::Text("x".to_string()));
    }

    #[test]
    fn import_from_oracle_handles_to_date_and_hextoraw() {
        let adapter = OracleAdapter::new();
        let script = "CREATE TABLE t (d DATE, b BLOB);
                      INSERT INTO t VALUES (TO_DATE('2024-01-01', 'YYYY-MM-DD'), HEXTORAW('DEADBEEF'));";
        let tables = adapter.import_from_oracle(script).unwrap();
        // TO_DATE → Timestamp
        match &tables[0].rows[0][0] {
            Value::Timestamp(_) => {}
            other => panic!("expected Timestamp, got {other:?}"),
        }
        // HEXTORAW → Blob
        assert_eq!(
            tables[0].rows[0][1],
            Value::Blob(vec![0xDE, 0xAD, 0xBE, 0xEF])
        );
    }

    #[test]
    fn import_from_oracle_skips_unrecognized_statements() {
        let adapter = OracleAdapter::new();
        let script = "CREATE TABLE t (id NUMBER);
                      CREATE INDEX idx ON t(id);
                      DROP TABLE old_table;
                      COMMIT;
                      INSERT INTO t VALUES (1);";
        let tables = adapter.import_from_oracle(script).unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].name, "t");
        assert_eq!(tables[0].rows.len(), 1);
    }

    #[test]
    fn import_from_oracle_rejects_insert_to_unknown_table() {
        let adapter = OracleAdapter::new();
        let script = "INSERT INTO nonexistent VALUES (1)";
        let result = adapter.import_from_oracle(script);
        assert!(matches!(result, Err(AdapterError::SqlParse(_))));
    }

    #[test]
    fn import_from_oracle_insert_select_from_dual() {
        // INSERT INTO ... SELECT ... FROM dual 现已支持
        let adapter = OracleAdapter::new();
        let script = "CREATE TABLE t (id NUMBER); INSERT INTO t SELECT 1 FROM dual";
        let tables = adapter.import_from_oracle(script).unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].rows.len(), 1);
        assert_eq!(tables[0].rows[0][0], Value::Int64(1));
    }

    #[test]
    fn import_from_oracle_insert_select_star() {
        // INSERT INTO target SELECT * FROM source
        let adapter = OracleAdapter::new();
        let script = "CREATE TABLE src (id NUMBER, name VARCHAR2(50));
                      INSERT INTO src VALUES (1, 'Alice');
                      INSERT INTO src VALUES (2, 'Bob');
                      CREATE TABLE dst (id NUMBER, name VARCHAR2(50));
                      INSERT INTO dst SELECT * FROM src";
        let tables = adapter.import_from_oracle(script).unwrap();
        assert_eq!(tables.len(), 2);
        let dst = tables.iter().find(|t| t.name == "dst").unwrap();
        assert_eq!(dst.rows.len(), 2);
        assert_eq!(dst.rows[0][0], Value::Int64(1));
        assert_eq!(dst.rows[0][1], Value::Text("Alice".to_string()));
        assert_eq!(dst.rows[1][0], Value::Int64(2));
    }

    #[test]
    fn import_from_oracle_insert_select_columns() {
        // INSERT INTO target (col1, col2) SELECT col1, col2 FROM source
        let adapter = OracleAdapter::new();
        let script = "CREATE TABLE src (id NUMBER, name VARCHAR2(50));
                      INSERT INTO src VALUES (1, 'Alice');
                      CREATE TABLE dst (id NUMBER, name VARCHAR2(50));
                      INSERT INTO dst (id, name) SELECT id, name FROM src";
        let tables = adapter.import_from_oracle(script).unwrap();
        let dst = tables.iter().find(|t| t.name == "dst").unwrap();
        assert_eq!(dst.rows.len(), 1);
        assert_eq!(dst.rows[0][0], Value::Int64(1));
        assert_eq!(dst.rows[0][1], Value::Text("Alice".to_string()));
    }

    #[test]
    fn import_from_oracle_handles_multiple_tables() {
        let adapter = OracleAdapter::new();
        let script = "CREATE TABLE a (x NUMBER);
                      CREATE TABLE b (y VARCHAR2(50));
                      INSERT INTO a VALUES (1);
                      INSERT INTO b VALUES ('hello');
                      INSERT INTO a VALUES (2);";
        let tables = adapter.import_from_oracle(script).unwrap();
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].name, "a");
        assert_eq!(tables[0].rows.len(), 2);
        assert_eq!(tables[1].name, "b");
        assert_eq!(tables[1].rows.len(), 1);
    }

    // -----------------------------------------------------------------
    //  export_to_oracle 测试
    // -----------------------------------------------------------------

    #[test]
    fn export_to_oracle_empty_tables_returns_empty_string() {
        let adapter = OracleAdapter::new();
        let result = adapter.export_to_oracle(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn export_to_oracle_generates_create_table() {
        let adapter = OracleAdapter::new();
        let table = OracleTable {
            name: "users".to_string(),
            columns: vec![
                OracleColumn {
                    name: "id".to_string(),
                    oracle_type: OracleType::number(10, 0).unwrap(),
                    not_null: true,
                },
                OracleColumn {
                    name: "name".to_string(),
                    oracle_type: OracleType::varchar2(100, true).unwrap(),
                    not_null: false,
                },
            ],
            rows: vec![],
        };
        let sql = adapter.export_to_oracle(&[table]).unwrap();
        assert!(sql.contains("CREATE TABLE users"));
        assert!(sql.contains("id NUMBER(10) NOT NULL"));
        assert!(sql.contains("name VARCHAR2(100 CHAR)"));
    }

    #[test]
    fn export_to_oracle_generates_insert_with_literals() {
        let adapter = OracleAdapter::new();
        let table = OracleTable {
            name: "t".to_string(),
            columns: vec![
                OracleColumn {
                    name: "id".to_string(),
                    oracle_type: OracleType::number_default(),
                    not_null: false,
                },
                OracleColumn {
                    name: "name".to_string(),
                    oracle_type: OracleType::varchar2(100, true).unwrap(),
                    not_null: false,
                },
            ],
            rows: vec![vec![Value::Int64(42), Value::Text("Alice".to_string())]],
        };
        let sql = adapter.export_to_oracle(&[table]).unwrap();
        assert!(sql.contains("INSERT INTO t VALUES"));
        assert!(sql.contains("42"));
        assert!(sql.contains("'Alice'"));
    }

    #[test]
    fn export_to_oracle_handles_null_and_blob() {
        let adapter = OracleAdapter::new();
        let table = OracleTable {
            name: "t".to_string(),
            columns: vec![
                OracleColumn {
                    name: "a".to_string(),
                    oracle_type: OracleType::number_default(),
                    not_null: false,
                },
                OracleColumn {
                    name: "b".to_string(),
                    oracle_type: OracleType::Blob,
                    not_null: false,
                },
            ],
            rows: vec![vec![Value::Null, Value::Blob(vec![0xDE, 0xAD])]],
        };
        let sql = adapter.export_to_oracle(&[table]).unwrap();
        assert!(sql.contains("NULL"));
        assert!(sql.contains("HEXTORAW('DEAD')"));
    }

    // -----------------------------------------------------------------
    //  往返测试：export → import
    // -----------------------------------------------------------------

    #[test]
    fn roundtrip_export_then_import_preserves_structure() {
        let adapter = OracleAdapter::new();
        let original = OracleTable {
            name: "products".to_string(),
            columns: vec![
                OracleColumn {
                    name: "id".to_string(),
                    oracle_type: OracleType::number(10, 0).unwrap(),
                    not_null: true,
                },
                OracleColumn {
                    name: "name".to_string(),
                    oracle_type: OracleType::varchar2(100, true).unwrap(),
                    not_null: false,
                },
            ],
            rows: vec![
                vec![Value::Int64(1), Value::Text("Widget".to_string())],
                vec![Value::Int64(2), Value::Text("Gadget".to_string())],
            ],
        };

        // 导出为 Oracle SQL 脚本
        let script = adapter
            .export_to_oracle(std::slice::from_ref(&original))
            .unwrap();

        // 重新导入
        let imported = adapter.import_from_oracle(&script).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].name, "products");
        assert_eq!(imported[0].columns.len(), 2);
        assert_eq!(imported[0].columns[0].name, "id");
        assert!(imported[0].columns[0].not_null);
        assert_eq!(imported[0].rows.len(), 2);
        assert_eq!(imported[0].rows[0][0], Value::Int64(1));
        assert_eq!(imported[0].rows[0][1], Value::Text("Widget".to_string()));
    }

    // -----------------------------------------------------------------
    //  辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn split_statements_handles_strings_with_semicolons() {
        let script = "INSERT INTO t VALUES ('a;b'); INSERT INTO t VALUES ('c');";
        let stmts = split_statements(script);
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].contains("'a;b'"));
        assert!(stmts[1].contains("'c'"));
    }

    #[test]
    fn split_statements_handles_trailing_without_semicolon() {
        let stmts = split_statements("SELECT 1; SELECT 2");
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[1], "SELECT 2");
    }

    #[test]
    fn find_matching_paren_handles_nested() {
        assert_eq!(find_matching_paren("a, b)"), Some(4));
        assert_eq!(find_matching_paren("a(x, y), b)"), Some(10));
        assert_eq!(find_matching_paren("a(b(c)), d)"), Some(10));
        assert_eq!(find_matching_paren("no close"), None);
    }

    #[test]
    fn parse_oracle_type_decl_number_variants() {
        let (t, _) = parse_oracle_type_decl("NUMBER").unwrap();
        assert_eq!(t, OracleType::number_default());

        let (t, _) = parse_oracle_type_decl("NUMBER(10)").unwrap();
        assert_eq!(t, OracleType::number(10, 0).unwrap());

        let (t, _) = parse_oracle_type_decl("NUMBER(10, 2)").unwrap();
        assert_eq!(t, OracleType::number(10, 2).unwrap());

        let (t, _) = parse_oracle_type_decl("INTEGER").unwrap();
        assert_eq!(t, OracleType::number(38, 0).unwrap());

        let (t, _) = parse_oracle_type_decl("DATE").unwrap();
        assert_eq!(t, OracleType::Date);

        let (t, _) = parse_oracle_type_decl("CLOB").unwrap();
        assert_eq!(t, OracleType::Clob);
    }

    #[test]
    fn parse_oracle_type_decl_rejects_unknown_type() {
        let result = parse_oracle_type_decl("UNSUPPORTED_TYPE(10)");
        assert!(matches!(result, Err(AdapterError::SqlParse(_))));
    }

    #[test]
    fn parse_oracle_value_integers_and_floats() {
        assert_eq!(parse_oracle_value("42"), Value::Int64(42));
        assert_eq!(parse_oracle_value("-17"), Value::Int64(-17));
        // 使用 2.5 而非 3.14 以避免 clippy::approx_constant 误报
        assert_eq!(parse_oracle_value("2.5"), Value::Float64(2.5));
    }

    #[test]
    fn parse_oracle_value_strings_and_null() {
        assert_eq!(parse_oracle_value("NULL"), Value::Null);
        assert_eq!(parse_oracle_value("null"), Value::Null);
        assert_eq!(
            parse_oracle_value("'hello'"),
            Value::Text("hello".to_string())
        );
        assert_eq!(
            parse_oracle_value("'it''s'"),
            Value::Text("it's".to_string())
        );
    }

    #[test]
    fn parse_oracle_value_sysdate_returns_null() {
        // 无运行时上下文，SYSDATE 返回 Null
        assert_eq!(parse_oracle_value("SYSDATE"), Value::Null);
        assert_eq!(parse_oracle_value("CURRENT_TIMESTAMP"), Value::Null);
    }

    #[test]
    fn parse_oracle_value_unknown_falls_back_to_text() {
        assert_eq!(
            parse_oracle_value("SOME_FUNCTION(1, 2)"),
            Value::Text("SOME_FUNCTION(1, 2)".to_string())
        );
    }

    #[test]
    fn parse_value_tuples_multiple_tuples() {
        let tuples = parse_value_tuples("(1, 'a'), (2, 'b'), (3, 'c')").unwrap();
        assert_eq!(tuples.len(), 3);
        assert_eq!(tuples[0].len(), 2);
        assert_eq!(tuples[0][0], "1");
        assert_eq!(tuples[0][1], "'a'");
        assert_eq!(tuples[2][0], "3");
    }

    #[test]
    fn oracle_table_builder_methods() {
        let mut table = OracleTable::new("test");
        table
            .add_column(OracleColumn {
                name: "id".to_string(),
                oracle_type: OracleType::number_default(),
                not_null: true,
            })
            .add_row(vec![Value::Int64(1)]);

        assert_eq!(table.name, "test");
        assert_eq!(table.columns.len(), 1);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0][0], Value::Int64(1));
    }

    #[test]
    fn extract_identifier_handles_quoted_names() {
        let (name, rest) = extract_identifier("\"my column\" rest").unwrap();
        assert_eq!(name, "my column");
        assert_eq!(rest, " rest");

        let (name, rest) = extract_identifier("schema.table rest").unwrap();
        assert_eq!(name, "schema.table");
        assert_eq!(rest, " rest");
    }

    #[test]
    fn parse_char_length_args_with_semantics() {
        let (size, char_sem) = parse_char_length_args("100 CHAR").unwrap();
        assert_eq!(size, 100);
        assert!(char_sem);

        let (size, char_sem) = parse_char_length_args("200 BYTE").unwrap();
        assert_eq!(size, 200);
        assert!(!char_sem);

        let (size, char_sem) = parse_char_length_args("50").unwrap();
        assert_eq!(size, 50);
        assert!(!char_sem); // 默认 BYTE
    }
}
