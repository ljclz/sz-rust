//! Phase 3.19 information_schema 测试 — 4 个标准视图。
//!
//! 覆盖类别：
//! - tables 视图（4）：空 catalog + 单表 + 多表 + 自定义 schema
//! - columns 视图（6）：空 catalog + ORDINAL_POSITION + NOT NULL + PRIMARY KEY + DEFAULT + DECIMAL 精度
//! - table_constraints 视图（5）：空 catalog + PRIMARY KEY + UNIQUE + CHECK + FOREIGN KEY
//! - referential_constraints 视图（4）：空 catalog + 默认 NO ACTION + CASCADE + 引用表不存在
//! - COLUMN_DEFAULT 格式化（3）：Int64 + Text 转义 + Bool
//! - catalog_name 参数（2）：tables_with_catalog + columns_with_catalog
//! - 列常量（1）：4 个 _COLUMNS 常量
//! - Schema 函数（4）：tables_schema + columns_schema + table_constraints_schema + referential_constraints_schema
//!
//! 共 29 个测试用例。

use crate::information_schema::{
    columns, columns_schema, columns_with_catalog, constraint_type, referential_constraints,
    referential_constraints_schema, referential_constraints_with_catalog, table_constraints,
    table_constraints_schema, table_constraints_with_catalog, tables, tables_schema,
    tables_with_catalog, COLUMNS_COLUMNS, REFERENTIAL_CONSTRAINTS_COLUMNS, TABLES_COLUMNS,
    TABLE_CONSTRAINTS_COLUMNS,
};
use crate::{ManagedCatalog, MutableCatalog};
use szrsql_sql::ast::{ColumnDefinition, Expr, ForeignKeyReference, ReferenceAction, TableName};
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

fn col_with_not_null(name: &str, ct: ColumnType) -> ColumnDefinition {
    let mut c = ColumnDefinition::new(name, ct);
    c.not_null = true;
    c
}

fn col_with_unique(name: &str, ct: ColumnType) -> ColumnDefinition {
    let mut c = ColumnDefinition::new(name, ct);
    c.unique = true;
    c
}

fn col_with_default(name: &str, ct: ColumnType, default: Expr) -> ColumnDefinition {
    let mut c = ColumnDefinition::new(name, ct);
    c.default = Some(default);
    c
}

fn col_with_check(name: &str, ct: ColumnType, check: Expr) -> ColumnDefinition {
    let mut c = ColumnDefinition::new(name, ct);
    c.check = Some(check);
    c
}

fn col_with_fk(name: &str, ct: ColumnType, fk: ForeignKeyReference) -> ColumnDefinition {
    let mut c = ColumnDefinition::new(name, ct);
    c.references = Some(fk);
    c
}

fn make_fk(ref_table: &str) -> ForeignKeyReference {
    ForeignKeyReference {
        table: TableName::new(ref_table),
        columns: None,
        on_delete: None,
        on_update: None,
    }
}

fn make_fk_cascade(ref_table: &str) -> ForeignKeyReference {
    ForeignKeyReference {
        table: TableName::new(ref_table),
        columns: None,
        on_delete: Some(ReferenceAction::Cascade),
        on_update: Some(ReferenceAction::Cascade),
    }
}

fn make_schema_with_schemaname(schema: &str, name: &str) -> TableSchema {
    TableSchema {
        name: TableName::with_schema(schema, name),
        columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
    }
}

// =====================================================================
//  tables 视图测试（4）
// =====================================================================

#[test]
fn test_tables_empty_catalog() {
    let catalog = ManagedCatalog::new();
    let rows = tables(&catalog);
    assert!(rows.is_empty());
}

#[test]
fn test_tables_single_table() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();

    let rows = tables(&catalog);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.len(), 4);
    assert_eq!(row[0], Value::Text("szrsql".into()));
    assert_eq!(row[1], Value::Text("public".into()));
    assert_eq!(row[2], Value::Text("users".into()));
    assert_eq!(row[3], Value::Text("BASE TABLE".into()));
}

