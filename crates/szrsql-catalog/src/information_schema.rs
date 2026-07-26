//! Phase 3.19 information_schema — SQL 标准元数据视图。
//!
//! # 设计目标
//!
//! 实现 ANSI SQL 标准的 `information_schema` 4 个核心视图，覆盖 DBeaver / DataGrip
//! 等数据库工具的表结构浏览需求：
//! - **`tables`** — 表清单（TABLE_CATALOG / TABLE_SCHEMA / TABLE_NAME / TABLE_TYPE）
//! - **`columns`** — 列清单（TABLE_*/COLUMN_NAME/ORDINAL_POSITION/COLUMN_DEFAULT/
//!   IS_NULLABLE/DATA_TYPE/CHARACTER_MAXIMUM_LENGTH/NUMERIC_PRECISION/NUMERIC_SCALE）
//! - **`table_constraints`** — 约束清单（CONSTRAINT_*/TABLE_*/CONSTRAINT_TYPE）
//! - **`referential_constraints`** — 外键约束详情（CONSTRAINT_*/UNIQUE_CONSTRAINT_*/
//!   MATCH_OPTION/UPDATE_RULE/DELETE_RULE）
//!
//! # 与 ANSI SQL 的差异
//!
//! - **TABLE_CATALOG**：固定为 `"szrsql"`（单数据库实例，可通过参数注入实际名）
//! - **TABLE_TYPE**：仅 `"BASE TABLE"`（SzRSQL 不支持 CREATE VIEW）
//! - **CHARACTER_MAXIMUM_LENGTH**：始终 NULL（SzRSQL TEXT 类型不限制长度）
//! - **COLUMN_DEFAULT**：仅支持字面量表达式（`Expr::Literal`），其他表达式返回 NULL
//! - **约束仅列级可见**：`TableSchema` 不存储表级约束，
//!   `table_constraints` 仅从 `ColumnDefinition` 字段提取（与 `navicat::pg_constraint` 同源）
//! - **MATCH_OPTION**：始终 `"NONE"`（SzRSQL 不支持 MATCH FULL/PARTIAL）
//!
//! 对应 `SzRSQL实施进度.md` Phase 3.19。

use crate::MutableCatalog;
use szrsql_sql::ast::{ColumnDefinition, Expr, ForeignKeyReference, ReferenceAction, TableName};
use szrsql_sql::plan::TableSchema;
use szrsql_types::value::{ColumnType, Value};

/// 系统表行 — 与 `system_tables::SysRow` 同类型
pub type SysRow = Vec<Value>;

/// 默认 catalog 名（单数据库实例）
const DEFAULT_CATALOG: &str = "szrsql";

/// 计算 schema 名（None 时返回 "public"）
fn schema_name(name: &TableName) -> String {
    name.schema.clone().unwrap_or_else(|| "public".into())
}

// =====================================================================
//  辅助：Expr → SQL 文本（用于 COLUMN_DEFAULT）
// =====================================================================

/// 将 `Expr::Literal(Value)` 格式化为 SQL 字面量
///
/// 仅支持字面量表达式（其他表达式返回 None，COLUMN_DEFAULT 视为 NULL）：
/// - `Value::Int64(n)` → `n.to_string()`
/// - `Value::Float64(n)` → `n.to_string()`
/// - `Value::Text(s)` → `'escaped'`
/// - `Value::Bool(true)` → `TRUE` / `Value::Bool(false)` → `FALSE`
/// - `Value::Null` → `NULL`
/// - 其他类型 → None（暂不支持）
fn format_literal_expr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Literal(v) => match v {
            Value::Int64(n) => Some(n.to_string()),
            Value::Float64(n) => Some(n.to_string()),
            Value::Text(s) => Some(format!("'{}'", s.replace('\'', "''"))),
            Value::Bool(b) => Some(if *b {
                "TRUE".into()
            } else {
                "FALSE".into()
            }),
            Value::Null => Some("NULL".into()),
            Value::Date(days) => Some(format!("DATE '{days}'")),
            Value::Timestamp(us) => Some(format!("TIMESTAMP '{us}'")),
            Value::Decimal(v, scale) => {
                let s = format_decimal(*v, *scale);
                Some(s)
            }
            _ => None, // Blob / Array / Enum / Range / Json 暂不支持
        },
        _ => None, // 非字面量表达式暂不支持
    }
}

