//! Phase 3.18 Navicat 兼容测试 — pg_catalog 核心视图。
//!
//! 覆盖类别：
//! - pg_database（2）：数据库列表 + 模板标记
//! - pg_namespace（3）：默认 schema + 用户 schema + 空 catalog
//! - pg_class（4）：表对象 + 索引对象 + relkind 区分 + 空 catalog
//! - pg_attribute（4）：列类型映射 + attnum 顺序 + attnotnull + atthasdef
//! - pg_type（3）：固定类型 + 列数 + OID 反查
//! - pg_index（4）：普通索引 + UNIQUE + 主键索引 + indkey 计算
//! - pg_constraint（5）：PRIMARY KEY + UNIQUE + CHECK + FOREIGN KEY + 复合 PK
//! - pg_description（1）：已实现，从 catalog 读取对象注释
//! - pg_views（1）：已实现，从 catalog 读取视图列表
//! - OID 稳定性（2）：相同 catalog 多次调用 OID 一致 + 不同 catalog 相同表名 OID 一致
//! - 类型映射辅助（3）：column_type_to_oid + column_type_to_name + column_type_display
//! - DDL 片段（2）：column_ddl_fragment + foreign_key_reference_ddl
//! - pg_description 实时注释（2）：表注释 + 列注释
//! - pg_views 实时视图（1）：从 catalog 读取视图列表
//! - pg_proc（2）：内置函数列表 + schema 结构
//! - pg_cast（2）：类型转换规则 + schema 结构
//! - pg_operator（2）：运算符列表 + schema 结构
//! - pg_authid（2）：单用户 + 多用户
//! - pg_collation（2）：默认排序规则 + schema 结构
//! - pg_stat_activity（1）：当前连接信息
//! - pg_tablespace（2）：默认表空间 + schema 结构
//! - pg_settings（3）：配置参数 + allowed_databases + schema 结构
//! - pg_roles/pg_shadow/pg_user 多用户（3）
//!
//! 共 61 个测试用例。

use crate::navicat::{
    column_ddl_fragment, column_type_display, column_type_to_name, column_type_to_oid, contype,
    foreign_key_reference_ddl, oid_attribute, oid_class_index, oid_class_table, oid_constraint,
    oid_namespace, pg_attribute, pg_authid, pg_authid_schema, pg_cast, pg_cast_schema, pg_class,
    pg_class_schema, pg_collation, pg_collation_schema, pg_constraint, pg_database,
    pg_database_schema, pg_description, pg_description_schema, pg_index, pg_namespace, pg_operator,
    pg_operator_schema, pg_proc, pg_proc_schema, pg_roles, pg_roles_schema, pg_settings,
    pg_settings_schema, pg_shadow, pg_shadow_schema, pg_stat_activity, pg_tablespace,
    pg_tablespace_schema, pg_type, pg_type_oid, pg_type_schema, pg_user, pg_user_schema, pg_views,
    pg_views_schema, relkind, PG_AUTHID_COLUMNS, PG_CAST_COLUMNS, PG_CLASS_COLUMNS,
    PG_COLLATION_COLUMNS, PG_DATABASE_COLUMNS, PG_DESCRIPTION_COLUMNS, PG_OPERATOR_COLUMNS,
    PG_PROC_COLUMNS, PG_ROLES_COLUMNS, PG_SETTINGS_COLUMNS, PG_SHADOW_COLUMNS,
    PG_STAT_ACTIVITY_COLUMNS, PG_TABLESPACE_COLUMNS, PG_TYPE_COLUMNS, PG_USER_COLUMNS,
    PG_VIEWS_COLUMNS,
};
use crate::{IndexInfo, ManagedCatalog, MutableCatalog};
use szrsql_sql::ast::{
    ColumnDefinition, ForeignKeyReference, IndexColumn, ReferenceAction, Statement, TableName,
};
use szrsql_sql::materialized_view::ViewDefinition;
use szrsql_sql::parser::parse_one;
use szrsql_sql::plan::TableSchema;
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

fn make_schema(name: &str, columns: Vec<(&str, ColumnType)>) -> TableSchema {
    let table_name = TableName::new(name);
    let cols = columns
        .into_iter()
        .map(|(n, t)| ColumnDefinition::new(n, t))
        .collect();
    TableSchema {
        name: table_name,
        columns: cols,
    }
}

fn make_schema_full(name: &str, columns: Vec<ColumnDefinition>) -> TableSchema {
    TableSchema {
        name: TableName::new(name),
        columns,
    }
}

fn col_with_pk(name: &str, ct: ColumnType) -> ColumnDefinition {
    let mut c = ColumnDefinition::new(name, ct);
    c.primary_key = true;
    c.not_null = true;
    c
}

fn col_with_unique(name: &str, ct: ColumnType) -> ColumnDefinition {
    let mut c = ColumnDefinition::new(name, ct);
    c.unique = true;
    c
}

fn col_with_default(
    name: &str,
    ct: ColumnType,
    default: szrsql_sql::ast::Expr,
) -> ColumnDefinition {
    let mut c = ColumnDefinition::new(name, ct);
    c.default = Some(default);
    c
}

fn col_with_check(name: &str, ct: ColumnType, check: szrsql_sql::ast::Expr) -> ColumnDefinition {
    let mut c = ColumnDefinition::new(name, ct);
    c.check = Some(check);
    c
}

fn col_with_fk(name: &str, ct: ColumnType, fk: ForeignKeyReference) -> ColumnDefinition {
    let mut c = ColumnDefinition::new(name, ct);
    c.references = Some(fk);
    c
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

fn make_schema_with_schemaname(schema: &str, name: &str) -> TableSchema {
    TableSchema {
        name: TableName::with_schema(schema, name),
        columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
    }
}

// =====================================================================
//  pg_database 测试（2）
// =====================================================================

#[test]
fn test_pg_database_returns_current_db() {
    let rows = pg_database("szrsql");
    // 与 PG_DATABASE_COLUMNS 列顺序一致（14 列）
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].len(), 14);
    assert_eq!(rows[1].len(), 14);
    // template1
    assert_eq!(rows[0][0], Value::Int64(1));         // oid
    assert_eq!(rows[0][1], Value::Text("template1".into())); // datname
    assert_eq!(rows[0][6], Value::Bool(true));       // datistemplate
    assert_eq!(rows[0][7], Value::Bool(false));      // datallowconn
    assert_eq!(rows[0][9], Value::Int64(1255));      // datlastsysoid
    // 当前数据库
    assert_eq!(rows[1][0], Value::Int64(16384));     // oid
    assert_eq!(rows[1][1], Value::Text("szrsql".into())); // datname
    assert_eq!(rows[1][6], Value::Bool(false));      // datistemplate
    assert_eq!(rows[1][7], Value::Bool(true));       // datallowconn
}

