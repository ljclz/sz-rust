//! Phase 3.1 单元测试 — 100 条 PG SQL 解析测试。
//!
//! 覆盖范围：
//! - DDL（20 条）：CREATE TABLE / DROP TABLE / CREATE INDEX / DROP INDEX
//! - INSERT（10 条）：VALUES / SELECT / DEFAULT VALUES / RETURNING
//! - UPDATE（8 条）：SET / WHERE / FROM / RETURNING
//! - DELETE（5 条）：WHERE / USING / RETURNING
//! - SELECT（25 条）：DISTINCT / JOIN / GROUP BY / HAVING / ORDER BY / LIMIT / OFFSET / 子查询
//! - 表达式（20 条）：字面量 / 标识符 / 二元/一元 / 函数 / CASE / CAST / IN / BETWEEN / LIKE / IS NULL / EXISTS / 元组
//! - 事务（8 条）：BEGIN / COMMIT / ROLLBACK / SAVEPOINT / RELEASE / SET TRANSACTION
//! - EXPLAIN（4 条）：EXPLAIN / EXPLAIN ANALYZE / EXPLAIN VERBOSE / EXPLAIN ANALYZE VERBOSE

use crate::ast::*;
use crate::parser::{parse_one, parse_sql};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 解析 SQL 并断言成功，返回 Statement
fn must_parse(sql: &str) -> Statement {
    match parse_one(sql) {
        Ok(stmt) => stmt,
        Err(e) => panic!("parse failed for SQL: {sql}\nerror: {e:?}"),
    }
}

/// 断言解析失败
fn must_fail(sql: &str) {
    assert!(
        parse_one(sql).is_err(),
        "expected parse failure for SQL: {sql}"
    );
}

// =====================================================================
//  DDL 测试（20 条）
// =====================================================================

