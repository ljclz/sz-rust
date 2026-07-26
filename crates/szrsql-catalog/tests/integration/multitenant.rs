//! Phase 3.11 集成测试 — 多租户隔离验证（两种模式）。
//!
//! # 验证目标（对应 `SzRSQL实施进度.md` Phase 3.11）
//!
//! - **SchemaPrefix 模式**：单 catalog + 单 storage 共享，通过 `SqlRewriter` 给每个租户的
//!   表名加前缀（如 `t` → `tA_t` / `tB_t`）实现隔离。租户 A 创建表 `t` 并 INSERT 数据后，
//!   租户 B 查询 `t` 时 AST 被重写为 `tB_t`，看不到租户 A 的数据
//! - **SeparateFile 模式**：每个租户独立的 catalog + storage 实例，物理上完全隔离。
//!   两个租户都可以创建表 `t` 而不冲突，数据互不可见
//! - **验收标准**：两种模式下租户隔离完整（0 数据泄漏）
//!
//! # 测试架构
//!
//! 测试不依赖 `Executor`（避免生命周期复杂度），直接使用：
//! 1. `szrsql_sql::parser::parse_one` 解析 SQL → AST
//! 2. `SqlRewriter::rewrite_statement` 重写 AST（仅 SchemaPrefix 模式）
//! 3. `ManagedCatalog` 直接执行 DDL（create_table / drop_table）
//! 4. `InMemoryTable` 直接执行 DML（insert / scan_iter）
//!
//! 这样测试聚焦于 catalog + multitenant 层的隔离语义，不引入执行器层的复杂性。

use std::collections::HashMap;

use szrsql_catalog::{
    multitenant::{SqlRewriter, TenantContext},
    ManagedCatalog, MutableCatalog,
};
use szrsql_sql::{
    ast::{ColumnDefinition, Expr, InsertSource, Statement, TableFactor, TableName},
    executor::{InMemoryTable, TableStorage},
    parser::parse_one,
    plan::{Catalog, TableSchema},
};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 构造一个 INT 列的表 Schema
fn int_table_schema(name: &str) -> TableSchema {
    TableSchema {
        name: TableName::new(name),
        columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
    }
}

/// 从 CREATE TABLE 语句提取表名
fn extract_create_table_name(stmt: &Statement) -> String {
    match stmt {
        Statement::CreateTable { name, .. } => name.name.clone(),
        other => panic!("expected CREATE TABLE, got {other:?}"),
    }
}

/// 从 SELECT 语句提取 FROM 主表名
fn extract_select_from_table(stmt: &Statement) -> String {
    match stmt {
        Statement::Select(s) => {
            if s.from.is_empty() {
                panic!("SELECT has no FROM clause");
            }
            match &s.from[0].relation {
                TableFactor::Table { name, .. } => name.name.clone(),
                other => panic!("expected TableFactor::Table, got {other:?}"),
            }
        }
        other => panic!("expected SELECT, got {other:?}"),
    }
}

/// 从 INSERT 语句提取目标表名 + VALUES 行
fn extract_insert_target_and_rows(stmt: &Statement) -> (String, Vec<Vec<Value>>) {
    match stmt {
        Statement::Insert { table, source, .. } => {
            let table_name = table.name.clone();
            let rows = match source {
                InsertSource::Values(expr_rows) => expr_rows
                    .iter()
                    .map(|expr_row| {
                        expr_row
                            .iter()
                            .map(|expr| match expr {
                                Expr::Literal(v) => v.clone(),
                                other => panic!("expected Literal, got {other:?}"),
                            })
                            .collect()
                    })
                    .collect(),
                other => panic!("expected InsertSource::Values, got {other:?}"),
            };
            (table_name, rows)
        }
        other => panic!("expected INSERT, got {other:?}"),
    }
}

/// 从 DROP TABLE 语句提取表名列表
fn extract_drop_table_names(stmt: &Statement) -> Vec<String> {
    match stmt {
        Statement::DropTable { names, .. } => names.iter().map(|n| n.name.clone()).collect(),
        other => panic!("expected DROP TABLE, got {other:?}"),
    }
}

/// 构造一个 InMemoryTable 并插入若干 INT 行
fn make_table_with_rows(name: &str, values: &[i64]) -> InMemoryTable {
    let mut table = InMemoryTable::new(int_table_schema(name));
    for v in values {
        table.insert(vec![Value::Int64(*v)]);
    }
    table
}