#[test]
fn test_pg_database_schema() {
    let schema = pg_database_schema();
    assert_eq!(schema.name.name, "pg_database");
    assert_eq!(schema.columns.len(), 14);
    assert_eq!(PG_DATABASE_COLUMNS.len(), 14);
    // 验证 datlastsysoid 列存在
    assert!(schema.columns.iter().any(|c| c.name == "datlastsysoid"));
}

// =====================================================================
//  pg_namespace 测试（3）
// =====================================================================

#[test]
fn test_pg_namespace_default_schemas() {
    let catalog = ManagedCatalog::new();
    let rows = pg_namespace(&catalog);
    // 默认 3 个：pg_catalog / public / information_schema
    assert!(rows.len() >= 3);
    let names: Vec<String> = rows
        .iter()
        .map(|r| match &r[1] {
            Value::Text(s) => s.clone(),
            _ => String::new(),
        })
        .collect();
    assert!(names.contains(&"pg_catalog".into()));
    assert!(names.contains(&"public".into()));
    assert!(names.contains(&"information_schema".into()));
}

#[test]
fn test_pg_namespace_user_schema() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema_with_schemaname("myapp", "users"), false)
        .unwrap();
    let rows = pg_namespace(&catalog);
    let names: Vec<String> = rows
        .iter()
        .map(|r| match &r[1] {
            Value::Text(s) => s.clone(),
            _ => String::new(),
        })
        .collect();
    assert!(names.contains(&"myapp".into()));
}

#[test]
fn test_pg_namespace_schema_oid_stable() {
    let oid1 = oid_namespace("public");
    let oid2 = oid_namespace("public");
    assert_eq!(oid1, oid2);
    // 不同 schema 名应有不同 OID
    let oid_other = oid_namespace("myapp");
    assert_ne!(oid1, oid_other);
}

// =====================================================================
//  pg_class 测试（4）
// =====================================================================

#[test]
fn test_pg_class_empty_catalog() {
    let catalog = ManagedCatalog::new();
    let rows = pg_class(&catalog);
    assert!(rows.is_empty());
}

#[test]
fn test_pg_class_table_relation() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();
    let rows = pg_class(&catalog);
    assert_eq!(rows.len(), 1);
    // pg_class 31 列结构：relkind 在索引 16，relnatts 在索引 17
    let row = &rows[0];
    assert_eq!(row.len(), 31);
    assert_eq!(row[1], Value::Text("users".into())); // relname
    assert_eq!(row[16], Value::Text(relkind::RELATION.into())); // relkind
    assert_eq!(row[17], Value::Int64(1)); // relnatts = 1
}

#[test]
fn test_pg_class_index_object() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();
    catalog
        .create_index(make_index("idx_users_id", "users", &["id"]), false)
        .unwrap();
    let rows = pg_class(&catalog);
    // 1 表 + 1 索引
    assert_eq!(rows.len(), 2);
    // pg_class 31 列结构：relkind 在索引 16
    let index_row = rows
        .iter()
        .find(|r| r[16] == Value::Text(relkind::INDEX.into()))
        .expect("应包含索引对象");
    assert_eq!(index_row[1], Value::Text("idx_users_id".into()));
}

#[test]
fn test_pg_class_schema_columns() {
    let schema = pg_class_schema();
    assert_eq!(schema.name.name, "pg_class");
    assert_eq!(schema.columns.len(), 31);
    assert_eq!(PG_CLASS_COLUMNS.len(), 31);
    // 验证 Navicat 常用列存在
    assert!(schema.columns.iter().any(|c| c.name == "relowner"));
    assert!(schema.columns.iter().any(|c| c.name == "reltablespace"));
    assert!(schema.columns.iter().any(|c| c.name == "relhastriggers"));
    assert!(schema.columns.iter().any(|c| c.name == "relrowsecurity"));
}

// =====================================================================
//  pg_attribute 测试（4）
// =====================================================================

#[test]
fn test_pg_attribute_column_count() {
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
    let rows = pg_attribute(&catalog);
    assert_eq!(rows.len(), 2);
}

#[test]
fn test_pg_attribute_attnum_order() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema(
                "users",
                vec![
                    ("id", ColumnType::Int64),
                    ("name", ColumnType::Text),
                    ("age", ColumnType::Int64),
                ],
            ),
            false,
        )
        .unwrap();
    let rows = pg_attribute(&catalog);
    // attnum 从 1 开始递增
    assert_eq!(rows[0][7], Value::Int64(1));
    assert_eq!(rows[1][7], Value::Int64(2));
    assert_eq!(rows[2][7], Value::Int64(3));
    // 列名匹配
    assert_eq!(rows[0][2], Value::Text("id".into()));
    assert_eq!(rows[1][2], Value::Text("name".into()));
    assert_eq!(rows[2][2], Value::Text("age".into()));
}

#[test]
fn test_pg_attribute_attnotnull_for_primary_key() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full(
                "users",
                vec![
                    col_with_pk("id", ColumnType::Int64),
                    ColumnDefinition::new("name", ColumnType::Text),
                ],
            ),
            false,
        )
        .unwrap();
    let rows = pg_attribute(&catalog);
    // id 列：PK → attnotnull=true
    let id_row = rows
        .iter()
        .find(|r| r[2] == Value::Text("id".into()))
        .unwrap();
    assert_eq!(id_row[5], Value::Bool(true));
    // name 列：无 NOT NULL → attnotnull=false
    let name_row = rows
        .iter()
        .find(|r| r[2] == Value::Text("name".into()))
        .unwrap();
    assert_eq!(name_row[5], Value::Bool(false));
}

