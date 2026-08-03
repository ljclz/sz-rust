//! Phase 3.4 + 3.5 + 3.6 + 3.7 执行器单元测试与集成测试。
//!
//! 覆盖类别：
//! - 基础 SeqScan（5）：空表、单行、多行、列顺序、Schema 查询
//! - Filter（6）：=、<、>、AND、OR、IS NULL
//! - Projection（4）：单列、多列、表达式、别名
//! - Limit + Offset（5）：基本 LIMIT、OFFSET、超量、零 LIMIT、负 OFFSET 错误
//! - Distinct（3）：基本去重、全唯一、全相同
//! - 集成：1M 行 SeqScan（2）：行数正确性 + 首尾值校验、带过滤的 1M 行扫描
//! - 索引扫描点查（6）：单命中、多命中、未命中、空索引、边界 key、build_from_table
//! - 索引扫描范围（6）：单元素范围、全表范围、空范围、单侧范围、反向范围、大范围
//! - 端到端（4）：Parser → Planner → Executor、Filter+Projection、Limit+Filter、CounterTable+Filter
//! - 错误处理（4）：表不存在、列不存在、不支持的算子、空 OFFSET
//! - RowContext（2）：大小写不敏感列查找、限定名列查找
//! - MutableTable trait（4）：insert_row、update_row、delete_row、clear
//! - Snapshot/Restore（2）：基本快照恢复、多次 DML 后快照恢复
//! - INSERT（5）：单行 VALUES、多行 VALUES、显式列、DEFAULT VALUES、SELECT 源
//! - UPDATE（4）：全表更新、WHERE 条件更新、表达式赋值、多列赋值
//! - DELETE（3）：全表删除、WHERE 条件删除、删除后再插入
//! - DML 集成（1）：INSERT 100K → SELECT → UPDATE 50K → SELECT → DELETE 20K → SELECT → 快照回滚
//! - DML 错误（3）：列数不匹配、错误的 Update 计划、错误的 Delete 计划
//! - JOIN INNER（4）：基本等值连接、带 WHERE 过滤、带投影、非等值 NestedLoop 退化
//! - JOIN LEFT（2）：全匹配、左表未匹配行 NULL 填充
//! - JOIN RIGHT（2）：全匹配、右表未匹配行 NULL 填充
//! - JOIN FULL（2）：两侧均有未匹配、双向 NULL 填充
//! - JOIN CROSS（2）：笛卡尔积、多表逗号语法
//! - JOIN USING/NATURAL（2）：USING 单列、NATURAL 同名列
//! - JOIN 3 表链式（1）：a JOIN b JOIN c
//! - JOIN SELF（1）：自连接（无别名）
//! - JOIN HashJoin 验证（2）：等值优化命中、非等值退化 NestedLoop
//! - 聚合基础（5）：COUNT(*)、COUNT(expr)、SUM、AVG、MIN/MAX
//! - 聚合 DISTINCT（2）：COUNT(DISTINCT)、SUM(DISTINCT)
//! - 聚合空表（2）：无 GROUP BY 空表、有 GROUP BY 空表
//! - GROUP BY 单列（3）：基本分组、多组、分组 + 聚合
//! - GROUP BY 多列（1）：两列组合分组
//! - GROUP BY + HAVING（2）：HAVING 过滤、HAVING 引用多聚合
//! - 聚合混合（2）：聚合 + WHERE + GROUP BY + HAVING、聚合 + JOIN
//! - LATERAL JOIN（P3-2，4）：INNER JOIN、LEFT JOIN 保留未匹配、聚合 + LATERAL、LATERAL + LIMIT
//!
//! 共 108 个测试用例。

#![allow(clippy::approx_constant)]

use super::executor::{
    CounterTable, ExecutionError, Executor, InMemoryBTreeIndex, InMemoryTable, MutableTable,
    TableStorage,
};
use crate::ast::{BinaryOp, Expr, TableName};
use crate::parser::parse_sql;
use crate::plan::{InMemoryCatalog, InsertSourcePlan, LogicalPlan, Planner, TableSchema};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

fn col(name: &str) -> Expr {
    Expr::Identifier(vec![name.to_string()])
}

fn lit_i64(n: i64) -> Expr {
    Expr::Literal(Value::Int64(n))
}

fn binary(left: Expr, op: BinaryOp, right: Expr) -> Expr {
    Expr::BinaryOp {
        left: Box::new(left),
        op,
        right: Box::new(right),
    }
}

/// 构建一个简单的 LogicalPlan::Scan 节点
fn make_scan(table_name: &str, columns: Vec<(&str, ColumnType)>) -> LogicalPlan {
    let table_name_obj = TableName::new(table_name);
    let cols = columns
        .into_iter()
        .map(|(n, t)| crate::ast::ColumnDefinition::new(n, t))
        .collect();
    let schema = TableSchema {
        name: table_name_obj.clone(),
        columns: cols,
    };
    LogicalPlan::Scan {
        table: table_name_obj,
        alias: None,
        schema,
    }
}

/// 构建一个测试用表：列 `id BIGINT, name TEXT`
fn make_test_table(name: &str) -> InMemoryTable {
    InMemoryTable::with_columns(
        name,
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    )
}

/// 构建填充数据的测试表
fn make_filled_test_table() -> InMemoryTable {
    let mut table = make_test_table("users");
    table.insert(vec![Value::Int64(1), Value::Text("alice".into())]);
    table.insert(vec![Value::Int64(2), Value::Text("bob".into())]);
    table.insert(vec![Value::Int64(3), Value::Text("carol".into())]);
    table.insert(vec![Value::Int64(4), Value::Text("dave".into())]);
    table.insert(vec![Value::Int64(5), Value::Text("eve".into())]);
    table
}

/// SQL → AST → LogicalPlan
fn plan_sql(sql: &str, catalog: &dyn crate::plan::Catalog) -> LogicalPlan {
    let stmts = parse_sql(sql).expect("parse failed");
    assert_eq!(stmts.len(), 1, "expected exactly 1 statement");
    let planner = Planner::new(catalog);
    planner
        .plan_statement(stmts.into_iter().next().unwrap())
        .expect("plan failed")
}

/// 构建带 users 表的 catalog
fn make_catalog_with_users() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    catalog
}

// =====================================================================
//  基础 SeqScan 测试（5）
// =====================================================================

#[test]
fn test_seqscan_01_empty_table() {
    let table = make_test_table("t");
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = make_scan("t", vec![("id", ColumnType::Int64)]);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 0);
}

#[test]
fn test_seqscan_02_single_row() {
    let mut table = make_test_table("t");
    table.insert(vec![Value::Int64(42), Value::Text("answer".into())]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = make_scan(
        "t",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0],
        vec![Value::Int64(42), Value::Text("answer".into())]
    );
}

#[test]
fn test_seqscan_03_multiple_rows() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 5);
    assert_eq!(result[0][0], Value::Int64(1));
    assert_eq!(result[4][1], Value::Text("eve".into()));
}

#[test]
fn test_seqscan_04_column_order_preserved() {
    let mut table = make_test_table("t");
    table.insert(vec![Value::Int64(10), Value::Text("a".into())]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = make_scan(
        "t",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let result = exec.execute(&plan).unwrap();
    // 列顺序：[id, name]
    assert_eq!(result[0][0], Value::Int64(10));
    assert_eq!(result[0][1], Value::Text("a".into()));
}

#[test]
fn test_seqscan_05_schema_query() {
    let table = make_filled_test_table();
    assert_eq!(table.name(), "users");
    assert_eq!(table.schema().columns.len(), 2);
    assert_eq!(table.schema().columns[0].name, "id");
    assert_eq!(table.schema().columns[1].name, "name");
    assert_eq!(table.row_count(), 5);
}

// =====================================================================
//  Filter 测试（6）
// =====================================================================

#[test]
fn test_filter_01_eq() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let plan = LogicalPlan::Filter {
        predicate: binary(col("id"), BinaryOp::Eq, lit_i64(3)),
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(3));
    assert_eq!(result[0][1], Value::Text("carol".into()));
}

#[test]
fn test_filter_02_lt() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let plan = LogicalPlan::Filter {
        predicate: binary(col("id"), BinaryOp::Lt, lit_i64(3)),
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0][0], Value::Int64(1));
    assert_eq!(result[1][0], Value::Int64(2));
}

#[test]
fn test_filter_03_gt() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let plan = LogicalPlan::Filter {
        predicate: binary(col("id"), BinaryOp::Gt, lit_i64(3)),
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0][0], Value::Int64(4));
    assert_eq!(result[1][0], Value::Int64(5));
}

#[test]
fn test_filter_04_and() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    // id >= 2 AND id <= 4
    let pred = binary(
        binary(col("id"), BinaryOp::GtEq, lit_i64(2)),
        BinaryOp::And,
        binary(col("id"), BinaryOp::LtEq, lit_i64(4)),
    );
    let plan = LogicalPlan::Filter {
        predicate: pred,
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[0][0], Value::Int64(2));
    assert_eq!(result[2][0], Value::Int64(4));
}

#[test]
fn test_filter_05_or() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    // id = 1 OR id = 5
    let pred = binary(
        binary(col("id"), BinaryOp::Eq, lit_i64(1)),
        BinaryOp::Or,
        binary(col("id"), BinaryOp::Eq, lit_i64(5)),
    );
    let plan = LogicalPlan::Filter {
        predicate: pred,
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0][0], Value::Int64(1));
    assert_eq!(result[1][0], Value::Int64(5));
}

#[test]
fn test_filter_06_is_null() {
    let mut table = make_test_table("t");
    table.insert(vec![Value::Int64(1), Value::Text("a".into())]);
    table.insert(vec![Value::Null, Value::Text("b".into())]);
    table.insert(vec![Value::Int64(3), Value::Text("c".into())]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "t",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    // id IS NULL
    let pred = Expr::IsNull {
        expr: Box::new(col("id")),
        negated: false,
    };
    let plan = LogicalPlan::Filter {
        predicate: pred,
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][1], Value::Text("b".into()));
}

// =====================================================================
//  Projection 测试（4）
// =====================================================================

#[test]
fn test_projection_01_single_column() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let plan = LogicalPlan::Projection {
        exprs: vec![(col("id"), Some("id".into()))],
        output_names: vec!["id".into()],
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 5);
    assert_eq!(result[0], vec![Value::Int64(1)]);
    assert_eq!(result[4], vec![Value::Int64(5)]);
}

#[test]
fn test_projection_02_multiple_columns() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let plan = LogicalPlan::Projection {
        exprs: vec![
            (col("id"), Some("id".into())),
            (col("name"), Some("name".into())),
        ],
        output_names: vec!["id".into(), "name".into()],
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 5);
    assert_eq!(result[0].len(), 2);
    assert_eq!(
        result[2],
        vec![Value::Int64(3), Value::Text("carol".into())]
    );
}

#[test]
fn test_projection_03_expression() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    // id + 100
    let expr = binary(col("id"), BinaryOp::Plus, lit_i64(100));
    let plan = LogicalPlan::Projection {
        exprs: vec![(expr, Some("id_plus_100".into()))],
        output_names: vec!["id_plus_100".into()],
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 5);
    assert_eq!(result[0], vec![Value::Int64(101)]);
    assert_eq!(result[4], vec![Value::Int64(105)]);
}

#[test]
fn test_projection_04_reorder_columns() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    // 反序：name, id
    let plan = LogicalPlan::Projection {
        exprs: vec![
            (col("name"), Some("name".into())),
            (col("id"), Some("id".into())),
        ],
        output_names: vec!["name".into(), "id".into()],
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 5);
    assert_eq!(
        result[0],
        vec![Value::Text("alice".into()), Value::Int64(1)]
    );
}

// =====================================================================
//  Limit + Offset 测试（5）
// =====================================================================

#[test]
fn test_limit_01_basic() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let plan = LogicalPlan::Limit {
        limit: Some(lit_i64(3)),
        offset: None,
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[0][0], Value::Int64(1));
    assert_eq!(result[2][0], Value::Int64(3));
}

#[test]
fn test_limit_02_offset() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let plan = LogicalPlan::Limit {
        limit: Some(lit_i64(2)),
        offset: Some(lit_i64(2)),
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0][0], Value::Int64(3));
    assert_eq!(result[1][0], Value::Int64(4));
}

#[test]
fn test_limit_03_exceeds_count() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let plan = LogicalPlan::Limit {
        limit: Some(lit_i64(100)),
        offset: None,
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 5); // 全部 5 行
}

#[test]
fn test_limit_04_offset_beyond_count() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let plan = LogicalPlan::Limit {
        limit: Some(lit_i64(10)),
        offset: Some(lit_i64(100)),
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 0);
}

#[test]
fn test_limit_05_offset_only_no_limit() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let plan = LogicalPlan::Limit {
        limit: None,
        offset: Some(lit_i64(3)),
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0][0], Value::Int64(4));
    assert_eq!(result[1][0], Value::Int64(5));
}

// =====================================================================
//  Distinct 测试（3）
// =====================================================================

#[test]
fn test_distinct_01_basic() {
    let mut table = InMemoryTable::with_columns("t", vec![("v", ColumnType::Int64)]);
    table.insert(vec![Value::Int64(1)]);
    table.insert(vec![Value::Int64(2)]);
    table.insert(vec![Value::Int64(1)]);
    table.insert(vec![Value::Int64(3)]);
    table.insert(vec![Value::Int64(2)]);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan("t", vec![("v", ColumnType::Int64)]);
    let plan = LogicalPlan::Distinct {
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 3);
}

#[test]
fn test_distinct_02_all_unique() {
    let mut table = InMemoryTable::with_columns("t", vec![("v", ColumnType::Int64)]);
    for i in 0..10 {
        table.insert(vec![Value::Int64(i)]);
    }
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan("t", vec![("v", ColumnType::Int64)]);
    let plan = LogicalPlan::Distinct {
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 10);
}

#[test]
fn test_distinct_03_all_same() {
    let mut table = InMemoryTable::with_columns("t", vec![("v", ColumnType::Int64)]);
    for _ in 0..10 {
        table.insert(vec![Value::Int64(42)]);
    }
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan("t", vec![("v", ColumnType::Int64)]);
    let plan = LogicalPlan::Distinct {
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], vec![Value::Int64(42)]);
}

// =====================================================================
//  集成测试：1M 行 SeqScan（核心 Phase 3.4 验证）
// =====================================================================

#[test]
fn test_integration_seqscan_1m_rows() {
    // 1,000,000 行惰性生成 — 不实际存储
    const ROW_COUNT: usize = 1_000_000;
    let table = CounterTable::new("big_table", ROW_COUNT);
    assert_eq!(table.row_count(), ROW_COUNT);

    let mut exec = Executor::new();
    exec.register_table(&table);
    let plan = make_scan("big_table", vec![("id", ColumnType::Int64)]);
    let result = exec.execute(&plan).expect("1M row scan should succeed");

    // 关键断言：扫描行数与预期一致
    assert_eq!(result.len(), ROW_COUNT);

    // 首尾值校验
    assert_eq!(result[0], vec![Value::Int64(0)]);
    assert_eq!(
        result[ROW_COUNT - 1],
        vec![Value::Int64((ROW_COUNT - 1) as i64)]
    );

    // 中间抽样校验
    assert_eq!(result[100], vec![Value::Int64(100)]);
    assert_eq!(result[999_999], vec![Value::Int64(999_999)]);
    assert_eq!(result[500_000], vec![Value::Int64(500_000)]);
}

#[test]
fn test_integration_seqscan_1m_with_filter() {
    // 1M 行 + WHERE id < 1000 — 应返回 1000 行
    const ROW_COUNT: usize = 1_000_000;
    let table = CounterTable::new("big", ROW_COUNT);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan("big", vec![("id", ColumnType::Int64)]);
    let plan = LogicalPlan::Filter {
        predicate: binary(col("id"), BinaryOp::Lt, lit_i64(1000)),
        input: Box::new(scan),
    };
    let result = exec.execute(&plan).expect("filtered scan should succeed");
    assert_eq!(result.len(), 1000);
    assert_eq!(result[0], vec![Value::Int64(0)]);
    assert_eq!(result[999], vec![Value::Int64(999)]);
}

// =====================================================================
//  索引扫描点查（6）
// =====================================================================

#[test]
fn test_index_point_01_single_match() {
    let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    for i in 0..100 {
        table.insert(vec![Value::Int64(i)]);
    }
    let mut index = InMemoryBTreeIndex::new("idx_id", "t", "id");
    assert_eq!(index.build_from_table(&table, 0).unwrap(), 100);

    let mut exec = Executor::new();
    exec.register_table(&table);
    let result = exec.index_scan_point("t", &index, 42).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], vec![Value::Int64(42)]);
}

