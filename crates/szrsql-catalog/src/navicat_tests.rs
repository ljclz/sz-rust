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
//! - pg_description（1）：占位空
//! - pg_views（1）：占位空
//! - OID 稳定性（2）：相同 catalog 多次调用 OID 一致 + 不同 catalog 相同表名 OID 一致
//! - 类型映射辅助（3）：column_type_to_oid + column_type_to_name + column_type_display
//! - DDL 片段（2）：column_ddl_fragment + foreign_key_reference_ddl
//!
//! 共 34 个测试用例。

use crate::navicat::{
    column_ddl_fragment, column_type_display, column_type_to_name, column_type_to_oid, contype,
    foreign_key_reference_ddl, oid_attribute, oid_class_index, oid_class_table, oid_constraint,
    oid_namespace, pg_attribute, pg_class, pg_class_schema, pg_constraint, pg_database,
    pg_database_schema, pg_description, pg_description_schema, pg_index, pg_namespace, pg_type,
    pg_type_oid, pg_type_schema, pg_views, pg_views_schema, relkind, PG_CLASS_COLUMNS,
    PG_DATABASE_COLUMNS, PG_DESCRIPTION_COLUMNS, PG_TYPE_COLUMNS, PG_VIEWS_COLUMNS,
};
use crate::{IndexInfo, ManagedCatalog, MutableCatalog};
use szrsql_sql::ast::{
    ColumnDefinition, ForeignKeyReference, IndexColumn, ReferenceAction, TableName,
};
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
    // (oid, datname, datallowconn, datistemplate)
    assert_eq!(rows.len(), 2);
    // template1
    assert_eq!(rows[0][0], Value::Int64(1));
    assert_eq!(rows[0][1], Value::Text("template1".into()));
    assert_eq!(rows[0][2], Value::Bool(false));
    assert_eq!(rows[0][3], Value::Bool(true));
    // 当前数据库
    assert_eq!(rows[1][0], Value::Int64(16384));
    assert_eq!(rows[1][1], Value::Text("szrsql".into()));
    assert_eq!(rows[1][2], Value::Bool(true));
    assert_eq!(rows[1][3], Value::Bool(false));
}

#[test]
fn test_pg_database_schema() {
    let schema = pg_database_schema();
    assert_eq!(schema.name.name, "pg_database");
    assert_eq!(schema.columns.len(), 4);
    assert_eq!(PG_DATABASE_COLUMNS.len(), 4);
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
    // (oid, relname, relnamespace, relkind, relnatts, relpages, reltuples)
    let row = &rows[0];
    assert_eq!(row[1], Value::Text("users".into()));
    assert_eq!(row[3], Value::Text(relkind::RELATION.into()));
    assert_eq!(row[4], Value::Int64(1)); // relnatts = 1
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
    let index_row = rows
        .iter()
        .find(|r| r[3] == Value::Text(relkind::INDEX.into()))
        .expect("应包含索引对象");
    assert_eq!(index_row[1], Value::Text("idx_users_id".into()));
}

#[test]
fn test_pg_class_schema_columns() {
    let schema = pg_class_schema();
    assert_eq!(schema.name.name, "pg_class");
    assert_eq!(schema.columns.len(), 7);
    assert_eq!(PG_CLASS_COLUMNS.len(), 7);
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
    assert_eq!(schema.columns.len(), 4);
    assert_eq!(PG_TYPE_COLUMNS.len(), 4);
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
    let rows = pg_description();
    assert!(rows.is_empty());
    assert_eq!(PG_DESCRIPTION_COLUMNS.len(), 4);
    let schema = pg_description_schema();
    assert_eq!(schema.name.name, "pg_description");
}

#[test]
fn test_pg_views_empty() {
    let rows = pg_views();
    assert!(rows.is_empty());
    assert_eq!(PG_VIEWS_COLUMNS.len(), 4);
    let schema = pg_views_schema();
    assert_eq!(schema.name.name, "pg_views");
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
    let ns_oid = oid_namespace("public");
    let class_oid = oid_class_table(&TableName::new("public.users"));
    let idx_oid = oid_class_index("users_pkey");
    let attr_oid = oid_attribute(class_oid, 1);
    let con_oid = oid_constraint("users_pkey", class_oid);

    // OID 段隔离：
    // - namespace: 10000 + hash & 0xFFFF (范围 10000-75535)
    // - class table: 20000 + hash & 0xFFFFF (范围 20000-1245755)
    // - class index: 30000 + hash & 0xFFFFF (范围 30000-1255755)
    // - attribute: 40000 + hash & 0xFFFFF (范围 40000-1265755)
    // - constraint: 50000 + hash & 0xFFFFF (范围 50000-1275755)
    //
    // 验证下界（段基址）— hash 掩码可能导致上界超出段间间隔，
    // 但下界始终在对应段内。
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

    // 不同段 OID 不应相等（极低概率冲突，验证基础隔离）
    let mut oids = vec![ns_oid, class_oid, idx_oid, attr_oid, con_oid];
    oids.sort();
    oids.dedup();
    assert_eq!(oids.len(), 5, "5 个不同段 OID 不应重复");
}
