//! Phase 3.8 Catalog 单元测试 — 表创建/删除/列出/列信息查询 + 索引 DDL。
//!
//! 覆盖类别：
//! - 表 DDL 基础（4）：create_table / drop_table / list_tables / get_table
//! - 表 DDL IF 语义（4）：CREATE IF NOT EXISTS / CREATE 已存在错误 / DROP IF EXISTS / DROP 不存在错误
//! - 表名大小写（1）：大小写不敏感查找
//! - 列信息查询（2）：Schema 列定义 / find_column
//! - DROP TABLE 级联（1）：drop_table 同时删除关联索引
//! - 索引 DDL 基础（4）：create_index / drop_index / list_indexes / get_index
//! - 索引 UNIQUE（1）：UNIQUE 索引创建与查询
//! - 索引 IF 语义（4）：CREATE IF NOT EXISTS / CREATE 已存在错误 / DROP IF EXISTS / DROP 不存在错误
//! - 索引表不存在（1）：为不存在的表创建索引报错
//! - 索引按表查询（1）：list_indexes_for_table
//! - 索引名大小写（1）：大小写不敏感
//! - 便捷方法（2）：add_simple_table / add_table_constraint 占位
//!
//! 共 26 个测试用例。

use crate::{CatalogError, IndexInfo, ManagedCatalog, MutableCatalog};
use szrsql_sql::ast::{IndexColumn, TableName};
use szrsql_sql::plan::{Catalog, TableSchema};
use szrsql_types::value::ColumnType;

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

fn make_users_schema() -> TableSchema {
    make_schema(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    )
}

fn make_orders_schema() -> TableSchema {
    make_schema(
        "orders",
        vec![
            ("id", ColumnType::Int64),
            ("user_id", ColumnType::Int64),
            ("amount", ColumnType::Float64),
        ],
    )
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

// =====================================================================
//  表 DDL 基础（4）
// =====================================================================

#[test]
fn test_create_table_basic() {
    let mut catalog = ManagedCatalog::new();
    assert!(!catalog.table_exists(&TableName::new("users")));

    let schema = make_users_schema();
    catalog.create_table(schema, false).unwrap();

    assert!(catalog.table_exists(&TableName::new("users")));
    assert_eq!(catalog.table_count(), 1);
}

#[test]
fn test_drop_table_basic() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();
    assert!(catalog.table_exists(&TableName::new("users")));

    catalog
        .drop_table(&TableName::new("users"), false, true)
        .unwrap();

    assert!(!catalog.table_exists(&TableName::new("users")));
    assert_eq!(catalog.table_count(), 0);
}

#[test]
fn test_list_tables_basic() {
    let mut catalog = ManagedCatalog::new();
    assert!(catalog.list_tables().is_empty());

    catalog.create_table(make_users_schema(), false).unwrap();
    catalog.create_table(make_orders_schema(), false).unwrap();

    let tables = catalog.list_tables();
    assert_eq!(tables.len(), 2);
    let names: Vec<String> = tables.iter().map(|t| t.name.clone()).collect();
    assert!(names.contains(&"users".to_string()));
    assert!(names.contains(&"orders".to_string()));
}

#[test]
fn test_get_table_basic() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();

    let schema = catalog.get_table(&TableName::new("users")).unwrap();
    assert_eq!(schema.name.name, "users");
    assert_eq!(schema.columns.len(), 2);
    assert_eq!(schema.columns[0].name, "id");
    assert_eq!(schema.columns[1].name, "name");
}

// =====================================================================
//  表 DDL IF 语义（4）
// =====================================================================

#[test]
fn test_create_table_if_not_exists_silent_skip() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();

    // IF NOT EXISTS — 静默跳过
    let result = catalog.create_table(make_users_schema(), true);
    assert!(result.is_ok());
    assert_eq!(catalog.table_count(), 1);
}