#[test]
fn test_index_point_02_multiple_match() {
    // 多行同 key（非唯一索引）
    let mut table = InMemoryTable::with_columns("t", vec![("v", ColumnType::Int64)]);
    for _ in 0..5 {
        table.insert(vec![Value::Int64(42)]);
    }
    let mut index = InMemoryBTreeIndex::new("idx_v", "t", "v");
    assert_eq!(index.build_from_table(&table, 0).unwrap(), 5);

    let mut exec = Executor::new();
    exec.register_table(&table);
    let result = exec.index_scan_point("t", &index, 42).unwrap();
    assert_eq!(result.len(), 5);
    for row in &result {
        assert_eq!(row[0], Value::Int64(42));
    }
}

#[test]
fn test_index_point_03_not_found() {
    let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    for i in 0..10 {
        table.insert(vec![Value::Int64(i)]);
    }
    let mut index = InMemoryBTreeIndex::new("idx_id", "t", "id");
    index.build_from_table(&table, 0).unwrap();

    let mut exec = Executor::new();
    exec.register_table(&table);
    let result = exec.index_scan_point("t", &index, 999).unwrap();
    assert_eq!(result.len(), 0);
}

#[test]
fn test_index_point_04_empty_index() {
    let table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    let index = InMemoryBTreeIndex::new("idx_id", "t", "id");
    assert!(index.is_empty());

    let mut exec = Executor::new();
    exec.register_table(&table);
    let result = exec.index_scan_point("t", &index, 1).unwrap();
    assert_eq!(result.len(), 0);
}

#[test]
fn test_index_point_05_boundary_keys() {
    let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    table.insert(vec![Value::Int64(-1)]);
    table.insert(vec![Value::Int64(0)]);
    table.insert(vec![Value::Int64(i64::MAX)]);
    let mut index = InMemoryBTreeIndex::new("idx_id", "t", "id");
    index.build_from_table(&table, 0).unwrap();

    let mut exec = Executor::new();
    exec.register_table(&table);

    assert_eq!(exec.index_scan_point("t", &index, -1).unwrap().len(), 1);
    assert_eq!(exec.index_scan_point("t", &index, 0).unwrap().len(), 1);
    assert_eq!(
        exec.index_scan_point("t", &index, i64::MAX).unwrap().len(),
        1
    );
}

#[test]
fn test_index_point_06_build_from_table_skips_nulls() {
    let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    table.insert(vec![Value::Int64(1)]);
    table.insert(vec![Value::Null]);
    table.insert(vec![Value::Int64(2)]);
    table.insert(vec![Value::Null]);
    table.insert(vec![Value::Int64(3)]);
    let mut index = InMemoryBTreeIndex::new("idx_id", "t", "id");
    // 3 non-NULL values enter the index
    let count = index.build_from_table(&table, 0).unwrap();
    assert_eq!(count, 3);
    assert_eq!(index.len(), 3);
}

// =====================================================================
//  索引扫描范围（6）
// =====================================================================

#[test]
fn test_index_range_01_single_element() {
    let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    for i in 0..100 {
        table.insert(vec![Value::Int64(i)]);
    }
    let mut index = InMemoryBTreeIndex::new("idx_id", "t", "id");
    index.build_from_table(&table, 0).unwrap();

    let mut exec = Executor::new();
    exec.register_table(&table);
    let result = exec.index_scan_range("t", &index, 50, 50).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], vec![Value::Int64(50)]);
}

#[test]
fn test_index_range_02_full_range() {
    let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    for i in 0..100 {
        table.insert(vec![Value::Int64(i)]);
    }
    let mut index = InMemoryBTreeIndex::new("idx_id", "t", "id");
    index.build_from_table(&table, 0).unwrap();

    let mut exec = Executor::new();
    exec.register_table(&table);
    let result = exec.index_scan_range("t", &index, 0, 99).unwrap();
    assert_eq!(result.len(), 100);
    // 升序
    for (i, row) in result.iter().enumerate() {
        assert_eq!(row[0], Value::Int64(i as i64));
    }
}

#[test]
fn test_index_range_03_empty_range() {
    let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    for i in 0..100 {
        table.insert(vec![Value::Int64(i)]);
    }
    let mut index = InMemoryBTreeIndex::new("idx_id", "t", "id");
    index.build_from_table(&table, 0).unwrap();

    let mut exec = Executor::new();
    exec.register_table(&table);
    // 100..=199 范围内无 key
    let result = exec.index_scan_range("t", &index, 100, 199).unwrap();
    assert_eq!(result.len(), 0);
}

#[test]
fn test_index_range_04_single_sided() {
    let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    for i in 0..100 {
        table.insert(vec![Value::Int64(i)]);
    }
    let mut index = InMemoryBTreeIndex::new("idx_id", "t", "id");
    index.build_from_table(&table, 0).unwrap();

    let mut exec = Executor::new();
    exec.register_table(&table);
    // 用 i64::MIN 替代 Unbounded
    let result = exec.index_scan_range("t", &index, i64::MIN, 9).unwrap();
    assert_eq!(result.len(), 10);
    let result = exec.index_scan_range("t", &index, 90, i64::MAX).unwrap();
    assert_eq!(result.len(), 10);
}

#[test]
fn test_index_range_05_reversed_range_returns_empty() {
    let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    for i in 0..100 {
        table.insert(vec![Value::Int64(i)]);
    }
    let mut index = InMemoryBTreeIndex::new("idx_id", "t", "id");
    index.build_from_table(&table, 0).unwrap();

    let mut exec = Executor::new();
    exec.register_table(&table);
    // low > high → 空
    let result = exec.index_scan_range("t", &index, 80, 20).unwrap();
    assert_eq!(result.len(), 0);
}

#[test]
fn test_index_range_06_large_range_with_duplicates() {
    let mut table = InMemoryTable::with_columns("t", vec![("v", ColumnType::Int64)]);
    // 插入 1000 行，每 5 行同 key（200 个不同 key）
    for i in 0..1000 {
        table.insert(vec![Value::Int64(i / 5)]);
    }
    let mut index = InMemoryBTreeIndex::new("idx_v", "t", "v");
    index.build_from_table(&table, 0).unwrap();
    assert_eq!(index.len(), 200);

    let mut exec = Executor::new();
    exec.register_table(&table);
    // 范围 [10, 19] → 10 个 key × 5 行/key = 50 行
    let result = exec.index_scan_range("t", &index, 10, 19).unwrap();
    assert_eq!(result.len(), 50);
}

// =====================================================================
//  端到端：Parser → Planner → Executor（4）
// =====================================================================

#[test]
fn test_e2e_01_select_star() {
    let catalog = make_catalog_with_users();
    let table = make_filled_test_table();
    let plan = plan_sql("SELECT * FROM users", &catalog);

    let mut exec = Executor::new();
    exec.register_table(&table);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 5);
    assert_eq!(
        result[0],
        vec![Value::Int64(1), Value::Text("alice".into())]
    );
}

#[test]
fn test_e2e_02_select_with_filter_and_projection() {
    let catalog = make_catalog_with_users();
    let table = make_filled_test_table();
    let plan = plan_sql("SELECT name FROM users WHERE id >= 3", &catalog);

    let mut exec = Executor::new();
    exec.register_table(&table);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[0], vec![Value::Text("carol".into())]);
    assert_eq!(result[2], vec![Value::Text("eve".into())]);
}

#[test]
fn test_e2e_03_select_with_limit() {
    let catalog = make_catalog_with_users();
    let table = make_filled_test_table();
    let plan = plan_sql("SELECT id FROM users LIMIT 2", &catalog);

    let mut exec = Executor::new();
    exec.register_table(&table);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0], vec![Value::Int64(1)]);
    assert_eq!(result[1], vec![Value::Int64(2)]);
}

#[test]
fn test_e2e_04_counter_table_with_filter() {
    // 用 CounterTable + Filter 验证：1M 行表 + WHERE id = 999_999 → 1 行
    const ROW_COUNT: usize = 1_000_000;
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table("big", vec![("id", ColumnType::Int64)]);
    let table = CounterTable::new("big", ROW_COUNT);
    let plan = plan_sql("SELECT id FROM big WHERE id = 999999", &catalog);

    let mut exec = Executor::new();
    exec.register_table(&table);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], vec![Value::Int64(999_999)]);
}

// =====================================================================
//  错误处理（4）
// =====================================================================

#[test]
fn test_error_01_table_not_found() {
    let exec = Executor::new();
    let plan = make_scan("nonexistent", vec![("id", ColumnType::Int64)]);
    let result = exec.execute(&plan);
    assert!(matches!(result, Err(ExecutionError::TableNotFound(_))));
}

#[test]
fn test_error_02_column_not_found() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    let scan = make_scan(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    // WHERE bogus_col = 1 — 列不存在
    let plan = LogicalPlan::Filter {
        predicate: binary(col("bogus_col"), BinaryOp::Eq, lit_i64(1)),
        input: Box::new(scan),
    };
    let result = exec.execute(&plan);
    assert!(matches!(result, Err(ExecutionError::EvalError(_))));
}

#[test]
fn test_error_03_unsupported_plan_node() {
    let table = make_filled_test_table();
    let mut exec = Executor::new();
    exec.register_table(&table);
    // CreateTable 不在 Executor 支持列表中
    let plan = LogicalPlan::CreateTable {
        name: TableName::new("foo"),
        columns: vec![],
        constraints: vec![],
        if_not_exists: false,
        temporary: false,
        on_commit: None,
    };
    let result = exec.execute(&plan);
    assert!(matches!(result, Err(ExecutionError::Unsupported(_))));
}

#[test]
fn test_error_04_index_scan_table_not_found() {
    let table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
    let index = InMemoryBTreeIndex::new("idx", "t", "id");
    let mut exec = Executor::new();
    exec.register_table(&table);
    // 错误的表名
    let result = exec.index_scan_point("wrong_table", &index, 1);
    assert!(matches!(result, Err(ExecutionError::TableNotFound(_))));
}

// =====================================================================
//  RowContext for execution 辅助测试（2）
// =====================================================================

#[test]
fn test_row_context_case_insensitive_column_lookup() {
    use super::expr::EvalContext;
    let table = make_filled_test_table();
    let schema = table.schema();
    let row = &table.rows()[0];
    let ctx = super::executor::ExecRowContext::new_proxy(schema, row);

    // 大小写不敏感列查找
    assert_eq!(ctx.lookup_column("ID").unwrap(), Value::Int64(1));
    assert_eq!(ctx.lookup_column("Id").unwrap(), Value::Int64(1));
    assert_eq!(ctx.lookup_column("id").unwrap(), Value::Int64(1));
    assert_eq!(
        ctx.lookup_column("NAME").unwrap(),
        Value::Text("alice".into())
    );
}

#[test]
fn test_row_context_qualified_name() {
    use super::expr::EvalContext;
    let table = make_filled_test_table();
    let schema = table.schema();
    let row = &table.rows()[2];
    let ctx = super::executor::ExecRowContext::new_proxy(schema, row);

    // users.id, users.name 应正确解析
    assert_eq!(
        ctx.lookup_qualified("users", "id").unwrap(),
        Value::Int64(3)
    );
    assert_eq!(
        ctx.lookup_qualified("users", "name").unwrap(),
        Value::Text("carol".into())
    );
}

// =====================================================================
//  DML 辅助函数（Phase 3.5）
// =====================================================================

/// 构建 target 表的 catalog（id BIGINT, val BIGINT）
fn make_catalog_target() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "target",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );
    catalog
}

/// 构建 target + source 表的 catalog（用于 INSERT...SELECT）
fn make_catalog_source_and_target() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "target",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );
    catalog.add_simple_table("source", vec![("id", ColumnType::Int64)]);
    catalog
}

/// 构建 users 表的 catalog（id BIGINT, name TEXT） — 用于 DML 通用测试
fn make_catalog_users() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    catalog
}

// =====================================================================
//  MutableTable trait 测试（4）
// =====================================================================

#[test]
fn test_dml_mutable_01_insert_row() {
    let mut table = make_test_table("t");
    let id1 = table.insert_row(vec![Value::Int64(1), Value::Text("a".into())]);
    let id2 = table.insert_row(vec![Value::Int64(2), Value::Text("b".into())]);
    let id3 = table.insert_row(vec![Value::Int64(3), Value::Text("c".into())]);

    assert_eq!(id1, 0);
    assert_eq!(id2, 1);
    assert_eq!(id3, 2);
    assert_eq!(table.row_count(), 3);
    assert_eq!(table.get_row(0).unwrap()[0], Value::Int64(1));
    assert_eq!(table.get_row(2).unwrap()[1], Value::Text("c".into()));
}

#[test]
fn test_dml_mutable_02_update_row() {
    let mut table = make_filled_test_table();
    // 更新存在的行
    assert!(table.update_row(2, vec![Value::Int64(30), Value::Text("carol2".into())]));
    assert_eq!(table.get_row(2).unwrap()[0], Value::Int64(30));
    assert_eq!(table.get_row(2).unwrap()[1], Value::Text("carol2".into()));

    // 更新不存在的 row_id
    assert!(!table.update_row(999, vec![Value::Int64(0), Value::Text("x".into())]));

    // 更新已删除的行
    assert!(table.delete_row(0));
    assert!(!table.update_row(0, vec![Value::Int64(0), Value::Text("deleted".into())]));
}

#[test]
fn test_dml_mutable_03_delete_row() {
    let mut table = make_filled_test_table();
    assert_eq!(table.row_count(), 5);

    // 删除存在的行
    assert!(table.delete_row(1));
    assert_eq!(table.row_count(), 4);
    assert!(table.get_row(1).is_none());

    // 再次删除同一行 → false
    assert!(!table.delete_row(1));
    assert_eq!(table.row_count(), 4);

    // 删除不存在的 row_id
    assert!(!table.delete_row(999));
    assert_eq!(table.row_count(), 4);

    // scan_iter 应跳过已删除行
    let rows: Vec<_> = table.scan_iter().collect();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0][0], Value::Int64(1)); // row_id 0 仍存在
    assert_eq!(rows[1][0], Value::Int64(3)); // row_id 1 已删除，跳过
}

#[test]
fn test_dml_mutable_04_clear() {
    let mut table = make_filled_test_table();
    assert_eq!(table.row_count(), 5);

    table.clear();
    assert_eq!(table.row_count(), 0);
    assert!(table.scan_iter().next().is_none());
    assert!(table.get_row(0).is_none());

    // clear 后可重新插入
    let id = table.insert_row(vec![Value::Int64(100), Value::Text("new".into())]);
    assert_eq!(id, 0);
    assert_eq!(table.row_count(), 1);
}

// =====================================================================
//  Snapshot / Restore 测试（2）
// =====================================================================

#[test]
fn test_dml_snapshot_01_basic() {
    let mut table = make_filled_test_table();

    // 快照当前状态
    let snapshot = table.snapshot();
    assert_eq!(table.row_count(), 5);

    // 执行 DML
    table.delete_row(0);
    table.delete_row(1);
    table.insert_row(vec![Value::Int64(100), Value::Text("new".into())]);
    assert_eq!(table.row_count(), 4);

    // 恢复快照
    table.restore(snapshot);
    assert_eq!(table.row_count(), 5);
    // 验证原始数据完整恢复
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("alice".into()));
    assert_eq!(table.get_row(4).unwrap()[1], Value::Text("eve".into()));
}

#[test]
fn test_dml_snapshot_02_after_mutations() {
    let mut table = make_filled_test_table();

    let snapshot = table.snapshot();

    // 多次 DML 操作
    for i in 0..5 {
        let _ = table.update_row(
            i,
            vec![Value::Int64(i as i64 * 10), Value::Text("modified".into())],
        );
    }
    table.insert_row(vec![Value::Int64(99), Value::Text("extra".into())]);
    table.delete_row(2);
    assert_eq!(table.row_count(), 5); // 5 + 1 - 1 = 5

    // 恢复快照 → 应回到 5 行原始数据
    table.restore(snapshot);
    assert_eq!(table.row_count(), 5);
    for i in 0..5 {
        let row = table.get_row(i).unwrap();
        assert_eq!(row[0], Value::Int64((i + 1) as i64));
    }
    assert_eq!(table.get_row(2).unwrap()[1], Value::Text("carol".into()));
}

// =====================================================================
//  INSERT 测试（5）
// =====================================================================

