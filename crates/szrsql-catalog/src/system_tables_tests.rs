//! Phase 3.8 系统表测试 — pg_tables / pg_indexes 只读视图。
//!
//! 覆盖类别：
//! - pg_tables 空表（1）：空 catalog
//! - pg_tables 基本（1）：单表查询
//! - pg_tables 多表（1）：多表查询 + 顺序
//! - pg_tables hasindexes（1）：索引标志位
//! - pg_tables schema 名（2）：默认 public / 自定义 schema
//! - pg_indexes 空（1）：空 catalog
//! - pg_indexes 基本（1）：单索引查询
//! - pg_indexes 多列（1）：复合索引列顺序
//! - pg_indexes UNIQUE（1）：UNIQUE 关键字
//! - pg_indexes indexdef 格式（1）：CREATE INDEX 语句格式
//! - pg_indexes 表删除后（1）：DROP TABLE 后索引从系统表消失
//! - schema 函数（1）：schema_name 辅助函数
//! - 列常量（1）：PG_TABLES_COLUMNS / PG_INDEXES_COLUMNS
//!
//! 共 14 个测试用例。

use crate::system_tables::{
    pg_indexes, pg_tables, schema_name, PG_INDEXES_COLUMNS, PG_TABLES_COLUMNS,
};
use crate::{IndexInfo, ManagedCatalog, MutableCatalog};
use szrsql_sql::ast::{IndexColumn, TableName};
use szrsql_sql::plan::TableSchema;
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

fn make_schema(name: &str, columns: Vec<(&str, ColumnType)>) -> TableSchema {
    let table_name = TableName::new(name);
    let cols = columns
        .into_iter()
        .map(|(n, t)| szrsql_sql::ast::ColumnDefinition::new(n, t))
        .collect();
    TableSchema {
        name: table_name,
        columns: cols,
    }
}

fn make_index(name: &str, table: &str, columns: &[&str]) -> IndexInfo {
    IndexInfo::new(
        name,
        TableName::new(table),
        columns.iter().map(|c| IndexColumn::new(*c)).collect(),
    )
}

fn make_unique_index(name: &str, table: &str, columns: &[&str]) -> IndexInfo {
    IndexInfo::new_unique(
        name,
        TableName::new(table),
        columns.iter().map(|c| IndexColumn::new(*c)).collect(),
    )
}

fn make_desc_index(name: &str, table: &str, columns: &[(&str, bool)]) -> IndexInfo {
    let cols: Vec<IndexColumn> = columns
        .iter()
        .map(|(c, asc)| {
            let mut col = IndexColumn::new(*c);
            col.asc = *asc;
            col
        })
        .collect();
    IndexInfo::new(name, TableName::new(table), cols)
}

fn make_schema_with_custom_schema(schemaname: &str, name: &str) -> TableSchema {
    let table_name = TableName::with_schema(schemaname, name);
    TableSchema {
        name: table_name,
        columns: vec![szrsql_sql::ast::ColumnDefinition::new(
            "id",
            ColumnType::Int64,
        )],
    }
}

// =====================================================================
//  pg_tables 空表（1）
// =====================================================================

#[test]
fn test_pg_tables_empty() {
    let catalog = ManagedCatalog::new();
    let rows = pg_tables(&catalog);
    assert!(rows.is_empty());
}

// =====================================================================
//  pg_tables 基本（1）
// =====================================================================

#[test]
fn test_pg_tables_single_table() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();

    let rows = pg_tables(&catalog);
    assert_eq!(rows.len(), 1);

    // (schemaname, tablename, tableowner, hasindexes)
    let row = &rows[0];
    assert_eq!(row.len(), 4);
    assert_eq!(row[0], Value::Text("public".into()));
    assert_eq!(row[1], Value::Text("users".into()));
    assert_eq!(row[2], Value::Text("szrsql".into()));
    assert_eq!(row[3], Value::Int64(0)); // 无索引
}

// =====================================================================
//  pg_tables 多表（1）
// =====================================================================

#[test]
fn test_pg_tables_multiple_tables() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();
    catalog
        .create_table(
            make_schema("orders", vec![("id", ColumnType::Int64)]),
            false,
        )
        .unwrap();
    catalog
        .create_table(
            make_schema("products", vec![("id", ColumnType::Int64)]),
            false,
        )
        .unwrap();

    let rows = pg_tables(&catalog);
    assert_eq!(rows.len(), 3);

    let names: Vec<String> = rows
        .iter()
        .map(|r| {
            if let Value::Text(s) = &r[1] {
                s.clone()
            } else {
                panic!("expected Text")
            }
        })
        .collect();
    assert!(names.contains(&"users".to_string()));
    assert!(names.contains(&"orders".to_string()));
    assert!(names.contains(&"products".to_string()));
}

// =====================================================================
//  pg_tables hasindexes（1）
// =====================================================================

#[test]
fn test_pg_tables_hasindexes_flag() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();
    catalog
        .create_table(
            make_schema("orders", vec![("id", ColumnType::Int64)]),
            false,
        )
        .unwrap();
    catalog
        .create_index(make_index("idx_users_id", "users", &["id"]), false)
        .unwrap();

    let rows = pg_tables(&catalog);
    let by_name: std::collections::HashMap<String, i64> = rows
        .iter()
        .map(|r| {
            let name = if let Value::Text(s) = &r[1] {
                s.clone()
            } else {
                panic!("expected Text")
            };
            let has_idx = if let Value::Int64(n) = &r[3] {
                *n
            } else {
                panic!("expected Int64")
            };
            (name, has_idx)
        })
        .collect();

    assert_eq!(by_name.get("users"), Some(&1)); // 有索引
    assert_eq!(by_name.get("orders"), Some(&0)); // 无索引
}