/// 格式化 Decimal(i128, scale) 为人类可读字符串
fn format_decimal(v: i128, scale: u8) -> String {
    if scale == 0 {
        return v.to_string();
    }
    let neg = v < 0;
    let abs = v.unsigned_abs();
    let s = abs.to_string();
    let scale = scale as usize;
    let (int_part, frac_part) = if s.len() > scale {
        let (i, f) = s.split_at(s.len() - scale);
        (i.to_string(), f.to_string())
    } else {
        let zeros = "0".repeat(scale - s.len());
        ("0".to_string(), format!("{zeros}{s}"))
    };
    let result = if frac_part.is_empty() {
        int_part
    } else {
        format!("{int_part}.{frac_part}")
    };
    if neg {
        format!("-{result}")
    } else {
        result
    }
}

// =====================================================================
//  tables 视图
// =====================================================================

/// `information_schema.tables` 列名
///
/// 列顺序：(TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE)
pub const TABLES_COLUMNS: &[&str] = &["TABLE_CATALOG", "TABLE_SCHEMA", "TABLE_NAME", "TABLE_TYPE"];

/// `information_schema.tables` 的 Schema
pub fn tables_schema() -> TableSchema {
    TableSchema {
        name: TableName::with_schema("information_schema", "tables"),
        columns: vec![
            ColumnDefinition::new("TABLE_CATALOG", ColumnType::Text),
            ColumnDefinition::new("TABLE_SCHEMA", ColumnType::Text),
            ColumnDefinition::new("TABLE_NAME", ColumnType::Text),
            ColumnDefinition::new("TABLE_TYPE", ColumnType::Text),
        ],
    }
}

/// 查询 `information_schema.tables`
///
/// 每行：`(TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE)`
/// - `TABLE_CATALOG` = `catalog` 参数（默认 `"szrsql"`）
/// - `TABLE_SCHEMA` = `TableName.schema` 或 `"public"`
/// - `TABLE_NAME` = `TableName.name`
/// - `TABLE_TYPE` = `"BASE TABLE"`（SzRSQL 不支持 CREATE VIEW）
pub fn tables(catalog: &dyn MutableCatalog) -> Vec<SysRow> {
    tables_with_catalog(catalog, DEFAULT_CATALOG)
}

/// `tables` 视图（可指定 catalog 名，Phase 4 pgwire 集成时使用）
pub fn tables_with_catalog(catalog: &dyn MutableCatalog, catalog_name: &str) -> Vec<SysRow> {
    catalog
        .list_tables()
        .into_iter()
        .map(|name| {
            vec![
                Value::Text(catalog_name.into()),
                Value::Text(schema_name(&name)),
                Value::Text(name.name.clone()),
                Value::Text("BASE TABLE".into()),
            ]
        })
        .collect()
}

// =====================================================================
//  columns 视图
// =====================================================================

/// `information_schema.columns` 列名
///
/// 列顺序：(TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, ORDINAL_POSITION,
///        COLUMN_DEFAULT, IS_NULLABLE, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH,
///        NUMERIC_PRECISION, NUMERIC_SCALE)
pub const COLUMNS_COLUMNS: &[&str] = &[
    "TABLE_CATALOG",
    "TABLE_SCHEMA",
    "TABLE_NAME",
    "COLUMN_NAME",
    "ORDINAL_POSITION",
    "COLUMN_DEFAULT",
    "IS_NULLABLE",
    "DATA_TYPE",
    "CHARACTER_MAXIMUM_LENGTH",
    "NUMERIC_PRECISION",
    "NUMERIC_SCALE",
];