/// 收集 InMemoryTable 所有行（按 row_count 顺序）
fn collect_rows(table: &InMemoryTable) -> Vec<Vec<Value>> {
    table.scan_iter().collect()
}

// =====================================================================
//  SchemaPrefix 模式 — 单 catalog + 单 storage，表名前缀重写
// =====================================================================

#[test]
fn test_schema_prefix_tenant_a_create_table_b_cannot_see() {
    // 单 catalog 共享
    let mut catalog = ManagedCatalog::new();
    let rewriter_a = SqlRewriter::new(TenantContext::new("tA"));
    let rewriter_b = SqlRewriter::new(TenantContext::new("tB"));

    // 租户 A: CREATE TABLE t (id INT) → 重写为 CREATE TABLE tA_t
    let stmt = parse_one("CREATE TABLE t (id INT)").unwrap();
    let rewritten = rewriter_a.rewrite_statement(stmt).unwrap();
    let physical_name = extract_create_table_name(&rewritten);
    assert_eq!(physical_name, "tA_t", "Tenant A 的 t 应重写为 tA_t");

    // 在 catalog 上执行重写后的 DDL
    let schema = int_table_schema(&physical_name);
    catalog.create_table(schema, false).unwrap();

    // 验证：catalog 中只有 tA_t，没有 t 也没有 tB_t
    assert!(
        catalog.table_exists(&TableName::new("tA_t")),
        "catalog 应包含 tA_t"
    );
    assert!(
        !catalog.table_exists(&TableName::new("t")),
        "catalog 不应包含原始表名 t"
    );
    assert!(
        !catalog.table_exists(&TableName::new("tB_t")),
        "catalog 不应包含 tB_t（租户 B 还没创建）"
    );

    // 租户 B: SELECT * FROM t → 重写为 SELECT * FROM tB_t
    let stmt = parse_one("SELECT * FROM t").unwrap();
    let rewritten_b = rewriter_b.rewrite_statement(stmt).unwrap();
    let from_table = extract_select_from_table(&rewritten_b);
    assert_eq!(from_table, "tB_t", "Tenant B 的 t 应重写为 tB_t");

    // 验证：租户 B 查询的 tB_t 在 catalog 中不存在（看不到租户 A 的数据）
    assert!(
        !catalog.table_exists(&TableName::new(&from_table)),
        "租户 B 查询的 {from_table} 在 catalog 中不应存在 → 看不到租户 A 的数据"
    );
}

#[test]
fn test_schema_prefix_tenant_a_insert_b_cannot_read() {
    // 单 catalog + 单 storage 共享
    let mut catalog = ManagedCatalog::new();
    let mut storage: HashMap<String, InMemoryTable> = HashMap::new();

    let rewriter_a = SqlRewriter::new(TenantContext::new("tA"));
    let rewriter_b = SqlRewriter::new(TenantContext::new("tB"));

    // 租户 A: CREATE TABLE t (id INT)
    let stmt = parse_one("CREATE TABLE t (id INT)").unwrap();
    let rewritten = rewriter_a.rewrite_statement(stmt).unwrap();
    let physical_name = extract_create_table_name(&rewritten);
    assert_eq!(physical_name, "tA_t");

    let schema = int_table_schema(&physical_name);
    catalog.create_table(schema.clone(), false).unwrap();
    storage.insert(physical_name.clone(), InMemoryTable::new(schema));

    // 租户 A: INSERT INTO t VALUES (1), (2), (3)
    let stmt = parse_one("INSERT INTO t VALUES (1), (2), (3)").unwrap();
    let rewritten = rewriter_a.rewrite_statement(stmt).unwrap();
    let (target_table, rows) = extract_insert_target_and_rows(&rewritten);
    assert_eq!(target_table, "tA_t", "INSERT 目标表应重写为 tA_t");
    assert_eq!(rows.len(), 3, "应插入 3 行");

    let table = storage.get_mut(&target_table).unwrap();
    for row in rows {
        table.insert(row);
    }

    // 验证：tA_t 表中有 3 行数据 [1, 2, 3]
    let stored_rows = collect_rows(storage.get("tA_t").unwrap());
    assert_eq!(stored_rows.len(), 3, "tA_t 应有 3 行");
    assert!(
        stored_rows.iter().any(|r| r[0] == Value::Int64(1)),
        "tA_t 应包含 id=1"
    );

    // 租户 B: SELECT * FROM t → 重写为 SELECT * FROM tB_t
    let stmt = parse_one("SELECT * FROM t").unwrap();
    let rewritten_b = rewriter_b.rewrite_statement(stmt).unwrap();
    let from_table = extract_select_from_table(&rewritten_b);
    assert_eq!(from_table, "tB_t");

    // 验证：storage 中没有 tB_t → 租户 B 看不到任何数据
    assert!(
        !storage.contains_key(&from_table),
        "storage 不应包含 {from_table} → 租户 B 看不到租户 A 的数据"
    );

    // 验证：storage 中有 tA_t，但租户 B 的查询不会命中它
    assert!(
        storage.contains_key("tA_t"),
        "storage 包含 tA_t（租户 A 的数据），但租户 B 的查询已重写为 tB_t，不会命中"
    );
}