#[test]
fn test_tables_multiple_tables() {
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

    let rows = tables(&catalog);
    assert_eq!(rows.len(), 3);

    let names: Vec<String> = rows
        .iter()
        .map(|r| match &r[2] {
            Value::Text(s) => s.clone(),
            _ => panic!("expected Text"),
        })
        .collect();
    assert!(names.contains(&"users".to_string()));
    assert!(names.contains(&"orders".to_string()));
    assert!(names.contains(&"products".to_string()));

    // 所有行的 TABLE_TYPE 应为 BASE TABLE
    for row in &rows {
        assert_eq!(row[3], Value::Text("BASE TABLE".into()));
    }
}

#[test]
fn test_tables_custom_schema() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema_with_schemaname("my_app", "users"), false)
        .unwrap();

    let rows = tables(&catalog);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][1], Value::Text("my_app".into()));
    assert_eq!(rows[0][2], Value::Text("users".into()));
}

// =====================================================================
//  columns 视图测试（6）
// =====================================================================

#[test]
fn test_columns_empty_catalog() {
    let catalog = ManagedCatalog::new();
    let rows = columns(&catalog);
    assert!(rows.is_empty());
}

#[test]
fn test_columns_ordinal_position() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema(
                "users",
                vec![
                    ("id", ColumnType::Int64),
                    ("name", ColumnType::Text),
                    ("email", ColumnType::Text),
                ],
            ),
            false,
        )
        .unwrap();

    let rows = columns(&catalog);
    assert_eq!(rows.len(), 3);

    // ORDINAL_POSITION 1, 2, 3
    assert_eq!(rows[0][4], Value::Int64(1));
    assert_eq!(rows[0][3], Value::Text("id".into()));
    assert_eq!(rows[1][4], Value::Int64(2));
    assert_eq!(rows[1][3], Value::Text("name".into()));
    assert_eq!(rows[2][4], Value::Int64(3));
    assert_eq!(rows[2][3], Value::Text("email".into()));
}

#[test]
fn test_columns_not_null_is_nullable_no() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full(
                "users",
                vec![
                    ColumnDefinition::new("id", ColumnType::Int64),
                    col_with_not_null("name", ColumnType::Text),
                ],
            ),
            false,
        )
        .unwrap();

    let rows = columns(&catalog);
    assert_eq!(rows.len(), 2);
    // id 默认可空 → YES
    assert_eq!(rows[0][6], Value::Text("YES".into()));
    // name NOT NULL → NO
    assert_eq!(rows[1][6], Value::Text("NO".into()));
}

#[test]
fn test_columns_primary_key_is_nullable_no() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full("users", vec![col_with_pk("id", ColumnType::Int64)]),
            false,
        )
        .unwrap();

    let rows = columns(&catalog);
    assert_eq!(rows.len(), 1);
    // PRIMARY KEY 隐含 NOT NULL → IS_NULLABLE = NO
    assert_eq!(rows[0][6], Value::Text("NO".into()));
}

#[test]
fn test_columns_default_value() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full(
                "users",
                vec![
                    ColumnDefinition::new("id", ColumnType::Int64),
                    col_with_default("age", ColumnType::Int64, Expr::Literal(Value::Int64(18))),
                    col_with_default(
                        "name",
                        ColumnType::Text,
                        Expr::Literal(Value::Text("alice".into())),
                    ),
                ],
            ),
            false,
        )
        .unwrap();

    let rows = columns(&catalog);
    assert_eq!(rows.len(), 3);

    // id 无默认 → NULL
    assert_eq!(rows[0][5], Value::Null);
    // age DEFAULT 18
    assert_eq!(rows[1][5], Value::Text("18".into()));
    // name DEFAULT 'alice'
    assert_eq!(rows[2][5], Value::Text("'alice'".into()));
}

