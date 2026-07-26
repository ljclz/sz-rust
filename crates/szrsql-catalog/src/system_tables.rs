//! 系统表 — PG 兼容的只读元数据视图（Phase 3.8）。
//!
//! # 支持的系统表
//!
//! - **`pg_tables`** — 列出所有用户表（schemaname / tablename / tableowner / hasindexes）
//! - **`pg_indexes`** — 列出所有索引（schemaname / tablename / indexname / indexdef）
//!
//! # 设计
//!
//! 系统表为**函数式视图**：每次调用实时从 Catalog 计算，不持久化。
//! Phase 4 pgwire 集成时，可将其作为 `SELECT * FROM pg_tables` 的数据源。
//!
//! 与 PG 的差异：
//! - `tableowner` 固定为 `"szrsql"`（当前无用户/角色系统）
//! - `schemaname` 取自 `TableName.schema`，None 时为 `"public"`（PG 默认 schema）
//! - `indexdef` 为简化格式：`CREATE [UNIQUE] INDEX name ON schema.table (col1 ASC, col2 DESC, ...)`
//! - 不包含 `tablespace` / `hasrules` / `hastriggers` / `rowsecurity` 等 PG 扩展字段

use crate::{IndexInfo, MutableCatalog};
use szrsql_sql::ast::{ColumnDefinition, TableName};
use szrsql_sql::plan::TableSchema;
use szrsql_types::value::{ColumnType, Value};

/// 系统表行 — 统一用 `Vec<Value>` 表示，列顺序由对应 `schema()` 定义
pub type SysRow = Vec<Value>;

/// schema 名解析 — None 时返回 "public"（PG 默认）
pub fn schema_name(name: &TableName) -> String {
    name.schema.clone().unwrap_or_else(|| "public".into())
}

fn column_def(name: &str, col_type: ColumnType) -> ColumnDefinition {
    ColumnDefinition::new(name, col_type)
}

// =====================================================================
//  pg_tables
// =====================================================================

/// `pg_tables` 系统表的列名
///
/// 列顺序：(schemaname, tablename, tableowner, hasindexes)
pub const PG_TABLES_COLUMNS: &[&str] = &["schemaname", "tablename", "tableowner", "hasindexes"];

/// `pg_tables` 系统表的 Schema
pub fn pg_tables_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_tables"),
        columns: vec![
            column_def("schemaname", ColumnType::Text),
            column_def("tablename", ColumnType::Text),
            column_def("tableowner", ColumnType::Text),
            column_def("hasindexes", ColumnType::Int64),
        ],
    }
}

/// 查询 `pg_tables` — 返回所有用户表
///
/// 每行：`(schemaname: Text, tablename: Text, tableowner: Text, hasindexes: Int64)`
/// - `hasindexes` = 1 表示该表有索引，0 表示无
///
/// 需要传入 `MutableCatalog` 以查询索引信息。
pub fn pg_tables(catalog: &dyn MutableCatalog) -> Vec<SysRow> {
    let tables = catalog.list_tables();
    tables
        .into_iter()
        .map(|name| {
            let schemaname = schema_name(&name);
            let tablename = name.name.clone();
            let tableowner = "szrsql".to_string();
            let hasindexes = if catalog.list_indexes_for_table(&name).is_empty() {
                0
            } else {
                1
            };
            vec![
                Value::Text(schemaname),
                Value::Text(tablename),
                Value::Text(tableowner),
                Value::Int64(hasindexes),
            ]
        })
        .collect()
}

// =====================================================================
//  pg_indexes
// =====================================================================

/// `pg_indexes` 系统表的列名
///
/// 列顺序：(schemaname, tablename, indexname, indexdef)
pub const PG_INDEXES_COLUMNS: &[&str] = &["schemaname", "tablename", "indexname", "indexdef"];

/// `pg_indexes` 系统表的 Schema
pub fn pg_indexes_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("pg_indexes"),
        columns: vec![
            column_def("schemaname", ColumnType::Text),
            column_def("tablename", ColumnType::Text),
            column_def("indexname", ColumnType::Text),
            column_def("indexdef", ColumnType::Text),
        ],
    }
}

/// 查询 `pg_indexes` — 返回所有索引
///
/// 每行：`(schemaname: Text, tablename: Text, indexname: Text, indexdef: Text)`
///
/// `indexdef` 格式：`CREATE [UNIQUE] INDEX name ON schema.tablename (col1 ASC NULLS LAST, ...)`
pub fn pg_indexes(catalog: &dyn MutableCatalog) -> Vec<SysRow> {
    MutableCatalog::list_indexes(catalog)
        .iter()
        .map(format_index_row)
        .collect()
}

/// 格式化单条索引为系统表行
fn format_index_row(idx: &IndexInfo) -> SysRow {
    let schemaname = schema_name(&idx.table);
    let tablename = idx.table.name.clone();
    let indexname = idx.name.clone();
    let indexdef = format_index_def(idx);
    vec![
        Value::Text(schemaname),
        Value::Text(tablename),
        Value::Text(indexname),
        Value::Text(indexdef),
    ]
}

/// 生成 `CREATE [UNIQUE] INDEX name ON schema.table (cols)` 语句
fn format_index_def(idx: &IndexInfo) -> String {
    let unique = if idx.unique {
        "UNIQUE "
    } else {
        ""
    };
    let schema = schema_name(&idx.table);
    let table = &idx.table.name;
    let cols: Vec<String> = idx
        .columns
        .iter()
        .map(|c| {
            let dir = if c.asc {
                "ASC"
            } else {
                "DESC"
            };
            let nulls = if c.nulls_first {
                " NULLS FIRST"
            } else {
                " NULLS LAST"
            };
            format!("{} {}{}", c.column, dir, nulls)
        })
        .collect();
    format!(
        "CREATE {unique}INDEX {} ON {}.{} ({})",
        idx.name,
        schema,
        table,
        cols.join(", ")
    )
}