#[test]
fn test_schema_prefix_both_tenants_create_same_table_no_conflict() {
    // 单 catalog 共享
    let mut catalog = ManagedCatalog::new();
    let rewriter_a = SqlRewriter::new(TenantContext::new("tA"));
    let rewriter_b = SqlRewriter::new(TenantContext::new("tB"));

    // 租户 A: CREATE TABLE t
    let stmt = parse_one("CREATE TABLE t (id INT)").unwrap();
    let rewritten = rewriter_a.rewrite_statement(stmt).unwrap();
    let name_a = extract_create_table_name(&rewritten);
    assert_eq!(name_a, "tA_t");
    catalog
        .create_table(int_table_schema(&name_a), false)
        .unwrap();

    // 租户 B: CREATE TABLE t（同名！）
    let stmt = parse_one("CREATE TABLE t (id INT)").unwrap();
    let rewritten = rewriter_b.rewrite_statement(stmt).unwrap();
    let name_b = extract_create_table_name(&rewritten);
    assert_eq!(name_b, "tB_t");
    catalog
        .create_table(int_table_schema(&name_b), false)
        .unwrap();

    // 验证：两个租户都能创建表 t，物理表名不同（tA_t vs tB_t），无冲突
    assert!(catalog.table_exists(&TableName::new("tA_t")));
    assert!(catalog.table_exists(&TableName::new("tB_t")));
    assert!(!catalog.table_exists(&TableName::new("t")));
    assert_eq!(
        catalog.table_count(),
        2,
        "catalog 应有 2 张表（tA_t + tB_t）"
    );
}

#[test]
fn test_schema_prefix_drop_isolation() {
    // 单 catalog + 单 storage 共享
    let mut catalog = ManagedCatalog::new();
    let mut storage: HashMap<String, InMemoryTable> = HashMap::new();

    let rewriter_a = SqlRewriter::new(TenantContext::new("tA"));
    let rewriter_b = SqlRewriter::new(TenantContext::new("tB"));

    // 两个租户都创建表 t
    for rewriter in [&rewriter_a, &rewriter_b] {
        let stmt = parse_one("CREATE TABLE t (id INT)").unwrap();
        let rewritten = rewriter.rewrite_statement(stmt).unwrap();
        let name = extract_create_table_name(&rewritten);
        let schema = int_table_schema(&name);
        catalog.create_table(schema.clone(), false).unwrap();
        storage.insert(name.clone(), InMemoryTable::new(schema));
    }

    assert_eq!(catalog.table_count(), 2);
    assert!(storage.contains_key("tA_t"));
    assert!(storage.contains_key("tB_t"));

    // 租户 A: DROP TABLE t → 重写为 DROP TABLE tA_t
    let stmt = parse_one("DROP TABLE t").unwrap();
    let rewritten = rewriter_a.rewrite_statement(stmt).unwrap();
    let drop_names = extract_drop_table_names(&rewritten);
    assert_eq!(drop_names, vec!["tA_t"]);

    // 在 catalog + storage 上执行 DROP
    for name in &drop_names {
        catalog
            .drop_table(&TableName::new(name), false, true)
            .unwrap();
        storage.remove(name);
    }

    // 验证：只有 tA_t 被删除，tB_t 仍然存在
    assert!(!catalog.table_exists(&TableName::new("tA_t")));
    assert!(catalog.table_exists(&TableName::new("tB_t")));
    assert!(!storage.contains_key("tA_t"));
    assert!(storage.contains_key("tB_t"));
    assert_eq!(catalog.table_count(), 1);
}