#[test]
fn test_columns_decimal_precision_scale() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full(
                "products",
                vec![
                    ColumnDefinition::new("id", ColumnType::Int64),
                    ColumnDefinition::new(
                        "price",
                        ColumnType::Decimal {
                            precision: 10,
                            scale: 2,
                        },
                    ),
                ],
            ),
            false,
        )
        .unwrap();

    let rows = columns(&catalog);
    assert_eq!(rows.len(), 2);

    // id — 非 DECIMAL → NUMERIC_PRECISION/SCALE 为 NULL
    assert_eq!(rows[0][9], Value::Null);
    assert_eq!(rows[0][10], Value::Null);

    // price DECIMAL(10,2) → precision=10, scale=2
    assert_eq!(rows[1][9], Value::Int64(10));
    assert_eq!(rows[1][10], Value::Int64(2));

    // DATA_TYPE 应包含 numeric(10,2)
    if let Value::Text(s) = &rows[1][7] {
        assert!(s.contains("numeric(10,2)"));
    } else {
        panic!("expected Text for DATA_TYPE");
    }
}

// =====================================================================
//  table_constraints 视图测试（5）
// =====================================================================

#[test]
fn test_table_constraints_empty_catalog() {
    let catalog = ManagedCatalog::new();
    let rows = table_constraints(&catalog);
    assert!(rows.is_empty());
}

#[test]
fn test_table_constraints_primary_key() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full("users", vec![col_with_pk("id", ColumnType::Int64)]),
            false,
        )
        .unwrap();

    let rows = table_constraints(&catalog);
    assert_eq!(rows.len(), 1);

    // (CONSTRAINT_CATALOG, CONSTRAINT_SCHEMA, CONSTRAINT_NAME,
    //  TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, CONSTRAINT_TYPE)
    let row = &rows[0];
    assert_eq!(row[0], Value::Text("szrsql".into()));
    assert_eq!(row[1], Value::Text("public".into()));
    assert_eq!(row[2], Value::Text("users_pkey".into()));
    assert_eq!(row[3], Value::Text("szrsql".into()));
    assert_eq!(row[4], Value::Text("public".into()));
    assert_eq!(row[5], Value::Text("users".into()));
    assert_eq!(row[6], Value::Text(constraint_type::PRIMARY_KEY.into()));
}

#[test]
fn test_table_constraints_unique() {
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

    let rows = table_constraints(&catalog);
    assert_eq!(rows.len(), 2);

    // 应包含 PK + UNIQUE
    let types: Vec<String> = rows
        .iter()
        .map(|r| match &r[6] {
            Value::Text(s) => s.clone(),
            _ => panic!("expected Text"),
        })
        .collect();
    assert!(types.contains(&constraint_type::PRIMARY_KEY.to_string()));
    assert!(types.contains(&constraint_type::UNIQUE.to_string()));

    // UNIQUE 约束名：{table}_{column}_key
    let unique_row = rows
        .iter()
        .find(|r| r[6] == Value::Text(constraint_type::UNIQUE.into()))
        .unwrap();
    assert_eq!(unique_row[2], Value::Text("users_email_key".into()));
}

#[test]
fn test_table_constraints_check() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full(
                "users",
                vec![
                    col_with_pk("id", ColumnType::Int64),
                    col_with_check("age", ColumnType::Int64, Expr::Literal(Value::Int64(0))),
                ],
            ),
            false,
        )
        .unwrap();

    let rows = table_constraints(&catalog);
    assert_eq!(rows.len(), 2);

    let check_row = rows
        .iter()
        .find(|r| r[6] == Value::Text(constraint_type::CHECK.into()))
        .unwrap();
    assert_eq!(check_row[2], Value::Text("users_age_check".into()));
}

#[test]
fn test_table_constraints_foreign_key() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full("users", vec![col_with_pk("id", ColumnType::Int64)]),
            false,
        )
        .unwrap();
    catalog
        .create_table(
            make_schema_full(
                "orders",
                vec![
                    col_with_pk("id", ColumnType::Int64),
                    col_with_fk(
                        "user_id",
                        ColumnType::Int64,
                        ForeignKeyReference {
                            table: TableName::new("users"),
                            columns: None,
                            on_delete: None,
                            on_update: None,
                        },
                    ),
                ],
            ),
            false,
        )
        .unwrap();

    let rows = table_constraints(&catalog);
    // users: 1 PK; orders: 1 PK + 1 FK → 总计 3
    assert_eq!(rows.len(), 3);

    let fk_row = rows
        .iter()
        .find(|r| r[6] == Value::Text(constraint_type::FOREIGN_KEY.into()))
        .unwrap();
    assert_eq!(fk_row[2], Value::Text("orders_user_id_fkey".into()));
    assert_eq!(fk_row[5], Value::Text("orders".into()));
}