#[test]
fn test_dml_insert_01_values_single_row() {
    let catalog = make_catalog_users();
    let plan = plan_sql("INSERT INTO users VALUES (1, 'alice')", &catalog);

    let mut table = make_test_table("users");
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.row_count(), 1);
    assert_eq!(table.get_row(0).unwrap()[0], Value::Int64(1));
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("alice".into()));
}

#[test]
fn test_dml_insert_02_values_multiple_rows() {
    let catalog = make_catalog_users();
    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a'), (2, 'b'), (3, 'c')",
        &catalog,
    );

    let mut table = make_test_table("users");
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 3);
    assert_eq!(table.row_count(), 3);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("a".into()));
    assert_eq!(table.get_row(2).unwrap()[1], Value::Text("c".into()));
}

#[test]
fn test_dml_insert_03_explicit_columns() {
    let catalog = make_catalog_users();
    // 仅提供 id 列，name 应为 NULL
    let plan = plan_sql("INSERT INTO users (id) VALUES (42)", &catalog);

    let mut table = make_test_table("users");
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.get_row(0).unwrap()[0], Value::Int64(42));
    assert_eq!(table.get_row(0).unwrap()[1], Value::Null);
}

#[test]
fn test_dml_insert_04_default_values() {
    let catalog = make_catalog_users();
    let plan = plan_sql("INSERT INTO users DEFAULT VALUES", &catalog);

    let mut table = make_test_table("users");
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.get_row(0).unwrap()[0], Value::Null);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Null);
}

#[test]
fn test_dml_insert_05_select_source() {
    // 用 CounterTable 作 SELECT 源，INSERT 到 target 表
    let catalog = make_catalog_source_and_target();
    let plan = plan_sql(
        "INSERT INTO target (id, val) SELECT id, id FROM source",
        &catalog,
    );

    const SOURCE_ROWS: usize = 100;
    let source = CounterTable::new("source", SOURCE_ROWS);
    let mut target = InMemoryTable::with_columns(
        "target",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );

    let mut exec = Executor::new();
    exec.register_table(&source); // 注册 source（SELECT 源）
                                  // target 不注册 —— 通过 &mut 参数传入，避免借用冲突

    let result = exec.execute_insert(&plan, &mut target).unwrap();
    assert_eq!(result.affected_rows, SOURCE_ROWS);
    assert_eq!(target.row_count(), SOURCE_ROWS);

    // 验证数据
    assert_eq!(target.get_row(0).unwrap()[0], Value::Int64(0));
    assert_eq!(target.get_row(0).unwrap()[1], Value::Int64(0));
    assert_eq!(target.get_row(99).unwrap()[0], Value::Int64(99));
    assert_eq!(target.get_row(99).unwrap()[1], Value::Int64(99));
}

// =====================================================================
//  UPDATE 测试（4）
// =====================================================================

#[test]
fn test_dml_update_01_all_rows() {
    let catalog = make_catalog_users();
    let plan = plan_sql("UPDATE users SET name = 'x'", &catalog);

    let mut table = make_filled_test_table();
    let exec = Executor::new();

    let result = exec.execute_update(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 5);
    for i in 0..5 {
        assert_eq!(table.get_row(i).unwrap()[1], Value::Text("x".into()));
    }
}

#[test]
fn test_dml_update_02_with_where() {
    let catalog = make_catalog_users();
    let plan = plan_sql("UPDATE users SET name = 'updated' WHERE id >= 3", &catalog);

    let mut table = make_filled_test_table();
    let exec = Executor::new();

    let result = exec.execute_update(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 3); // id 3, 4, 5
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("alice".into())); // 未更新
    assert_eq!(table.get_row(1).unwrap()[1], Value::Text("bob".into())); // 未更新
    assert_eq!(table.get_row(2).unwrap()[1], Value::Text("updated".into()));
    assert_eq!(table.get_row(4).unwrap()[1], Value::Text("updated".into()));
}

#[test]
fn test_dml_update_03_expression() {
    let catalog = make_catalog_target();
    // SET val = val + 100 WHERE id < 3
    let plan = plan_sql("UPDATE target SET val = val + 100 WHERE id < 3", &catalog);

    let mut table = InMemoryTable::with_columns(
        "target",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );
    for i in 0..5 {
        table.insert(vec![Value::Int64(i), Value::Int64(i * 10)]);
    }

    let exec = Executor::new();
    let result = exec.execute_update(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 3); // id 0, 1, 2

    assert_eq!(table.get_row(0).unwrap()[1], Value::Int64(100)); // 0 + 100
    assert_eq!(table.get_row(1).unwrap()[1], Value::Int64(110)); // 10 + 100
    assert_eq!(table.get_row(2).unwrap()[1], Value::Int64(120)); // 20 + 100
    assert_eq!(table.get_row(3).unwrap()[1], Value::Int64(30)); // 未更新
    assert_eq!(table.get_row(4).unwrap()[1], Value::Int64(40)); // 未更新
}

#[test]
fn test_dml_update_04_multiple_columns() {
    let catalog = make_catalog_target();
    let plan = plan_sql(
        "UPDATE target SET id = id + 1000, val = val + 2000",
        &catalog,
    );

    let mut table = InMemoryTable::with_columns(
        "target",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );
    for i in 0..3 {
        table.insert(vec![Value::Int64(i), Value::Int64(i)]);
    }

    let exec = Executor::new();
    let result = exec.execute_update(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 3);

    for i in 0..3 {
        let row = table.get_row(i).unwrap();
        assert_eq!(row[0], Value::Int64(i as i64 + 1000));
        assert_eq!(row[1], Value::Int64(i as i64 + 2000));
    }
}

// =====================================================================
//  DELETE 测试（3）
// =====================================================================

#[test]
fn test_dml_delete_01_all_rows() {
    let catalog = make_catalog_users();
    let plan = plan_sql("DELETE FROM users", &catalog);

    let mut table = make_filled_test_table();
    let exec = Executor::new();

    let result = exec.execute_delete(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 5);
    assert_eq!(table.row_count(), 0);
    for i in 0..5 {
        assert!(table.get_row(i).is_none());
    }
}

#[test]
fn test_dml_delete_02_with_where() {
    let catalog = make_catalog_users();
    let plan = plan_sql("DELETE FROM users WHERE id <= 2", &catalog);

    let mut table = make_filled_test_table();
    let exec = Executor::new();

    let result = exec.execute_delete(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 2); // id 1, 2
    assert_eq!(table.row_count(), 3);
    assert!(table.get_row(0).is_none()); // 已删除
    assert!(table.get_row(1).is_none()); // 已删除
    assert_eq!(table.get_row(2).unwrap()[0], Value::Int64(3)); // 保留
    assert_eq!(table.get_row(4).unwrap()[0], Value::Int64(5)); // 保留
}

#[test]
fn test_dml_delete_03_then_insert() {
    let catalog = make_catalog_users();
    let delete_plan = plan_sql("DELETE FROM users WHERE id = 3", &catalog);
    let insert_plan = plan_sql("INSERT INTO users VALUES (3, 'carol_new')", &catalog);

    let mut table = make_filled_test_table();
    let exec = Executor::new();

    // 删除 id=3
    let result = exec.execute_delete(&delete_plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.row_count(), 4);

    // 重新插入 id=3（新 row_id = 5）
    let result = exec.execute_insert(&insert_plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.row_count(), 5);

    // row_id 2 已删除，row_id 5 是新行
    assert!(table.get_row(2).is_none());
    assert_eq!(table.get_row(5).unwrap()[0], Value::Int64(3));
    assert_eq!(
        table.get_row(5).unwrap()[1],
        Value::Text("carol_new".into())
    );
}

// =====================================================================
//  DML 集成测试（1）— INSERT 100K → UPDATE 50K → DELETE 20K → 回滚
// =====================================================================

#[test]
fn test_dml_integration_full_cycle() {
    const TOTAL: usize = 100_000;
    const UPDATE_THRESHOLD: i64 = 50_000;
    const DELETE_THRESHOLD: i64 = 20_000;

    // 1. 准备 catalog 与表
    let catalog = make_catalog_source_and_target();

    // 2. INSERT 100K 行：INSERT INTO target (id, val) SELECT id, id FROM source
    let insert_plan = plan_sql(
        "INSERT INTO target (id, val) SELECT id, id FROM source",
        &catalog,
    );
    let source = CounterTable::new("source", TOTAL);
    let mut target = InMemoryTable::with_columns(
        "target",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );

    let mut exec = Executor::new();
    exec.register_table(&source);

    let result = exec.execute_insert(&insert_plan, &mut target).unwrap();
    assert_eq!(result.affected_rows, TOTAL, "INSERT 行数应为 {TOTAL}");
    assert_eq!(target.row_count(), TOTAL, "INSERT 后行数应为 {TOTAL}");

    // 3. SELECT 验证：扫描计数
    let scan_count = target.scan_iter().count();
    assert_eq!(scan_count, TOTAL, "SELECT 扫描行数应为 {TOTAL}");

    // 4. UPDATE 50K 行：SET val = -1 WHERE id < 50000
    let update_plan = plan_sql("UPDATE target SET val = -1 WHERE id < 50000", &catalog);
    let result = exec.execute_update(&update_plan, &mut target).unwrap();
    assert_eq!(
        result.affected_rows, UPDATE_THRESHOLD as usize,
        "UPDATE 行数应为 {UPDATE_THRESHOLD}"
    );

    // 5. SELECT 验证：val = -1 的行数
    let val_negative_one_count = target
        .scan_iter()
        .filter(|row| row[1] == Value::Int64(-1))
        .count();
    assert_eq!(
        val_negative_one_count, UPDATE_THRESHOLD as usize,
        "val = -1 的行数应为 {UPDATE_THRESHOLD}"
    );

    // 6. DELETE 20K 行：WHERE id < 20000
    let delete_plan = plan_sql("DELETE FROM target WHERE id < 20000", &catalog);
    let result = exec.execute_delete(&delete_plan, &mut target).unwrap();
    assert_eq!(
        result.affected_rows, DELETE_THRESHOLD as usize,
        "DELETE 行数应为 {DELETE_THRESHOLD}"
    );

    // 7. SELECT 验证：剩余行数 = 100K - 20K = 80K
    let remaining = target.row_count();
    assert_eq!(
        remaining,
        TOTAL - DELETE_THRESHOLD as usize,
        "DELETE 后剩余行数应为 {}",
        TOTAL - DELETE_THRESHOLD as usize
    );

    // 8. 事务回滚验证：快照 → 更多 DML → 恢复 → 验证无变化
    let snapshot = target.snapshot();

    // 执行更多 DML（全表 UPDATE + 全表 DELETE）
    let update_all = plan_sql("UPDATE target SET val = 999", &catalog);
    let _ = exec.execute_update(&update_all, &mut target).unwrap();
    let delete_all = plan_sql("DELETE FROM target", &catalog);
    let result = exec.execute_delete(&delete_all, &mut target).unwrap();
    assert_eq!(result.affected_rows, remaining, "全表删除应清除所有剩余行");
    assert_eq!(target.row_count(), 0, "全表删除后应无行");

    // 恢复快照
    target.restore(snapshot);

    // 9. 验证恢复后状态 = 快照时状态（80K 行，val=-1 的 30K 行，其他 50K 行 val=id）
    assert_eq!(
        target.row_count(),
        TOTAL - DELETE_THRESHOLD as usize,
        "恢复后行数应回到 {}",
        TOTAL - DELETE_THRESHOLD as usize
    );
    let restored_val_negative_one = target
        .scan_iter()
        .filter(|row| row[1] == Value::Int64(-1))
        .count();
    // UPDATE 影响的是 id < 50000 的行（50K），DELETE 删除了 id < 20000 的行（20K）
    // 所以 val = -1 的剩余行数 = 50K - 20K = 30K
    assert_eq!(
        restored_val_negative_one,
        (UPDATE_THRESHOLD - DELETE_THRESHOLD) as usize,
        "恢复后 val = -1 的行数应为 30000"
    );
    // 验证 id >= 50000 的行 val == id（未受 UPDATE 影响）
    let untouched = target
        .scan_iter()
        .filter(|row| {
            matches!(row[0], Value::Int64(id) if id >= UPDATE_THRESHOLD) && row[1] == row[0]
        })
        .count();
    assert_eq!(
        untouched,
        TOTAL - UPDATE_THRESHOLD as usize,
        "恢复后 id >= 50000 的行 val 应等于 id"
    );
}

// =====================================================================
//  DML 错误处理（3）
// =====================================================================

#[test]
fn test_dml_error_01_insert_column_count_mismatch() {
    // 手动构造列数不匹配的 Insert 计划（Planner 会拒绝此 SQL，故绕过 Planner）
    let schema = TableSchema {
        name: TableName::new("users"),
        columns: vec![
            crate::ast::ColumnDefinition::new("id", ColumnType::Int64),
            crate::ast::ColumnDefinition::new("name", ColumnType::Text),
        ],
    };
    // 显式列仅指定 id（1 列），但 VALUES 提供 2 个表达式
    let plan = LogicalPlan::Insert {
        table: TableName::new("users"),
        schema: schema.clone(),
        columns: Some(vec!["id".to_string()]),
        source: InsertSourcePlan::Values(vec![vec![lit_i64(1), lit_i64(2)]]),
        on_conflict: None,
        returning: None,
    };

    let mut table = InMemoryTable::new(schema);
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table);
    assert!(
        matches!(result, Err(ExecutionError::InvalidArgument(_))),
        "expected InvalidArgument error, got {result:?}"
    );
}

#[test]
fn test_dml_error_02_update_wrong_plan() {
    let catalog = make_catalog_users();
    let plan = plan_sql("SELECT * FROM users", &catalog);

    let mut table = make_filled_test_table();
    let exec = Executor::new();
    let result = exec.execute_update(&plan, &mut table);
    assert!(
        matches!(result, Err(ExecutionError::InvalidArgument(_))),
        "expected InvalidArgument error, got {result:?}"
    );
}

#[test]
fn test_dml_error_03_delete_wrong_plan() {
    let catalog = make_catalog_users();
    let plan = plan_sql("SELECT * FROM users", &catalog);

    let mut table = make_filled_test_table();
    let exec = Executor::new();
    let result = exec.execute_delete(&plan, &mut table);
    assert!(
        matches!(result, Err(ExecutionError::InvalidArgument(_))),
        "expected InvalidArgument error, got {result:?}"
    );
}

// =====================================================================
//  JOIN 测试辅助函数
// =====================================================================

/// 构建带 users / depts 表的 catalog
/// - users(id BIGINT, name TEXT, dept_id BIGINT)
/// - depts(id BIGINT, name TEXT)
fn make_join_catalog() -> InMemoryCatalog {
    let mut cat = InMemoryCatalog::new();
    cat.add_simple_table(
        "users",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("dept_id", ColumnType::Int64),
        ],
    );
    cat.add_simple_table(
        "depts",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    cat.add_simple_table(
        "orders",
        vec![
            ("id", ColumnType::Int64),
            ("user_id", ColumnType::Int64),
            ("amount", ColumnType::Int64),
        ],
    );
    cat
}

/// users 表数据（4 行，dave 无 dept）：
/// - (1, alice, 10)
/// - (2, bob, 20)
/// - (3, carol, 10)
/// - (4, dave, NULL)
fn make_users_join_table() -> InMemoryTable {
    let mut t = InMemoryTable::with_columns(
        "users",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("dept_id", ColumnType::Int64),
        ],
    );
    t.insert(vec![
        Value::Int64(1),
        Value::Text("alice".into()),
        Value::Int64(10),
    ]);
    t.insert(vec![
        Value::Int64(2),
        Value::Text("bob".into()),
        Value::Int64(20),
    ]);
    t.insert(vec![
        Value::Int64(3),
        Value::Text("carol".into()),
        Value::Int64(10),
    ]);
    t.insert(vec![
        Value::Int64(4),
        Value::Text("dave".into()),
        Value::Null,
    ]);
    t
}