#[test]
fn test_schema_prefix_cross_tenant_data_no_leak() {
    // 单 catalog + 单 storage，两个租户都创建 t 并插入不同的数据
    let mut catalog = ManagedCatalog::new();
    let mut storage: HashMap<String, InMemoryTable> = HashMap::new();

    let rewriter_a = SqlRewriter::new(TenantContext::new("tA"));
    let rewriter_b = SqlRewriter::new(TenantContext::new("tB"));

    // 租户 A: CREATE TABLE t + INSERT (1), (2), (3)
    let stmt = parse_one("CREATE TABLE t (id INT)").unwrap();
    let rewritten = rewriter_a.rewrite_statement(stmt).unwrap();
    let name_a = extract_create_table_name(&rewritten);
    assert_eq!(name_a, "tA_t");
    let schema = int_table_schema(&name_a);
    catalog.create_table(schema.clone(), false).unwrap();
    storage.insert(name_a.clone(), make_table_with_rows(&name_a, &[1, 2, 3]));

    // 租户 B: CREATE TABLE t + INSERT (10), (20), (30)
    let stmt = parse_one("CREATE TABLE t (id INT)").unwrap();
    let rewritten = rewriter_b.rewrite_statement(stmt).unwrap();
    let name_b = extract_create_table_name(&rewritten);
    assert_eq!(name_b, "tB_t");
    let schema = int_table_schema(&name_b);
    catalog.create_table(schema.clone(), false).unwrap();
    storage.insert(name_b.clone(), make_table_with_rows(&name_b, &[10, 20, 30]));

    // 验证：两个租户的数据完全隔离
    let rows_a = collect_rows(storage.get("tA_t").unwrap());
    let rows_b = collect_rows(storage.get("tB_t").unwrap());

    assert_eq!(rows_a.len(), 3);
    assert_eq!(rows_b.len(), 3);

    // 租户 A 的数据全是 [1, 2, 3]，没有 [10, 20, 30]
    let a_values: Vec<i64> = rows_a
        .iter()
        .filter_map(|r| match r[0] {
            Value::Int64(v) => Some(v),
            _ => None,
        })
        .collect();
    assert_eq!(a_values, vec![1, 2, 3]);

    // 租户 B 的数据全是 [10, 20, 30]，没有 [1, 2, 3]
    let b_values: Vec<i64> = rows_b
        .iter()
        .filter_map(|r| match r[0] {
            Value::Int64(v) => Some(v),
            _ => None,
        })
        .collect();
    assert_eq!(b_values, vec![10, 20, 30]);

    // 0 数据泄漏：租户 A 看不到 10/20/30，租户 B 看不到 1/2/3
    assert!(!a_values.contains(&10));
    assert!(!a_values.contains(&20));
    assert!(!a_values.contains(&30));
    assert!(!b_values.contains(&1));
    assert!(!b_values.contains(&2));
    assert!(!b_values.contains(&3));
}

// =====================================================================
//  SeparateFile 模式 — 独立 catalog + 独立 storage，物理隔离
// =====================================================================

#[test]
fn test_separate_file_both_tenants_create_same_table_no_conflict() {
    // 两个独立的 catalog（物理隔离）
    let mut catalog_a = ManagedCatalog::new();
    let mut catalog_b = ManagedCatalog::new();

    // 两个租户都创建表 t（原名，不重写）
    catalog_a
        .create_table(int_table_schema("t"), false)
        .unwrap();
    catalog_b
        .create_table(int_table_schema("t"), false)
        .unwrap();

    // 验证：两个 catalog 各自独立，都有表 t
    assert!(catalog_a.table_exists(&TableName::new("t")));
    assert!(catalog_b.table_exists(&TableName::new("t")));
    assert_eq!(catalog_a.table_count(), 1);
    assert_eq!(catalog_b.table_count(), 1);

    // 验证：catalog_a 和 catalog_b 是不同的实例
    // 在 catalog_a 中删除 t 不影响 catalog_b
    catalog_a
        .drop_table(&TableName::new("t"), false, true)
        .unwrap();
    assert!(!catalog_a.table_exists(&TableName::new("t")));
    assert!(
        catalog_b.table_exists(&TableName::new("t")),
        "catalog_b 的 t 应仍存在（物理隔离）"
    );
}