// =====================================================================
//  referential_constraints 视图测试（4）
// =====================================================================

#[test]
fn test_referential_constraints_empty_catalog() {
    let catalog = ManagedCatalog::new();
    let rows = referential_constraints(&catalog);
    assert!(rows.is_empty());
}

#[test]
fn test_referential_constraints_default_no_action() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full("users", vec![col_with_pk("id", ColumnType::Int64)]),
            false,
        )
        .unwrap();
    catalog
        .create_table(
            make_schema_full(
                "orders",
                vec![col_with_fk("user_id", ColumnType::Int64, make_fk("users"))],
            ),
            false,
        )
        .unwrap();

    let rows = referential_constraints(&catalog);
    assert_eq!(rows.len(), 1);

    let row = &rows[0];
    // CONSTRAINT_NAME = orders_user_id_fkey
    assert_eq!(row[2], Value::Text("orders_user_id_fkey".into()));
    // UNIQUE_CONSTRAINT_NAME = users_pkey（被引用表有 PK）
    assert_eq!(row[5], Value::Text("users_pkey".into()));
    // MATCH_OPTION = NONE
    assert_eq!(row[6], Value::Text("NONE".into()));
    // UPDATE_RULE = NO ACTION（默认）
    assert_eq!(row[7], Value::Text("NO ACTION".into()));
    // DELETE_RULE = NO ACTION（默认）
    assert_eq!(row[8], Value::Text("NO ACTION".into()));
}

#[test]
fn test_referential_constraints_cascade() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full("users", vec![col_with_pk("id", ColumnType::Int64)]),
            false,
        )
        .unwrap();
    catalog
        .create_table(
            make_schema_full(
                "orders",
                vec![col_with_fk(
                    "user_id",
                    ColumnType::Int64,
                    make_fk_cascade("users"),
                )],
            ),
            false,
        )
        .unwrap();

    let rows = referential_constraints(&catalog);
    assert_eq!(rows.len(), 1);

    let row = &rows[0];
    assert_eq!(row[7], Value::Text("CASCADE".into())); // UPDATE_RULE
    assert_eq!(row[8], Value::Text("CASCADE".into())); // DELETE_RULE
}

#[test]
fn test_referential_constraints_ref_table_no_pk() {
    let mut catalog = ManagedCatalog::new();
    // users 表无 PK
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();
    catalog
        .create_table(
            make_schema_full(
                "orders",
                vec![col_with_fk("user_id", ColumnType::Int64, make_fk("users"))],
            ),
            false,
        )
        .unwrap();

    let rows = referential_constraints(&catalog);
    assert_eq!(rows.len(), 1);

    let row = &rows[0];
    // 被引用表无 PK → UNIQUE_CONSTRAINT_NAME 为 NULL
    assert_eq!(row[5], Value::Null);
    // UNIQUE_CONSTRAINT_SCHEMA 仍为 schema 名
    assert_eq!(row[4], Value::Text("public".into()));
}

// =====================================================================
//  COLUMN_DEFAULT 格式化测试（3）
// =====================================================================

#[test]
fn test_column_default_int64() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full(
                "t",
                vec![col_with_default(
                    "n",
                    ColumnType::Int64,
                    Expr::Literal(Value::Int64(42)),
                )],
            ),
            false,
        )
        .unwrap();

    let rows = columns(&catalog);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][5], Value::Text("42".into()));
}