/// `information_schema.columns` 的 Schema
pub fn columns_schema() -> TableSchema {
    TableSchema {
        name: TableName::with_schema("information_schema", "columns"),
        columns: vec![
            ColumnDefinition::new("TABLE_CATALOG", ColumnType::Text),
            ColumnDefinition::new("TABLE_SCHEMA", ColumnType::Text),
            ColumnDefinition::new("TABLE_NAME", ColumnType::Text),
            ColumnDefinition::new("COLUMN_NAME", ColumnType::Text),
            ColumnDefinition::new("ORDINAL_POSITION", ColumnType::Int64),
            ColumnDefinition::new("COLUMN_DEFAULT", ColumnType::Text),
            ColumnDefinition::new("IS_NULLABLE", ColumnType::Text),
            ColumnDefinition::new("DATA_TYPE", ColumnType::Text),
            ColumnDefinition::new("CHARACTER_MAXIMUM_LENGTH", ColumnType::Int64),
            ColumnDefinition::new("NUMERIC_PRECISION", ColumnType::Int64),
            ColumnDefinition::new("NUMERIC_SCALE", ColumnType::Int64),
        ],
    }
}

/// 查询 `information_schema.columns`
///
/// 每行列含义：
/// - `ORDINAL_POSITION`：1-indexed 列位置
/// - `COLUMN_DEFAULT`：NULL 或字面量 SQL 字符串（非字面量表达式返回 NULL）
/// - `IS_NULLABLE`：`"YES"` / `"NO"`（NOT NULL 或 PRIMARY KEY 列为 NO）
/// - `DATA_TYPE`：SQL 标准类型名（BIGINT / DOUBLE PRECISION / TEXT / BOOLEAN / DATE /
///   TIMESTAMP WITHOUT TIME ZONE / numeric(p,s) / bytea / json）
/// - `CHARACTER_MAXIMUM_LENGTH`：始终 NULL（TEXT 不限长度）
/// - `NUMERIC_PRECISION`：仅 DECIMAL 有值
/// - `NUMERIC_SCALE`：仅 DECIMAL 有值
pub fn columns(catalog: &dyn MutableCatalog) -> Vec<SysRow> {
    columns_with_catalog(catalog, DEFAULT_CATALOG)
}

/// `columns` 视图（可指定 catalog 名）
pub fn columns_with_catalog(catalog: &dyn MutableCatalog, catalog_name: &str) -> Vec<SysRow> {
    let mut rows = Vec::new();
    for name in catalog.list_tables() {
        let schema = match catalog.get_table(&name) {
            Some(s) => s,
            None => continue,
        };
        let table_schema = schema_name(&name);
        for (idx, col) in schema.columns.iter().enumerate() {
            let ordinal = (idx + 1) as i64;
            let column_default = col
                .default
                .as_ref()
                .and_then(format_literal_expr)
                .map(Value::Text)
                .unwrap_or(Value::Null);
            let is_nullable = if col.not_null || col.primary_key {
                "NO"
            } else {
                "YES"
            };
            let data_type = sql_data_type(&col.data_type);
            let (num_precision, num_scale) = numeric_precision_scale(&col.data_type);

            rows.push(vec![
                Value::Text(catalog_name.into()),
                Value::Text(table_schema.clone()),
                Value::Text(name.name.clone()),
                Value::Text(col.name.clone()),
                Value::Int64(ordinal),
                column_default,
                Value::Text(is_nullable.into()),
                Value::Text(data_type),
                Value::Null, // CHARACTER_MAXIMUM_LENGTH — TEXT 不限长度
                num_precision.map(Value::Int64).unwrap_or(Value::Null),
                num_scale.map(Value::Int64).unwrap_or(Value::Null),
            ]);
        }
    }
    rows
}

/// SzRSQL ColumnType → SQL 标准类型名
///
/// 返回 `DATA_TYPE` 列内容（与 `navicat::column_type_display` 一致，
/// 但使用 ANSI SQL 大写形式）
fn sql_data_type(ct: &ColumnType) -> String {
    match ct {
        ColumnType::Int64 => "BIGINT".into(),
        ColumnType::Float64 => "DOUBLE PRECISION".into(),
        ColumnType::Text => "TEXT".into(),
        ColumnType::Bool => "BOOLEAN".into(),
        ColumnType::Date => "DATE".into(),
        ColumnType::Timestamp => "TIMESTAMP WITHOUT TIME ZONE".into(),
        ColumnType::Decimal { precision, scale } => {
            format!("numeric({precision},{scale})")
        }
        ColumnType::Enum(_) => "TEXT".into(),
        ColumnType::Null => "TEXT".into(),
        ColumnType::Blob => "bytea".into(),
        ColumnType::Array(_) => "TEXT".into(),
        ColumnType::Range(_) => "TEXT".into(),
        ColumnType::Json => "json".into(),
        ColumnType::TsVector => "tsvector".into(),
        ColumnType::TsQuery => "tsquery".into(),
    }
}