#[test]
fn test_pg_attribute_atthasdef() {
    use szrsql_sql::ast::Expr;
    use szrsql_types::value::Value as V;
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full(
                "users",
                vec![
                    ColumnDefinition::new("id", ColumnType::Int64),
                    col_with_default("active", ColumnType::Bool, Expr::Literal(V::Bool(true))),
                ],
            ),
            false,
        )
        .unwrap();
    let rows = pg_attribute(&catalog);
    let active_row = rows
        .iter()
        .find(|r| r[2] == Value::Text("active".into()))
        .unwrap();
    assert_eq!(active_row[6], Value::Bool(true)); // atthasdef
    let id_row = rows
        .iter()
        .find(|r| r[2] == Value::Text("id".into()))
        .unwrap();
    assert_eq!(id_row[6], Value::Bool(false));
}

// =====================================================================
//  pg_type 测试（3）
// =====================================================================

#[test]
fn test_pg_type_returns_builtin_types() {
    let rows = pg_type();
    // 至少包含 SzRSQL 支持的 9 种核心类型
    assert!(rows.len() >= 9);
    let names: Vec<String> = rows
        .iter()
        .map(|r| match &r[1] {
            Value::Text(s) => s.clone(),
            _ => String::new(),
        })
        .collect();
    for expected in [
        "bool",
        "int8",
        "int4",
        "text",
        "float8",
        "varchar",
        "date",
        "timestamp",
        "numeric",
    ] {
        assert!(names.contains(&expected.into()), "缺少类型: {expected}");
    }
}

#[test]
fn test_pg_type_oid_consistent_with_pg() {
    let rows = pg_type();
    let find_oid = |name: &str| -> i64 {
        rows.iter()
            .find(|r| r[1] == Value::Text(name.into()))
            .map(|r| match r[0] {
                Value::Int64(oid) => oid,
                _ => -1,
            })
            .unwrap_or(-1)
    };
    assert_eq!(find_oid("bool"), pg_type_oid::BOOL);
    assert_eq!(find_oid("int8"), pg_type_oid::INT8);
    assert_eq!(find_oid("text"), pg_type_oid::TEXT);
    assert_eq!(find_oid("numeric"), pg_type_oid::NUMERIC);
}

#[test]
fn test_pg_type_schema() {
    let schema = pg_type_schema();
    assert_eq!(schema.name.name, "pg_type");
    assert_eq!(schema.columns.len(), 5);
    assert_eq!(PG_TYPE_COLUMNS.len(), 5);
}

// =====================================================================
//  pg_index 测试（4）
// =====================================================================

#[test]
fn test_pg_index_basic() {
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
        .create_index(make_index("idx_users_name", "users", &["name"]), false)
        .unwrap();
    let rows = pg_index(&catalog);
    assert_eq!(rows.len(), 1);
    // (indexrelid, indrelid, indkey, indisunique, indisprimary, indnatts)
    let row = &rows[0];
    assert_eq!(row[2], Value::Text("2".into())); // name 是第 2 列
    assert_eq!(row[3], Value::Bool(false)); // 非唯一
    assert_eq!(row[4], Value::Bool(false)); // 非主键
    assert_eq!(row[5], Value::Int64(1)); // 1 列
}

#[test]
fn test_pg_index_unique() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema(
                "users",
                vec![("id", ColumnType::Int64), ("email", ColumnType::Text)],
            ),
            false,
        )
        .unwrap();
    catalog
        .create_index(
            make_unique_index("idx_users_email_unique", "users", &["email"]),
            false,
        )
        .unwrap();
    let rows = pg_index(&catalog);
    let row = &rows[0];
    assert_eq!(row[3], Value::Bool(true)); // indisunique
    assert_eq!(row[4], Value::Bool(false)); // 非主键
}

#[test]
fn test_pg_index_primary_key_index() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full("users", vec![col_with_pk("id", ColumnType::Int64)]),
            false,
        )
        .unwrap();
    catalog
        .create_index(make_unique_index("users_pkey", "users", &["id"]), false)
        .unwrap();
    let rows = pg_index(&catalog);
    let row = &rows[0];
    assert_eq!(row[3], Value::Bool(true)); // UNIQUE
    assert_eq!(row[4], Value::Bool(true)); // 主键
}

#[test]
fn test_pg_index_indkey_multiple_columns() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema(
                "users",
                vec![
                    ("id", ColumnType::Int64),
                    ("name", ColumnType::Text),
                    ("age", ColumnType::Int64),
                ],
            ),
            false,
        )
        .unwrap();
    catalog
        .create_index(
            make_index("idx_users_name_age", "users", &["name", "age"]),
            false,
        )
        .unwrap();
    let rows = pg_index(&catalog);
    let row = &rows[0];
    assert_eq!(row[2], Value::Text("2 3".into())); // name=2, age=3
    assert_eq!(row[5], Value::Int64(2));
}

// =====================================================================
//  pg_constraint 测试（5）
// =====================================================================

#[test]
fn test_pg_constraint_primary_key() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full("users", vec![col_with_pk("id", ColumnType::Int64)]),
            false,
        )
        .unwrap();
    let rows = pg_constraint(&catalog);
    assert_eq!(rows.len(), 1);
    // (oid, conname, conrelid, contype, conkey)
    let row = &rows[0];
    assert_eq!(row[1], Value::Text("users_pkey".into()));
    assert_eq!(row[3], Value::Text(contype::PRIMARY_KEY.into()));
    assert_eq!(row[4], Value::Text("1".into())); // 第 1 列
}

#[test]
fn test_pg_constraint_unique() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full(
                "users",
                vec![
                    col_with_pk("id", ColumnType::Int64),
                    col_with_unique("email", ColumnType::Text),
                ],
            ),
            false,
        )
        .unwrap();
    let rows = pg_constraint(&catalog);
    // 1 PK + 1 UNIQUE
    let unique_rows: Vec<_> = rows
        .iter()
        .filter(|r| r[3] == Value::Text(contype::UNIQUE.into()))
        .collect();
    assert_eq!(unique_rows.len(), 1);
    assert_eq!(unique_rows[0][1], Value::Text("users_email_key".into()));
}