#[test]
fn test_create_table_already_exists_error() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();

    // 不带 IF NOT EXISTS — 返回错误
    let result = catalog.create_table(make_users_schema(), false);
    assert_eq!(
        result,
        Err(CatalogError::TableAlreadyExists("users".into()))
    );
}

#[test]
fn test_drop_table_if_exists_silent_skip() {
    let mut catalog = ManagedCatalog::new();
    // 空表，DROP IF EXISTS — 静默跳过
    let result = catalog.drop_table(&TableName::new("users"), true, true);
    assert!(result.is_ok());
}

#[test]
fn test_drop_table_not_found_error() {
    let mut catalog = ManagedCatalog::new();
    let result = catalog.drop_table(&TableName::new("users"), false, true);
    assert_eq!(result, Err(CatalogError::TableNotFound("users".into())));
}

// =====================================================================
//  表名大小写（1）
// =====================================================================

#[test]
fn test_table_name_case_insensitive() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();

    // 大小写不敏感查找
    assert!(catalog.table_exists(&TableName::new("USERS")));
    assert!(catalog.table_exists(&TableName::new("Users")));
    assert!(catalog.table_exists(&TableName::new("users")));

    let schema = catalog.get_table(&TableName::new("USERS")).unwrap();
    assert_eq!(schema.name.name, "users");
}

// =====================================================================
//  列信息查询（2）
// =====================================================================

#[test]
fn test_get_table_columns_info() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_orders_schema(), false).unwrap();

    let schema = catalog.get_table(&TableName::new("orders")).unwrap();
    assert_eq!(schema.column_names(), vec!["id", "user_id", "amount"]);

    let id_col = schema.find_column("id").unwrap();
    assert_eq!(id_col.data_type, ColumnType::Int64);

    let amount_col = schema.find_column("amount").unwrap();
    assert_eq!(amount_col.data_type, ColumnType::Float64);

    // 大小写不敏感列查找
    assert!(schema.find_column("ID").is_some());
    assert!(schema.find_column("Amount").is_some());
    assert!(schema.find_column("nonexistent").is_none());
}

#[test]
fn test_get_table_not_found() {
    let catalog = ManagedCatalog::new();
    assert!(catalog.get_table(&TableName::new("nonexistent")).is_none());
}

// =====================================================================
//  DROP TABLE 级联（1）
// =====================================================================

#[test]
fn test_drop_table_cascades_to_indexes() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();
    catalog
        .create_index(make_index("idx_users_id", "users", &["id"]), false)
        .unwrap();
    catalog
        .create_index(make_index("idx_users_name", "users", &["name"]), false)
        .unwrap();
    assert_eq!(catalog.index_count(), 2);

    // DROP TABLE — 关联索引应被删除
    catalog
        .drop_table(&TableName::new("users"), false, true)
        .unwrap();

    assert_eq!(catalog.index_count(), 0);
    assert!(catalog.get_index("idx_users_id").is_none());
    assert!(catalog.get_index("idx_users_name").is_none());
}

// =====================================================================
//  索引 DDL 基础（4）
// =====================================================================

#[test]
fn test_create_index_basic() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();

    catalog
        .create_index(make_index("idx_users_id", "users", &["id"]), false)
        .unwrap();

    assert_eq!(catalog.index_count(), 1);
    let idx = catalog.get_index("idx_users_id").unwrap();
    assert_eq!(idx.name, "idx_users_id");
    assert_eq!(idx.table.name, "users");
    assert_eq!(idx.column_names(), vec!["id"]);
    assert!(!idx.unique);
}

#[test]
fn test_drop_index_basic() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();
    catalog
        .create_index(make_index("idx_users_id", "users", &["id"]), false)
        .unwrap();

    catalog.drop_index("idx_users_id", false).unwrap();
    assert_eq!(catalog.index_count(), 0);
    assert!(catalog.get_index("idx_users_id").is_none());
}