/// 返回 (NUMERIC_PRECISION, NUMERIC_SCALE) — 仅 DECIMAL 类型有值
fn numeric_precision_scale(ct: &ColumnType) -> (Option<i64>, Option<i64>) {
    match ct {
        ColumnType::Decimal { precision, scale } => {
            (Some(i64::from(*precision)), Some(i64::from(*scale)))
        }
        // 其他类型暂不返回精度（PG 对 INT4/INT8/BIGINT 也返回精度，
        // 但 SzRSQL 信息模式简化处理，仅 DECIMAL 返回）
        _ => (None, None),
    }
}

// =====================================================================
//  table_constraints 视图
// =====================================================================

/// `information_schema.table_constraints` 列名
///
/// 列顺序：(CONSTRAINT_CATALOG, CONSTRAINT_SCHEMA, CONSTRAINT_NAME,
///        TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, CONSTRAINT_TYPE)
pub const TABLE_CONSTRAINTS_COLUMNS: &[&str] = &[
    "CONSTRAINT_CATALOG",
    "CONSTRAINT_SCHEMA",
    "CONSTRAINT_NAME",
    "TABLE_CATALOG",
    "TABLE_SCHEMA",
    "TABLE_NAME",
    "CONSTRAINT_TYPE",
];

/// `information_schema.table_constraints` 的 Schema
pub fn table_constraints_schema() -> TableSchema {
    TableSchema {
        name: TableName::with_schema("information_schema", "table_constraints"),
        columns: vec![
            ColumnDefinition::new("CONSTRAINT_CATALOG", ColumnType::Text),
            ColumnDefinition::new("CONSTRAINT_SCHEMA", ColumnType::Text),
            ColumnDefinition::new("CONSTRAINT_NAME", ColumnType::Text),
            ColumnDefinition::new("TABLE_CATALOG", ColumnType::Text),
            ColumnDefinition::new("TABLE_SCHEMA", ColumnType::Text),
            ColumnDefinition::new("TABLE_NAME", ColumnType::Text),
            ColumnDefinition::new("CONSTRAINT_TYPE", ColumnType::Text),
        ],
    }
}

/// 约束类型枚举（ANSI SQL 标准）
pub mod constraint_type {
    /// 主键
    pub const PRIMARY_KEY: &str = "PRIMARY KEY";
    /// 唯一
    pub const UNIQUE: &str = "UNIQUE";
    /// 外键
    pub const FOREIGN_KEY: &str = "FOREIGN KEY";
    /// 检查
    pub const CHECK: &str = "CHECK";
}

/// 查询 `information_schema.table_constraints`
///
/// 来源（仅列级约束 — 表级约束未存储到 `TableSchema`）：
/// - PRIMARY KEY：列级 `primary_key=true` 合并为单条约束
/// - UNIQUE：列级 `unique=true` 每列一条
/// - CHECK：列级 `check=Some` 每列一条
/// - FOREIGN KEY：列级 `references=Some` 每列一条
///
/// 约束命名规则：
/// - PRIMARY KEY：`{table}_pkey`
/// - UNIQUE：`{table}_{column}_key`
/// - CHECK：`{table}_{column}_check`
/// - FOREIGN KEY：`{table}_{column}_fkey`
pub fn table_constraints(catalog: &dyn MutableCatalog) -> Vec<SysRow> {
    table_constraints_with_catalog(catalog, DEFAULT_CATALOG)
}