#[test]
fn test_pg_constraint_check() {
    use szrsql_sql::ast::{BinaryOp, Expr};
    use szrsql_types::value::Value as V;
    let mut catalog = ManagedCatalog::new();
    let age_col = col_with_check(
        "age",
        ColumnType::Int64,
        Expr::BinaryOp {
            op: BinaryOp::Gt,
            left: Box::new(Expr::Identifier(vec!["age".into()])),
            right: Box::new(Expr::Literal(V::Int64(0))),
        },
    );
    catalog
        .create_table(make_schema_full("users", vec![age_col]), false)
        .unwrap();
    let rows = pg_constraint(&catalog);
    let check_rows: Vec<_> = rows
        .iter()
        .filter(|r| r[3] == Value::Text(contype::CHECK.into()))
        .collect();
    assert_eq!(check_rows.len(), 1);
    assert_eq!(check_rows[0][1], Value::Text("users_age_check".into()));
}

#[test]
fn test_pg_constraint_foreign_key() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full("users", vec![col_with_pk("id", ColumnType::Int64)]),
            false,
        )
        .unwrap();
    let fk = ForeignKeyReference {
        table: TableName::new("users"),
        columns: Some(vec!["id".into()]),
        on_delete: Some(ReferenceAction::Cascade),
        on_update: None,
    };
    catalog
        .create_table(
            make_schema_full(
                "orders",
                vec![col_with_fk("user_id", ColumnType::Int64, fk)],
            ),
            false,
        )
        .unwrap();
    let rows = pg_constraint(&catalog);
    let fk_rows: Vec<_> = rows
        .iter()
        .filter(|r| r[3] == Value::Text(contype::FOREIGN_KEY.into()))
        .collect();
    assert_eq!(fk_rows.len(), 1);
    assert_eq!(fk_rows[0][1], Value::Text("orders_user_id_fkey".into()));
}

#[test]
fn test_pg_constraint_composite_primary_key() {
    let mut catalog = ManagedCatalog::new();
    // 复合 PK：两列均设 primary_key=true（简化模拟）
    let mut col_a = ColumnDefinition::new("a", ColumnType::Int64);
    col_a.primary_key = true;
    col_a.not_null = true;
    let mut col_b = ColumnDefinition::new("b", ColumnType::Int64);
    col_b.primary_key = true;
    col_b.not_null = true;
    catalog
        .create_table(make_schema_full("t", vec![col_a, col_b]), false)
        .unwrap();
    let rows = pg_constraint(&catalog);
    // 应只有 1 条 PK 约束（合并）
    let pk_rows: Vec<_> = rows
        .iter()
        .filter(|r| r[3] == Value::Text(contype::PRIMARY_KEY.into()))
        .collect();
    assert_eq!(pk_rows.len(), 1);
    assert_eq!(pk_rows[0][4], Value::Text("1 2".into())); // conkey = "1 2"
}

// =====================================================================
//  pg_description / pg_views 测试（2）
// =====================================================================

#[test]
fn test_pg_description_empty() {
    let catalog = ManagedCatalog::new();
    let rows = pg_description(&catalog);
    assert!(rows.is_empty());
    assert_eq!(PG_DESCRIPTION_COLUMNS.len(), 4);
    let schema = pg_description_schema();
    assert_eq!(schema.name.name, "pg_description");
}

#[test]
fn test_pg_views_empty() {
    let catalog = ManagedCatalog::new();
    let rows = pg_views(&catalog);
    assert!(rows.is_empty());
    assert_eq!(PG_VIEWS_COLUMNS.len(), 4);
    let schema = pg_views_schema();
    assert_eq!(schema.name.name, "pg_views");
}

// =====================================================================
//  pg_roles / pg_shadow / pg_user 测试（3）— Navicat JOIN 兼容
// =====================================================================

#[test]
fn test_pg_roles_single_postgres() {
    let rows = pg_roles(&[]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 10);
    assert_eq!(rows[0][0], Value::Int64(10)); // oid
    assert_eq!(rows[0][1], Value::Text("postgres".into())); // rolname
    assert_eq!(rows[0][2], Value::Bool(true)); // rolsuper
    assert_eq!(PG_ROLES_COLUMNS.len(), 10);
    let schema = pg_roles_schema();
    assert_eq!(schema.name.name, "pg_roles");
    assert_eq!(schema.columns.len(), 10);
}

#[test]
fn test_pg_shadow_single_postgres() {
    let rows = pg_shadow(&[]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 9);
    assert_eq!(rows[0][0], Value::Text("postgres".into())); // usename
    assert_eq!(rows[0][1], Value::Int64(10)); // usesysid
    assert_eq!(rows[0][3], Value::Bool(true)); // usesuper
    assert_eq!(PG_SHADOW_COLUMNS.len(), 9);
    let schema = pg_shadow_schema();
    assert_eq!(schema.name.name, "pg_shadow");
    assert_eq!(schema.columns.len(), 9);
}

#[test]
fn test_pg_user_single_postgres() {
    let rows = pg_user(&[]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 8);
    assert_eq!(rows[0][0], Value::Text("postgres".into())); // usename
    assert_eq!(rows[0][1], Value::Int64(10)); // usesysid
    assert_eq!(PG_USER_COLUMNS.len(), 8);
    let schema = pg_user_schema();
    assert_eq!(schema.name.name, "pg_user");
    assert_eq!(schema.columns.len(), 8);
}

// =====================================================================
//  OID 稳定性测试（2）
// =====================================================================

#[test]
fn test_oid_stability_same_catalog_multiple_calls() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();
    let oid1 = oid_class_table(&TableName::new("users"));
    let oid2 = oid_class_table(&TableName::new("users"));
    assert_eq!(oid1, oid2);
}

#[test]
fn test_oid_stability_different_catalog_same_name() {
    let oid1 = oid_class_table(&TableName::new("users"));
    let oid2 = oid_class_table(&TableName::new("users"));
    // OID 应基于 hash(qualified_name)，不依赖 catalog 实例
    assert_eq!(oid1, oid2);
}