#[test]
fn test_separate_file_data_isolation() {
    // 两个独立的 catalog + storage
    let mut catalog_a = ManagedCatalog::new();
    let mut catalog_b = ManagedCatalog::new();
    let mut storage_a: HashMap<String, InMemoryTable> = HashMap::new();
    let mut storage_b: HashMap<String, InMemoryTable> = HashMap::new();

    // 两个租户都创建表 t 并插入不同的数据
    catalog_a
        .create_table(int_table_schema("t"), false)
        .unwrap();
    catalog_b
        .create_table(int_table_schema("t"), false)
        .unwrap();
    storage_a.insert("t".to_string(), make_table_with_rows("t", &[1, 2, 3]));
    storage_b.insert("t".to_string(), make_table_with_rows("t", &[10, 20, 30]));

    // 验证：两个 storage 完全独立
    let rows_a = collect_rows(storage_a.get("t").unwrap());
    let rows_b = collect_rows(storage_b.get("t").unwrap());

    assert_eq!(rows_a.len(), 3);
    assert_eq!(rows_b.len(), 3);

    let a_values: Vec<i64> = rows_a
        .iter()
        .filter_map(|r| match r[0] {
            Value::Int64(v) => Some(v),
            _ => None,
        })
        .collect();
    let b_values: Vec<i64> = rows_b
        .iter()
        .filter_map(|r| match r[0] {
            Value::Int64(v) => Some(v),
            _ => None,
        })
        .collect();

    assert_eq!(a_values, vec![1, 2, 3]);
    assert_eq!(b_values, vec![10, 20, 30]);

    // 0 数据泄漏
    assert!(!a_values.iter().any(|&v| v >= 10));
    assert!(!b_values.iter().any(|&v| v < 10));
}

#[test]
fn test_separate_file_ddl_isolation() {
    // 两个独立的 catalog
    let mut catalog_a = ManagedCatalog::new();
    let mut catalog_b = ManagedCatalog::new();

    // 两个租户都创建表 t
    catalog_a
        .create_table(int_table_schema("t"), false)
        .unwrap();
    catalog_b
        .create_table(int_table_schema("t"), false)
        .unwrap();

    // 租户 A 创建额外的表 u
    catalog_a
        .create_table(int_table_schema("u"), false)
        .unwrap();

    // 验证：catalog_a 有 [t, u]，catalog_b 只有 [t]
    assert_eq!(catalog_a.table_count(), 2);
    assert_eq!(catalog_b.table_count(), 1);
    assert!(catalog_a.table_exists(&TableName::new("u")));
    assert!(
        !catalog_b.table_exists(&TableName::new("u")),
        "catalog_b 不应有表 u（物理隔离）"
    );

    // 租户 A 删除 t，catalog_b 的 t 不受影响
    catalog_a
        .drop_table(&TableName::new("t"), false, true)
        .unwrap();
    assert!(!catalog_a.table_exists(&TableName::new("t")));
    assert!(catalog_b.table_exists(&TableName::new("t")));
    assert_eq!(catalog_a.table_count(), 1);
    assert_eq!(catalog_b.table_count(), 1);
}