// =====================================================================
//  pg_tables schema 名（2）
// =====================================================================

#[test]
fn test_pg_tables_default_schema_is_public() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();

    let rows = pg_tables(&catalog);
    let row = &rows[0];
    assert_eq!(row[0], Value::Text("public".into()));
}

#[test]
fn test_pg_tables_custom_schema() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema_with_custom_schema("my_app", "users"), false)
        .unwrap();

    let rows = pg_tables(&catalog);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row[0], Value::Text("my_app".into()));
    assert_eq!(row[1], Value::Text("users".into()));
}

// =====================================================================
//  pg_indexes 空（1）
// =====================================================================

#[test]
fn test_pg_indexes_empty() {
    let catalog = ManagedCatalog::new();
    let rows = pg_indexes(&catalog);
    assert!(rows.is_empty());
}

// =====================================================================
//  pg_indexes 基本（1）
// =====================================================================

#[test]
fn test_pg_indexes_single_index() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();
    catalog
        .create_index(make_index("idx_users_id", "users", &["id"]), false)
        .unwrap();

    let rows = pg_indexes(&catalog);
    assert_eq!(rows.len(), 1);

    // (schemaname, tablename, indexname, indexdef)
    let row = &rows[0];
    assert_eq!(row.len(), 4);
    assert_eq!(row[0], Value::Text("public".into()));
    assert_eq!(row[1], Value::Text("users".into()));
    assert_eq!(row[2], Value::Text("idx_users_id".into()));

    if let Value::Text(def) = &row[3] {
        assert!(def.contains("CREATE INDEX"));
        assert!(def.contains("idx_users_id"));
        assert!(def.contains("ON public.users"));
        assert!(def.contains("id ASC"));
    } else {
        panic!("expected Text for indexdef");
    }
}

// =====================================================================
//  pg_indexes 多列（1）
// =====================================================================

#[test]
fn test_pg_indexes_multi_column() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema(
                "orders",
                vec![
                    ("user_id", ColumnType::Int64),
                    ("created_at", ColumnType::Text),
                ],
            ),
            false,
        )
        .unwrap();
    catalog
        .create_index(
            make_index("idx_orders_user_date", "orders", &["user_id", "created_at"]),
            false,
        )
        .unwrap();

    let rows = pg_indexes(&catalog);
    assert_eq!(rows.len(), 1);

    if let Value::Text(def) = &rows[0][3] {
        assert!(def.contains("user_id ASC"));
        assert!(def.contains("created_at ASC"));
    } else {
        panic!("expected Text for indexdef");
    }
}

// =====================================================================
//  pg_indexes UNIQUE（1）
// =====================================================================

#[test]
fn test_pg_indexes_unique() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();
    catalog
        .create_index(
            make_unique_index("idx_users_id_unique", "users", &["id"]),
            false,
        )
        .unwrap();

    let rows = pg_indexes(&catalog);
    assert_eq!(rows.len(), 1);

    if let Value::Text(def) = &rows[0][3] {
        assert!(def.starts_with("CREATE UNIQUE INDEX"));
    } else {
        panic!("expected Text for indexdef");
    }
}

// =====================================================================
//  pg_indexes indexdef 格式（1）
// =====================================================================

#[test]
fn test_pg_indexes_indexdef_format() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema(
                "users",
                vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
            ),
            false,
        )
        .unwrap();
    catalog
        .create_index(
            make_desc_index("idx_users_desc", "users", &[("id", false), ("name", true)]),
            false,
        )
        .unwrap();

    let rows = pg_indexes(&catalog);
    assert_eq!(rows.len(), 1);

    if let Value::Text(def) = &rows[0][3] {
        // 验证完整格式
        assert_eq!(
            def,
            "CREATE INDEX idx_users_desc ON public.users (id DESC NULLS LAST, name ASC NULLS LAST)"
        );
    } else {
        panic!("expected Text for indexdef");
    }
}

// =====================================================================
//  pg_indexes 表删除后（1）
// =====================================================================

#[test]
fn test_pg_indexes_dropped_with_table() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();
    catalog
        .create_index(make_index("idx_users_id", "users", &["id"]), false)
        .unwrap();

    assert_eq!(pg_indexes(&catalog).len(), 1);

    // DROP TABLE — 索引应从系统表消失
    catalog
        .drop_table(&TableName::new("users"), false, true)
        .unwrap();

    assert_eq!(pg_indexes(&catalog).len(), 0);
    assert_eq!(pg_tables(&catalog).len(), 0);
}

// =====================================================================
//  schema 函数（1）
// =====================================================================

#[test]
fn test_schema_name_helper() {
    assert_eq!(schema_name(&TableName::new("users")), "public");
    assert_eq!(
        schema_name(&TableName::with_schema("my_app", "users")),
        "my_app"
    );
    assert_eq!(
        schema_name(&TableName::with_schema("PUBLIC", "users")),
        "PUBLIC"
    );
}

// =====================================================================
//  列常量（1）
// =====================================================================

#[test]
fn test_system_table_column_constants() {
    assert_eq!(
        PG_TABLES_COLUMNS,
        &["schemaname", "tablename", "tableowner", "hasindexes"]
    );
    assert_eq!(
        PG_INDEXES_COLUMNS,
        &["schemaname", "tablename", "indexname", "indexdef"]
    );
}