// =====================================================================
//  类型映射辅助函数测试（3）
// =====================================================================

#[test]
fn test_column_type_to_oid_all_variants() {
    assert_eq!(column_type_to_oid(&ColumnType::Int64), pg_type_oid::INT8);
    assert_eq!(
        column_type_to_oid(&ColumnType::Float64),
        pg_type_oid::FLOAT8
    );
    assert_eq!(column_type_to_oid(&ColumnType::Text), pg_type_oid::TEXT);
    assert_eq!(column_type_to_oid(&ColumnType::Bool), pg_type_oid::BOOL);
    assert_eq!(column_type_to_oid(&ColumnType::Date), pg_type_oid::DATE);
    assert_eq!(
        column_type_to_oid(&ColumnType::Timestamp),
        pg_type_oid::TIMESTAMP
    );
    assert_eq!(
        column_type_to_oid(&ColumnType::Decimal {
            precision: 10,
            scale: 2
        }),
        pg_type_oid::NUMERIC
    );
    // 扩展类型映射为 TEXT
    assert_eq!(
        column_type_to_oid(&ColumnType::Enum(vec![])),
        pg_type_oid::TEXT
    );
    assert_eq!(column_type_to_oid(&ColumnType::Null), pg_type_oid::TEXT);
    assert_eq!(column_type_to_oid(&ColumnType::Blob), pg_type_oid::TEXT);
    assert_eq!(column_type_to_oid(&ColumnType::Json), pg_type_oid::TEXT);
}

#[test]
fn test_column_type_to_name_all_variants() {
    assert_eq!(column_type_to_name(&ColumnType::Int64), "int8");
    assert_eq!(column_type_to_name(&ColumnType::Text), "text");
    assert_eq!(column_type_to_name(&ColumnType::Bool), "bool");
    assert_eq!(
        column_type_to_name(&ColumnType::Decimal {
            precision: 10,
            scale: 2
        }),
        "numeric"
    );
    assert_eq!(column_type_to_name(&ColumnType::Json), "json");
    assert_eq!(column_type_to_name(&ColumnType::Blob), "bytea");
}

#[test]
fn test_column_type_display_format() {
    assert_eq!(column_type_display(&ColumnType::Int64), "bigint");
    assert_eq!(
        column_type_display(&ColumnType::Float64),
        "double precision"
    );
    assert_eq!(column_type_display(&ColumnType::Text), "text");
    assert_eq!(column_type_display(&ColumnType::Bool), "boolean");
    assert_eq!(column_type_display(&ColumnType::Date), "date");
    assert_eq!(
        column_type_display(&ColumnType::Timestamp),
        "timestamp without time zone"
    );
    assert_eq!(
        column_type_display(&ColumnType::Decimal {
            precision: 10,
            scale: 2
        }),
        "numeric(10,2)"
    );
    assert_eq!(column_type_display(&ColumnType::Json), "json");
}

// =====================================================================
//  DDL 片段辅助函数测试（2）
// =====================================================================

#[test]
fn test_column_ddl_fragment_basic() {
    let col = ColumnDefinition::new("name", ColumnType::Text);
    assert_eq!(column_ddl_fragment(&col), "name text");
}

#[test]
fn test_column_ddl_fragment_with_constraints() {
    let mut col = ColumnDefinition::new("id", ColumnType::Int64);
    col.not_null = true;
    col.primary_key = true;
    let ddl = column_ddl_fragment(&col);
    assert!(ddl.contains("NOT NULL"));
    assert!(ddl.contains("PRIMARY KEY"));
    assert!(ddl.starts_with("id bigint"));
}

#[test]
fn test_foreign_key_reference_ddl() {
    let fk = ForeignKeyReference {
        table: TableName::new("users"),
        columns: Some(vec!["id".into()]),
        on_delete: Some(ReferenceAction::Cascade),
        on_update: Some(ReferenceAction::SetNull),
    };
    let ddl = foreign_key_reference_ddl(&fk);
    assert!(ddl.contains("REFERENCES users (id)"));
    assert!(ddl.contains("ON DELETE CASCADE"));
    assert!(ddl.contains("ON UPDATE SET NULL"));
}

// =====================================================================
//  OID 段隔离测试（1）
// =====================================================================

#[test]
fn test_oid_segment_isolation() {
    // 不同 OID 段不冲突
    // 注意：内置 schema (pg_catalog/public/information_schema) 使用 PG 硬编码 OID，
    // 不在 10000+ 段内。用自定义 schema 验证 hash 段隔离。
    let ns_oid = oid_namespace("myapp");
    let class_oid = oid_class_table(&TableName::new("myapp.users"));
    let idx_oid = oid_class_index("users_pkey");
    let attr_oid = oid_attribute(class_oid, 1);
    let con_oid = oid_constraint("users_pkey", class_oid);

    // OID 段隔离（仅对自定义 schema）：
    // - namespace: 10000 + hash & 0xFFFF (范围 10000-75535)
    // - class table: 20000 + hash & 0xFFFFF (范围 20000-1245755)
    // - class index: 30000 + hash & 0xFFFFF (范围 30000-1255755)
    // - attribute: 40000 + hash & 0xFFFFF (范围 40000-1265755)
    // - constraint: 50000 + hash & 0xFFFFF (范围 50000-1275755)
    assert!(ns_oid >= 10000, "namespace OID 应 >= 10000, 实际: {ns_oid}");
    assert!(
        class_oid >= 20000,
        "class table OID 应 >= 20000, 实际: {class_oid}"
    );
    assert!(
        idx_oid >= 30000,
        "class index OID 应 >= 30000, 实际: {idx_oid}"
    );
    assert!(
        attr_oid >= 40000,
        "attribute OID 应 >= 40000, 实际: {attr_oid}"
    );
    assert!(
        con_oid >= 50000,
        "constraint OID 应 >= 50000, 实际: {con_oid}"
    );

    // 内置 schema 使用 PG 硬编码 OID（与 pg_namespace() 返回值一致）
    assert_eq!(oid_namespace("pg_catalog"), 11, "pg_catalog OID 应为 11");
    assert_eq!(oid_namespace("public"), 2200, "public OID 应为 2200");
    assert_eq!(
        oid_namespace("information_schema"),
        13078,
        "information_schema OID 应为 13078"
    );

    // 不同段 OID 不应相等（极低概率冲突，验证基础隔离）
    let mut oids = vec![ns_oid, class_oid, idx_oid, attr_oid, con_oid];
    oids.sort();
    oids.dedup();
    assert_eq!(oids.len(), 5, "5 个不同段 OID 不应重复");
}