#[test]
fn test_separate_file_no_cross_contamination() {
    // 两个独立的 catalog + storage
    let mut catalog_a = ManagedCatalog::new();
    let mut catalog_b = ManagedCatalog::new();
    let mut storage_a: HashMap<String, InMemoryTable> = HashMap::new();
    let mut storage_b: HashMap<String, InMemoryTable> = HashMap::new();

    // 租户 A 创建表 t 并插入数据
    catalog_a
        .create_table(int_table_schema("t"), false)
        .unwrap();
    storage_a.insert("t".to_string(), make_table_with_rows("t", &[100, 200]));

    // 租户 B 此时还没有任何表
    assert_eq!(catalog_b.table_count(), 0);
    assert!(storage_b.is_empty());

    // 租户 B 创建表 t 并插入不同的数据
    catalog_b
        .create_table(int_table_schema("t"), false)
        .unwrap();
    storage_b.insert("t".to_string(), make_table_with_rows("t", &[300, 400]));

    // 验证：租户 A 的数据未受影响
    let rows_a = collect_rows(storage_a.get("t").unwrap());
    let a_values: Vec<i64> = rows_a
        .iter()
        .filter_map(|r| match r[0] {
            Value::Int64(v) => Some(v),
            _ => None,
        })
        .collect();
    assert_eq!(a_values, vec![100, 200], "租户 A 的数据未受租户 B 影响");

    // 验证：租户 B 的数据是独立的
    let rows_b = collect_rows(storage_b.get("t").unwrap());
    let b_values: Vec<i64> = rows_b
        .iter()
        .filter_map(|r| match r[0] {
            Value::Int64(v) => Some(v),
            _ => None,
        })
        .collect();
    assert_eq!(b_values, vec![300, 400], "租户 B 的数据是独立的");

    // 租户 A 添加新行，租户 B 看不到
    storage_a
        .get_mut("t")
        .unwrap()
        .insert(vec![Value::Int64(500)]);
    let rows_b_after = collect_rows(storage_b.get("t").unwrap());
    assert_eq!(
        rows_b_after.len(),
        2,
        "租户 A 添加数据后，租户 B 的数据量不变（无交叉污染）"
    );

    // 租户 B 添加新行，租户 A 看不到
    storage_b
        .get_mut("t")
        .unwrap()
        .insert(vec![Value::Int64(600)]);
    let rows_a_after = collect_rows(storage_a.get("t").unwrap());
    assert_eq!(
        rows_a_after.len(),
        3,
        "租户 B 添加数据后，租户 A 的数据量不变（无交叉污染）"
    );

    // 验证：a_values 包含 500（A 自己加的），不包含 600（B 加的）
    let a_values_after: Vec<i64> = rows_a_after
        .iter()
        .filter_map(|r| match r[0] {
            Value::Int64(v) => Some(v),
            _ => None,
        })
        .collect();
    assert!(a_values_after.contains(&500));
    assert!(!a_values_after.contains(&600));
}

// =====================================================================
//  两种模式对比 — 验证都达到完整隔离
// =====================================================================

#[test]
fn test_both_modes_achieve_isolation() {
    // ===== SchemaPrefix 模式 =====
    let mut catalog_sp = ManagedCatalog::new();
    let mut storage_sp: HashMap<String, InMemoryTable> = HashMap::new();
    let rewriter_a = SqlRewriter::new(TenantContext::new("tA"));
    let rewriter_b = SqlRewriter::new(TenantContext::new("tB"));

    // 租户 A 创建 t 并插入数据
    let stmt = parse_one("CREATE TABLE t (id INT)").unwrap();
    let rewritten = rewriter_a.rewrite_statement(stmt).unwrap();
    let name = extract_create_table_name(&rewritten);
    let schema = int_table_schema(&name);
    catalog_sp.create_table(schema.clone(), false).unwrap();
    storage_sp.insert(name.clone(), make_table_with_rows(&name, &[1, 2]));

    // 租户 B 查询 t → 重写为 tB_t → 找不到
    let stmt = parse_one("SELECT * FROM t").unwrap();
    let rewritten = rewriter_b.rewrite_statement(stmt).unwrap();
    let from_table = extract_select_from_table(&rewritten);
    let sp_b_can_see = storage_sp.contains_key(&from_table);
    assert!(
        !sp_b_can_see,
        "SchemaPrefix 模式：租户 B 看不到租户 A 的数据"
    );

    // ===== SeparateFile 模式 =====
    let mut catalog_sf_a = ManagedCatalog::new();
    let mut catalog_sf_b = ManagedCatalog::new();
    let mut storage_sf_a: HashMap<String, InMemoryTable> = HashMap::new();
    let mut storage_sf_b: HashMap<String, InMemoryTable> = HashMap::new();

    // 租户 A 创建 t 并插入数据
    catalog_sf_a
        .create_table(int_table_schema("t"), false)
        .unwrap();
    storage_sf_a.insert("t".to_string(), make_table_with_rows("t", &[1, 2]));

    // 租户 B 创建自己的 t 并插入不同的数据
    catalog_sf_b
        .create_table(int_table_schema("t"), false)
        .unwrap();
    storage_sf_b.insert("t".to_string(), make_table_with_rows("t", &[10, 20]));

    // 验证：两个 storage 完全独立
    let sf_a_rows = collect_rows(storage_sf_a.get("t").unwrap());
    let sf_b_rows = collect_rows(storage_sf_b.get("t").unwrap());
    let sf_a_values: Vec<i64> = sf_a_rows
        .iter()
        .filter_map(|r| match r[0] {
            Value::Int64(v) => Some(v),
            _ => None,
        })
        .collect();
    let sf_b_values: Vec<i64> = sf_b_rows
        .iter()
        .filter_map(|r| match r[0] {
            Value::Int64(v) => Some(v),
            _ => None,
        })
        .collect();

    assert_eq!(sf_a_values, vec![1, 2]);
    assert_eq!(sf_b_values, vec![10, 20]);
    assert!(
        !sf_a_values.iter().any(|&v| sf_b_values.contains(&v)),
        "SeparateFile 模式：租户间 0 数据重叠"
    );

    // 两种模式都达到完整隔离
}