#[test]
fn test_column_default_text_escaped() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full(
                "t",
                vec![col_with_default(
                    "s",
                    ColumnType::Text,
                    Expr::Literal(Value::Text("it's a test".into())),
                )],
            ),
            false,
        )
        .unwrap();

    let rows = columns(&catalog);
    assert_eq!(rows.len(), 1);
    // 单引号应被转义为 ''
    assert_eq!(rows[0][5], Value::Text("'it''s a test'".into()));
}

#[test]
fn test_column_default_bool() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full(
                "t",
                vec![
                    col_with_default("a", ColumnType::Bool, Expr::Literal(Value::Bool(true))),
                    col_with_default("b", ColumnType::Bool, Expr::Literal(Value::Bool(false))),
                ],
            ),
            false,
        )
        .unwrap();

    let rows = columns(&catalog);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][5], Value::Text("TRUE".into()));
    assert_eq!(rows[1][5], Value::Text("FALSE".into()));
}

// =====================================================================
//  catalog_name 参数测试（2）
// =====================================================================

#[test]
fn test_tables_with_custom_catalog_name() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();

    let rows = tables_with_catalog(&catalog, "my_db");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Text("my_db".into()));
}

#[test]
fn test_columns_with_custom_catalog_name() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(make_schema("users", vec![("id", ColumnType::Int64)]), false)
        .unwrap();

    let rows = columns_with_catalog(&catalog, "my_db");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Text("my_db".into()));
}

/// 改进 P1：COMMENT ON 暴露到 information_schema.columns
///
/// 验证：
/// 1. 未设置注释时 COMMENT 列为 NULL
/// 2. set_column_comment 后 COMMENT 列反映注释
/// 3. set_table_comment 不影响列级 COMMENT
/// 4. 行长度为 12（11 标准 + 1 扩展 COMMENT）
#[test]
fn test_columns_comment_column_exposed() {
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

    // 1. 初始状态：所有列 COMMENT 为 NULL
    let rows = columns(&catalog);
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert_eq!(row.len(), 12, "行长度应为 12（11 标准 + 1 COMMENT）");
        assert_eq!(row[11], Value::Null, "未设置注释时 COMMENT 应为 NULL");
    }

    // 2. set_column_comment 后 COMMENT 列反映注释
    catalog
        .set_column_comment(
            &TableName::new("users"),
            "name",
            Some("用户姓名".to_string()),
        )
        .unwrap();
    let rows = columns(&catalog);
    let name_row = rows
        .iter()
        .find(|r| r[3] == Value::Text("name".into()))
        .expect("应找到 name 列");
    assert_eq!(
        name_row[11],
        Value::Text("用户姓名".into()),
        "COMMENT 列应反映 set_column_comment 设置的注释"
    );

    // 3. set_table_comment 不影响列级 COMMENT
    catalog
        .set_table_comment(
            &TableName::new("users"),
            Some("用户主表".to_string()),
        )
        .unwrap();
    let rows = columns(&catalog);
    let id_row = rows
        .iter()
        .find(|r| r[3] == Value::Text("id".into()))
        .expect("应找到 id 列");
    assert_eq!(
        id_row[11], Value::Null,
        "set_table_comment 不应影响列级 COMMENT"
    );
    let name_row = rows
        .iter()
        .find(|r| r[3] == Value::Text("name".into()))
        .expect("应找到 name 列");
    assert_eq!(
        name_row[11],
        Value::Text("用户姓名".into()),
        "列级 COMMENT 应保持不变"
    );

    // 4. 删除列注释后 COMMENT 列回到 NULL
    catalog
        .set_column_comment(&TableName::new("users"), "name", None)
        .unwrap();
    let rows = columns(&catalog);
    let name_row = rows
        .iter()
        .find(|r| r[3] == Value::Text("name".into()))
        .expect("应找到 name 列");
    assert_eq!(
        name_row[11], Value::Null,
        "删除注释后 COMMENT 列应回到 NULL"
    );
}

// =====================================================================
//  列常量测试（1）
// =====================================================================