/// `table_constraints` 视图（可指定 catalog 名）
pub fn table_constraints_with_catalog(
    catalog: &dyn MutableCatalog,
    catalog_name: &str,
) -> Vec<SysRow> {
    let mut rows = Vec::new();
    for name in catalog.list_tables() {
        let schema = match catalog.get_table(&name) {
            Some(s) => s,
            None => continue,
        };
        let table_schema = schema_name(&name);
        let table_name = &name.name;

        // 列级 PRIMARY KEY：合并为单条约束
        let has_pk = schema.columns.iter().any(|c| c.primary_key);
        if has_pk {
            rows.push(make_table_constraint_row(
                catalog_name,
                &table_schema,
                &format!("{table_name}_pkey"),
                &table_schema,
                table_name,
                constraint_type::PRIMARY_KEY,
            ));
        }

        // 列级 UNIQUE：每列一条
        for col in &schema.columns {
            if col.unique {
                rows.push(make_table_constraint_row(
                    catalog_name,
                    &table_schema,
                    &format!("{table_name}_{}_key", col.name),
                    &table_schema,
                    table_name,
                    constraint_type::UNIQUE,
                ));
            }
        }

        // 列级 CHECK：每列一条
        for col in &schema.columns {
            if col.check.is_some() {
                rows.push(make_table_constraint_row(
                    catalog_name,
                    &table_schema,
                    &format!("{table_name}_{}_check", col.name),
                    &table_schema,
                    table_name,
                    constraint_type::CHECK,
                ));
            }
        }

        // 列级 FOREIGN KEY：每列一条
        for col in &schema.columns {
            if col.references.is_some() {
                rows.push(make_table_constraint_row(
                    catalog_name,
                    &table_schema,
                    &format!("{table_name}_{}_fkey", col.name),
                    &table_schema,
                    table_name,
                    constraint_type::FOREIGN_KEY,
                ));
            }
        }
    }
    rows
}

/// 构造 table_constraints 单行
fn make_table_constraint_row(
    catalog: &str,
    constraint_schema: &str,
    constraint_name: &str,
    table_schema: &str,
    table_name: &str,
    ctype: &str,
) -> SysRow {
    vec![
        Value::Text(catalog.into()),
        Value::Text(constraint_schema.into()),
        Value::Text(constraint_name.into()),
        Value::Text(catalog.into()),
        Value::Text(table_schema.into()),
        Value::Text(table_name.into()),
        Value::Text(ctype.into()),
    ]
}

// =====================================================================
//  referential_constraints 视图
// =====================================================================

/// `information_schema.referential_constraints` 列名
///
/// 列顺序：(CONSTRAINT_CATALOG, CONSTRAINT_SCHEMA, CONSTRAINT_NAME,
///        UNIQUE_CONSTRAINT_CATALOG, UNIQUE_CONSTRAINT_SCHEMA, UNIQUE_CONSTRAINT_NAME,
///        MATCH_OPTION, UPDATE_RULE, DELETE_RULE)
pub const REFERENTIAL_CONSTRAINTS_COLUMNS: &[&str] = &[
    "CONSTRAINT_CATALOG",
    "CONSTRAINT_SCHEMA",
    "CONSTRAINT_NAME",
    "UNIQUE_CONSTRAINT_CATALOG",
    "UNIQUE_CONSTRAINT_SCHEMA",
    "UNIQUE_CONSTRAINT_NAME",
    "MATCH_OPTION",
    "UPDATE_RULE",
    "DELETE_RULE",
];

/// `information_schema.referential_constraints` 的 Schema
pub fn referential_constraints_schema() -> TableSchema {
    TableSchema {
        name: TableName::with_schema("information_schema", "referential_constraints"),
        columns: vec![
            ColumnDefinition::new("CONSTRAINT_CATALOG", ColumnType::Text),
            ColumnDefinition::new("CONSTRAINT_SCHEMA", ColumnType::Text),
            ColumnDefinition::new("CONSTRAINT_NAME", ColumnType::Text),
            ColumnDefinition::new("UNIQUE_CONSTRAINT_CATALOG", ColumnType::Text),
            ColumnDefinition::new("UNIQUE_CONSTRAINT_SCHEMA", ColumnType::Text),
            ColumnDefinition::new("UNIQUE_CONSTRAINT_NAME", ColumnType::Text),
            ColumnDefinition::new("MATCH_OPTION", ColumnType::Text),
            ColumnDefinition::new("UPDATE_RULE", ColumnType::Text),
            ColumnDefinition::new("DELETE_RULE", ColumnType::Text),
        ],
    }
}