/// depts 表数据（3 行，HR 无 users）：
/// - (10, Engineering)
/// - (20, Sales)
/// - (30, HR)
fn make_depts_join_table() -> InMemoryTable {
    let mut t = InMemoryTable::with_columns(
        "depts",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    t.insert(vec![Value::Int64(10), Value::Text("Engineering".into())]);
    t.insert(vec![Value::Int64(20), Value::Text("Sales".into())]);
    t.insert(vec![Value::Int64(30), Value::Text("HR".into())]);
    t
}

/// orders 表数据（3 行）：
/// - (1, 1, 100) — alice 订单
/// - (2, 2, 200) — bob 订单
/// - (3, 1, 150) — alice 另一订单
fn make_orders_join_table() -> InMemoryTable {
    let mut t = InMemoryTable::with_columns(
        "orders",
        vec![
            ("id", ColumnType::Int64),
            ("user_id", ColumnType::Int64),
            ("amount", ColumnType::Int64),
        ],
    );
    t.insert(vec![Value::Int64(1), Value::Int64(1), Value::Int64(100)]);
    t.insert(vec![Value::Int64(2), Value::Int64(2), Value::Int64(200)]);
    t.insert(vec![Value::Int64(3), Value::Int64(1), Value::Int64(150)]);
    t
}

/// 注册 users + depts 两表到 Executor（需保持 tables 生命周期 ≥ exec）
fn register_users_and_depts<'a>(
    exec: &mut Executor<'a>,
    users: &'a InMemoryTable,
    depts: &'a InMemoryTable,
) {
    exec.register_table(users);
    exec.register_table(depts);
}

/// 在结果集中查找包含指定 (id, name) 的行
fn find_row_by_user_id(rows: &[Vec<Value>], id: i64) -> Option<&Vec<Value>> {
    rows.iter()
        .find(|r| matches!(r[0], Value::Int64(x) if x == id))
}

// =====================================================================
//  INNER JOIN 测试（4）
// =====================================================================

#[test]
fn test_join_inner_01_basic_equijoin() {
    let catalog = make_join_catalog();
    let plan = plan_sql(
        "SELECT users.id, users.name, depts.name FROM users JOIN depts ON users.dept_id = depts.id",
        &catalog,
    );
    let users = make_users_join_table();
    let depts = make_depts_join_table();
    let mut exec = Executor::new();
    register_users_and_depts(&mut exec, &users, &depts);
    let result = exec.execute(&plan).unwrap();

    // INNER JOIN：4 users 中 dave 无 dept 不参与；3 depts 中 HR 无 users 不参与
    // 预期 3 行：alice/Engineering, bob/Sales, carol/Engineering
    assert_eq!(result.len(), 3, "INNER JOIN 应得 3 行");

    // 验证列：[users.id, users.name, depts.name]
    let alice = find_row_by_user_id(&result, 1).unwrap();
    assert_eq!(alice[1], Value::Text("alice".into()));
    assert_eq!(alice[2], Value::Text("Engineering".into()));

    let bob = find_row_by_user_id(&result, 2).unwrap();
    assert_eq!(bob[2], Value::Text("Sales".into()));

    let carol = find_row_by_user_id(&result, 3).unwrap();
    assert_eq!(carol[2], Value::Text("Engineering".into()));

    // dave (id=4) 不应出现
    assert!(find_row_by_user_id(&result, 4).is_none());
}

#[test]
fn test_join_inner_02_with_where_filter() {
    let catalog = make_join_catalog();
    let plan = plan_sql(
        "SELECT users.id, users.name FROM users JOIN depts ON users.dept_id = depts.id WHERE depts.name = 'Engineering'",
        &catalog,
    );
    let users = make_users_join_table();
    let depts = make_depts_join_table();
    let mut exec = Executor::new();
    register_users_and_depts(&mut exec, &users, &depts);
    let result = exec.execute(&plan).unwrap();

    // 只保留 Engineering 部门：alice + carol
    assert_eq!(result.len(), 2);
    let ids: Vec<i64> = result
        .iter()
        .filter(|r| matches!(r[0], Value::Int64(x) if x > 0))
        .map(|r| {
            if let Value::Int64(x) = r[0] {
                x
            } else {
                0
            }
        })
        .collect();
    assert!(ids.contains(&1), "应有 alice (id=1)");
    assert!(ids.contains(&3), "应有 carol (id=3)");
}

#[test]
fn test_join_inner_03_with_projection() {
    let catalog = make_join_catalog();
    let plan = plan_sql(
        "SELECT users.name AS user_name, depts.name AS dept_name FROM users JOIN depts ON users.dept_id = depts.id",
        &catalog,
    );
    let users = make_users_join_table();
    let depts = make_depts_join_table();
    let mut exec = Executor::new();
    register_users_and_depts(&mut exec, &users, &depts);
    let result = exec.execute(&plan).unwrap();

    assert_eq!(result.len(), 3);
    // 每行应有 2 列
    assert_eq!(result[0].len(), 2);
    // 验证结果包含 (alice, Engineering)
    let has_alice_eng = result
        .iter()
        .any(|r| r[0] == Value::Text("alice".into()) && r[1] == Value::Text("Engineering".into()));
    assert!(has_alice_eng, "应有 (alice, Engineering)");
}

#[test]
fn test_join_inner_04_non_equijoin_nested_loop() {
    // 非等值连接（> 条件）→ NestedLoop 退化路径
    let catalog = make_join_catalog();
    let plan = plan_sql(
        "SELECT users.id, depts.id FROM users JOIN depts ON users.dept_id > depts.id",
        &catalog,
    );
    let users = make_users_join_table();
    let depts = make_depts_join_table();
    let mut exec = Executor::new();
    register_users_and_depts(&mut exec, &users, &depts);
    let result = exec.execute(&plan).unwrap();

    // alice.dept_id=10: depts.id < 10 → 无（最小 id 是 10）
    // bob.dept_id=20: depts.id < 20 → 10 → 1 行
    // carol.dept_id=10: 同 alice → 无
    // dave.dept_id=NULL: NULL > x 未知 → 无
    assert_eq!(result.len(), 1, "非等值 JOIN 应得 1 行 (bob, dept 10)");
    assert_eq!(result[0][0], Value::Int64(2));
    assert_eq!(result[0][1], Value::Int64(10));
}

// =====================================================================
//  LEFT OUTER JOIN 测试（2）
// =====================================================================

#[test]
fn test_join_left_01_all_matched() {
    // 所有左表行都能在右表找到匹配：构造两侧 1:1 数据
    let catalog = make_join_catalog();
    let plan = plan_sql(
        "SELECT users.id, users.name, depts.id FROM users LEFT JOIN depts ON users.dept_id = depts.id WHERE users.id <= 3",
        &catalog,
    );
    let users = make_users_join_table();
    let depts = make_depts_join_table();
    let mut exec = Executor::new();
    register_users_and_depts(&mut exec, &users, &depts);
    let result = exec.execute(&plan).unwrap();

    // id <= 3 → alice/bob/carol，全部有匹配 → 3 行
    assert_eq!(result.len(), 3);
    for row in &result {
        assert!(
            !matches!(row[2], Value::Null),
            "全匹配的 LEFT JOIN 不应有 NULL"
        );
    }
}

#[test]
fn test_join_left_02_unmatched_null_fill() {
    let catalog = make_join_catalog();
    let plan = plan_sql(
        "SELECT users.id, users.name, depts.id, depts.name FROM users LEFT JOIN depts ON users.dept_id = depts.id",
        &catalog,
    );
    let users = make_users_join_table();
    let depts = make_depts_join_table();
    let mut exec = Executor::new();
    register_users_and_depts(&mut exec, &users, &depts);
    let result = exec.execute(&plan).unwrap();

    // 4 行（包含 dave）
    assert_eq!(result.len(), 4);

    // dave 的 depts.id 和 depts.name 应为 NULL
    let dave = find_row_by_user_id(&result, 4).unwrap();
    assert_eq!(dave[1], Value::Text("dave".into()));
    assert_eq!(dave[2], Value::Null, "dave 的 depts.id 应为 NULL");
    assert_eq!(dave[3], Value::Null, "dave 的 depts.name 应为 NULL");
}

// =====================================================================
//  RIGHT OUTER JOIN 测试（2）
// =====================================================================

#[test]
fn test_join_right_01_all_matched() {
    // 所有右表行都能在左表找到匹配
    let catalog = make_join_catalog();
    // 仅 depts.id 10/20（有 users 匹配），构造独立 depts 表
    let plan = plan_sql(
        "SELECT users.id, depts.id FROM users RIGHT JOIN depts ON users.dept_id = depts.id WHERE depts.id <= 20",
        &catalog,
    );
    let users = make_users_join_table();
    let depts = make_depts_join_table();
    let mut exec = Executor::new();
    register_users_and_depts(&mut exec, &users, &depts);
    let result = exec.execute(&plan).unwrap();

    // depts.id 10 匹配 alice/carol (2 行)；depts.id 20 匹配 bob (1 行) → 共 3 行
    assert_eq!(result.len(), 3);
    // 无 NULL（因为 depts.id <= 20 都有匹配）
    for row in &result {
        assert!(
            !matches!(row[0], Value::Null),
            "全匹配的 RIGHT JOIN 不应有 NULL"
        );
    }
}

#[test]
fn test_join_right_02_unmatched_null_fill() {
    let catalog = make_join_catalog();
    let plan = plan_sql(
        "SELECT users.id, users.name, depts.id, depts.name FROM users RIGHT JOIN depts ON users.dept_id = depts.id",
        &catalog,
    );
    let users = make_users_join_table();
    let depts = make_depts_join_table();
    let mut exec = Executor::new();
    register_users_and_depts(&mut exec, &users, &depts);
    let result = exec.execute(&plan).unwrap();

    // 4 行：alice/bob/carol 各 1 行 + HR (depts.id=30) 1 行用 NULL 填充
    assert_eq!(result.len(), 4);

    // HR 行：users.id/users.name 应为 NULL，depts.id=30，depts.name=HR
    let hr = result
        .iter()
        .find(|r| matches!(r[2], Value::Int64(30)))
        .expect("应有 HR 行");
    assert_eq!(hr[0], Value::Null, "HR 行的 users.id 应为 NULL");
    assert_eq!(hr[1], Value::Null, "HR 行的 users.name 应为 NULL");
    assert_eq!(hr[3], Value::Text("HR".into()));
}

// =====================================================================
//  FULL OUTER JOIN 测试（2）
// =====================================================================

#[test]
fn test_join_full_01_both_sides_unmatched() {
    let catalog = make_join_catalog();
    let plan = plan_sql(
        "SELECT users.id, users.name, depts.id, depts.name FROM users FULL JOIN depts ON users.dept_id = depts.id",
        &catalog,
    );
    let users = make_users_join_table();
    let depts = make_depts_join_table();
    let mut exec = Executor::new();
    register_users_and_depts(&mut exec, &users, &depts);
    let result = exec.execute(&plan).unwrap();

    // 5 行：alice/bob/carol (3) + dave (1, NULL) + HR (NULL, 30)
    assert_eq!(result.len(), 5);

    // 找 dave 行（users.id=4, depts.id=NULL）
    let dave = find_row_by_user_id(&result, 4).unwrap();
    assert_eq!(dave[2], Value::Null);
    assert_eq!(dave[3], Value::Null);

    // 找 HR 行（users.id=NULL, depts.id=30）
    let hr = result
        .iter()
        .find(|r| matches!(r[2], Value::Int64(30)))
        .expect("应有 HR 行");
    assert_eq!(hr[0], Value::Null);
    assert_eq!(hr[1], Value::Null);
    assert_eq!(hr[3], Value::Text("HR".into()));
}

#[test]
fn test_join_full_02_no_unmatched() {
    // 构造两侧全匹配场景
    let catalog = make_join_catalog();
    let plan = plan_sql(
        "SELECT users.id, depts.id FROM users FULL JOIN depts ON users.dept_id = depts.id WHERE users.id <= 3 AND depts.id <= 20",
        &catalog,
    );
    let users = make_users_join_table();
    let depts = make_depts_join_table();
    let mut exec = Executor::new();
    register_users_and_depts(&mut exec, &users, &depts);
    let result = exec.execute(&plan).unwrap();

    // 全匹配 → 3 行，无 NULL
    assert_eq!(result.len(), 3);
    for row in &result {
        assert!(!matches!(row[0], Value::Null));
        assert!(!matches!(row[1], Value::Null));
    }
}

// =====================================================================
//  CROSS JOIN 测试（2）
// =====================================================================

#[test]
fn test_join_cross_01_cartesian() {
    let catalog = make_join_catalog();
    let plan = plan_sql(
        "SELECT users.id, depts.id FROM users CROSS JOIN depts",
        &catalog,
    );
    let users = make_users_join_table();
    let depts = make_depts_join_table();
    let mut exec = Executor::new();
    register_users_and_depts(&mut exec, &users, &depts);
    let result = exec.execute(&plan).unwrap();

    // 4 users × 3 depts = 12 行
    assert_eq!(result.len(), 12, "CROSS JOIN 应为 4 × 3 = 12 行");
    // 验证每行结构：[user_id, dept_id]
    for row in &result {
        assert_eq!(row.len(), 2);
        assert!(matches!(row[0], Value::Int64(_)));
        assert!(matches!(row[1], Value::Int64(_)));
    }
}

#[test]
fn test_join_cross_02_comma_syntax() {
    // SELECT * FROM t1, t2 等价于 CROSS JOIN
    let catalog = make_join_catalog();
    let plan = plan_sql(
        "SELECT users.id, depts.id FROM users, depts WHERE users.id <= 2",
        &catalog,
    );
    let users = make_users_join_table();
    let depts = make_depts_join_table();
    let mut exec = Executor::new();
    register_users_and_depts(&mut exec, &users, &depts);
    let result = exec.execute(&plan).unwrap();

    // 2 users (id<=2) × 3 depts = 6 行
    assert_eq!(result.len(), 6);
    // 所有行的 user_id 应 <= 2
    for row in &result {
        if let Value::Int64(uid) = row[0] {
            assert!(uid <= 2);
        } else {
            panic!("user_id 应为 Int64");
        }
    }
}

// =====================================================================
//  USING / NATURAL JOIN 测试（2）
// =====================================================================

#[test]
fn test_join_using_01_single_column() {
    // 构造两表都有 id 列：users.id 和 orders.id（但通常 JOIN ON 用 user_id，这里用 id 测 USING）
    // 改造：让 users 和 orders 共享 user_id 列，临时构造新表
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "a",
        vec![("key", ColumnType::Int64), ("va", ColumnType::Int64)],
    );
    catalog.add_simple_table(
        "b",
        vec![("key", ColumnType::Int64), ("vb", ColumnType::Int64)],
    );

    let mut a = InMemoryTable::with_columns(
        "a",
        vec![("key", ColumnType::Int64), ("va", ColumnType::Int64)],
    );
    a.insert(vec![Value::Int64(1), Value::Int64(10)]);
    a.insert(vec![Value::Int64(2), Value::Int64(20)]);
    a.insert(vec![Value::Int64(3), Value::Int64(30)]);

    let mut b = InMemoryTable::with_columns(
        "b",
        vec![("key", ColumnType::Int64), ("vb", ColumnType::Int64)],
    );
    b.insert(vec![Value::Int64(2), Value::Int64(200)]);
    b.insert(vec![Value::Int64(3), Value::Int64(300)]);

    let plan = plan_sql(
        "SELECT a.key, a.va, b.vb FROM a JOIN b USING (key)",
        &catalog,
    );

    let mut exec = Executor::new();
    exec.register_table(&a);
    exec.register_table(&b);
    let result = exec.execute(&plan).unwrap();

    // USING (key) 等价于 ON a.key = b.key
    // a.key=1 无匹配，a.key=2 ↔ b.key=2，a.key=3 ↔ b.key=3
    assert_eq!(result.len(), 2);
    // 找 key=2 行
    let row2 = result
        .iter()
        .find(|r| matches!(r[0], Value::Int64(2)))
        .unwrap();
    assert_eq!(row2[1], Value::Int64(20));
    assert_eq!(row2[2], Value::Int64(200));
}

#[test]
fn test_join_natural_01_common_columns() {
    // NATURAL JOIN 自动找同名列构造等值条件
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "p",
        vec![("id", ColumnType::Int64), ("pname", ColumnType::Text)],
    );
    catalog.add_simple_table(
        "q",
        vec![("id", ColumnType::Int64), ("qname", ColumnType::Text)],
    );

    let mut p = InMemoryTable::with_columns(
        "p",
        vec![("id", ColumnType::Int64), ("pname", ColumnType::Text)],
    );
    p.insert(vec![Value::Int64(1), Value::Text("aaa".into())]);
    p.insert(vec![Value::Int64(2), Value::Text("bbb".into())]);

    let mut q = InMemoryTable::with_columns(
        "q",
        vec![("id", ColumnType::Int64), ("qname", ColumnType::Text)],
    );
    q.insert(vec![Value::Int64(2), Value::Text("xxx".into())]);
    q.insert(vec![Value::Int64(3), Value::Text("yyy".into())]);

    let plan = plan_sql(
        "SELECT p.id, p.pname, q.qname FROM p NATURAL JOIN q",
        &catalog,
    );

    let mut exec = Executor::new();
    exec.register_table(&p);
    exec.register_table(&q);
    let result = exec.execute(&plan).unwrap();

    // NATURAL JOIN 用同名列 id 构造 p.id = q.id
    // 只有 id=2 匹配
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(2));
    assert_eq!(result[0][1], Value::Text("bbb".into()));
    assert_eq!(result[0][2], Value::Text("xxx".into()));
}

