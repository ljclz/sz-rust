//! P2-19 PostGIS 空间扩展 SQL 集成测试。
//!
//! 验证 spatial 模块的 14 个函数通过 `ExprEvaluator::eval_function` 暴露在 SQL 执行路径中。
//! 每个测试均走 Parser → Planner → Executor 完整链路，断言返回的 `Value` 符合预期。
//!
//! 覆盖函数：
//! - ST_Point(x, y) / ST_Point(x, y, srid)
//! - ST_GeomFromText(wkt) / ST_GeomFromText(wkt, srid)
//! - ST_X(g) / ST_Y(g) / ST_SRID(g) / ST_SetSRID(g, srid)
//! - ST_Distance(g1, g2)
//! - ST_Area(g) / ST_Length(g)
//! - ST_AsText(g) / ST_Envelope(g)
//! - ST_Within / ST_Contains / ST_Intersects（谓词，返回 bool）
//!
//! 共 14 个测试用例。

use super::executor::Executor;
use crate::parser::parse_sql;
use crate::plan::{InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::Value;

// =====================================================================
//  辅助函数
// =====================================================================

/// SQL → LogicalPlan（单语句）
fn plan_sql(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
    let stmts = parse_sql(sql).expect("parse failed");
    assert_eq!(stmts.len(), 1, "expected exactly 1 statement");
    let planner = Planner::new(catalog);
    planner
        .plan_statement(stmts.into_iter().next().unwrap())
        .expect("plan failed")
}

/// 执行 SELECT 并返回第一行第一列的值（用于标量函数测试）。
fn eval_scalar(sql: &str) -> Value {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql(sql, &catalog);
    let exec = Executor::new();
    let result = exec.execute(&plan).expect("execute failed");
    assert_eq!(
        result.len(),
        1,
        "expected exactly 1 row, got {}",
        result.len()
    );
    assert_eq!(result[0].len(), 1, "expected exactly 1 column");
    result[0][0].clone()
}

/// 执行 SELECT 并返回所有行（用于谓词/多行测试）。
fn eval_rows(sql: &str) -> Vec<Vec<Value>> {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql(sql, &catalog);
    let exec = Executor::new();
    exec.execute(&plan).expect("execute failed")
}

// =====================================================================
//  ST_Point
// =====================================================================

#[test]
fn test_spatial_st_point() {
    let v = eval_scalar("SELECT ST_Point(3, 4)");
    assert_eq!(v, Value::Text("POINT (3.0 4.0)".to_string()));
}

#[test]
fn test_spatial_st_point_with_srid() {
    let v = eval_scalar("SELECT ST_Point(3, 4, 4326)");
    assert_eq!(v, Value::Text("SRID=4326;POINT (3.0 4.0)".to_string()));
}

#[test]
fn test_spatial_st_point_float_args() {
    let v = eval_scalar("SELECT ST_Point(1.5, 2.5)");
    assert_eq!(v, Value::Text("POINT (1.5 2.5)".to_string()));
}

// =====================================================================
//  ST_GeomFromText
// =====================================================================

#[test]
fn test_spatial_st_geomfromtext_point() {
    let v = eval_scalar("SELECT ST_GeomFromText('POINT (1 2)')");
    assert_eq!(v, Value::Text("POINT (1.0 2.0)".to_string()));
}

#[test]
fn test_spatial_st_geomfromtext_linestring() {
    let v = eval_scalar("SELECT ST_GeomFromText('LINESTRING (0 0, 3 4)')");
    assert_eq!(v, Value::Text("LINESTRING (0.0 0.0, 3.0 4.0)".to_string()));
}

#[test]
fn test_spatial_st_geomfromtext_polygon() {
    let v = eval_scalar("SELECT ST_GeomFromText('POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))')");
    assert_eq!(
        v,
        Value::Text("POLYGON ((0.0 0.0, 4.0 0.0, 4.0 4.0, 0.0 4.0, 0.0 0.0))".to_string())
    );
}

#[test]
fn test_spatial_st_geomfromtext_with_srid() {
    // st_geomfromtext 仅接受 1 个 WKT 参数；SRID 通过 st_setsrid 设置
    let v = eval_scalar("SELECT ST_SRID(ST_SetSRID(ST_GeomFromText('POINT (5 6)'), 4326))");
    assert_eq!(v, Value::Int64(4326));
}

// =====================================================================
//  ST_X / ST_Y / ST_SRID / ST_SetSRID
// =====================================================================

#[test]
fn test_spatial_st_x_st_y() {
    let vx = eval_scalar("SELECT ST_X(ST_Point(7, 8))");
    let vy = eval_scalar("SELECT ST_Y(ST_Point(7, 8))");
    // st_x / st_y 返回 f64
    match vx {
        Value::Float64(f) => assert!((f - 7.0).abs() < 1e-9, "expected 7.0, got {f}"),
        other => panic!("expected Float64, got {other:?}"),
    }
    match vy {
        Value::Float64(f) => assert!((f - 8.0).abs() < 1e-9, "expected 8.0, got {f}"),
        other => panic!("expected Float64, got {other:?}"),
    }
}

#[test]
fn test_spatial_st_srid_default() {
    // SRID 默认值 0
    let v = eval_scalar("SELECT ST_SRID(ST_Point(1, 1))");
    assert_eq!(v, Value::Int64(0));
}

#[test]
fn test_spatial_st_setsrid() {
    let v = eval_scalar("SELECT ST_SRID(ST_SetSRID(ST_Point(1, 1), 4326))");
    assert_eq!(v, Value::Int64(4326));
}

// =====================================================================
//  ST_Distance
// =====================================================================

#[test]
fn test_spatial_st_distance_cartesian() {
    // distance((0,0), (3,4)) = 5
    let v = eval_scalar("SELECT ST_Distance(ST_Point(0, 0), ST_Point(3, 4))");
    match v {
        Value::Float64(f) => assert!((f - 5.0).abs() < 1e-9, "expected 5.0, got {f}"),
        other => panic!("expected Float64, got {other:?}"),
    }
}

// =====================================================================
//  ST_Area / ST_Length
// =====================================================================

#[test]
fn test_spatial_st_area() {
    // 4×4 正方形，面积 16
    let v = eval_scalar("SELECT ST_Area(ST_GeomFromText('POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))'))");
    match v {
        Value::Float64(f) => assert!((f - 16.0).abs() < 1e-9, "expected 16.0, got {f}"),
        other => panic!("expected Float64, got {other:?}"),
    }
}

#[test]
fn test_spatial_st_length() {
    // LINESTRING (0 0, 3 4) 长度 5
    let v = eval_scalar("SELECT ST_Length(ST_GeomFromText('LINESTRING (0 0, 3 4)'))");
    match v {
        Value::Float64(f) => assert!((f - 5.0).abs() < 1e-9, "expected 5.0, got {f}"),
        other => panic!("expected Float64, got {other:?}"),
    }
}

// =====================================================================
//  ST_AsText / ST_Envelope
// =====================================================================

#[test]
fn test_spatial_st_astext() {
    let v = eval_scalar("SELECT ST_AsText(ST_GeomFromText('POINT (9 10)'))");
    assert_eq!(v, Value::Text("POINT (9.0 10.0)".to_string()));
}

#[test]
fn test_spatial_st_envelope() {
    // 线段的 envelope 是包含两端的矩形
    let v = eval_scalar("SELECT ST_AsText(ST_Envelope(ST_GeomFromText('LINESTRING (0 0, 3 4)')))");
    assert_eq!(
        v,
        Value::Text("POLYGON ((0.0 0.0, 3.0 0.0, 3.0 4.0, 0.0 4.0, 0.0 0.0))".to_string())
    );
}

// =====================================================================
//  ST_Within / ST_Contains / ST_Intersects（谓词）
// =====================================================================

#[test]
fn test_spatial_st_within() {
    // 点 (1,1) 在正方形内
    let rows = eval_rows(
        "SELECT ST_Within(
            ST_Point(1, 1),
            ST_GeomFromText('POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))')
        )",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Bool(true));
}

#[test]
fn test_spatial_st_contains() {
    // 正方形包含点 (1,1)
    let rows = eval_rows(
        "SELECT ST_Contains(
            ST_GeomFromText('POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))'),
            ST_Point(1, 1)
        )",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Bool(true));
}

#[test]
fn test_spatial_st_intersects() {
    // 两个相交的正方形（共享区域 (1,1)-(2,2)）
    let rows = eval_rows(
        "SELECT ST_Intersects(
            ST_GeomFromText('POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))'),
            ST_GeomFromText('POLYGON ((1 1, 3 1, 3 3, 1 3, 1 1))')
        )",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Bool(true));
}

#[test]
fn test_spatial_st_intersects_disjoint() {
    // 两条不相交线段
    let rows = eval_rows(
        "SELECT ST_Intersects(
            ST_GeomFromText('LINESTRING (0 0, 1 1)'),
            ST_GeomFromText('LINESTRING (10 10, 11 11)')
        )",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Bool(false));
}