/// 查询 `information_schema.referential_constraints`
///
/// 仅列出 FOREIGN KEY 约束（来源：列级 `references=Some`）
///
/// - `UNIQUE_CONSTRAINT_NAME`：best-effort，若被引用表有 PK 则返回 `{ref_table}_pkey`，
///   否则返回 NULL
/// - `MATCH_OPTION`：始终 `"NONE"`（SzRSQL 不支持 MATCH FULL/PARTIAL）
/// - `UPDATE_RULE` / `DELETE_RULE`：`"NO ACTION"` / `"RESTRICT"` / `"CASCADE"` /
///   `"SET NULL"` / `"SET DEFAULT"`，默认 `"NO ACTION"`
pub fn referential_constraints(catalog: &dyn MutableCatalog) -> Vec<SysRow> {
    referential_constraints_with_catalog(catalog, DEFAULT_CATALOG)
}

/// `referential_constraints` 视图（可指定 catalog 名）
pub fn referential_constraints_with_catalog(
    catalog: &dyn MutableCatalog,
    catalog_name: &str,
) -> Vec<SysRow> {
    let mut rows = Vec::new();
    for name in catalog.list_tables() {
        let schema = match catalog.get_table(&name) {
            Some(s) => s,
            None => continue,
        };
        let table_schema = schema_name(&name);
        let table_name = &name.name;

        for col in &schema.columns {
            let fk = match &col.references {
                Some(f) => f,
                None => continue,
            };
            let constraint_name = format!("{table_name}_{}_fkey", col.name);
            let (unique_schema, unique_name) = lookup_unique_constraint(catalog, fk);

            rows.push(vec![
                Value::Text(catalog_name.into()),
                Value::Text(table_schema.clone()),
                Value::Text(constraint_name),
                Value::Text(catalog_name.into()),
                Value::Text(unique_schema),
                unique_name.map(Value::Text).unwrap_or(Value::Null),
                Value::Text("NONE".into()), // MATCH_OPTION 固定 NONE
                Value::Text(reference_rule_str(
                    fk.on_update.as_ref(),
                    ReferenceAction::NoAction,
                )),
                Value::Text(reference_rule_str(
                    fk.on_delete.as_ref(),
                    ReferenceAction::NoAction,
                )),
            ]);
        }
    }
    rows
}

/// 查找被引用表的 PK 约束名（best-effort）
///
/// 返回 (UNIQUE_CONSTRAINT_SCHEMA, Option<UNIQUE_CONSTRAINT_NAME>)
/// - 若被引用表存在且有 PK，返回 `(schema, Some("{ref_table}_pkey"))`
/// - 否则返回 `(schema, None)`
fn lookup_unique_constraint(
    catalog: &dyn MutableCatalog,
    fk: &ForeignKeyReference,
) -> (String, Option<String>) {
    let ref_schema = schema_name(&fk.table);
    if let Some(s) = catalog.get_table(&fk.table) {
        let has_pk = s.columns.iter().any(|c| c.primary_key);
        if has_pk {
            return (ref_schema, Some(format!("{}_pkey", fk.table.name)));
        }
    }
    (ref_schema, None)
}

/// ReferenceAction → ANSI SQL 规则字符串
fn reference_rule_str(action: Option<&ReferenceAction>, default: ReferenceAction) -> String {
    let a = action.copied().unwrap_or(default);
    match a {
        ReferenceAction::NoAction => "NO ACTION".into(),
        ReferenceAction::Restrict => "RESTRICT".into(),
        ReferenceAction::Cascade => "CASCADE".into(),
        ReferenceAction::SetNull => "SET NULL".into(),
        ReferenceAction::SetDefault => "SET DEFAULT".into(),
    }
}