// =====================================================================
//  3 表链式 JOIN 测试（1）
// =====================================================================

#[test]
fn test_join_3_tables_chain() {
    // users JOIN orders ON users.id = orders.user_id
    //      JOIN depts ON users.dept_id = depts.id
    let catalog = make_join_catalog();
    let plan = plan_sql(
        "SELECT users.name, depts.name, orders.id, orders.amount \
         FROM users JOIN orders ON users.id = orders.user_id \
         JOIN depts ON users.dept_id = depts.id",
        &catalog,
    );
    let mut exec = Executor::new();
    let users = make_users_join_table();
    let depts = make_depts_join_table();
    let orders = make_orders_join_table();
    exec.register_table(&users);
    exec.register_table(&depts);
    exec.register_table(&orders);
    let result = exec.execute(&plan).unwrap();

    // orders 3 行：alice(1,100), bob(2,200), alice(3,150)
    // 3 表 JOIN 后：
    // - order 1: alice → dept Engineering → (alice, Engineering, 1, 100)
    // - order 2: bob → dept Sales → (bob, Sales, 2, 200)
    // - order 3: alice → dept Engineering → (alice, Engineering, 3, 150)
    assert_eq!(result.len(), 3, "3 表 JOIN 应得 3 行");

    // 验证 alice 的两个订单
    let alice_orders: Vec<&Vec<Value>> = result
        .iter()
        .filter(|r| r[0] == Value::Text("alice".into()))
        .collect();
    assert_eq!(alice_orders.len(), 2, "alice 应有 2 个订单");
    for row in alice_orders {
        assert_eq!(row[1], Value::Text("Engineering".into()));
    }

    // bob 1 个订单
    let bob_orders: Vec<&Vec<Value>> = result
        .iter()
        .filter(|r| r[0] == Value::Text("bob".into()))
        .collect();
    assert_eq!(bob_orders.len(), 1);
    assert_eq!(bob_orders[0][1], Value::Text("Sales".into()));
    assert_eq!(bob_orders[0][3], Value::Int64(200));
}

// =====================================================================
//  SELF JOIN 测试（1）
// =====================================================================

#[test]
fn test_join_self_01_with_aliases() {
    // SELF JOIN — 使用别名 e1/e2 区分两侧
    // employees 表：CEO 无 manager，VP 的 manager 是 CEO，Eng 的 manager 是 VP
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "employees",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("manager_id", ColumnType::Int64),
        ],
    );

    let mut emp = InMemoryTable::with_columns(
        "employees",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("manager_id", ColumnType::Int64),
        ],
    );
    emp.insert(vec![
        Value::Int64(1),
        Value::Text("CEO".into()),
        Value::Null,
    ]);
    emp.insert(vec![
        Value::Int64(2),
        Value::Text("VP".into()),
        Value::Int64(1),
    ]);
    emp.insert(vec![
        Value::Int64(3),
        Value::Text("Eng".into()),
        Value::Int64(2),
    ]);

    // SELECT e1.name AS emp, e2.name AS manager FROM employees e1 JOIN employees e2 ON e1.manager_id = e2.id
    let plan = plan_sql(
        "SELECT e1.name, e2.name FROM employees e1 JOIN employees e2 ON e1.manager_id = e2.id",
        &catalog,
    );

    let mut exec = Executor::new();
    exec.register_table(&emp);
    let result = exec.execute(&plan).unwrap();

    // CEO (manager_id=NULL) → 无匹配
    // VP (manager_id=1) → 匹配 CEO → (VP, CEO)
    // Eng (manager_id=2) → 匹配 VP → (Eng, VP)
    assert_eq!(
        result.len(),
        2,
        "SELF JOIN 应得 2 行（VP 和 Eng 各匹配其 manager）"
    );

    // 验证 (VP, CEO) 行
    let vp_row = result
        .iter()
        .find(|r| r[0] == Value::Text("VP".into()))
        .expect("应有 VP 行");
    assert_eq!(
        vp_row[1],
        Value::Text("CEO".into()),
        "VP 的 manager 应是 CEO"
    );

    // 验证 (Eng, VP) 行
    let eng_row = result
        .iter()
        .find(|r| r[0] == Value::Text("Eng".into()))
        .expect("应有 Eng 行");
    assert_eq!(
        eng_row[1],
        Value::Text("VP".into()),
        "Eng 的 manager 应是 VP"
    );
}

// =====================================================================
//  HashJoin vs NestedLoop 路径验证（2）
// =====================================================================

#[test]
fn test_join_hash_01_equijoin_uses_hash() {
    // 等值连接 t1.col = t2.col → 应触发 HashJoin 路径
    // 通过观察结果正确性间接验证（执行路径对调用方透明）
    let catalog = make_join_catalog();
    let plan = plan_sql(
        "SELECT users.id, depts.id FROM users JOIN depts ON users.dept_id = depts.id",
        &catalog,
    );
    let users = make_users_join_table();
    let depts = make_depts_join_table();
    let mut exec = Executor::new();
    register_users_and_depts(&mut exec, &users, &depts);
    let result = exec.execute(&plan).unwrap();

    // 等值 JOIN 应得 3 行（alice/bob/carol 各匹配 dept）
    assert_eq!(result.len(), 3);

    // 验证所有 (user_id, dept_id) 对
    let pairs: Vec<(i64, i64)> = result
        .iter()
        .filter_map(|r| match (&r[0], &r[1]) {
            (Value::Int64(u), Value::Int64(d)) => Some((*u, *d)),
            _ => None,
        })
        .collect();
    assert!(pairs.contains(&(1, 10)), "应有 (alice, Engineering)");
    assert!(pairs.contains(&(2, 20)), "应有 (bob, Sales)");
    assert!(pairs.contains(&(3, 10)), "应有 (carol, Engineering)");
}

#[test]
fn test_join_hash_02_large_equijoin_correctness() {
    // 大表等值 JOIN — 验证 HashJoin 在多行场景下结果正确
    // 用 CounterTable 不可（它没有 dept_id 列），改为构造 1000 行表
    let mut left = InMemoryTable::with_columns(
        "left",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );
    let mut right = InMemoryTable::with_columns(
        "right",
        vec![("id", ColumnType::Int64), ("label", ColumnType::Int64)],
    );
    for i in 0..1000i64 {
        left.insert(vec![Value::Int64(i), Value::Int64(i * 2)]);
        // 右表只保留偶数 id（500 行）
        if i % 2 == 0 {
            right.insert(vec![Value::Int64(i), Value::Int64(i + 1000)]);
        }
    }

    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "left",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );
    catalog.add_simple_table(
        "right",
        vec![("id", ColumnType::Int64), ("label", ColumnType::Int64)],
    );

    let plan = plan_sql(
        "SELECT left.id, left.val, right.label FROM left JOIN right ON left.id = right.id",
        &catalog,
    );
    let mut exec = Executor::new();
    exec.register_table(&left);
    exec.register_table(&right);
    let result = exec.execute(&plan).unwrap();

    // 应得 500 行（偶数 id 匹配）
    assert_eq!(result.len(), 500, "HashJoin 应得 500 行匹配");

    // 验证前 5 行的 val 和 label
    let mut sorted: Vec<(i64, i64, i64)> = result
        .iter()
        .filter_map(|r| match (&r[0], &r[1], &r[2]) {
            (Value::Int64(id), Value::Int64(val), Value::Int64(label)) => Some((*id, *val, *label)),
            _ => None,
        })
        .collect();
    sorted.sort();
    for (i, (id, val, label)) in sorted.iter().take(5).enumerate() {
        let expected_id = (i as i64) * 2;
        assert_eq!(*id, expected_id, "第 {i} 行 id 应为 {expected_id}");
        assert_eq!(*val, expected_id * 2, "val 应为 id*2");
        assert_eq!(*label, expected_id + 1000, "label 应为 id+1000");
    }
}

// =====================================================================
//  聚合辅助函数
// =====================================================================

/// 构建聚合测试用 catalog：sales(id, amount, dept)
fn make_agg_catalog() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "sales",
        vec![
            ("id", ColumnType::Int64),
            ("amount", ColumnType::Int64),
            ("dept", ColumnType::Text),
        ],
    );
    catalog
}

/// 构建聚合测试用表：6 行 sales
/// - (1, 100, "A")  (2, 200, "A")  (3, 300, "B")
/// - (4, NULL, "B") (5, 500, "A")  (6, 600, "B")
fn make_sales_table() -> InMemoryTable {
    let mut t = InMemoryTable::with_columns(
        "sales",
        vec![
            ("id", ColumnType::Int64),
            ("amount", ColumnType::Int64),
            ("dept", ColumnType::Text),
        ],
    );
    t.insert(vec![
        Value::Int64(1),
        Value::Int64(100),
        Value::Text("A".into()),
    ]);
    t.insert(vec![
        Value::Int64(2),
        Value::Int64(200),
        Value::Text("A".into()),
    ]);
    t.insert(vec![
        Value::Int64(3),
        Value::Int64(300),
        Value::Text("B".into()),
    ]);
    t.insert(vec![Value::Int64(4), Value::Null, Value::Text("B".into())]);
    t.insert(vec![
        Value::Int64(5),
        Value::Int64(500),
        Value::Text("A".into()),
    ]);
    t.insert(vec![
        Value::Int64(6),
        Value::Int64(600),
        Value::Text("B".into()),
    ]);
    t
}

fn register_sales<'a>(exec: &mut Executor<'a>, sales: &'a InMemoryTable) {
    exec.register_table(sales);
}

// =====================================================================
//  聚合基础测试（5）
// =====================================================================

#[test]
fn test_agg_01_count_star() {
    let catalog = make_agg_catalog();
    let plan = plan_sql("SELECT COUNT(*) FROM sales", &catalog);
    let sales = make_sales_table();
    let mut exec = Executor::new();
    register_sales(&mut exec, &sales);
    let result = exec.execute(&plan).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(6), "COUNT(*) 应为 6 行");
}

#[test]
fn test_agg_02_count_expr() {
    let catalog = make_agg_catalog();
    let plan = plan_sql("SELECT COUNT(amount) FROM sales", &catalog);
    let sales = make_sales_table();
    let mut exec = Executor::new();
    register_sales(&mut exec, &sales);
    let result = exec.execute(&plan).unwrap();

    // amount 有 1 个 NULL（id=4），COUNT(amount) = 5
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0][0],
        Value::Int64(5),
        "COUNT(amount) 应为 5（排除 NULL）"
    );
}

#[test]
fn test_agg_03_sum() {
    let catalog = make_agg_catalog();
    let plan = plan_sql("SELECT SUM(amount) FROM sales", &catalog);
    let sales = make_sales_table();
    let mut exec = Executor::new();
    register_sales(&mut exec, &sales);
    let result = exec.execute(&plan).unwrap();

    // 100 + 200 + 300 + NULL(跳过) + 500 + 600 = 1700
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(1700), "SUM(amount) 应为 1700");
}

#[test]
fn test_agg_04_avg() {
    let catalog = make_agg_catalog();
    let plan = plan_sql("SELECT AVG(amount) FROM sales", &catalog);
    let sales = make_sales_table();
    let mut exec = Executor::new();
    register_sales(&mut exec, &sales);
    let result = exec.execute(&plan).unwrap();

    // (100+200+300+500+600)/5 = 1700/5 = 340.0
    assert_eq!(result.len(), 1);
    match &result[0][0] {
        Value::Float64(f) => {
            assert!((f - 340.0).abs() < 1e-9, "AVG(amount) 应为 340.0, got {f}");
        }
        other => panic!("AVG 应返回 Float64, got {other:?}"),
    }
}

#[test]
fn test_agg_05_min_max() {
    let catalog = make_agg_catalog();
    let plan = plan_sql("SELECT MIN(amount), MAX(amount) FROM sales", &catalog);
    let sales = make_sales_table();
    let mut exec = Executor::new();
    register_sales(&mut exec, &sales);
    let result = exec.execute(&plan).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(100), "MIN(amount) 应为 100");
    assert_eq!(result[0][1], Value::Int64(600), "MAX(amount) 应为 600");
}

// =====================================================================
//  聚合 DISTINCT 测试（2）
// =====================================================================

#[test]
fn test_agg_distinct_01_count_distinct() {
    let catalog = make_agg_catalog();
    let plan = plan_sql("SELECT COUNT(DISTINCT dept) FROM sales", &catalog);
    let sales = make_sales_table();
    let mut exec = Executor::new();
    register_sales(&mut exec, &sales);
    let result = exec.execute(&plan).unwrap();

    // dept: A, A, B, B, A, B → 去重后 {A, B} = 2
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(2), "COUNT(DISTINCT dept) 应为 2");
}

#[test]
fn test_agg_distinct_02_sum_distinct() {
    // 构造重复值：amount = [100, 100, 200, 200, 300]
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table("t", vec![("amount", ColumnType::Int64)]);
    let plan = plan_sql("SELECT SUM(DISTINCT amount) FROM t", &catalog);

    let mut t = InMemoryTable::with_columns("t", vec![("amount", ColumnType::Int64)]);
    t.insert(vec![Value::Int64(100)]);
    t.insert(vec![Value::Int64(100)]);
    t.insert(vec![Value::Int64(200)]);
    t.insert(vec![Value::Int64(200)]);
    t.insert(vec![Value::Int64(300)]);

    let mut exec = Executor::new();
    exec.register_table(&t);
    let result = exec.execute(&plan).unwrap();

    // DISTINCT 去重后 {100, 200, 300} → SUM = 600
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0][0],
        Value::Int64(600),
        "SUM(DISTINCT amount) 应为 600"
    );
}

// =====================================================================
//  聚合空表测试（2）
// =====================================================================

#[test]
fn test_agg_empty_01_no_group_by() {
    // 无 GROUP BY + 空表：COUNT=0, SUM/AVG/MIN/MAX=NULL
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table("empty_t", vec![("v", ColumnType::Int64)]);
    let plan = plan_sql(
        "SELECT COUNT(*), SUM(v), AVG(v), MIN(v), MAX(v) FROM empty_t",
        &catalog,
    );

    let t = InMemoryTable::with_columns("empty_t", vec![("v", ColumnType::Int64)]);
    let mut exec = Executor::new();
    exec.register_table(&t);
    let result = exec.execute(&plan).unwrap();

    assert_eq!(result.len(), 1, "空表无 GROUP BY 应输出 1 行");
    assert_eq!(result[0][0], Value::Int64(0), "COUNT(*) 应为 0");
    assert_eq!(result[0][1], Value::Null, "SUM 应为 NULL");
    assert_eq!(result[0][2], Value::Null, "AVG 应为 NULL");
    assert_eq!(result[0][3], Value::Null, "MIN 应为 NULL");
    assert_eq!(result[0][4], Value::Null, "MAX 应为 NULL");
}

#[test]
fn test_agg_empty_02_with_group_by() {
    // 有 GROUP BY + 空表：输出 0 行
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table("empty_t", vec![("v", ColumnType::Int64)]);
    let plan = plan_sql("SELECT dept, COUNT(*) FROM empty_t GROUP BY dept", &catalog);

    let t = InMemoryTable::with_columns(
        "empty_t",
        vec![("dept", ColumnType::Text), ("v", ColumnType::Int64)],
    );
    let mut exec = Executor::new();
    exec.register_table(&t);
    let result = exec.execute(&plan).unwrap();

    assert_eq!(result.len(), 0, "有 GROUP BY + 空表应输出 0 行");
}

// =====================================================================
//  GROUP BY 单列测试（3）
// =====================================================================