#[test]
fn test_column_constants() {
    assert_eq!(
        TABLES_COLUMNS,
        &["TABLE_CATALOG", "TABLE_SCHEMA", "TABLE_NAME", "TABLE_TYPE"]
    );
    assert_eq!(
        COLUMNS_COLUMNS,
        &[
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
            "COMMENT",
        ]
    );
    assert_eq!(
        TABLE_CONSTRAINTS_COLUMNS,
        &[
            "CONSTRAINT_CATALOG",
            "CONSTRAINT_SCHEMA",
            "CONSTRAINT_NAME",
            "TABLE_CATALOG",
            "TABLE_SCHEMA",
            "TABLE_NAME",
            "CONSTRAINT_TYPE",
        ]
    );
    assert_eq!(
        REFERENTIAL_CONSTRAINTS_COLUMNS,
        &[
            "CONSTRAINT_CATALOG",
            "CONSTRAINT_SCHEMA",
            "CONSTRAINT_NAME",
            "UNIQUE_CONSTRAINT_CATALOG",
            "UNIQUE_CONSTRAINT_SCHEMA",
            "UNIQUE_CONSTRAINT_NAME",
            "MATCH_OPTION",
            "UPDATE_RULE",
            "DELETE_RULE",
        ]
    );
}

// =====================================================================
//  Schema 函数测试（4）
// =====================================================================

#[test]
fn test_tables_schema_columns() {
    let schema = tables_schema();
    assert_eq!(schema.name.name, "tables");
    assert_eq!(schema.name.schema.as_deref(), Some("information_schema"));
    assert_eq!(schema.columns.len(), 4);
    assert_eq!(schema.columns[0].name, "TABLE_CATALOG");
    assert_eq!(schema.columns[3].name, "TABLE_TYPE");
}

#[test]
fn test_columns_schema_columns() {
    let schema = columns_schema();
    assert_eq!(schema.name.name, "columns");
    assert_eq!(schema.columns.len(), 12);
    assert_eq!(schema.columns[0].name, "TABLE_CATALOG");
    assert_eq!(schema.columns[10].name, "NUMERIC_SCALE");
    // szrsql 扩展列：COMMENT（暴露 COMMENT ON COLUMN 设置的注释）
    assert_eq!(schema.columns[11].name, "COMMENT");
}

#[test]
fn test_table_constraints_schema_columns() {
    let schema = table_constraints_schema();
    assert_eq!(schema.name.name, "table_constraints");
    assert_eq!(schema.columns.len(), 7);
    assert_eq!(schema.columns[0].name, "CONSTRAINT_CATALOG");
    assert_eq!(schema.columns[6].name, "CONSTRAINT_TYPE");
}

#[test]
fn test_referential_constraints_schema_columns() {
    let schema = referential_constraints_schema();
    assert_eq!(schema.name.name, "referential_constraints");
    assert_eq!(schema.columns.len(), 9);
    assert_eq!(schema.columns[0].name, "CONSTRAINT_CATALOG");
    assert_eq!(schema.columns[8].name, "DELETE_RULE");
}

// =====================================================================
//  _with_catalog 函数变体测试（2）
// =====================================================================

#[test]
fn test_table_constraints_with_custom_catalog() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full("users", vec![col_with_pk("id", ColumnType::Int64)]),
            false,
        )
        .unwrap();

    let rows = table_constraints_with_catalog(&catalog, "my_db");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Text("my_db".into()));
    assert_eq!(rows[0][3], Value::Text("my_db".into()));
}

#[test]
fn test_referential_constraints_with_custom_catalog() {
    let mut catalog = ManagedCatalog::new();
    catalog
        .create_table(
            make_schema_full("users", vec![col_with_pk("id", ColumnType::Int64)]),
            false,
        )
        .unwrap();
    catalog
        .create_table(
            make_schema_full(
                "orders",
                vec![col_with_fk("user_id", ColumnType::Int64, make_fk("users"))],
            ),
            false,
        )
        .unwrap();

    let rows = referential_constraints_with_catalog(&catalog, "my_db");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Text("my_db".into()));
    assert_eq!(rows[0][3], Value::Text("my_db".into()));
}