// =====================================================================
//  端到端：多语句场景 — 完整租户生命周期
// =====================================================================

#[test]
fn test_schema_prefix_full_lifecycle_isolation() {
    // 模拟完整的多租户使用场景：CREATE → INSERT → SELECT → UPDATE → DELETE → DROP
    let mut catalog = ManagedCatalog::new();
    let mut storage: HashMap<String, InMemoryTable> = HashMap::new();

    let rewriter_a = SqlRewriter::new(TenantContext::new("tenant_a"));
    let rewriter_b = SqlRewriter::new(TenantContext::new("tenant_b"));

    // ===== 租户 A 生命周期 =====
    // CREATE TABLE
    let stmt = parse_one("CREATE TABLE accounts (id INT)").unwrap();
    let rewritten = rewriter_a.rewrite_statement(stmt).unwrap();
    let name = extract_create_table_name(&rewritten);
    assert_eq!(name, "tenant_a_accounts");
    let schema = int_table_schema(&name);
    catalog.create_table(schema.clone(), false).unwrap();
    storage.insert(name.clone(), InMemoryTable::new(schema));

    // INSERT
    let stmt = parse_one("INSERT INTO accounts VALUES (1), (2), (3)").unwrap();
    let rewritten = rewriter_a.rewrite_statement(stmt).unwrap();
    let (target, rows) = extract_insert_target_and_rows(&rewritten);
    assert_eq!(target, "tenant_a_accounts");
    let table = storage.get_mut(&target).unwrap();
    for row in rows {
        table.insert(row);
    }

    // 验证：租户 A 有 3 行
    assert_eq!(storage.get("tenant_a_accounts").unwrap().row_count(), 3);

    // ===== 租户 B 生命周期 =====
    // CREATE TABLE（同名 accounts）
    let stmt = parse_one("CREATE TABLE accounts (id INT)").unwrap();
    let rewritten = rewriter_b.rewrite_statement(stmt).unwrap();
    let name = extract_create_table_name(&rewritten);
    assert_eq!(name, "tenant_b_accounts");
    let schema = int_table_schema(&name);
    catalog.create_table(schema.clone(), false).unwrap();
    storage.insert(name.clone(), InMemoryTable::new(schema));

    // INSERT 不同的数据
    let stmt = parse_one("INSERT INTO accounts VALUES (100), (200)").unwrap();
    let rewritten = rewriter_b.rewrite_statement(stmt).unwrap();
    let (target, rows) = extract_insert_target_and_rows(&rewritten);
    assert_eq!(target, "tenant_b_accounts");
    let table = storage.get_mut(&target).unwrap();
    for row in rows {
        table.insert(row);
    }

    // 验证：租户 B 有 2 行
    assert_eq!(storage.get("tenant_b_accounts").unwrap().row_count(), 2);

    // ===== 跨租户验证 =====
    // 租户 A 的查询重写后命中 tenant_a_accounts，看不到 tenant_b_accounts 的数据
    let stmt = parse_one("SELECT * FROM accounts").unwrap();
    let rewritten_a = rewriter_a.rewrite_statement(stmt.clone()).unwrap();
    let from_a = extract_select_from_table(&rewritten_a);
    assert_eq!(from_a, "tenant_a_accounts");

    let rewritten_b = rewriter_b.rewrite_statement(stmt).unwrap();
    let from_b = extract_select_from_table(&rewritten_b);
    assert_eq!(from_b, "tenant_b_accounts");

    // 两个租户看到的是完全不同的物理表
    assert_ne!(from_a, from_b);

    // 验证：两个表中的数据完全不重叠
    let rows_a = collect_rows(storage.get("tenant_a_accounts").unwrap());
    let rows_b = collect_rows(storage.get("tenant_b_accounts").unwrap());

    let values_a: Vec<i64> = rows_a
        .iter()
        .filter_map(|r| match r[0] {
            Value::Int64(v) => Some(v),
            _ => None,
        })
        .collect();
    let values_b: Vec<i64> = rows_b
        .iter()
        .filter_map(|r| match r[0] {
            Value::Int64(v) => Some(v),
            _ => None,
        })
        .collect();

    assert_eq!(values_a, vec![1, 2, 3]);
    assert_eq!(values_b, vec![100, 200]);
    assert!(
        !values_a.iter().any(|v| values_b.contains(v)),
        "租户间数据 0 重叠"
    );

    // ===== 租户 A DROP 表，不影响租户 B =====
    let stmt = parse_one("DROP TABLE accounts").unwrap();
    let rewritten = rewriter_a.rewrite_statement(stmt).unwrap();
    let drop_names = extract_drop_table_names(&rewritten);
    assert_eq!(drop_names, vec!["tenant_a_accounts"]);

    for name in &drop_names {
        catalog
            .drop_table(&TableName::new(name), false, true)
            .unwrap();
        storage.remove(name);
    }

    // 验证：tenant_a_accounts 被删除，tenant_b_accounts 仍存在
    assert!(!storage.contains_key("tenant_a_accounts"));
    assert!(!catalog.table_exists(&TableName::new("tenant_a_accounts")));
    assert!(storage.contains_key("tenant_b_accounts"));
    assert!(catalog.table_exists(&TableName::new("tenant_b_accounts")));

    // 验证：租户 B 的数据仍然完整
    let rows_b_after = collect_rows(storage.get("tenant_b_accounts").unwrap());
    assert_eq!(rows_b_after.len(), 2, "租户 B 的数据未受租户 A DROP 影响");
}