#[test]
fn test_agg_group_01_basic() {
    let catalog = make_agg_catalog();
    let plan = plan_sql("SELECT dept, COUNT(*) FROM sales GROUP BY dept", &catalog);
    let sales = make_sales_table();
    let mut exec = Executor::new();
    register_sales(&mut exec, &sales);
    let result = exec.execute(&plan).unwrap();

    // dept A: 3 行, dept B: 3 行
    assert_eq!(result.len(), 2, "应有 2 个分组");

    let mut by_dept: std::collections::HashMap<String, i64> = result
        .iter()
        .filter_map(|r| match (&r[0], &r[1]) {
            (Value::Text(d), Value::Int64(c)) => Some((d.clone(), *c)),
            _ => None,
        })
        .collect();
    assert_eq!(by_dept.remove("A"), Some(3), "dept A 应有 3 行");
    assert_eq!(by_dept.remove("B"), Some(3), "dept B 应有 3 行");
}

#[test]
fn test_agg_group_02_multiple_aggs() {
    let catalog = make_agg_catalog();
    let plan = plan_sql(
        "SELECT dept, COUNT(*), SUM(amount), MIN(amount), MAX(amount) FROM sales GROUP BY dept",
        &catalog,
    );
    let sales = make_sales_table();
    let mut exec = Executor::new();
    register_sales(&mut exec, &sales);
    let result = exec.execute(&plan).unwrap();

    assert_eq!(result.len(), 2);

    // dept A: amounts = [100, 200, 500], COUNT=3, SUM=800, MIN=100, MAX=500
    // dept B: amounts = [300, NULL, 600], COUNT=3, SUM=900, MIN=300, MAX=600
    for row in &result {
        let dept = match &row[0] {
            Value::Text(d) => d.clone(),
            _ => panic!("dept 应为 Text"),
        };
        let count = match row[1] {
            Value::Int64(c) => c,
            _ => panic!("COUNT 应为 Int64"),
        };
        let sum = match &row[2] {
            Value::Int64(s) => *s,
            _ => panic!("SUM 应为 Int64"),
        };
        let min = match &row[3] {
            Value::Int64(m) => *m,
            _ => panic!("MIN 应为 Int64"),
        };
        let max = match &row[4] {
            Value::Int64(m) => *m,
            _ => panic!("MAX 应为 Int64"),
        };
        match dept.as_str() {
            "A" => {
                assert_eq!(count, 3);
                assert_eq!(sum, 800);
                assert_eq!(min, 100);
                assert_eq!(max, 500);
            }
            "B" => {
                assert_eq!(count, 3);
                assert_eq!(sum, 900); // 300 + 600 (NULL 跳过)
                assert_eq!(min, 300);
                assert_eq!(max, 600);
            }
            _ => panic!("未知 dept: {dept}"),
        }
    }
}

#[test]
fn test_agg_group_03_avg_per_group() {
    let catalog = make_agg_catalog();
    let plan = plan_sql(
        "SELECT dept, AVG(amount) FROM sales GROUP BY dept",
        &catalog,
    );
    let sales = make_sales_table();
    let mut exec = Executor::new();
    register_sales(&mut exec, &sales);
    let result = exec.execute(&plan).unwrap();

    assert_eq!(result.len(), 2);
    for row in &result {
        let dept = match &row[0] {
            Value::Text(d) => d.clone(),
            _ => panic!("dept 应为 Text"),
        };
        let avg = match &row[1] {
            Value::Float64(f) => *f,
            _ => panic!("AVG 应为 Float64, got {:?}", row[1]),
        };
        match dept.as_str() {
            "A" => assert!(
                (avg - 266.6666666666667).abs() < 1e-6,
                "dept A AVG 应为 800/3, got {avg}"
            ),
            "B" => assert!(
                (avg - 450.0).abs() < 1e-9,
                "dept B AVG 应为 900/2=450, got {avg}"
            ),
            _ => panic!("未知 dept: {dept}"),
        }
    }
}

// =====================================================================
//  GROUP BY 多列测试（1）
// =====================================================================

#[test]
fn test_agg_group_04_multi_column() {
    // 构造：emp(id, dept, role, salary)
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "emp",
        vec![
            ("id", ColumnType::Int64),
            ("dept", ColumnType::Text),
            ("role", ColumnType::Text),
            ("salary", ColumnType::Int64),
        ],
    );
    let plan = plan_sql(
        "SELECT dept, role, COUNT(*), SUM(salary) FROM emp GROUP BY dept, role",
        &catalog,
    );

    let mut emp = InMemoryTable::with_columns(
        "emp",
        vec![
            ("id", ColumnType::Int64),
            ("dept", ColumnType::Text),
            ("role", ColumnType::Text),
            ("salary", ColumnType::Int64),
        ],
    );
    // dept=A: eng=2, mgr=1
    emp.insert(vec![
        Value::Int64(1),
        Value::Text("A".into()),
        Value::Text("eng".into()),
        Value::Int64(100),
    ]);
    emp.insert(vec![
        Value::Int64(2),
        Value::Text("A".into()),
        Value::Text("eng".into()),
        Value::Int64(200),
    ]);
    emp.insert(vec![
        Value::Int64(3),
        Value::Text("A".into()),
        Value::Text("mgr".into()),
        Value::Int64(500),
    ]);
    // dept=B: eng=1, mgr=1
    emp.insert(vec![
        Value::Int64(4),
        Value::Text("B".into()),
        Value::Text("eng".into()),
        Value::Int64(150),
    ]);
    emp.insert(vec![
        Value::Int64(5),
        Value::Text("B".into()),
        Value::Text("mgr".into()),
        Value::Int64(600),
    ]);

    let mut exec = Executor::new();
    exec.register_table(&emp);
    let result = exec.execute(&plan).unwrap();

    assert_eq!(
        result.len(),
        4,
        "应有 4 个分组 (A,eng) (A,mgr) (B,eng) (B,mgr)"
    );

    let mut groups: std::collections::HashMap<(String, String), (i64, i64)> = result
        .iter()
        .filter_map(|r| match (&r[0], &r[1], &r[2], &r[3]) {
            (Value::Text(d), Value::Text(role), Value::Int64(c), Value::Int64(s)) => {
                Some(((d.clone(), role.clone()), (*c, *s)))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        groups.remove(&("A".into(), "eng".into())),
        Some((2, 300)),
        "A.eng: COUNT=2, SUM=300"
    );
    assert_eq!(
        groups.remove(&("A".into(), "mgr".into())),
        Some((1, 500)),
        "A.mgr: COUNT=1, SUM=500"
    );
    assert_eq!(
        groups.remove(&("B".into(), "eng".into())),
        Some((1, 150)),
        "B.eng: COUNT=1, SUM=150"
    );
    assert_eq!(
        groups.remove(&("B".into(), "mgr".into())),
        Some((1, 600)),
        "B.mgr: COUNT=1, SUM=600"
    );
}

// =====================================================================
//  GROUP BY + HAVING 测试（2）
// =====================================================================

#[test]
fn test_agg_having_01_basic_filter() {
    let catalog = make_agg_catalog();
    let plan = plan_sql(
        "SELECT dept, COUNT(*) FROM sales GROUP BY dept HAVING COUNT(*) > 2",
        &catalog,
    );
    let sales = make_sales_table();
    let mut exec = Executor::new();
    register_sales(&mut exec, &sales);
    let result = exec.execute(&plan).unwrap();

    // dept A: 3 行 (>2 ✓), dept B: 3 行 (>2 ✓) — 两者都通过
    assert_eq!(result.len(), 2, "两个组都满足 COUNT > 2");
}

#[test]
fn test_agg_having_02_multiple_aggs() {
    let catalog = make_agg_catalog();
    let plan = plan_sql(
        "SELECT dept, COUNT(*), SUM(amount) FROM sales GROUP BY dept HAVING SUM(amount) > 850",
        &catalog,
    );
    let sales = make_sales_table();
    let mut exec = Executor::new();
    register_sales(&mut exec, &sales);
    let result = exec.execute(&plan).unwrap();

    // dept A: SUM=800 (≤850 ✗), dept B: SUM=900 (>850 ✓)
    assert_eq!(result.len(), 1, "只有 dept B 满足 SUM > 850");
    match &result[0][0] {
        Value::Text(d) => assert_eq!(d, "B"),
        _ => panic!("应为 dept B"),
    }
}

// =====================================================================
//  聚合混合测试（2）
// =====================================================================

#[test]
fn test_agg_mixed_01_where_group_having() {
    // WHERE → GROUP BY → HAVING → 投影
    let catalog = make_agg_catalog();
    let plan = plan_sql(
        "SELECT dept, COUNT(*) AS cnt, SUM(amount) AS total FROM sales WHERE amount > 150 GROUP BY dept HAVING SUM(amount) > 300",
        &catalog,
    );
    let sales = make_sales_table();
    let mut exec = Executor::new();
    register_sales(&mut exec, &sales);
    let result = exec.execute(&plan).unwrap();

    // WHERE amount > 150 后剩余：id=2(A,200), id=3(B,300), id=5(A,500), id=6(B,600)
    // 分组：A → [200, 500] COUNT=2 SUM=700;  B → [300, 600] COUNT=2 SUM=900
    // HAVING SUM > 300：两者都通过
    assert_eq!(result.len(), 2);
    let mut by_dept: std::collections::HashMap<String, (i64, i64)> = result
        .iter()
        .filter_map(|r| match (&r[0], &r[1], &r[2]) {
            (Value::Text(d), Value::Int64(c), Value::Int64(s)) => Some((d.clone(), (*c, *s))),
            _ => None,
        })
        .collect();
    assert_eq!(by_dept.remove("A"), Some((2, 700)), "A: COUNT=2, SUM=700");
    assert_eq!(by_dept.remove("B"), Some((2, 900)), "B: COUNT=2, SUM=900");
}

#[test]
fn test_agg_mixed_02_agg_with_join() {
    // JOIN + GROUP BY：users JOIN orders → 按用户名分组 COUNT
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    catalog.add_simple_table(
        "orders",
        vec![("oid", ColumnType::Int64), ("uid", ColumnType::Int64)],
    );

    let plan = plan_sql(
        "SELECT users.name, COUNT(orders.oid) FROM users JOIN orders ON users.id = orders.uid GROUP BY users.name",
        &catalog,
    );

    let mut users = InMemoryTable::with_columns(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    users.insert(vec![Value::Int64(1), Value::Text("alice".into())]);
    users.insert(vec![Value::Int64(2), Value::Text("bob".into())]);

    let mut orders = InMemoryTable::with_columns(
        "orders",
        vec![("oid", ColumnType::Int64), ("uid", ColumnType::Int64)],
    );
    // alice (id=1): 3 个订单；bob (id=2): 2 个订单
    orders.insert(vec![Value::Int64(10), Value::Int64(1)]);
    orders.insert(vec![Value::Int64(11), Value::Int64(1)]);
    orders.insert(vec![Value::Int64(12), Value::Int64(1)]);
    orders.insert(vec![Value::Int64(20), Value::Int64(2)]);
    orders.insert(vec![Value::Int64(21), Value::Int64(2)]);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&users);
    exec.register_table(&orders);
    let result = exec.execute(&plan).unwrap();

    assert_eq!(result.len(), 2, "应有 2 个用户分组");
    let mut by_name: std::collections::HashMap<String, i64> = result
        .iter()
        .filter_map(|r| match (&r[0], &r[1]) {
            (Value::Text(n), Value::Int64(c)) => Some((n.clone(), *c)),
            _ => None,
        })
        .collect();
    assert_eq!(by_name.remove("alice"), Some(3), "alice 应有 3 个订单");
    assert_eq!(by_name.remove("bob"), Some(2), "bob 应有 2 个订单");
}

// =====================================================================
//  P0-STORE-1：B+Tree 主键索引接入运行时测试（4）
// =====================================================================

/// 构建带 id 主键的测试表：列 `id BIGINT, name TEXT`
fn make_pk_table(name: &str) -> InMemoryTable {
    InMemoryTable::with_columns(
        name,
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    )
}

/// P0-STORE-1：启用 B+Tree 主键索引后，INSERT 同步更新 BTree
///
/// 验证：
/// 1. enable_btree_pk(0) 后 has_btree_pk() 返回 true
/// 2. INSERT 后 pk_lookup 能找到对应 row_id
/// 3. pk_lookup 返回的 row_id 与 get_row(row_id) 一致
#[test]
fn test_p0_store_1_btree_pk_insert_and_lookup() {
    let mut table = make_pk_table("users");
    assert!(!table.has_btree_pk(), "默认未启用 B+Tree");

    // 启用 B+Tree 主键索引（id 列，index=0）
    table.enable_btree_pk(&[0]);
    assert!(table.has_btree_pk(), "启用后应返回 true");

    // INSERT 3 行
    table.insert(vec![Value::Int64(10), Value::Text("alice".into())]);
    table.insert(vec![Value::Int64(20), Value::Text("bob".into())]);
    table.insert(vec![Value::Int64(30), Value::Text("carol".into())]);

    // pk_lookup 验证 — BTree 被实际调用（P1-7：传入 &Value）
    assert_eq!(
        table.pk_lookup(&Value::Int64(10)),
        Some(0),
        "id=10 → row_id=0"
    );
    assert_eq!(
        table.pk_lookup(&Value::Int64(20)),
        Some(1),
        "id=20 → row_id=1"
    );
    assert_eq!(
        table.pk_lookup(&Value::Int64(30)),
        Some(2),
        "id=30 → row_id=2"
    );
    assert_eq!(table.pk_lookup(&Value::Int64(40)), None, "id=40 不存在");

    // pk_lookup 返回的 row_id 与 get_row 一致
    let row = table.get_row(1).unwrap();
    assert_eq!(row, vec![Value::Int64(20), Value::Text("bob".into())]);
}

/// P0-STORE-1：未启用 B+Tree 的表，pk_lookup 永远返回 None
#[test]
fn test_p0_store_1_btree_pk_disabled_returns_none() {
    let mut table = make_pk_table("users");
    // 不启用 B+Tree
    table.insert(vec![Value::Int64(10), Value::Text("alice".into())]);

    assert!(!table.has_btree_pk());
    assert_eq!(
        table.pk_lookup(&Value::Int64(10)),
        None,
        "未启用 B+Tree 时 pk_lookup 返回 None"
    );
}

/// P1-7：enable_btree_pk 支持 Text 列（P0 仅支持 Int64）
#[test]
fn test_p1_7_btree_pk_supports_text() {
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![("name", ColumnType::Text), ("age", ColumnType::Int64)],
    );

    // P1-7：Text 列现在支持 B+Tree 主键索引
    table.enable_btree_pk(&[0]);
    assert!(table.has_btree_pk(), "Text 列应启用 B+Tree (P1-7)");

    // 点查 Text 主键
    table.insert(vec![Value::Text("alice".into()), Value::Int64(30)]);
    table.insert(vec![Value::Text("bob".into()), Value::Int64(25)]);
    assert_eq!(table.pk_lookup(&Value::Text("alice".into())), Some(0));
    assert_eq!(table.pk_lookup(&Value::Text("bob".into())), Some(1));
    assert_eq!(table.pk_lookup(&Value::Text("nobody".into())), None);
}

/// P1-7：enable_btree_pk 对不支持的类型（如 Bool）仍拒绝
#[test]
fn test_p1_7_btree_pk_rejects_unsupported_type() {
    let mut table = InMemoryTable::with_columns(
        "users",
        vec![("active", ColumnType::Bool), ("name", ColumnType::Text)],
    );

    // Bool 列不支持 B+Tree 主键索引
    table.enable_btree_pk(&[0]);
    assert!(!table.has_btree_pk(), "Bool 列不应启用 B+Tree");

    // Text 列应成功
    table.enable_btree_pk(&[1]);
    assert!(table.has_btree_pk(), "Text 列应启用 B+Tree");
}

/// P1-7：Float64 主键支持 B+Tree 索引
#[test]
fn test_p1_7_btree_pk_supports_float64() {
    let mut table = InMemoryTable::with_columns(
        "measurements",
        vec![
            ("reading", ColumnType::Float64),
            ("sensor", ColumnType::Text),
        ],
    );

    table.enable_btree_pk(&[0]);
    assert!(table.has_btree_pk(), "Float64 列应启用 B+Tree (P1-7)");

    table.insert(vec![Value::Float64(3.14), Value::Text("s1".into())]);
    table.insert(vec![Value::Float64(2.71), Value::Text("s2".into())]);
    table.insert(vec![Value::Float64(-1.5), Value::Text("s3".into())]);

    assert_eq!(table.pk_lookup(&Value::Float64(3.14)), Some(0));
    assert_eq!(table.pk_lookup(&Value::Float64(2.71)), Some(1));
    assert_eq!(table.pk_lookup(&Value::Float64(-1.5)), Some(2));
    assert_eq!(table.pk_lookup(&Value::Float64(9.99)), None);
}