#[test]
fn test_list_indexes_basic() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();
    catalog.create_table(make_orders_schema(), false).unwrap();
    catalog
        .create_index(make_index("idx_users_id", "users", &["id"]), false)
        .unwrap();
    catalog
        .create_index(
            make_index("idx_orders_user_id", "orders", &["user_id"]),
            false,
        )
        .unwrap();

    let indexes = MutableCatalog::list_indexes(&catalog);
    assert_eq!(indexes.len(), 2);
    let names: Vec<String> = indexes.iter().map(|i| i.name.clone()).collect();
    assert!(names.contains(&"idx_users_id".to_string()));
    assert!(names.contains(&"idx_orders_user_id".to_string()));
}

#[test]
fn test_get_index_basic() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();
    catalog
        .create_index(make_index("idx_users_id", "users", &["id"]), false)
        .unwrap();

    let idx = catalog.get_index("idx_users_id").unwrap();
    assert_eq!(idx.name, "idx_users_id");
    assert_eq!(idx.table.name, "users");
}

// =====================================================================
//  索引 UNIQUE（1）
// =====================================================================

#[test]
fn test_create_index_unique() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();

    catalog
        .create_index(
            make_unique_index("idx_users_id_unique", "users", &["id"]),
            false,
        )
        .unwrap();

    let idx = catalog.get_index("idx_users_id_unique").unwrap();
    assert!(idx.unique);
}

// =====================================================================
//  索引 IF 语义（4）
// =====================================================================

#[test]
fn test_create_index_if_not_exists_silent_skip() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();
    catalog
        .create_index(make_index("idx_users_id", "users", &["id"]), false)
        .unwrap();

    let result = catalog.create_index(make_index("idx_users_id", "users", &["id"]), true);
    assert!(result.is_ok());
    assert_eq!(catalog.index_count(), 1);
}

#[test]
fn test_create_index_already_exists_error() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();
    catalog
        .create_index(make_index("idx_users_id", "users", &["id"]), false)
        .unwrap();

    let result = catalog.create_index(make_index("idx_users_id", "users", &["id"]), false);
    assert_eq!(
        result,
        Err(CatalogError::IndexAlreadyExists("idx_users_id".into()))
    );
}

#[test]
fn test_drop_index_if_exists_silent_skip() {
    let mut catalog = ManagedCatalog::new();
    let result = catalog.drop_index("nonexistent", true);
    assert!(result.is_ok());
}

#[test]
fn test_drop_index_not_found_error() {
    let mut catalog = ManagedCatalog::new();
    let result = catalog.drop_index("nonexistent", false);
    assert_eq!(
        result,
        Err(CatalogError::IndexNotFound("nonexistent".into()))
    );
}

// =====================================================================
//  索引表不存在（1）
// =====================================================================

#[test]
fn test_create_index_table_not_found() {
    let mut catalog = ManagedCatalog::new();
    // 未创建 users 表
    let result = catalog.create_index(make_index("idx", "users", &["id"]), false);
    assert_eq!(result, Err(CatalogError::TableNotFound("users".into())));
}

// =====================================================================
//  索引按表查询（1）
// =====================================================================

#[test]
fn test_list_indexes_for_table() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();
    catalog.create_table(make_orders_schema(), false).unwrap();
    catalog
        .create_index(make_index("idx_users_id", "users", &["id"]), false)
        .unwrap();
    catalog
        .create_index(make_index("idx_users_name", "users", &["name"]), false)
        .unwrap();
    catalog
        .create_index(
            make_index("idx_orders_user_id", "orders", &["user_id"]),
            false,
        )
        .unwrap();

    let users_indexes = catalog.list_indexes_for_table(&TableName::new("users"));
    assert_eq!(users_indexes.len(), 2);
    let names: Vec<String> = users_indexes.iter().map(|i| i.name.clone()).collect();
    assert!(names.contains(&"idx_users_id".to_string()));
    assert!(names.contains(&"idx_users_name".to_string()));

    let orders_indexes = catalog.list_indexes_for_table(&TableName::new("orders"));
    assert_eq!(orders_indexes.len(), 1);
    assert_eq!(orders_indexes[0].name, "idx_orders_user_id");

    // 不存在的表 — 返回空 Vec
    let empty = catalog.list_indexes_for_table(&TableName::new("nonexistent"));
    assert!(empty.is_empty());
}