// =====================================================================
//  配额集成 — 验证 Phase 3.10 配额与隔离协同工作
// =====================================================================

#[test]
fn test_quota_integration_with_isolation() {
    use szrsql_catalog::quota::{QuotaError, QuotaManager, TenantQuota};

    // 配额管理器：租户 A 限额 2 connections + 100 bytes；租户 B 限额 10 + 10000
    let mut quota_mgr = QuotaManager::new();
    quota_mgr.set_quota(
        "tA",
        TenantQuota::new()
            .with_max_connections(2)
            .with_max_storage_bytes(100),
    );
    quota_mgr.set_quota(
        "tB",
        TenantQuota::new()
            .with_max_connections(10)
            .with_max_storage_bytes(10000),
    );

    // 租户 A 达到连接上限
    quota_mgr.try_acquire_connection("tA").unwrap();
    quota_mgr.try_acquire_connection("tA").unwrap();
    // 第三次应被拒绝
    let result = quota_mgr.try_acquire_connection("tA");
    assert!(matches!(
        result,
        Err(QuotaError::ConnectionLimitExceeded { .. })
    ));

    // 租户 B 仍可正常获取连接（隔离）
    quota_mgr.try_acquire_connection("tB").unwrap();
    quota_mgr.try_acquire_connection("tB").unwrap();
    quota_mgr.try_acquire_connection("tB").unwrap();

    // 租户 A 达到存储上限
    quota_mgr.try_consume_storage("tA", 100).unwrap();
    let result = quota_mgr.try_consume_storage("tA", 1);
    assert!(matches!(
        result,
        Err(QuotaError::StorageLimitExceeded { .. })
    ));

    // 租户 B 仍可正常消耗存储（隔离）
    quota_mgr.try_consume_storage("tB", 500).unwrap();
    quota_mgr.try_consume_storage("tB", 1000).unwrap();

    // 验证：租户 A 的配额限制不影响租户 B
    let usage_b = quota_mgr.usage("tB").unwrap();
    assert_eq!(usage_b.connections, 3);
    assert_eq!(usage_b.storage_bytes, 1500);

    let usage_a = quota_mgr.usage("tA").unwrap();
    assert_eq!(usage_a.connections, 2);
    assert_eq!(usage_a.storage_bytes, 100);
}