// =====================================================================
//  pg_description 实时注释测试（2）— 从 catalog comments 字段读取
// =====================================================================

#[test]
fn test_pg_description_with_table_comment() {
    let mut catalog = ManagedCatalog::new();
    let table_name = TableName::new("users");
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();
    catalog
        .set_table_comment(&table_name, Some("用户表".into()))
        .unwrap();
    let rows = pg_description(&catalog);
    // 表级注释：1 行
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 4);
    // objsubid=0 表示表级注释
    assert_eq!(rows[0][2], Value::Int64(0));
    assert_eq!(rows[0][3], Value::Text("用户表".into()));
    // classoid 应为 pg_class 的 OID (1259)
    assert_eq!(rows[0][1], Value::Int64(1259));
}

#[test]
fn test_pg_description_with_column_comment() {
    let mut catalog = ManagedCatalog::new();
    let table_name = TableName::new("users");
    catalog
        .create_table(
            make_schema(
                "users",
                vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
            ),
            false,
        )
        .unwrap();
    // 设置列注释
    catalog
        .set_column_comment(&table_name, "id", Some("主键ID".into()))
        .unwrap();
    catalog
        .set_column_comment(&table_name, "name", Some("用户名".into()))
        .unwrap();
    let rows = pg_description(&catalog);
    // 2 行列注释
    assert_eq!(rows.len(), 2);
    // 第一列 attnum=1，第二列 attnum=2
    let id_row = rows
        .iter()
        .find(|r| r[2] == Value::Int64(1))
        .expect("应包含 id 列注释");
    assert_eq!(id_row[3], Value::Text("主键ID".into()));
    let name_row = rows
        .iter()
        .find(|r| r[2] == Value::Int64(2))
        .expect("应包含 name 列注释");
    assert_eq!(name_row[3], Value::Text("用户名".into()));
}

// =====================================================================
//  pg_views 实时视图测试（1）— 从 catalog 视图列表读取
// =====================================================================

/// 辅助函数：从 SQL 字符串构造 ViewDefinition
fn make_view(name: &str, sql: &str) -> ViewDefinition {
    let table_name = TableName::new(name);
    let stmt = parse_one(sql).unwrap();
    let query = match stmt {
        Statement::Select(s) => s,
        other => panic!("expected Select, got {other:?}"),
    };
    ViewDefinition::new_view(table_name, query)
}

#[test]
fn test_pg_views_with_views() {
    let mut catalog = ManagedCatalog::new();
    catalog.add_view(make_view("active_users", "SELECT id FROM users"));
    catalog.add_view(make_view("admin_list", "SELECT id FROM admins"));
    let rows = pg_views(&catalog);
    assert_eq!(rows.len(), 2);
    // 每行 4 列：schemaname, viewname, viewowner, definition
    assert_eq!(rows[0].len(), 4);
    // viewowner 固定为 postgres
    assert!(rows.iter().all(|r| r[2] == Value::Text("postgres".into())));
    // schemaname 默认为 public
    assert!(rows.iter().all(|r| r[0] == Value::Text("public".into())));
    // 验证视图名存在
    let view_names: Vec<String> = rows
        .iter()
        .map(|r| match &r[1] {
            Value::Text(s) => s.clone(),
            _ => String::new(),
        })
        .collect();
    assert!(view_names.contains(&"active_users".into()));
    assert!(view_names.contains(&"admin_list".into()));
}

// =====================================================================
//  pg_proc 测试（2）— 内置函数列表
// =====================================================================

#[test]
fn test_pg_proc_returns_builtin_functions() {
    let rows = pg_proc();
    // 至少包含 24 个内置函数
    assert!(rows.len() >= 24);
    // 每行 30 列（与 PG_PROC_COLUMNS 一致）
    assert_eq!(rows[0].len(), 30);
    assert_eq!(PG_PROC_COLUMNS.len(), 30);
    // 验证关键函数存在
    let func_names: Vec<String> = rows
        .iter()
        .map(|r| match &r[1] {
            Value::Text(s) => s.clone(),
            _ => String::new(),
        })
        .collect();
    for expected in ["count", "sum", "avg", "min", "max", "now", "length", "lower"] {
        assert!(
            func_names.contains(&expected.into()),
            "缺少内置函数: {expected}"
        );
    }
    // OID 从 20000 开始递增
    assert_eq!(rows[0][0], Value::Int64(20000));
    // pronamespace 应为 public 的 OID (2200)
    assert_eq!(rows[0][2], Value::Int64(2200));
}

#[test]
fn test_pg_proc_schema() {
    let schema = pg_proc_schema();
    assert_eq!(schema.name.name, "pg_proc");
    assert_eq!(schema.columns.len(), 30);
    // 验证关键列存在
    assert!(schema.columns.iter().any(|c| c.name == "proname"));
    assert!(schema.columns.iter().any(|c| c.name == "prorettype"));
    assert!(schema.columns.iter().any(|c| c.name == "pronargs"));
}

// =====================================================================
//  pg_cast 测试（2）— 类型转换规则
// =====================================================================

#[test]
fn test_pg_cast_returns_type_conversions() {
    let rows = pg_cast();
    // 至少包含 10 条转换规则
    assert!(rows.len() >= 10);
    // 每行 6 列
    assert_eq!(rows[0].len(), 6);
    assert_eq!(PG_CAST_COLUMNS.len(), 6);
    // OID 从 30000 开始
    assert_eq!(rows[0][0], Value::Int64(30000));
    // 验证 int8 → int4 转换存在（隐式转换）
    let int8_to_int4 = rows.iter().find(|r| {
        r[1] == Value::Int64(pg_type_oid::INT8) && r[2] == Value::Int64(pg_type_oid::INT4)
    });
    assert!(int8_to_int4.is_some(), "应包含 int8 → int4 转换");
    // castcontext 'i' = implicit
    assert_eq!(int8_to_int4.unwrap()[4], Value::Text("i".into()));
}