// =====================================================================
//  索引名大小写（1）
// =====================================================================

#[test]
fn test_index_name_case_insensitive() {
    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();
    catalog
        .create_index(make_index("IDX_USERS_ID", "users", &["id"]), false)
        .unwrap();

    // 大小写不敏感查找
    assert!(catalog.get_index("idx_users_id").is_some());
    assert!(catalog.get_index("Idx_Users_Id").is_some());
    assert!(catalog.get_index("IDX_USERS_ID").is_some());

    // 大小写不敏感删除
    catalog.drop_index("idx_users_id", false).unwrap();
    assert!(catalog.get_index("IDX_USERS_ID").is_none());
}

// =====================================================================
//  便捷方法（2）
// =====================================================================

#[test]
fn test_add_simple_table_convenience() {
    let mut catalog = ManagedCatalog::new();
    catalog.add_simple_table("users", vec![("id", ColumnType::Int64)]);

    assert!(catalog.table_exists(&TableName::new("users")));
    let schema = catalog.get_table(&TableName::new("users")).unwrap();
    assert_eq!(schema.columns.len(), 1);
    assert_eq!(schema.columns[0].name, "id");

    // 重复调用 add_simple_table — IF NOT EXISTS 语义，不报错
    catalog.add_simple_table("users", vec![("id", ColumnType::Int64)]);
    assert_eq!(catalog.table_count(), 1);
}

#[test]
fn test_add_table_constraint_placeholder() {
    use szrsql_sql::ast::TableConstraint;

    let mut catalog = ManagedCatalog::new();
    catalog.create_table(make_users_schema(), false).unwrap();

    // 占位 API — 应返回 Ok
    let constraint = TableConstraint::PrimaryKey {
        name: None,
        columns: vec!["id".into()],
    };
    let result = catalog.add_table_constraint(&TableName::new("users"), constraint);
    assert!(result.is_ok());

    // 不存在的表 — 返回错误
    let constraint = TableConstraint::PrimaryKey {
        name: None,
        columns: vec!["id".into()],
    };
    let result = catalog.add_table_constraint(&TableName::new("nonexistent"), constraint);
    assert_eq!(
        result,
        Err(CatalogError::TableNotFound("nonexistent".into()))
    );
}

// =====================================================================
//  Phase TDengine-P2: COMMENT ON 存储测试
// =====================================================================

#[test]
fn test_comment_storage() {
    let mut catalog = ManagedCatalog::new();
    let table_name = TableName::new("test_table");

    // 设置表注释
    catalog
        .set_table_comment(&table_name, Some("测试表".to_string()))
        .unwrap();
    assert_eq!(
        catalog.get_table_comment(&table_name),
        Some("测试表".to_string())
    );

    // 删除表注释（传 None）
    catalog.set_table_comment(&table_name, None).unwrap();
    assert_eq!(catalog.get_table_comment(&table_name), None);

    // 设置列注释
    catalog
        .set_column_comment(&table_name, "name", Some("名称列".to_string()))
        .unwrap();
    assert_eq!(
        catalog.get_column_comment(&table_name, "name"),
        Some("名称列".to_string())
    );

    // 列名大小写不敏感（存储时统一转小写）
    assert_eq!(
        catalog.get_column_comment(&table_name, "NAME"),
        Some("名称列".to_string())
    );

    // 删除列注释
    catalog
        .set_column_comment(&table_name, "name", None)
        .unwrap();
    assert_eq!(catalog.get_column_comment(&table_name, "name"), None);
}