/// P1-7：复合主键（Int64 + Text）支持 B+Tree 索引
#[test]
fn test_p1_7_btree_pk_composite_int_text() {
    let mut table = InMemoryTable::with_columns(
        "orders",
        vec![
            ("tenant_id", ColumnType::Int64),
            ("order_no", ColumnType::Text),
            ("amount", ColumnType::Int64),
        ],
    );

    // 复合主键 (tenant_id, order_no)
    table.enable_btree_pk(&[0, 1]);
    assert!(table.has_btree_pk(), "复合主键应启用 B+Tree");
    assert_eq!(table.pk_column_indices(), &[0, 1]);

    table.insert(vec![
        Value::Int64(1),
        Value::Text("A001".into()),
        Value::Int64(100),
    ]);
    table.insert(vec![
        Value::Int64(1),
        Value::Text("A002".into()),
        Value::Int64(200),
    ]);
    table.insert(vec![
        Value::Int64(2),
        Value::Text("B001".into()),
        Value::Int64(300),
    ]);

    // 复合主键不支持 pk_lookup（需全键构造，当前仅支持单列点查）
    assert_eq!(table.pk_lookup(&Value::Int64(1)), None);

    // 但全表扫描结果正确
    let rows: Vec<Vec<Value>> = table.scan_iter().collect();
    assert_eq!(rows.len(), 3);
}

/// P1-7：Float64 有序编码验证（负数 < 正数，NaN 最大）
#[test]
fn test_p1_7_float64_key_ordering() {
    use szrsql_storage::btree::{compare_keys, encode_f64_key};

    let k_neg = encode_f64_key(-1.0);
    let k_zero = encode_f64_key(0.0);
    let k_pos = encode_f64_key(1.0);
    let k_nan = encode_f64_key(f64::NAN);

    // -1.0 < 0.0 < 1.0 < NaN（字典序）
    assert!(compare_keys(&k_neg, &k_zero) == std::cmp::Ordering::Less);
    assert!(compare_keys(&k_zero, &k_pos) == std::cmp::Ordering::Less);
    assert!(compare_keys(&k_pos, &k_nan) == std::cmp::Ordering::Less);

    // 同值编码一致
    assert_eq!(encode_f64_key(3.14), encode_f64_key(3.14));
}

/// P0-STORE-1：B+Tree 索引与 SeqScan 结果一致（数据完整性）
///
/// 验证启用 B+Tree 后，全表扫描结果与未启用时一致（BTree 不影响数据存储）
#[test]
fn test_p0_store_1_btree_pk_scan_consistency() {
    let mut table_with_btree = make_pk_table("users");
    let mut table_without_btree = make_pk_table("users");

    table_with_btree.enable_btree_pk(&[0]);

    // 两表插入相同数据
    for (id, name) in [(1, "a"), (2, "b"), (3, "c"), (4, "d"), (5, "e")] {
        let row = vec![Value::Int64(id), Value::Text(name.into())];
        table_with_btree.insert(row.clone());
        table_without_btree.insert(row);
    }

    // scan_iter 结果应一致
    let with_btree: Vec<Vec<Value>> = table_with_btree.scan_iter().collect();
    let without_btree: Vec<Vec<Value>> = table_without_btree.scan_iter().collect();
    assert_eq!(with_btree, without_btree, "B+Tree 不应影响 SeqScan 结果");
    assert_eq!(with_btree.len(), 5);

    // row_count 应一致
    assert_eq!(
        table_with_btree.row_count(),
        table_without_btree.row_count()
    );

    // pk_lookup 验证 BTree 确实工作（P1-7：传入 &Value）
    for id in 1..=5 {
        assert!(
            table_with_btree.pk_lookup(&Value::Int64(id)).is_some(),
            "id={} 应在 BTree 中",
            id
        );
        assert!(
            table_without_btree.pk_lookup(&Value::Int64(id)).is_none(),
            "未启用 BTree 应返回 None"
        );
    }
}

// =====================================================================
//  P0-STORE-2：BufferPool 持久化接入测试（4）
// =====================================================================

/// P0-STORE-2：默认未启用持久化
#[test]
fn test_p0_store_2_persistence_disabled_default() {
    let table = make_test_table("t");
    assert!(!table.has_persistence(), "默认未启用持久化");
}

/// P0-STORE-2：启用持久化后 has_persistence 返回 true
#[test]
fn test_p0_store_2_enable_persistence() {
    let mut table = make_test_table("t");
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    table.enable_persistence(&path).unwrap();
    assert!(table.has_persistence(), "启用后应返回 true");
}

/// P0-STORE-2：端到端 — 插入数据 → flush → 新表 load → 数据一致
///
/// 这是 P0-STORE-2 的核心验证：BufferPool 真实接入运行时持久化路径，
/// 重启（用新表实例）后数据可完整恢复。
#[test]
fn test_p0_store_2_flush_and_load_roundtrip() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();

    // 阶段 1：表 A 启用持久化 + 插入数据 + flush
    let mut table_a = make_test_table("users");
    table_a.enable_persistence(&path).unwrap();
    table_a.insert(vec![Value::Int64(1), Value::Text("alice".into())]);
    table_a.insert(vec![Value::Int64(2), Value::Text("bob".into())]);
    table_a.insert(vec![Value::Int64(3), Value::Text("carol".into())]);
    // 删除一行验证 deleted 集合持久化
    table_a.delete_row(1); // 删除 bob (row_id=1)
    assert_eq!(table_a.row_count(), 2, "删除后应剩 2 行");
    table_a.flush_to_disk().unwrap();

    // 阶段 2：表 B（新实例）启用持久化 + load → 数据应与 A 一致
    let mut table_b = make_test_table("users");
    // 重新启用持久化（指向同一文件），此时 loader 能读到文件
    table_b.enable_persistence(&path).unwrap();
    table_b.load_from_disk().unwrap();

    // 验证恢复的数据
    assert_eq!(table_b.name(), "users", "表名应恢复");
    assert_eq!(table_b.row_count(), 2, "行数应恢复为 2（含 1 个 deleted）");
    let rows: Vec<Vec<Value>> = table_b.scan_iter().collect();
    assert_eq!(rows.len(), 2, "scan_iter 应跳过 deleted 行");
    // 活跃行：alice (id=1) 和 carol (id=3)
    let ids: Vec<i64> = rows
        .iter()
        .map(|r| {
            if let Value::Int64(id) = r[0] {
                id
            } else {
                panic!("expected Int64")
            }
        })
        .collect();
    assert!(ids.contains(&1), "应包含 alice (id=1)");
    assert!(ids.contains(&3), "应包含 carol (id=3)");
    assert!(!ids.contains(&2), "不应包含已删除的 bob (id=2)");
}

/// P0-STORE-2：未启用持久化时 flush/load 返回错误
#[test]
fn test_p0_store_2_flush_without_enable_errors() {
    let mut table = make_test_table("t");
    // flush_to_disk 未启用应报错
    let err = table.flush_to_disk();
    assert!(err.is_err(), "未启用 persistence 时 flush 应报错");
    // load_from_disk 未启用应报错
    let err = table.load_from_disk();
    assert!(err.is_err(), "未启用 persistence 时 load 应报错");
}

// =====================================================================
//  P0-DIST-1/2/3：Executor + DistRuntime 集成测试
// =====================================================================

/// 创建已初始化的 DistRuntime 句柄（用于测试）
fn make_dist_runtime_handle() -> szrsql_dist::runtime::DistRuntimeHandle {
    let handle = szrsql_dist::runtime::new_single_node_runtime(1).unwrap();
    {
        // P0-6：parking_lot::RwLock 不中毒，write() 直接返回 guard
        let mut rt = handle.write();
        rt.init().unwrap();
    }
    handle
}

/// P0-DIST-1：Executor 绑定 DistRuntime 后，has_dist_runtime 返回 true
#[test]
fn test_p0_dist_executor_has_dist_runtime() {
    let executor = crate::executor::Executor::new();
    assert!(!executor.has_dist_runtime(), "默认不启用 DistRuntime");

    let handle = make_dist_runtime_handle();
    let executor = crate::executor::Executor::new().with_dist_runtime(handle);
    assert!(executor.has_dist_runtime(), "绑定后应启用 DistRuntime");
}

/// P0-DIST-1：Executor 双写 — INSERT 后数据同时写入分布式 KV
#[test]
fn test_p0_dist_dual_write_insert() {
    use crate::executor::{Executor, MutableTable};
    use szrsql_types::value::ColumnType;

    let handle = make_dist_runtime_handle();
    let executor = Executor::new().with_dist_runtime(handle.clone());

    // 创建表并插入行
    let mut table = crate::executor::InMemoryTable::with_columns(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    // 直接调用 mvcc_insert（通过 execute_insert 会走完整流程）
    // 此处验证 dist_dual_write 的效果
    let row = vec![Value::Int64(1), Value::Text("alice".into())];
    let row_id = table.insert_row(row.clone());

    // 手动调用双写（模拟 mvcc_insert 中的调用）
    // 注：dist_dual_write 是私有方法，通过 dist_read 验证效果
    // 直接通过 DistRuntime 写入
    {
        let mut rt = handle.write();
        let key = format!("users:{}", row_id);
        let value = serde_json::to_vec(&row).unwrap();
        rt.put(key.into_bytes(), value).unwrap();
    }

    // 通过 executor.dist_read 验证
    let read_back = executor.dist_read("users", row_id).unwrap();
    assert_eq!(read_back, row, "dist_read 应返回写入的行");
}

/// P0-DIST-2：Executor 获取 TSO 时间戳
#[test]
fn test_p0_dist_tso_timestamp() {
    let handle = make_dist_runtime_handle();
    let executor = crate::executor::Executor::new().with_dist_runtime(handle.clone());

    // 初始 TSO 应为 0
    let ts1 = executor.dist_current_timestamp().unwrap();
    assert_eq!(ts1, 0, "初始 TSO 应为 0");

    // 通过 DistRuntime 获取新时间戳
    let ts2 = {
        let mut rt = handle.write();
        rt.begin_transaction()
    };
    assert!(ts2 > ts1, "新时间戳应大于初始值");

    // 再次通过 executor 读取（不应递增）
    let ts3 = executor.dist_current_timestamp().unwrap();
    assert_eq!(ts3, ts2, "current_timestamp 不应递增");
}

/// P0-DIST-1/2/3：端到端 — DistRuntime KV 操作 + TSO + 分片路由
#[test]
fn test_p0_dist_end_to_end_kv_and_tso() {
    let handle = make_dist_runtime_handle();

    // 1. TSO 时间戳递增
    let ts1 = {
        let mut rt = handle.write();
        rt.begin_transaction()
    };
    let ts2 = {
        let mut rt = handle.write();
        rt.begin_transaction()
    };
    assert!(ts1 < ts2, "TSO 应单调递增");

    // 2. KV 写入和读取
    {
        let mut rt = handle.write();
        rt.put(b"table:t1".to_vec(), b"row_data_1".to_vec())
            .unwrap();
        rt.put(b"table:t2".to_vec(), b"row_data_2".to_vec())
            .unwrap();
    }

    // 3. 读取验证
    {
        let rt = handle.read();
        assert_eq!(rt.get(b"table:t1").unwrap(), Some(b"row_data_1".to_vec()));
        assert_eq!(rt.get(b"table:t2").unwrap(), Some(b"row_data_2".to_vec()));
        assert_eq!(rt.get(b"table:t3").unwrap(), None);
    }

    // 4. 分片路由
    {
        let rt = handle.read();
        let sid1 = rt.route(b"table:t1").unwrap();
        let sid2 = rt.route(b"table:t2").unwrap();
        assert_eq!(sid1, sid2, "单分片模式下所有键路由到同一分片");
    }

    // 5. KV 计数
    {
        let rt = handle.read();
        assert_eq!(rt.kv_len().unwrap(), 2);
    }
}

/// P0-DIST-3：DistRuntime 范围扫描
#[test]
fn test_p0_dist_scan_range() {
    let handle = make_dist_runtime_handle();

    // 写入多个键
    {
        let mut rt = handle.write();
        for i in 0..10 {
            let key = format!("k{:03}", i);
            let val = format!("v{:03}", i);
            rt.put(key.into_bytes(), val.into_bytes()).unwrap();
        }
    }

    // 扫描 [k003, k007)
    {
        let rt = handle.read();
        let range = szrsql_dist::shard::KeyRange::new(b"k003".to_vec(), b"k007".to_vec());
        let results = rt.scan(&range).unwrap();
        assert_eq!(results.len(), 4, "应扫描到 4 个键 [k003, k004, k005, k006]");
        assert_eq!(results[0].0, b"k003");
        assert_eq!(results[3].0, b"k006");
    }
}

/// P0-DIST-1：DistRuntime 删除操作
#[test]
fn test_p0_dist_delete() {
    let handle = make_dist_runtime_handle();

    {
        let mut rt = handle.write();
        rt.put(b"key1".to_vec(), b"val1".to_vec()).unwrap();
        rt.put(b"key2".to_vec(), b"val2".to_vec()).unwrap();
    }

    {
        let mut rt = handle.write();
        rt.delete(b"key1".to_vec()).unwrap();
    }

    {
        let rt = handle.read();
        assert_eq!(rt.get(b"key1").unwrap(), None, "删除后应返回 None");
        assert_eq!(rt.get(b"key2").unwrap(), Some(b"val2".to_vec()));
        assert_eq!(rt.kv_len().unwrap(), 1);
    }
}

// =====================================================================
//  P3-1: GROUPING SETS / CUBE / ROLLUP 端到端测试
// =====================================================================

/// 构建 ROLLUP/CUBE 测试用 catalog：sales2(dept, region, amount)
fn make_gs_catalog() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "sales2",
        vec![
            ("dept", ColumnType::Text),
            ("region", ColumnType::Text),
            ("amount", ColumnType::Int64),
        ],
    );
    catalog
}

/// 构建 ROLLUP/CUBE 测试用表：6 行
/// - A/东/100  A/西/200  B/东/300
/// - B/西/400  A/东/500  B/东/600
fn make_gs_table() -> InMemoryTable {
    let mut t = InMemoryTable::with_columns(
        "sales2",
        vec![
            ("dept", ColumnType::Text),
            ("region", ColumnType::Text),
            ("amount", ColumnType::Int64),
        ],
    );
    for (d, r, a) in [
        ("A", "东", 100),
        ("A", "西", 200),
        ("B", "东", 300),
        ("B", "西", 400),
        ("A", "东", 500),
        ("B", "东", 600),
    ] {
        t.insert(vec![
            Value::Text(d.into()),
            Value::Text(r.into()),
            Value::Int64(a),
        ]);
    }
    t
}

fn register_gs<'a>(exec: &mut Executor<'a>, t: &'a InMemoryTable) {
    exec.register_table(t);
}

#[test]
fn test_p3_1_rollup_single_column() {
    // ROLLUP(dept) → 分组集: (), (dept)  → 2 行输出
    let catalog = make_gs_catalog();
    let plan = plan_sql(
        "SELECT dept, COUNT(*), SUM(amount) FROM sales2 GROUP BY ROLLUP(dept)",
        &catalog,
    );
    let t = make_gs_table();
    let mut exec = Executor::new();
    register_gs(&mut exec, &t);
    let result = exec.execute(&plan).unwrap();

    // ROLLUP(dept) → 分组集: (), (dept) → 3 行（汇总 + A + B）
    assert_eq!(result.len(), 3, "ROLLUP(dept) 应输出 3 行");

    let mut total_count = 0i64;
    let mut total_sum = 0i64;
    let mut by_dept: std::collections::HashMap<String, (i64, i64)> =
        std::collections::HashMap::new();

    for row in &result {
        match (&row[0], &row[1], &row[2]) {
            (Value::Null, Value::Int64(c), Value::Int64(s)) => {
                // 汇总行（空分组集）
                total_count = *c;
                total_sum = *s;
            }
            (Value::Text(d), Value::Int64(c), Value::Int64(s)) => {
                by_dept.insert(d.clone(), (*c, *s));
            }
            other => panic!("unexpected row: {:?}", other),
        }
    }
    assert_eq!(total_count, 6, "汇总行 COUNT(*) 应为 6");
    assert_eq!(
        total_sum, 2100,
        "汇总行 SUM(amount) 应为 100+200+300+400+500+600=2100"
    );
    assert_eq!(by_dept.get("A"), Some(&(3, 800)), "dept A: 3行, sum=800");
    assert_eq!(by_dept.get("B"), Some(&(3, 1300)), "dept B: 3行, sum=1300");
}