#[test]
fn test_pg_cast_schema() {
    let schema = pg_cast_schema();
    assert_eq!(schema.name.name, "pg_cast");
    assert_eq!(schema.columns.len(), 6);
    assert_eq!(PG_CAST_COLUMNS.len(), 6);
    assert!(schema.columns.iter().any(|c| c.name == "castsource"));
    assert!(schema.columns.iter().any(|c| c.name == "casttarget"));
    assert!(schema.columns.iter().any(|c| c.name == "castcontext"));
}

// =====================================================================
//  pg_operator 测试（2）— 运算符列表
// =====================================================================

#[test]
fn test_pg_operator_returns_operators() {
    let rows = pg_operator();
    // 至少包含 15 个运算符
    assert!(rows.len() >= 15);
    // 每行 15 列
    assert_eq!(rows[0].len(), 15);
    assert_eq!(PG_OPERATOR_COLUMNS.len(), 15);
    // OID 从 40000 开始
    assert_eq!(rows[0][0], Value::Int64(40000));
    // 验证 '=' 运算符存在
    let eq_ops: Vec<_> = rows.iter().filter(|r| r[1] == Value::Text("=".into())).collect();
    assert!(!eq_ops.is_empty(), "应包含 = 运算符");
    // 验证算术运算符 '+' 存在
    let plus_ops: Vec<_> = rows.iter().filter(|r| r[1] == Value::Text("+".into())).collect();
    assert!(!plus_ops.is_empty(), "应包含 + 运算符");
    // oprnamespace 应为 public 的 OID (2200)
    assert_eq!(rows[0][2], Value::Int64(2200));
}

#[test]
fn test_pg_operator_schema() {
    let schema = pg_operator_schema();
    assert_eq!(schema.name.name, "pg_operator");
    assert_eq!(schema.columns.len(), 15);
    assert_eq!(PG_OPERATOR_COLUMNS.len(), 15);
    assert!(schema.columns.iter().any(|c| c.name == "oprname"));
    assert!(schema.columns.iter().any(|c| c.name == "oprleft"));
    assert!(schema.columns.iter().any(|c| c.name == "oprright"));
}

// =====================================================================
//  pg_authid 测试（2）— 角色认证信息
// =====================================================================

#[test]
fn test_pg_authid_single_postgres() {
    let rows = pg_authid(&[]);
    assert_eq!(rows.len(), 1);
    // pg_authid 11 列（比 pg_roles 多 rolreplication）
    assert_eq!(rows[0].len(), 11);
    assert_eq!(PG_AUTHID_COLUMNS.len(), 11);
    assert_eq!(rows[0][0], Value::Int64(10)); // oid
    assert_eq!(rows[0][1], Value::Text("postgres".into())); // rolname
    // rolreplication 应为 false
    assert_eq!(rows[0][7], Value::Bool(false)); // rolreplication
    let schema = pg_authid_schema();
    assert_eq!(schema.name.name, "pg_authid");
    assert_eq!(schema.columns.len(), 11);
}

#[test]
fn test_pg_authid_multiple_users() {
    let users = vec!["admin".to_string(), "reader".to_string(), "writer".to_string()];
    let rows = pg_authid(&users);
    assert_eq!(rows.len(), 3);
    // OID 从 10 开始递增
    assert_eq!(rows[0][0], Value::Int64(10));
    assert_eq!(rows[1][0], Value::Int64(11));
    assert_eq!(rows[2][0], Value::Int64(12));
    // 用户名匹配
    assert_eq!(rows[0][1], Value::Text("admin".into()));
    assert_eq!(rows[1][1], Value::Text("reader".into()));
    assert_eq!(rows[2][1], Value::Text("writer".into()));
}

// =====================================================================
//  pg_collation 测试（2）— 默认排序规则
// =====================================================================

#[test]
fn test_pg_collation_returns_defaults() {
    let rows = pg_collation();
    // 至少包含 C 和 default
    assert!(rows.len() >= 2);
    // 每行 9 列
    assert_eq!(rows[0].len(), 9);
    assert_eq!(PG_COLLATION_COLUMNS.len(), 9);
    let coll_names: Vec<String> = rows
        .iter()
        .map(|r| match &r[1] {
            Value::Text(s) => s.clone(),
            _ => String::new(),
        })
        .collect();
    assert!(coll_names.contains(&"C".into()), "应包含 C 排序规则");
    assert!(
        coll_names.contains(&"default".into()),
        "应包含 default 排序规则"
    );
    // C 排序规则的 OID 应为 100
    let c_row = rows.iter().find(|r| r[1] == Value::Text("C".into())).unwrap();
    assert_eq!(c_row[0], Value::Int64(100));
}

#[test]
fn test_pg_collation_schema() {
    let schema = pg_collation_schema();
    assert_eq!(schema.name.name, "pg_collation");
    assert_eq!(schema.columns.len(), 9);
    assert_eq!(PG_COLLATION_COLUMNS.len(), 9);
    assert!(schema.columns.iter().any(|c| c.name == "collname"));
    assert!(schema.columns.iter().any(|c| c.name == "collisdefault"));
}

// =====================================================================
//  pg_stat_activity 测试（1）— 当前连接信息
// =====================================================================

#[test]
fn test_pg_stat_activity_returns_current_connection() {
    let rows = pg_stat_activity("szrsql");
    // 返回单行占位表示自身连接
    assert_eq!(rows.len(), 1);
    // 每行 19 列（与 PG_STAT_ACTIVITY_COLUMNS 一致）
    assert_eq!(rows[0].len(), 19);
    assert_eq!(PG_STAT_ACTIVITY_COLUMNS.len(), 19);
    // datname 应为传入的当前数据库名
    assert_eq!(rows[0][1], Value::Text("szrsql".into()));
    // state 应为 idle
    assert_eq!(rows[0][5], Value::Text("idle".into()));
    // backend_type 应为 "client backend"
    assert_eq!(rows[0][18], Value::Text("client backend".into()));
    // usesysid 应为 10（postgres）
    assert_eq!(rows[0][3], Value::Int64(10));
}