#[test]
fn test_create_table_minimal() {
    let stmt = must_parse("CREATE TABLE t (id INT)");
    match stmt {
        Statement::CreateTable {
            name,
            columns,
            if_not_exists,
            ..
        } => {
            assert_eq!(name.qualified_name(), "t");
            assert_eq!(columns.len(), 1);
            assert!(!if_not_exists);
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_with_schema() {
    let stmt = must_parse("CREATE TABLE public.users (id INT)");
    match stmt {
        Statement::CreateTable { name, .. } => {
            assert_eq!(name.schema.as_deref(), Some("public"));
            assert_eq!(name.name, "users");
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_if_not_exists() {
    let stmt = must_parse("CREATE TABLE IF NOT EXISTS t (id INT)");
    match stmt {
        Statement::CreateTable { if_not_exists, .. } => assert!(if_not_exists),
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_various_types() {
    let stmt = must_parse(
        "CREATE TABLE t (a INT, b BIGINT, c TEXT, d VARCHAR(255), e BOOLEAN, f DECIMAL(10,2), g TIMESTAMP, h DATE)",
    );
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert_eq!(columns.len(), 8);
            assert_eq!(columns[0].data_type, ColumnType::Int64);
            assert_eq!(columns[1].data_type, ColumnType::Int64);
            assert_eq!(columns[2].data_type, ColumnType::Text);
            assert_eq!(columns[3].data_type, ColumnType::Text);
            assert_eq!(columns[4].data_type, ColumnType::Bool);
            assert_eq!(
                columns[5].data_type,
                ColumnType::Decimal {
                    precision: 10,
                    scale: 2
                }
            );
            assert_eq!(columns[6].data_type, ColumnType::Timestamp);
            assert_eq!(columns[7].data_type, ColumnType::Date);
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_not_null() {
    let stmt = must_parse("CREATE TABLE t (id INT NOT NULL)");
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert!(columns[0].not_null);
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_primary_key_column() {
    let stmt = must_parse("CREATE TABLE t (id INT PRIMARY KEY)");
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert!(columns[0].primary_key);
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_unique_column() {
    let stmt = must_parse("CREATE TABLE t (email TEXT UNIQUE)");
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert!(columns[0].unique);
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_default() {
    let stmt = must_parse("CREATE TABLE t (active BOOLEAN DEFAULT TRUE)");
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert!(columns[0].default.is_some());
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_check_constraint_column() {
    let stmt = must_parse("CREATE TABLE t (age INT CHECK (age >= 0))");
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert!(columns[0].check.is_some());
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_foreign_key_column() {
    let stmt = must_parse("CREATE TABLE t (uid INT REFERENCES users(id))");
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert!(columns[0].references.is_some());
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_table_level_primary_key() {
    let stmt = must_parse("CREATE TABLE t (id INT, PRIMARY KEY (id))");
    match stmt {
        Statement::CreateTable { constraints, .. } => {
            assert_eq!(constraints.len(), 1);
            assert!(matches!(constraints[0], TableConstraint::PrimaryKey { .. }));
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_table_level_unique() {
    let stmt = must_parse("CREATE TABLE t (email TEXT, UNIQUE (email))");
    match stmt {
        Statement::CreateTable { constraints, .. } => {
            assert_eq!(constraints.len(), 1);
            assert!(matches!(constraints[0], TableConstraint::Unique { .. }));
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_table_level_foreign_key() {
    let stmt = must_parse(
        "CREATE TABLE t (uid INT, CONSTRAINT fk1 FOREIGN KEY (uid) REFERENCES users(id))",
    );
    match stmt {
        Statement::CreateTable { constraints, .. } => {
            assert_eq!(constraints.len(), 1);
            if let TableConstraint::ForeignKey { name, .. } = &constraints[0] {
                assert_eq!(name.as_deref(), Some("fk1"));
            } else {
                panic!("expected ForeignKey");
            }
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_table_level_check() {
    let stmt = must_parse("CREATE TABLE t (a INT, CHECK (a > 0))");
    match stmt {
        Statement::CreateTable { constraints, .. } => {
            assert_eq!(constraints.len(), 1);
            assert!(matches!(constraints[0], TableConstraint::Check { .. }));
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_composite_primary_key() {
    let stmt = must_parse("CREATE TABLE t (a INT, b INT, PRIMARY KEY (a, b))");
    match stmt {
        Statement::CreateTable { constraints, .. } => {
            if let TableConstraint::PrimaryKey { columns, .. } = &constraints[0] {
                assert_eq!(columns.len(), 2);
                assert_eq!(columns[0], "a");
                assert_eq!(columns[1], "b");
            } else {
                panic!("expected PrimaryKey");
            }
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_drop_table_simple() {
    let stmt = must_parse("DROP TABLE t");
    match stmt {
        Statement::DropTable {
            names,
            if_exists,
            cascade,
        } => {
            assert_eq!(names.len(), 1);
            assert!(!if_exists);
            assert!(!cascade);
        }
        other => panic!("expected DropTable, got {other:?}"),
    }
}

#[test]
fn test_drop_table_if_exists() {
    let stmt = must_parse("DROP TABLE IF EXISTS t");
    match stmt {
        Statement::DropTable { if_exists, .. } => assert!(if_exists),
        other => panic!("expected DropTable, got {other:?}"),
    }
}

#[test]
fn test_drop_table_cascade() {
    let stmt = must_parse("DROP TABLE t CASCADE");
    match stmt {
        Statement::DropTable { cascade, .. } => assert!(cascade),
        other => panic!("expected DropTable, got {other:?}"),
    }
}

#[test]
fn test_create_index_simple() {
    let stmt = must_parse("CREATE INDEX idx_name ON t (name)");
    match stmt {
        Statement::CreateIndex {
            name,
            table,
            columns,
            unique,
            ..
        } => {
            assert_eq!(name.as_deref(), Some("idx_name"));
            assert_eq!(table.name, "t");
            assert_eq!(columns.len(), 1);
            assert!(!unique);
        }
        other => panic!("expected CreateIndex, got {other:?}"),
    }
}

#[test]
fn test_create_unique_index() {
    let stmt = must_parse("CREATE UNIQUE INDEX idx_email ON t (email)");
    match stmt {
        Statement::CreateIndex { unique, .. } => assert!(unique),
        other => panic!("expected CreateIndex, got {other:?}"),
    }
}

#[test]
fn test_drop_index() {
    let stmt = must_parse("DROP INDEX idx_name");
    match stmt {
        Statement::DropIndex { names, if_exists } => {
            assert_eq!(names, vec!["idx_name".to_string()]);
            assert!(!if_exists);
        }
        other => panic!("expected DropIndex, got {other:?}"),
    }
}

// =====================================================================
//  INSERT 测试（10 条）
// =====================================================================

#[test]
fn test_insert_values_single_row() {
    let stmt = must_parse("INSERT INTO t (a, b) VALUES (1, 'x')");
    match stmt {
        Statement::Insert {
            table,
            columns,
            source,
            ..
        } => {
            assert_eq!(table.name, "t");
            assert_eq!(columns, Some(vec!["a".to_string(), "b".to_string()]));
            if let InsertSource::Values(rows) = source {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].len(), 2);
            } else {
                panic!("expected Values source");
            }
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

#[test]
fn test_insert_values_multiple_rows() {
    let stmt = must_parse("INSERT INTO t VALUES (1, 'a'), (2, 'b'), (3, 'c')");
    match stmt {
        Statement::Insert { source, .. } => {
            if let InsertSource::Values(rows) = source {
                assert_eq!(rows.len(), 3);
            } else {
                panic!("expected Values source");
            }
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

#[test]
fn test_insert_with_columns() {
    let stmt = must_parse("INSERT INTO t (a, b, c) VALUES (1, 2, 3)");
    match stmt {
        Statement::Insert { columns, .. } => {
            assert_eq!(
                columns,
                Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
            );
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

#[test]
fn test_insert_default_values() {
    let stmt = must_parse("INSERT INTO t DEFAULT VALUES");
    match stmt {
        Statement::Insert { source, .. } => {
            assert!(matches!(source, InsertSource::DefaultValues));
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

#[test]
fn test_insert_select() {
    let stmt = must_parse("INSERT INTO t SELECT * FROM s");
    match stmt {
        Statement::Insert { source, .. } => {
            assert!(matches!(source, InsertSource::Select(_)));
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

#[test]
fn test_insert_returning() {
    let stmt = must_parse("INSERT INTO t (a) VALUES (1) RETURNING a");
    match stmt {
        Statement::Insert { returning, .. } => {
            assert!(returning.is_some());
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

#[test]
fn test_insert_with_string_value() {
    let stmt = must_parse("INSERT INTO t (s) VALUES ('hello world')");
    match stmt {
        Statement::Insert { source, .. } => {
            if let InsertSource::Values(rows) = source {
                if let Expr::Literal(Value::Text(s)) = &rows[0][0] {
                    assert_eq!(s, "hello world");
                } else {
                    panic!("expected Text literal");
                }
            } else {
                panic!("expected Values source");
            }
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

#[test]
fn test_insert_with_null_value() {
    let stmt = must_parse("INSERT INTO t (x) VALUES (NULL)");
    match stmt {
        Statement::Insert { source, .. } => {
            if let InsertSource::Values(rows) = source {
                assert!(matches!(&rows[0][0], Expr::Literal(Value::Null)));
            } else {
                panic!("expected Values source");
            }
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

#[test]
fn test_insert_with_bool_value() {
    let stmt = must_parse("INSERT INTO t (b) VALUES (TRUE)");
    match stmt {
        Statement::Insert { source, .. } => {
            if let InsertSource::Values(rows) = source {
                assert!(matches!(&rows[0][0], Expr::Literal(Value::Bool(true))));
            } else {
                panic!("expected Values source");
            }
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

#[test]
fn test_insert_with_decimal_value() {
    let stmt = must_parse("INSERT INTO t (p) VALUES (123.45)");
    match stmt {
        Statement::Insert { source, .. } => {
            if let InsertSource::Values(rows) = source {
                assert!(matches!(&rows[0][0], Expr::Literal(Value::Float64(_))));
            } else {
                panic!("expected Values source");
            }
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}

// =====================================================================
//  UPDATE 测试（8 条）
// =====================================================================

#[test]
fn test_update_simple() {
    let stmt = must_parse("UPDATE t SET a = 1");
    match stmt {
        Statement::Update {
            table, assignments, ..
        } => {
            assert_eq!(table.name, "t");
            assert_eq!(assignments.len(), 1);
        }
        other => panic!("expected Update, got {other:?}"),
    }
}

#[test]
fn test_update_where() {
    let stmt = must_parse("UPDATE t SET a = 1 WHERE id = 5");
    match stmt {
        Statement::Update { where_clause, .. } => assert!(where_clause.is_some()),
        other => panic!("expected Update, got {other:?}"),
    }
}

#[test]
fn test_update_multiple_set() {
    let stmt = must_parse("UPDATE t SET a = 1, b = 2, c = 3");
    match stmt {
        Statement::Update { assignments, .. } => assert_eq!(assignments.len(), 3),
        other => panic!("expected Update, got {other:?}"),
    }
}

#[test]
fn test_update_from() {
    let stmt = must_parse("UPDATE t SET a = s.x FROM s WHERE t.id = s.id");
    match stmt {
        Statement::Update { from, .. } => assert_eq!(from.len(), 1),
        other => panic!("expected Update, got {other:?}"),
    }
}

#[test]
fn test_update_returning() {
    let stmt = must_parse("UPDATE t SET a = 1 RETURNING a");
    match stmt {
        Statement::Update { returning, .. } => assert!(returning.is_some()),
        other => panic!("expected Update, got {other:?}"),
    }
}

#[test]
fn test_update_with_expression() {
    let stmt = must_parse("UPDATE t SET a = a + 1");
    match stmt {
        Statement::Update { assignments, .. } => {
            assert_eq!(assignments[0].column, "a");
            assert!(matches!(assignments[0].value, Expr::BinaryOp { .. }));
        }
        other => panic!("expected Update, got {other:?}"),
    }
}

#[test]
fn test_update_with_alias() {
    let stmt = must_parse("UPDATE t AS x SET a = 1 WHERE x.id = 5");
    match stmt {
        Statement::Update { alias, .. } => assert_eq!(alias.as_deref(), Some("x")),
        other => panic!("expected Update, got {other:?}"),
    }
}

#[test]
fn test_update_with_subquery_in_where() {
    let stmt = must_parse("UPDATE t SET a = 1 WHERE id IN (SELECT id FROM s)");
    match stmt {
        Statement::Update { where_clause, .. } => {
            assert!(where_clause.is_some());
        }
        other => panic!("expected Update, got {other:?}"),
    }
}

// =====================================================================
//  DELETE 测试（5 条）
// =====================================================================

#[test]
fn test_delete_simple() {
    let stmt = must_parse("DELETE FROM t");
    match stmt {
        Statement::Delete { table, .. } => assert_eq!(table.name, "t"),
        other => panic!("expected Delete, got {other:?}"),
    }
}

#[test]
fn test_delete_where() {
    let stmt = must_parse("DELETE FROM t WHERE id = 5");
    match stmt {
        Statement::Delete { where_clause, .. } => assert!(where_clause.is_some()),
        other => panic!("expected Delete, got {other:?}"),
    }
}

#[test]
fn test_delete_using() {
    let stmt = must_parse("DELETE FROM t USING s WHERE t.id = s.id");
    match stmt {
        Statement::Delete { using, .. } => assert_eq!(using.len(), 1),
        other => panic!("expected Delete, got {other:?}"),
    }
}

#[test]
fn test_delete_returning() {
    // sqlparser 0.53 的 RESERVED_FOR_TABLE_ALIAS 不含 RETURNING，
    // 故 `DELETE FROM t RETURNING *` 中 RETURNING 会被当作 t 的别名消费。
    // 使用显式 AS 别名 t1 即可正确解析 RETURNING 子句。
    let stmt = must_parse("DELETE FROM t AS t1 RETURNING *");
    match stmt {
        Statement::Delete {
            returning, alias, ..
        } => {
            assert!(returning.is_some());
            assert_eq!(alias, Some("t1".to_string()));
        }
        other => panic!("expected Delete, got {other:?}"),
    }
}

#[test]
fn test_delete_with_complex_where() {
    let stmt = must_parse("DELETE FROM t WHERE a > 0 AND b < 10 OR c = 'x'");
    match stmt {
        Statement::Delete { where_clause, .. } => {
            let w = where_clause.unwrap();
            assert!(matches!(
                w,
                Expr::BinaryOp {
                    op: BinaryOp::Or,
                    ..
                }
            ));
        }
        other => panic!("expected Delete, got {other:?}"),
    }
}

// =====================================================================
//  SELECT 测试（25 条）
// =====================================================================

#[test]
fn test_select_star() {
    let stmt = must_parse("SELECT * FROM t");
    match stmt {
        Statement::Select(s) => {
            assert_eq!(s.projection.len(), 1);
            assert!(matches!(s.projection[0], SelectItem::Wildcard));
            assert_eq!(s.from.len(), 1);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_columns() {
    let stmt = must_parse("SELECT a, b, c FROM t");
    match stmt {
        Statement::Select(s) => assert_eq!(s.projection.len(), 3),
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_distinct() {
    let stmt = must_parse("SELECT DISTINCT a FROM t");
    match stmt {
        Statement::Select(s) => assert!(s.distinct),
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_with_alias() {
    let stmt = must_parse("SELECT a AS x FROM t");
    match stmt {
        Statement::Select(s) => {
            if let SelectItem::ExprWithAlias { alias, .. } = &s.projection[0] {
                assert_eq!(alias, "x");
            } else {
                panic!("expected ExprWithAlias");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_where() {
    let stmt = must_parse("SELECT a FROM t WHERE a > 5");
    match stmt {
        Statement::Select(s) => assert!(s.where_clause.is_some()),
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_order_by() {
    let stmt = must_parse("SELECT a FROM t ORDER BY a");
    match stmt {
        Statement::Select(s) => assert_eq!(s.order_by.len(), 1),
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_order_by_desc() {
    let stmt = must_parse("SELECT a FROM t ORDER BY a DESC");
    match stmt {
        Statement::Select(s) => assert!(!s.order_by[0].asc),
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_limit() {
    let stmt = must_parse("SELECT a FROM t LIMIT 10");
    match stmt {
        Statement::Select(s) => assert!(s.limit.is_some()),
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_offset() {
    let stmt = must_parse("SELECT a FROM t OFFSET 5");
    match stmt {
        Statement::Select(s) => assert!(s.offset.is_some()),
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_limit_offset() {
    let stmt = must_parse("SELECT a FROM t LIMIT 10 OFFSET 5");
    match stmt {
        Statement::Select(s) => {
            assert!(s.limit.is_some());
            assert!(s.offset.is_some());
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_group_by() {
    let stmt = must_parse("SELECT a, COUNT(*) FROM t GROUP BY a");
    match stmt {
        Statement::Select(s) => assert_eq!(s.group_by.len(), 1),
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_having() {
    let stmt = must_parse("SELECT a, COUNT(*) FROM t GROUP BY a HAVING COUNT(*) > 1");
    match stmt {
        Statement::Select(s) => assert!(s.having.is_some()),
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_inner_join() {
    let stmt = must_parse("SELECT a FROM t1 INNER JOIN t2 ON t1.id = t2.id");
    match stmt {
        Statement::Select(s) => {
            assert_eq!(s.from[0].joins.len(), 1);
            assert_eq!(s.from[0].joins[0].join_type, JoinType::Inner);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_left_join() {
    let stmt = must_parse("SELECT a FROM t1 LEFT JOIN t2 ON t1.id = t2.id");
    match stmt {
        Statement::Select(s) => {
            assert_eq!(s.from[0].joins[0].join_type, JoinType::LeftOuter);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_right_join() {
    let stmt = must_parse("SELECT a FROM t1 RIGHT JOIN t2 ON t1.id = t2.id");
    match stmt {
        Statement::Select(s) => {
            assert_eq!(s.from[0].joins[0].join_type, JoinType::RightOuter);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_full_join() {
    let stmt = must_parse("SELECT a FROM t1 FULL JOIN t2 ON t1.id = t2.id");
    match stmt {
        Statement::Select(s) => {
            assert_eq!(s.from[0].joins[0].join_type, JoinType::FullOuter);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_cross_join() {
    let stmt = must_parse("SELECT a FROM t1 CROSS JOIN t2");
    match stmt {
        Statement::Select(s) => {
            assert_eq!(s.from[0].joins[0].join_type, JoinType::Cross);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_join_with_using() {
    let stmt = must_parse("SELECT a FROM t1 JOIN t2 USING (id)");
    match stmt {
        Statement::Select(s) => {
            if let JoinCondition::Using(cols) = &s.from[0].joins[0].condition {
                assert_eq!(cols, &vec!["id".to_string()]);
            } else {
                panic!("expected Using");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_join_with_natural() {
    let stmt = must_parse("SELECT a FROM t1 NATURAL JOIN t2");
    match stmt {
        Statement::Select(s) => {
            assert!(matches!(
                s.from[0].joins[0].condition,
                JoinCondition::Natural
            ));
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_qualified_columns() {
    let stmt = must_parse("SELECT t.a, t.b FROM t");
    match stmt {
        Statement::Select(s) => assert_eq!(s.projection.len(), 2),
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_subquery_in_where() {
    let stmt = must_parse("SELECT a FROM t WHERE id IN (SELECT id FROM s)");
    match stmt {
        Statement::Select(s) => assert!(s.where_clause.is_some()),
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_subquery_in_from() {
    let stmt = must_parse("SELECT a FROM (SELECT a FROM t) AS sub");
    match stmt {
        Statement::Select(s) => {
            if let TableFactor::Derived { alias, .. } = &s.from[0].relation {
                assert_eq!(alias.name, "sub");
            } else {
                panic!("expected Derived table");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_aggregate() {
    let stmt = must_parse("SELECT COUNT(*), SUM(a), AVG(a), MIN(a), MAX(a) FROM t");
    match stmt {
        Statement::Select(s) => assert_eq!(s.projection.len(), 5),
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_count_distinct() {
    let stmt = must_parse("SELECT COUNT(DISTINCT a) FROM t");
    match stmt {
        Statement::Select(s) => {
            if let SelectItem::UnnamedExpr(Expr::Function { distinct, .. }) = &s.projection[0] {
                assert!(distinct);
            } else {
                panic!("expected Function with distinct");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_complex_query() {
    let stmt = must_parse(
        "SELECT a, COUNT(*) AS cnt FROM t1 JOIN t2 ON t1.id = t2.id WHERE a > 0 GROUP BY a HAVING COUNT(*) > 1 ORDER BY cnt DESC LIMIT 10",
    );
    match stmt {
        Statement::Select(s) => {
            assert_eq!(s.projection.len(), 2);
            assert!(s.where_clause.is_some());
            assert_eq!(s.group_by.len(), 1);
            assert!(s.having.is_some());
            assert_eq!(s.order_by.len(), 1);
            assert!(s.limit.is_some());
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_select_table_alias() {
    let stmt = must_parse("SELECT * FROM t AS x");
    match stmt {
        Statement::Select(s) => {
            if let TableFactor::Table { alias, .. } = &s.from[0].relation {
                assert_eq!(alias.as_ref().unwrap().name, "x");
            } else {
                panic!("expected Table");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

// =====================================================================
//  表达式测试（20 条）
// =====================================================================

#[test]
fn test_expr_literal_int() {
    let stmt = must_parse("SELECT 42");
    match stmt {
        Statement::Select(s) => {
            if let SelectItem::UnnamedExpr(Expr::Literal(Value::Int64(n))) = &s.projection[0] {
                assert_eq!(*n, 42);
            } else {
                panic!("expected Int64 literal");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_literal_string() {
    let stmt = must_parse("SELECT 'hello'");
    match stmt {
        Statement::Select(s) => {
            assert!(matches!(
                &s.projection[0],
                SelectItem::UnnamedExpr(Expr::Literal(Value::Text(_)))
            ));
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_literal_bool() {
    let stmt = must_parse("SELECT TRUE");
    match stmt {
        Statement::Select(s) => {
            assert!(matches!(
                &s.projection[0],
                SelectItem::UnnamedExpr(Expr::Literal(Value::Bool(_)))
            ));
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_literal_null() {
    let stmt = must_parse("SELECT NULL");
    match stmt {
        Statement::Select(s) => {
            assert!(matches!(
                &s.projection[0],
                SelectItem::UnnamedExpr(Expr::Literal(Value::Null))
            ));
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_identifier() {
    let stmt = must_parse("SELECT a FROM t");
    match stmt {
        Statement::Select(s) => {
            if let SelectItem::UnnamedExpr(Expr::Identifier(parts)) = &s.projection[0] {
                assert_eq!(parts, &vec!["a".to_string()]);
            } else {
                panic!("expected Identifier");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_compound_identifier() {
    let stmt = must_parse("SELECT t.a FROM t");
    match stmt {
        Statement::Select(s) => {
            if let SelectItem::UnnamedExpr(Expr::Identifier(parts)) = &s.projection[0] {
                assert_eq!(parts, &vec!["t".to_string(), "a".to_string()]);
            } else {
                panic!("expected CompoundIdentifier");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_binary_op() {
    let stmt = must_parse("SELECT a + b FROM t");
    match stmt {
        Statement::Select(s) => {
            assert!(matches!(
                &s.projection[0],
                SelectItem::UnnamedExpr(Expr::BinaryOp {
                    op: BinaryOp::Plus,
                    ..
                })
            ));
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_unary_op() {
    let stmt = must_parse("SELECT -a FROM t");
    match stmt {
        Statement::Select(s) => {
            assert!(matches!(
                &s.projection[0],
                SelectItem::UnnamedExpr(Expr::UnaryOp {
                    op: UnaryOp::Minus,
                    ..
                })
            ));
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_function_call() {
    let stmt = must_parse("SELECT UPPER(a) FROM t");
    match stmt {
        Statement::Select(s) => {
            if let SelectItem::UnnamedExpr(Expr::Function { name, args, .. }) = &s.projection[0] {
                assert_eq!(name, "upper");
                assert_eq!(args.len(), 1);
            } else {
                panic!("expected Function");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_case_searched() {
    let stmt = must_parse("SELECT CASE WHEN a > 0 THEN 'pos' ELSE 'neg' END FROM t");
    match stmt {
        Statement::Select(s) => {
            assert!(matches!(
                &s.projection[0],
                SelectItem::UnnamedExpr(Expr::Case { .. })
            ));
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_case_simple() {
    let stmt = must_parse("SELECT CASE a WHEN 1 THEN 'one' WHEN 2 THEN 'two' END FROM t");
    match stmt {
        Statement::Select(s) => {
            if let SelectItem::UnnamedExpr(Expr::Case { when_then, .. }) = &s.projection[0] {
                assert_eq!(when_then.len(), 2);
            } else {
                panic!("expected Case");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_cast() {
    let stmt = must_parse("SELECT CAST(a AS TEXT) FROM t");
    match stmt {
        Statement::Select(s) => {
            if let SelectItem::UnnamedExpr(Expr::Cast { data_type, .. }) = &s.projection[0] {
                assert_eq!(*data_type, ColumnType::Text);
            } else {
                panic!("expected Cast");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_in_list() {
    let stmt = must_parse("SELECT a FROM t WHERE a IN (1, 2, 3)");
    match stmt {
        Statement::Select(s) => {
            if let Some(Expr::InList { list, .. }) = &s.where_clause {
                assert_eq!(list.len(), 3);
            } else {
                panic!("expected InList");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_in_subquery() {
    let stmt = must_parse("SELECT a FROM t WHERE a IN (SELECT a FROM s)");
    match stmt {
        Statement::Select(s) => {
            assert!(matches!(&s.where_clause, Some(Expr::InSubquery { .. })));
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_between() {
    let stmt = must_parse("SELECT a FROM t WHERE a BETWEEN 1 AND 10");
    match stmt {
        Statement::Select(s) => {
            assert!(matches!(&s.where_clause, Some(Expr::Between { .. })));
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_not_between() {
    let stmt = must_parse("SELECT a FROM t WHERE a NOT BETWEEN 1 AND 10");
    match stmt {
        Statement::Select(s) => {
            if let Some(Expr::Between { negated, .. }) = &s.where_clause {
                assert!(negated);
            } else {
                panic!("expected Between");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_like() {
    let stmt = must_parse("SELECT a FROM t WHERE a LIKE 'pre%'");
    match stmt {
        Statement::Select(s) => {
            assert!(matches!(&s.where_clause, Some(Expr::Like { .. })));
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_is_null() {
    let stmt = must_parse("SELECT a FROM t WHERE a IS NULL");
    match stmt {
        Statement::Select(s) => {
            if let Some(Expr::IsNull { negated, .. }) = &s.where_clause {
                assert!(!negated);
            } else {
                panic!("expected IsNull");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_is_not_null() {
    let stmt = must_parse("SELECT a FROM t WHERE a IS NOT NULL");
    match stmt {
        Statement::Select(s) => {
            if let Some(Expr::IsNull { negated, .. }) = &s.where_clause {
                assert!(negated);
            } else {
                panic!("expected IsNotNull");
            }
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_exists() {
    let stmt = must_parse("SELECT a FROM t WHERE EXISTS (SELECT 1 FROM s)");
    match stmt {
        Statement::Select(s) => {
            assert!(matches!(&s.where_clause, Some(Expr::Exists { .. })));
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_expr_tuple() {
    let stmt = must_parse("SELECT a FROM t WHERE (a, b) IN (SELECT x, y FROM s)");
    match stmt {
        Statement::Select(s) => {
            assert!(s.where_clause.is_some());
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

// =====================================================================
//  事务测试（8 条）
// =====================================================================

#[test]
fn test_begin() {
    let stmt = must_parse("BEGIN");
    assert!(matches!(stmt, Statement::Begin { .. }));
}

#[test]
fn test_begin_isolation_read_committed() {
    let stmt = must_parse("BEGIN ISOLATION LEVEL READ COMMITTED");
    match stmt {
        Statement::Begin { isolation, .. } => {
            assert_eq!(isolation, Some(TransactionIsolation::ReadCommitted));
        }
        other => panic!("expected Begin, got {other:?}"),
    }
}

#[test]
fn test_begin_read_only() {
    let stmt = must_parse("BEGIN READ ONLY");
    match stmt {
        Statement::Begin { access, .. } => {
            assert_eq!(access, Some(TransactionAccess::ReadOnly));
        }
        other => panic!("expected Begin, got {other:?}"),
    }
}

#[test]
fn test_commit() {
    let stmt = must_parse("COMMIT");
    assert!(matches!(stmt, Statement::Commit));
}

#[test]
fn test_rollback() {
    let stmt = must_parse("ROLLBACK");
    assert!(matches!(stmt, Statement::Rollback { savepoint: None }));
}

#[test]
fn test_rollback_to_savepoint() {
    let stmt = must_parse("ROLLBACK TO SAVEPOINT sp1");
    match stmt {
        Statement::Rollback { savepoint } => {
            assert_eq!(savepoint.as_deref(), Some("sp1"));
        }
        other => panic!("expected Rollback, got {other:?}"),
    }
}

#[test]
fn test_savepoint() {
    let stmt = must_parse("SAVEPOINT sp1");
    match stmt {
        Statement::Savepoint(name) => assert_eq!(name, "sp1"),
        other => panic!("expected Savepoint, got {other:?}"),
    }
}

#[test]
fn test_release_savepoint() {
    let stmt = must_parse("RELEASE SAVEPOINT sp1");
    match stmt {
        Statement::ReleaseSavepoint(name) => assert_eq!(name, "sp1"),
        other => panic!("expected ReleaseSavepoint, got {other:?}"),
    }
}

#[test]
fn test_set_transaction() {
    let stmt = must_parse("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE");
    match stmt {
        Statement::SetTransaction { isolation, .. } => {
            assert_eq!(isolation, Some(TransactionIsolation::Serializable));
        }
        other => panic!("expected SetTransaction, got {other:?}"),
    }
}

// =====================================================================
//  EXPLAIN 测试（4 条）
// =====================================================================

#[test]
fn test_explain() {
    let stmt = must_parse("EXPLAIN SELECT * FROM t");
    match stmt {
        Statement::Explain {
            analyze, verbose, ..
        } => {
            assert!(!analyze);
            assert!(!verbose);
        }
        other => panic!("expected Explain, got {other:?}"),
    }
}

#[test]
fn test_explain_analyze() {
    let stmt = must_parse("EXPLAIN ANALYZE SELECT * FROM t");
    match stmt {
        Statement::Explain { analyze, .. } => assert!(analyze),
        other => panic!("expected Explain, got {other:?}"),
    }
}

#[test]
fn test_explain_verbose() {
    let stmt = must_parse("EXPLAIN VERBOSE SELECT * FROM t");
    match stmt {
        Statement::Explain { verbose, .. } => assert!(verbose),
        other => panic!("expected Explain, got {other:?}"),
    }
}

#[test]
fn test_explain_analyze_verbose() {
    let stmt = must_parse("EXPLAIN ANALYZE VERBOSE SELECT * FROM t");
    match stmt {
        Statement::Explain {
            analyze, verbose, ..
        } => {
            assert!(analyze);
            assert!(verbose);
        }
        other => panic!("expected Explain, got {other:?}"),
    }
}

// =====================================================================
//  多语句与边界测试（剩余条数补足）
// =====================================================================

#[test]
fn test_parse_multiple_statements() {
    let stmts = parse_sql("SELECT 1; SELECT 2;").unwrap();
    assert_eq!(stmts.len(), 2);
}

#[test]
fn test_parse_empty_input() {
    let result = parse_sql("");
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[test]
fn test_parse_unsupported_statement() {
    // 当前 SzRSQL 已支持 TRUNCATE TABLE（Phase 6.x），改为测试真正不支持的语句
    // FLASHBACK 已通过预处理支持，这里使用完全无效的 DBLINK 语句
    let result = parse_one("CREATE DATABASE LINK link1 CONNECT TO user IDENTIFIED BY pass USING 'db'");
    assert!(result.is_err(), "expected CREATE DATABASE LINK to be unsupported");
}

#[test]
fn test_parse_invalid_sql() {
    must_fail("SELECT FROM");
}

#[test]
fn test_select_qualified_wildcard() {
    let stmt = must_parse("SELECT t.* FROM t");
    match stmt {
        Statement::Select(s) => {
            assert!(matches!(&s.projection[0], SelectItem::QualifiedWildcard(_)));
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_create_table_with_json_type() {
    let stmt = must_parse("CREATE TABLE t (data JSON)");
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert_eq!(columns[0].data_type, ColumnType::Json);
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_with_bytea_type() {
    let stmt = must_parse("CREATE TABLE t (data BYTEA)");
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert_eq!(columns[0].data_type, ColumnType::Blob);
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_create_table_with_array_type() {
    let stmt = must_parse("CREATE TABLE t (arr INT[])");
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert!(matches!(columns[0].data_type, ColumnType::Array(_)));
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_select_with_multiple_joins() {
    let stmt = must_parse("SELECT a FROM t1 JOIN t2 ON t1.id = t2.id JOIN t3 ON t2.id = t3.id");
    match stmt {
        Statement::Select(s) => {
            assert_eq!(s.from[0].joins.len(), 2);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

// =====================================================================
//  Phase TDengine-P2: COMMENT ON 解析测试
// =====================================================================

#[test]
fn test_parse_comment_on_table() {
    let stmts = parse_sql("COMMENT ON TABLE products IS '商品主表'").unwrap();
    assert_eq!(stmts.len(), 1);
    match &stmts[0] {
        Statement::Comment {
            object_type,
            object_name,
            column_name,
            comment,
        } => {
            assert_eq!(*object_type, CommentObjectType::Table);
            assert_eq!(object_name.name, "products");
            assert!(column_name.is_none());
            assert_eq!(comment.as_ref().unwrap(), "商品主表");
        }
        other => panic!("expected Comment, got {other:?}"),
    }
}

#[test]
fn test_parse_comment_on_column() {
    let stmts = parse_sql("COMMENT ON COLUMN products.price IS '商品零售价'").unwrap();
    assert_eq!(stmts.len(), 1);
    match &stmts[0] {
        Statement::Comment {
            object_type,
            object_name,
            column_name,
            comment,
        } => {
            assert_eq!(*object_type, CommentObjectType::Column);
            assert_eq!(object_name.name, "products");
            assert_eq!(column_name.as_ref().unwrap(), "price");
            assert_eq!(comment.as_ref().unwrap(), "商品零售价");
        }
        other => panic!("expected Comment, got {other:?}"),
    }
}

#[test]
fn test_parse_comment_on_null() {
    let stmts = parse_sql("COMMENT ON TABLE products IS NULL").unwrap();
    match &stmts[0] {
        Statement::Comment { comment, .. } => {
            assert!(comment.is_none());
        }
        _ => panic!("expected Comment"),
    }
}

// =====================================================================
//  Navicat 兼容：无值 SET 语句归一化测试
// =====================================================================

#[test]
fn test_parse_set_autocommit_no_value() {
    // Navicat 连接时发送 "SET AUTOCOMMIT"（无值），归一化后应能正常解析
    let stmts = parse_sql("SET AUTOCOMMIT").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_lowercase_no_value() {
    let stmts = parse_sql("set autocommit").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_with_value_unchanged() {
    // 有值的 SET 语句应正常解析，不受归一化影响
    let stmts = parse_sql("SET AUTOCOMMIT = 1").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_names_unchanged() {
    // SET NAMES 是 MySQL 语法，应走 MySqlDialect 路径
    let stmts = parse_sql("SET NAMES utf8mb4").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_search_path_to() {
    let stmts = parse_sql("SET search_path TO public").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_multiple_set_statements() {
    // 多语句混合：无值 + 有值
    let stmts = parse_sql("SET AUTOCOMMIT; SET extra_float_digits = 3").unwrap();
    assert_eq!(stmts.len(), 2);
}

#[test]
fn test_parse_select_not_affected() {
    // 非 SET 语句不受归一化影响
    let stmts = parse_sql("SELECT 1").unwrap();
    assert_eq!(stmts.len(), 1);
}

// =====================================================================
// Navicat 兼容性测试：各种 SET 语句变体
// =====================================================================

#[test]
fn test_parse_set_autocommit_on() {
    // Navicat 连接时发送 SET AUTOCOMMIT ON，归一化为 SET AUTOCOMMIT = 'on'
    let stmts = parse_sql("SET AUTOCOMMIT ON").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_autocommit_off() {
    let stmts = parse_sql("SET AUTOCOMMIT OFF").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_standard_conforming_strings_on() {
    let stmts = parse_sql("SET STANDARD_CONFORMING_STRINGS ON").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_standard_conforming_strings_off() {
    let stmts = parse_sql("SET STANDARD_CONFORMING_STRINGS OFF").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_variable_equal_no_value() {
    // SET variable = （等号后无值）→ SET variable = 1
    let stmts = parse_sql("SET AUTOCOMMIT =").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_variable_to_no_value() {
    // SET variable TO （TO 后无值）→ SET variable = 1
    let stmts = parse_sql("SET AUTOCOMMIT TO").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_variable_equal_with_space_no_value() {
    // 等号后只有空格，无值
    let stmts = parse_sql("SET extra_float_digits = ").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_variable_to_with_space_no_value() {
    let stmts = parse_sql("SET search_path TO ").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_character_set() {
    // MySQL 特有语法：SET CHARACTER SET charset
    let stmts = parse_sql("SET CHARACTER SET utf8mb4").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_session_authorization_default() {
    // PG 会话授权语法：SET SESSION AUTHORIZATION DEFAULT
    let stmts = parse_sql("SET SESSION AUTHORIZATION DEFAULT").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_session_authorization_user() {
    let stmts = parse_sql("SET SESSION AUTHORIZATION postgres").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_time_zone_default() {
    // PG 时区语法：SET TIME ZONE DEFAULT
    let stmts = parse_sql("SET TIME ZONE DEFAULT").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_time_zone_utc() {
    let stmts = parse_sql("SET TIME ZONE 'UTC'").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_time_zone_offset() {
    // SET TIME ZONE +08:00（PG interval 语法）
    let stmts = parse_sql("SET TIME ZONE 'Asia/Shanghai'").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_role_default() {
    // SET ROLE DEFAULT — Navicat 连接时常见
    let stmts = parse_sql("SET ROLE DEFAULT").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_role_none() {
    let stmts = parse_sql("SET ROLE NONE").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_transaction_isolation() {
    // SET TRANSACTION ISOLATION LEVEL READ COMMITTED
    let stmts = parse_sql("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_session_characteristics() {
    let stmts = parse_sql(
        "SET SESSION CHARACTERISTICS AS TRANSACTION ISOLATION LEVEL READ COMMITTED",
    )
    .unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_empty() {
    // 完全无变量名：SET → SET autocommit = 1
    let stmts = parse_sql("SET").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_with_trailing_space() {
    let stmts = parse_sql("SET ").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_with_semicolon_only() {
    // SET; → SET autocommit = 1;
    let stmts = parse_sql("SET ;").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_multiple_set_statements_with_on_off() {
    // 多语句混合：ON 形式 + 有值形式
    let stmts = parse_sql("SET AUTOCOMMIT ON; SET extra_float_digits = 3").unwrap();
    assert_eq!(stmts.len(), 2);
}

#[test]
fn test_parse_set_names_with_charset_not_affected() {
    // SET NAMES 应该走 sqlparser 的 SetNames 路径，不被归一化影响
    let stmts = parse_sql("SET NAMES 'UTF8'").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_with_dotted_variable() {
    // 带点的变量名：SET pg_catalog.timezone = 'UTC'
    let stmts = parse_sql("SET pg_catalog.timezone = 'UTC'").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_local_statement_timeout() {
    let stmts = parse_sql("SET LOCAL statement_timeout = 0").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_session_statement_timeout() {
    let stmts = parse_sql("SET SESSION statement_timeout = 0").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_with_newline_after_equal() {
    // 等号后跟换行符
    let stmts = parse_sql("SET AUTOCOMMIT =\n").unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn test_parse_set_with_newline_after_to() {
    let stmts = parse_sql("SET AUTOCOMMIT TO\n").unwrap();
    assert_eq!(stmts.len(), 1);
}