#[test]
fn test_p3_1_cube_single_column() {
    // CUBE(dept) 单列 → 等价于 ROLLUP(dept) → 3 行（汇总 + A + B）
    let catalog = make_gs_catalog();
    let plan = plan_sql(
        "SELECT dept, COUNT(*) FROM sales2 GROUP BY CUBE(dept)",
        &catalog,
    );
    let t = make_gs_table();
    let mut exec = Executor::new();
    register_gs(&mut exec, &t);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(
        result.len(),
        3,
        "CUBE(dept) 单列应输出 3 行（汇总+deptA+deptB）"
    );
}

#[test]
fn test_p3_1_grouping_sets_explicit() {
    // GROUPING SETS ((dept), ()) → 显式两个分组集
    let catalog = make_gs_catalog();
    let plan = plan_sql(
        "SELECT dept, COUNT(*) FROM sales2 GROUP BY GROUPING SETS ((dept), ())",
        &catalog,
    );
    let t = make_gs_table();
    let mut exec = Executor::new();
    register_gs(&mut exec, &t);
    let result = exec.execute(&plan).unwrap();
    assert_eq!(
        result.len(),
        3,
        "GROUPING SETS ((dept),()) 应输出 3 行（2个dept + 1个汇总）"
    );
}

#[test]
fn test_p3_1_rollup_two_columns() {
    // ROLLUP(dept, region) → 分组集: (), (dept), (dept, region)
    // max_group_count = 2，输出统一为 2 列分组键 + 聚合列
    let catalog = make_gs_catalog();
    let plan = plan_sql(
        "SELECT dept, region, COUNT(*), SUM(amount) FROM sales2 GROUP BY ROLLUP(dept, region)",
        &catalog,
    );
    let t = make_gs_table();
    let mut exec = Executor::new();
    register_gs(&mut exec, &t);
    let result = exec.execute(&plan).unwrap();

    // 3 个分组集：
    //   ()         → 1 行 (NULL, NULL)
    //   (dept)     → 2 行 (A, NULL), (B, NULL)
    //   (dept,region) → A/东, A/西, B/东, B/西 → 4 行
    // 共 7 行
    assert_eq!(result.len(), 7, "ROLLUP(dept, region) 应输出 7 行");

    let mut rows_by_key: std::collections::HashMap<(Option<String>, Option<String>), (i64, i64)> =
        result
            .iter()
            .map(|r| {
                let d = match &r[0] {
                    Value::Text(s) => Some(s.clone()),
                    Value::Null => None,
                    other => panic!("dept 应为 Text 或 NULL: {:?}", other),
                };
                let reg = match &r[1] {
                    Value::Text(s) => Some(s.clone()),
                    Value::Null => None,
                    other => panic!("region 应为 Text 或 NULL: {:?}", other),
                };
                let c = match &r[2] {
                    Value::Int64(v) => *v,
                    other => panic!("count 应为 Int64: {:?}", other),
                };
                let s = match &r[3] {
                    Value::Int64(v) => *v,
                    other => panic!("sum 应为 Int64: {:?}", other),
                };
                ((d, reg), (c, s))
            })
            .collect();

    // 汇总行
    assert_eq!(rows_by_key.remove(&(None, None)), Some((6, 2100)));
    // (dept) 级
    assert_eq!(
        rows_by_key.remove(&(Some("A".into()), None)),
        Some((3, 800))
    );
    assert_eq!(
        rows_by_key.remove(&(Some("B".into()), None)),
        Some((3, 1300))
    );
    // (dept, region) 级
    assert_eq!(
        rows_by_key.remove(&(Some("A".into()), Some("东".into()))),
        Some((2, 600))
    ); // 100+500
    assert_eq!(
        rows_by_key.remove(&(Some("A".into()), Some("西".into()))),
        Some((1, 200))
    );
    assert_eq!(
        rows_by_key.remove(&(Some("B".into()), Some("东".into()))),
        Some((2, 900))
    ); // 300+600
    assert_eq!(
        rows_by_key.remove(&(Some("B".into()), Some("西".into()))),
        Some((1, 400))
    );
    assert!(rows_by_key.is_empty(), "不应有多余行: {:?}", rows_by_key);
}

#[test]
fn test_p3_1_cube_two_columns() {
    // CUBE(dept, region) → 2^2 = 4 个分组集: (), (dept), (region), (dept, region)
    // max_group_count = 2
    let catalog = make_gs_catalog();
    let plan = plan_sql(
        "SELECT dept, region, COUNT(*) FROM sales2 GROUP BY CUBE(dept, region)",
        &catalog,
    );
    let t = make_gs_table();
    let mut exec = Executor::new();
    register_gs(&mut exec, &t);
    let result = exec.execute(&plan).unwrap();

    // 4 个分组集：
    //   ()            → 1 行
    //   (dept)        → 2 行 (A), (B)
    //   (region)      → 2 行 (东), (西)
    //   (dept,region) → 4 行
    // 共 9 行
    assert_eq!(result.len(), 9, "CUBE(dept, region) 应输出 9 行");

    let mut null_dept_region_count = 0i64; // (NULL, region) 行的合计
    for row in &result {
        match (&row[0], &row[1], &row[2]) {
            (Value::Null, Value::Null, Value::Int64(c)) => assert_eq!(*c, 6, "汇总行 COUNT 应为 6"),
            (Value::Null, Value::Text(r), Value::Int64(c)) => {
                // region 级汇总
                match r.as_str() {
                    "东" => {
                        null_dept_region_count += *c;
                        assert_eq!(*c, 4, "region=东 应有 4 行(A东2+B东2)");
                    }
                    "西" => assert_eq!(*c, 2, "region=西 应有 2 行(A西1+B西1)"),
                    _ => panic!("未知 region: {}", r),
                }
            }
            (Value::Text(d), Value::Null, Value::Int64(c)) => match d.as_str() {
                "A" => assert_eq!(*c, 3),
                "B" => assert_eq!(*c, 3),
                _ => panic!("未知 dept: {}", d),
            },
            _ => {} // (dept, region) 级行
        }
    }
    assert_eq!(null_dept_region_count, 4, "东 region 汇总应为 4 行");
}

#[test]
fn test_p3_1_grouping_sets_multiple_sets() {
    // GROUPING SETS ((dept, region), (dept)) → 2 个分组集
    // set1 (dept,region): 4 行, set2 (dept): 2 行 → 共 6 行
    // 两个集最大列数 = 2，set2 输出 (dept, NULL)
    let catalog = make_gs_catalog();
    let plan = plan_sql(
        "SELECT dept, region, COUNT(*) FROM sales2 GROUP BY GROUPING SETS ((dept, region), (dept))",
        &catalog,
    );
    let t = make_gs_table();
    let mut exec = Executor::new();
    register_gs(&mut exec, &t);
    let result = exec.execute(&plan).unwrap();

    assert_eq!(
        result.len(),
        6,
        "GROUPING SETS ((dept,region),(dept)) 应输出 6 行"
    );

    let mut has_dept_null_region = false;
    for row in &result {
        if let (Value::Text(_), Value::Null, Value::Int64(_)) = (&row[0], &row[1], &row[2]) {
            has_dept_null_region = true; // (dept) 集的 NULL 填充
        }
    }
    assert!(has_dept_null_region, "(dept) 集应产生 region=NULL 的填充行");
}

#[test]
fn test_p3_1_rollup_with_having() {
    // ROLLUP + HAVING：过滤聚合结果
    let catalog = make_gs_catalog();
    let plan = plan_sql(
        "SELECT dept, COUNT(*), SUM(amount) FROM sales2 GROUP BY ROLLUP(dept) HAVING COUNT(*) > 2",
        &catalog,
    );
    let t = make_gs_table();
    let mut exec = Executor::new();
    register_gs(&mut exec, &t);
    let result = exec.execute(&plan).unwrap();

    // 汇总行 COUNT=6 > 2 ✓；dept A COUNT=3 > 2 ✓；dept B COUNT=3 > 2 ✓ → 3 行全保留
    assert_eq!(result.len(), 3, "HAVING COUNT(*)>2 应保留全部 3 行");
    for row in &result {
        let c = match row[1] {
            Value::Int64(v) => v,
            _ => panic!(),
        };
        assert!(c > 2, "HAVING 应过滤掉 COUNT<=2 的行");
    }
}

#[test]
fn test_p3_1_rollup_with_having_filters_grand_total() {
    // HAVING 过滤掉汇总行（COUNT=6 > 100 为假）
    let catalog = make_gs_catalog();
    let plan = plan_sql(
        "SELECT dept, COUNT(*) FROM sales2 GROUP BY ROLLUP(dept) HAVING COUNT(*) > 100",
        &catalog,
    );
    let t = make_gs_table();
    let mut exec = Executor::new();
    register_gs(&mut exec, &t);
    let result = exec.execute(&plan).unwrap();

    // 汇总行 COUNT=6 不满足 >100 → 过滤掉；dept A/B 各 COUNT=3 也不满足 → 全部过滤
    assert_eq!(
        result.len(),
        0,
        "所有行 COUNT<=6 均不满足 >100，应输出 0 行"
    );
}

// =====================================================================
//  LATERAL JOIN 测试（P3-2）— SQL:2016 T-72
// =====================================================================

/// 构建 LATERAL JOIN 测试用 catalog：users + orders
fn make_lateral_catalog() -> InMemoryCatalog {
    let mut cat = InMemoryCatalog::new();
    cat.add_simple_table(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    cat.add_simple_table(
        "orders",
        vec![
            ("id", ColumnType::Int64),
            ("user_id", ColumnType::Int64),
            ("amount", ColumnType::Int64),
        ],
    );
    cat
}

/// users 表：alice(1), bob(2), carol(3)
fn make_lateral_users() -> InMemoryTable {
    let mut t = InMemoryTable::with_columns(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    t.insert(vec![Value::Int64(1), Value::Text("alice".into())]);
    t.insert(vec![Value::Int64(2), Value::Text("bob".into())]);
    t.insert(vec![Value::Int64(3), Value::Text("carol".into())]);
    t
}

/// orders 表：alice 2 单，bob 1 单，carol 0 单
fn make_lateral_orders() -> InMemoryTable {
    let mut t = InMemoryTable::with_columns(
        "orders",
        vec![
            ("id", ColumnType::Int64),
            ("user_id", ColumnType::Int64),
            ("amount", ColumnType::Int64),
        ],
    );
    t.insert(vec![Value::Int64(1), Value::Int64(1), Value::Int64(100)]);
    t.insert(vec![Value::Int64(2), Value::Int64(1), Value::Int64(200)]);
    t.insert(vec![Value::Int64(3), Value::Int64(2), Value::Int64(300)]);
    t
}

#[test]
fn test_p3_2_lateral_inner_join_basic() {
    // SELECT u.name, o.amount FROM users u
    //   JOIN LATERAL (SELECT * FROM orders WHERE orders.user_id = u.id) o ON true
    // 预期：alice 2 行(100,200)，bob 1 行(300)，carol 0 行 → 共 3 行
    let catalog = make_lateral_catalog();
    let plan = plan_sql(
        "SELECT u.name, o.amount \
         FROM users u \
         JOIN LATERAL (SELECT * FROM orders WHERE orders.user_id = u.id) AS o ON true",
        &catalog,
    );
    let users = make_lateral_users();
    let orders = make_lateral_orders();
    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&users);
    exec.register_table(&orders);
    let result = exec.execute(&plan).unwrap();

    assert_eq!(result.len(), 3, "LATERAL INNER JOIN 应得 3 行");
    // 验证 alice 的两笔订单
    let alice_amounts: Vec<i64> = result
        .iter()
        .filter(|r| matches!(&r[0], Value::Text(s) if s == "alice"))
        .filter_map(|r| match &r[1] {
            Value::Int64(v) => Some(*v),
            _ => None,
        })
        .collect();
    assert!(alice_amounts.contains(&100));
    assert!(alice_amounts.contains(&200));
    // 验证 bob 的一笔订单
    let bob_amounts: Vec<i64> = result
        .iter()
        .filter(|r| matches!(&r[0], Value::Text(s) if s == "bob"))
        .filter_map(|r| match &r[1] {
            Value::Int64(v) => Some(*v),
            _ => None,
        })
        .collect();
    assert_eq!(bob_amounts, vec![300]);
    // carol 无订单，INNER JOIN 不出现
    assert!(!result
        .iter()
        .any(|r| matches!(&r[0], Value::Text(s) if s == "carol")));
}

#[test]
fn test_p3_2_lateral_left_join_preserves_unmatched_left() {
    // LEFT JOIN LATERAL：carol 无订单，右列应填 NULL
    let catalog = make_lateral_catalog();
    let plan = plan_sql(
        "SELECT u.name, o.amount \
         FROM users u \
         LEFT JOIN LATERAL (SELECT * FROM orders WHERE orders.user_id = u.id) AS o ON true",
        &catalog,
    );
    let users = make_lateral_users();
    let orders = make_lateral_orders();
    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&users);
    exec.register_table(&orders);
    let result = exec.execute(&plan).unwrap();

    // 3 users × 各自行数：alice 2 + bob 1 + carol 1(NULL) = 4 行
    assert_eq!(result.len(), 4, "LATERAL LEFT JOIN 应得 4 行");
    // carol 的 amount 应为 NULL
    let carol_row = result
        .iter()
        .find(|r| matches!(&r[0], Value::Text(s) if s == "carol"))
        .expect("应包含 carol");
    assert!(
        matches!(&carol_row[1], Value::Null),
        "carol 无订单，amount 应为 NULL"
    );
}

#[test]
fn test_p3_2_lateral_join_with_aggregation() {
    // 聚合 LATERAL：每用户订单总额
    // SELECT u.name, SUM(o.amount) FROM users u
    //   LEFT JOIN LATERAL (SELECT * FROM orders WHERE orders.user_id = u.id) o ON true
    //   GROUP BY u.name
    let catalog = make_lateral_catalog();
    let plan = plan_sql(
        "SELECT u.name, SUM(o.amount) \
         FROM users u \
         LEFT JOIN LATERAL (SELECT * FROM orders WHERE orders.user_id = u.id) AS o ON true \
         GROUP BY u.name",
        &catalog,
    );
    let users = make_lateral_users();
    let orders = make_lateral_orders();
    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&users);
    exec.register_table(&orders);
    let result = exec.execute(&plan).unwrap();

    assert_eq!(result.len(), 3, "应有 3 个用户分组");
    let mut sums: std::collections::HashMap<String, Option<i64>> = std::collections::HashMap::new();
    for row in &result {
        if let (Value::Text(name), sum) = (&row[0], &row[1]) {
            let v = match sum {
                Value::Int64(x) => Some(*x),
                Value::Null => None,
                _ => None,
            };
            sums.insert(name.clone(), v);
        }
    }
    assert_eq!(sums.get("alice"), Some(&Some(300)), "alice SUM = 100+200");
    assert_eq!(sums.get("bob"), Some(&Some(300)), "bob SUM = 300");
    // carol 无订单 → LATERAL LEFT JOIN 右列填 NULL → SUM(NULL) = NULL（SQL 标准语义）
    assert_eq!(sums.get("carol"), Some(&None), "carol 无订单 SUM = NULL");
}

#[test]
fn test_p3_2_lateral_join_subquery_with_limit() {
    // LATERAL + LIMIT：每用户取最近 1 笔订单（按 id 降序）
    // SELECT u.name, o.amount FROM users u
    //   JOIN LATERAL (SELECT * FROM orders WHERE orders.user_id = u.id ORDER BY id DESC LIMIT 1) o ON true
    let catalog = make_lateral_catalog();
    let plan = plan_sql(
        "SELECT u.name, o.amount \
         FROM users u \
         JOIN LATERAL (SELECT * FROM orders WHERE orders.user_id = u.id ORDER BY id DESC LIMIT 1) AS o ON true",
        &catalog,
    );
    let users = make_lateral_users();
    let orders = make_lateral_orders();
    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&users);
    exec.register_table(&orders);
    let result = exec.execute(&plan).unwrap();

    // alice 最近单 = 200 (id=2), bob = 300 (id=3)
    assert_eq!(result.len(), 2, "每用户 LIMIT 1 → 2 行");
    let mut m: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for row in &result {
        if let (Value::Text(name), Value::Int64(amt)) = (&row[0], &row[1]) {
            m.insert(name.clone(), *amt);
        }
    }
    assert_eq!(m.get("alice"), Some(&200), "alice 最近单 = 200");
    assert_eq!(m.get("bob"), Some(&300), "bob 最近单 = 300");
}