// =====================================================================
//  pg_tablespace 测试（2）— 默认表空间
// =====================================================================

#[test]
fn test_pg_tablespace_returns_defaults() {
    let rows = pg_tablespace();
    // 返回 pg_default 和 pg_global
    assert_eq!(rows.len(), 2);
    // 每行 6 列
    assert_eq!(rows[0].len(), 6);
    assert_eq!(PG_TABLESPACE_COLUMNS.len(), 6);
    // pg_default OID=1663, pg_global OID=1664
    assert_eq!(rows[0][0], Value::Int64(1663));
    assert_eq!(rows[0][1], Value::Text("pg_default".into()));
    assert_eq!(rows[1][0], Value::Int64(1664));
    assert_eq!(rows[1][1], Value::Text("pg_global".into()));
}

#[test]
fn test_pg_tablespace_schema() {
    let schema = pg_tablespace_schema();
    assert_eq!(schema.name.name, "pg_tablespace");
    assert_eq!(schema.columns.len(), 6);
    assert_eq!(PG_TABLESPACE_COLUMNS.len(), 6);
    assert!(schema.columns.iter().any(|c| c.name == "spcname"));
    assert!(schema.columns.iter().any(|c| c.name == "spcowner"));
}

// =====================================================================
//  pg_settings 测试（2）— 服务器配置参数
// =====================================================================

#[test]
fn test_pg_settings_returns_config() {
    let rows = pg_settings("15.0-szrsql", &[]);
    // 至少返回 15 个核心配置参数
    assert!(rows.len() >= 15);
    // 每行 14 列
    assert_eq!(rows[0].len(), 14);
    assert_eq!(PG_SETTINGS_COLUMNS.len(), 14);
    // 验证 server_version 配置存在
    let version_row = rows
        .iter()
        .find(|r| r[0] == Value::Text("server_version".into()))
        .expect("应包含 server_version 配置");
    assert_eq!(version_row[1], Value::Text("15.0-szrsql".into()));
    // 验证 server_encoding 为 UTF8
    let encoding_row = rows
        .iter()
        .find(|r| r[0] == Value::Text("server_encoding".into()))
        .expect("应包含 server_encoding 配置");
    assert_eq!(encoding_row[1], Value::Text("UTF8".into()));
    // 验证 search_path 默认为 public（空 allowed_databases 时）
    let search_path_row = rows
        .iter()
        .find(|r| r[0] == Value::Text("search_path".into()))
        .expect("应包含 search_path 配置");
    assert_eq!(search_path_row[1], Value::Text("public".into()));
}

#[test]
fn test_pg_settings_with_allowed_databases() {
    let dbs = vec!["mydb".to_string(), "testdb".to_string()];
    let rows = pg_settings("14.0", &dbs);
    // search_path 应包含 allowed_databases
    let search_path_row = rows
        .iter()
        .find(|r| r[0] == Value::Text("search_path".into()))
        .unwrap();
    let search_path = match &search_path_row[1] {
        Value::Text(s) => s.clone(),
        _ => String::new(),
    };
    assert!(search_path.contains("mydb"), "search_path 应包含 mydb");
    assert!(search_path.contains("testdb"), "search_path 应包含 testdb");
    assert!(search_path.contains("public"), "search_path 应包含 public");
    // server_version 应使用传入的版本
    let version_row = rows
        .iter()
        .find(|r| r[0] == Value::Text("server_version".into()))
        .unwrap();
    assert_eq!(version_row[1], Value::Text("14.0".into()));
}

#[test]
fn test_pg_settings_schema() {
    let schema = pg_settings_schema();
    assert_eq!(schema.name.name, "pg_settings");
    assert_eq!(schema.columns.len(), 14);
    assert_eq!(PG_SETTINGS_COLUMNS.len(), 14);
    assert!(schema.columns.iter().any(|c| c.name == "name"));
    assert!(schema.columns.iter().any(|c| c.name == "setting"));
    assert!(schema.columns.iter().any(|c| c.name == "category"));
}

// =====================================================================
//  pg_roles / pg_shadow / pg_user 多用户测试（3）
// =====================================================================

#[test]
fn test_pg_roles_multiple_users() {
    let users = vec!["admin".to_string(), "app_user".to_string()];
    let rows = pg_roles(&users);
    assert_eq!(rows.len(), 2);
    // OID 从 10 开始递增
    assert_eq!(rows[0][0], Value::Int64(10));
    assert_eq!(rows[1][0], Value::Int64(11));
    // rolname 匹配
    assert_eq!(rows[0][1], Value::Text("admin".into()));
    assert_eq!(rows[1][1], Value::Text("app_user".into()));
    // 所有用户都应有 rolsuper=true
    assert!(rows.iter().all(|r| r[2] == Value::Bool(true)));
}

#[test]
fn test_pg_shadow_multiple_users() {
    let users = vec!["admin".to_string(), "app_user".to_string()];
    let rows = pg_shadow(&users);
    assert_eq!(rows.len(), 2);
    // usesysid 从 10 开始递增
    assert_eq!(rows[0][1], Value::Int64(10));
    assert_eq!(rows[1][1], Value::Int64(11));
    // usename 匹配
    assert_eq!(rows[0][0], Value::Text("admin".into()));
    assert_eq!(rows[1][0], Value::Text("app_user".into()));
    // 所有用户 usesuper=true
    assert!(rows.iter().all(|r| r[3] == Value::Bool(true)));
}

#[test]
fn test_pg_user_multiple_users() {
    let users = vec!["admin".to_string(), "app_user".to_string()];
    let rows = pg_user(&users);
    assert_eq!(rows.len(), 2);
    // usesysid 从 10 开始递增
    assert_eq!(rows[0][1], Value::Int64(10));
    assert_eq!(rows[1][1], Value::Int64(11));
    // usename 匹配
    assert_eq!(rows[0][0], Value::Text("admin".into()));
    assert_eq!(rows[1][0], Value::Text("app_user".into()));
}
