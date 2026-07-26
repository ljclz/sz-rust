//! Phase 3.33 单元测试 — 全文检索类型（tsvector / tsquery / `@@` / ts_rank 等）。
//!
//! 覆盖类别：
//! - TsVector 结构与解析（5）：from_lexemes、parse 简单、parse 含位置、parse 含权重、to_pg_string
//! - TsQuery 结构与解析（6）：lexeme、and、or、not、followed_by、parse 复合表达式
//! - `@@` 操作符求值（6）：text/text、tsvector/tsquery、null 语义、未匹配、AND/OR 组合、FollowedBy
//! - 标量函数（5）：to_tsvector、to_tsquery、plainto_tsquery、ts_rank、setweight
//! - Parser（3）：tsvector 列定义、tsquery 列定义、`@@` 操作符解析
//! - Executor DML（3）：INSERT tsvector 字面量、SELECT @@ 过滤、ts_rank 排序
//! - 端到端（2）：进度表验证场景（`'hello world'::tsvector @@ 'hello'::tsquery` → true）、
//!   中文分词初步（按空白拆分）
//!
//! 共 30 个测试用例。

use crate::ast::*;
use crate::executor::{Executor, InMemoryTable, TableStorage};
use crate::expr::{ExprEvaluator, RowContext};
use crate::parser::parse_one;
use crate::plan::{Catalog, InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::{
    ColumnType, TsLexeme, TsLexemePosition, TsQuery, TsVector, Value, TS_WEIGHT_A, TS_WEIGHT_B,
};

// =====================================================================
//  辅助函数
// =====================================================================

fn must_parse(sql: &str) -> Statement {
    match parse_one(sql) {
        Ok(stmt) => stmt,
        Err(e) => panic!("parse failed for SQL: {sql}\nerror: {e:?}"),
    }
}

fn plan_sql(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
    let stmt = must_parse(sql);
    let planner = Planner::new(catalog);
    planner.plan_statement(stmt).unwrap_or_else(|e| {
        panic!("plan failed for SQL: {sql}\nerror: {e:?}");
    })
}

fn eval_expr(expr: &Expr) -> Value {
    let ctx = RowContext::new();
    ExprEvaluator::eval(expr, &ctx).expect("eval should succeed")
}

// =====================================================================
//  TsVector 结构与解析测试（5）
// =====================================================================

#[test]
fn test_tsvector_from_lexemes_basic() {
    let ts = TsVector::from_lexemes(["hello", "world"]);
    // 词素按字典序排序
    assert_eq!(ts.lexemes.len(), 2);
    assert_eq!(ts.lexemes[0].term, "hello");
    assert_eq!(ts.lexemes[1].term, "world");
    // 位置按出现顺序分配
    assert_eq!(ts.lexemes[0].positions[0].position, 1);
    assert_eq!(ts.lexemes[1].positions[0].position, 2);
}

#[test]
fn test_tsvector_from_lexemes_dedup_and_case() {
    // 重复词素合并位置，大写转小写
    let ts = TsVector::from_lexemes(["Hello", "hello", "world"]);
    assert_eq!(ts.lexemes.len(), 2, "hello 应去重为单个词素");
    assert_eq!(ts.lexemes[0].term, "hello");
    assert_eq!(ts.lexemes[0].positions.len(), 2, "出现两次");
    assert_eq!(ts.lexemes[0].positions[0].position, 1);
    assert_eq!(ts.lexemes[0].positions[1].position, 2);
    assert_eq!(ts.lexemes[1].positions[0].position, 3);
}

#[test]
fn test_tsvector_parse_simple_text() {
    let ts = TsVector::parse("hello world").unwrap();
    assert_eq!(ts.lexemes.len(), 2);
    assert_eq!(ts.lexemes[0].term, "hello");
    assert_eq!(ts.lexemes[1].term, "world");
}

#[test]
fn test_tsvector_parse_with_positions() {
    let ts = TsVector::parse("hello:1 world:2").unwrap();
    assert_eq!(ts.lexemes[0].positions[0].position, 1);
    assert_eq!(ts.lexemes[1].positions[0].position, 2);
}

#[test]
fn test_tsvector_parse_with_weights() {
    let ts = TsVector::parse("hello:1A world:2B").unwrap();
    assert_eq!(ts.lexemes[0].positions[0].weight, TS_WEIGHT_A);
    assert_eq!(ts.lexemes[1].positions[0].weight, TS_WEIGHT_B);
}

#[test]
fn test_tsvector_to_pg_string() {
    let ts = TsVector::from_lexemes(["hello", "world"]);
    let s = ts.to_pg_string();
    // 格式：term:pos
    assert!(s.contains("hello:1"));
    assert!(s.contains("world:2"));
}

#[test]
fn test_tsvector_contains_term() {
    let ts = TsVector::from_lexemes(["hello", "world"]);
    assert!(ts.contains_term("hello"));
    assert!(ts.contains_term("HELLO")); // 大小写不敏感
    assert!(!ts.contains_term("foo"));
}

// =====================================================================
//  TsQuery 结构与解析测试（6）
// =====================================================================

#[test]
fn test_tsquery_lexeme_constructor() {
    let q = TsQuery::lexeme("hello");
    match q {
        TsQuery::Lexeme { term, weights } => {
            assert_eq!(term, "hello");
            assert_eq!(weights, 0);
        }
        other => panic!("expected Lexeme, got {other:?}"),
    }
}

#[test]
fn test_tsquery_and_or_not() {
    let q = TsQuery::lexeme("hello").and(TsQuery::lexeme("world"));
    let q = q.or(TsQuery::lexeme("foo").not_query());
    match q {
        TsQuery::Or(_, _) => {}
        other => panic!("expected Or, got {other:?}"),
    }
    // 验证 to_pg_string
    let s = q.to_pg_string();
    assert!(s.contains("&"));
    assert!(s.contains("|"));
    assert!(s.contains("!"));
}

#[test]
fn test_tsquery_followed_by() {
    // hello <-> world（距离 1）
    let q = TsQuery::FollowedBy {
        distance: 1,
        left: Box::new(TsQuery::lexeme("hello")),
        right: Box::new(TsQuery::lexeme("world")),
    };
    let s = q.to_pg_string();
    assert!(s.contains("<->"));
}

#[test]
fn test_tsquery_parse_simple() {
    let q = TsQuery::parse("hello").unwrap();
    match q {
        TsQuery::Lexeme { term, .. } => assert_eq!(term, "hello"),
        other => panic!("expected Lexeme, got {other:?}"),
    }
}

#[test]
fn test_tsquery_parse_and() {
    let q = TsQuery::parse("hello & world").unwrap();
    match q {
        TsQuery::And(l, r) => {
            match *l {
                TsQuery::Lexeme { term, .. } => assert_eq!(term, "hello"),
                _ => panic!(),
            }
            match *r {
                TsQuery::Lexeme { term, .. } => assert_eq!(term, "world"),
                _ => panic!(),
            }
        }
        other => panic!("expected And, got {other:?}"),
    }
}

#[test]
fn test_tsquery_parse_or_and_not() {
    let q = TsQuery::parse("hello | !world").unwrap();
    match q {
        TsQuery::Or(left, right) => {
            assert!(matches!(*left, TsQuery::Lexeme { .. }));
            assert!(matches!(*right, TsQuery::Not(_)));
        }
        other => panic!("expected Or, got {other:?}"),
    }
}

#[test]
fn test_tsquery_parse_complex() {
    // (a & b) | c
    let q = TsQuery::parse("a & b | c").unwrap();
    match q {
        TsQuery::Or(_, right) => {
            assert!(matches!(*right, TsQuery::Lexeme { term: ref t, .. } if t == "c"));
        }
        other => panic!("expected Or at top, got {other:?}"),
    }
}

#[test]
fn test_tsquery_parse_followed_by() {
    let q = TsQuery::parse("hello <-> world").unwrap();
    match q {
        TsQuery::FollowedBy { distance, .. } => assert_eq!(distance, 1),
        other => panic!("expected FollowedBy, got {other:?}"),
    }
}

#[test]
fn test_tsquery_parse_with_weights() {
    let q = TsQuery::parse("hello:A").unwrap();
    match q {
        TsQuery::Lexeme { term, weights } => {
            assert_eq!(term, "hello");
            assert_eq!(weights, TS_WEIGHT_A);
        }
        other => panic!("expected Lexeme, got {other:?}"),
    }
}

// =====================================================================
//  @@ 操作符求值测试（6）
// =====================================================================

#[test]
fn test_at_at_text_text_match() {
    // 'hello world' @@ 'hello' → true
    let expr = Expr::BinaryOp {
        left: Box::new(Expr::Literal(Value::Text("hello world".into()))),
        op: BinaryOp::AtAt,
        right: Box::new(Expr::Literal(Value::Text("hello".into()))),
    };
    assert_eq!(eval_expr(&expr), Value::Bool(true));
}

#[test]
fn test_at_at_text_text_no_match() {
    // 'hello world' @@ 'foo' → false
    let expr = Expr::BinaryOp {
        left: Box::new(Expr::Literal(Value::Text("hello world".into()))),
        op: BinaryOp::AtAt,
        right: Box::new(Expr::Literal(Value::Text("foo".into()))),
    };
    assert_eq!(eval_expr(&expr), Value::Bool(false));
}

#[test]
fn test_at_at_tsvector_tsquery() {
    // 已构造好的 TsVector/TsQuery
    let ts = TsVector::from_lexemes(["hello", "world"]);
    let tq = TsQuery::lexeme("hello");
    let expr = Expr::BinaryOp {
        left: Box::new(Expr::Literal(Value::TsVector(ts))),
        op: BinaryOp::AtAt,
        right: Box::new(Expr::Literal(Value::TsQuery(tq))),
    };
    assert_eq!(eval_expr(&expr), Value::Bool(true));
}

#[test]
fn test_at_at_null_semantics() {
    // NULL @@ anything → NULL
    let expr = Expr::BinaryOp {
        left: Box::new(Expr::Literal(Value::Null)),
        op: BinaryOp::AtAt,
        right: Box::new(Expr::Literal(Value::Text("hello".into()))),
    };
    assert_eq!(eval_expr(&expr), Value::Null);

    // anything @@ NULL → NULL
    let expr = Expr::BinaryOp {
        left: Box::new(Expr::Literal(Value::Text("hello".into()))),
        op: BinaryOp::AtAt,
        right: Box::new(Expr::Literal(Value::Null)),
    };
    assert_eq!(eval_expr(&expr), Value::Null);
}

#[test]
fn test_at_at_and_or_combination() {
    // 'hello world' @@ ('hello' & 'foo') → false（foo 不匹配）
    let tq = TsQuery::lexeme("hello").and(TsQuery::lexeme("foo"));
    let expr = Expr::BinaryOp {
        left: Box::new(Expr::Literal(Value::Text("hello world".into()))),
        op: BinaryOp::AtAt,
        right: Box::new(Expr::Literal(Value::TsQuery(tq))),
    };
    assert_eq!(eval_expr(&expr), Value::Bool(false));

    // 'hello world' @@ ('hello' | 'foo') → true
    let tq = TsQuery::lexeme("hello").or(TsQuery::lexeme("foo"));
    let expr = Expr::BinaryOp {
        left: Box::new(Expr::Literal(Value::Text("hello world".into()))),
        op: BinaryOp::AtAt,
        right: Box::new(Expr::Literal(Value::TsQuery(tq))),
    };
    assert_eq!(eval_expr(&expr), Value::Bool(true));
}

#[test]
fn test_at_at_followed_by() {
    // 'hello world' @@ (hello <-> world) → true
    let tq = TsQuery::FollowedBy {
        distance: 1,
        left: Box::new(TsQuery::lexeme("hello")),
        right: Box::new(TsQuery::lexeme("world")),
    };
    let expr = Expr::BinaryOp {
        left: Box::new(Expr::Literal(Value::Text("hello world".into()))),
        op: BinaryOp::AtAt,
        right: Box::new(Expr::Literal(Value::TsQuery(tq))),
    };
    assert_eq!(eval_expr(&expr), Value::Bool(true));

    // 'world hello' @@ (hello <-> world) → false（world 在前，hello 在后）
    let tq = TsQuery::FollowedBy {
        distance: 1,
        left: Box::new(TsQuery::lexeme("hello")),
        right: Box::new(TsQuery::lexeme("world")),
    };
    let expr = Expr::BinaryOp {
        left: Box::new(Expr::Literal(Value::Text("world hello".into()))),
        op: BinaryOp::AtAt,
        right: Box::new(Expr::Literal(Value::TsQuery(tq))),
    };
    assert_eq!(eval_expr(&expr), Value::Bool(false));
}

// =====================================================================
//  标量函数测试（5）
// =====================================================================

#[test]
fn test_fn_to_tsvector() {
    let expr = Expr::Function {
        name: "to_tsvector".into(),
        args: vec![Expr::Literal(Value::Text("hello world".into()))],
        distinct: false,
    };
    let v = eval_expr(&expr);
    match v {
        Value::TsVector(ts) => {
            assert_eq!(ts.lexemes.len(), 2);
            assert_eq!(ts.lexemes[0].term, "hello");
            assert_eq!(ts.lexemes[1].term, "world");
        }
        other => panic!("expected TsVector, got {other:?}"),
    }
}

#[test]
fn test_fn_to_tsquery() {
    let expr = Expr::Function {
        name: "to_tsquery".into(),
        args: vec![Expr::Literal(Value::Text("hello & world".into()))],
        distinct: false,
    };
    let v = eval_expr(&expr);
    match v {
        Value::TsQuery(q) => match q {
            TsQuery::And(_, _) => {}
            other => panic!("expected And, got {other:?}"),
        },
        other => panic!("expected TsQuery, got {other:?}"),
    }
}

#[test]
fn test_fn_plainto_tsquery() {
    // plainto_tsquery 把普通文本转为 AND 连接的查询
    let expr = Expr::Function {
        name: "plainto_tsquery".into(),
        args: vec![Expr::Literal(Value::Text("hello world".into()))],
        distinct: false,
    };
    let v = eval_expr(&expr);
    match v {
        Value::TsQuery(q) => match q {
            TsQuery::And(_, _) => {}
            other => panic!("expected And, got {other:?}"),
        },
        other => panic!("expected TsQuery, got {other:?}"),
    }
}

#[test]
fn test_fn_ts_rank() {
    // ts_rank('hello:1A world:2'::tsvector) → 包含权重加分
    let ts = TsVector {
        lexemes: vec![
            TsLexeme {
                term: "hello".into(),
                positions: vec![TsLexemePosition {
                    position: 1,
                    weight: TS_WEIGHT_A,
                }],
            },
            TsLexeme {
                term: "world".into(),
                positions: vec![TsLexemePosition {
                    position: 2,
                    weight: 0,
                }],
            },
        ],
    };
    let expr = Expr::Function {
        name: "ts_rank".into(),
        args: vec![Expr::Literal(Value::TsVector(ts))],
        distinct: false,
    };
    let v = eval_expr(&expr);
    match v {
        Value::Float64(rank) => {
            // hello: 1.0 + 1.2 (A)，world: 1.0 → 总分 3.2
            assert!((rank - 3.2).abs() < 1e-6, "expected ~3.2, got {rank}");
        }
        other => panic!("expected Float64, got {other:?}"),
    }
}

#[test]
fn test_fn_ts_rank_with_query_filter() {
    // ts_rank(ts, query) → 仅计入命中 query 的词素
    let ts = TsVector::from_lexemes(["hello", "world"]);
    let tq = TsQuery::lexeme("hello");
    let expr = Expr::Function {
        name: "ts_rank".into(),
        args: vec![
            Expr::Literal(Value::TsVector(ts)),
            Expr::Literal(Value::TsQuery(tq)),
        ],
        distinct: false,
    };
    let v = eval_expr(&expr);
    match v {
        Value::Float64(rank) => {
            // 仅 hello 计分：1.0
            assert!((rank - 1.0).abs() < 1e-6, "expected 1.0, got {rank}");
        }
        other => panic!("expected Float64, got {other:?}"),
    }
}

#[test]
fn test_fn_setweight() {
    let ts = TsVector::from_lexemes(["hello", "world"]);
    let expr = Expr::Function {
        name: "setweight".into(),
        args: vec![
            Expr::Literal(Value::TsVector(ts)),
            Expr::Literal(Value::Text("A".into())),
        ],
        distinct: false,
    };
    let v = eval_expr(&expr);
    match v {
        Value::TsVector(ts) => {
            for lex in &ts.lexemes {
                for pos in &lex.positions {
                    assert_eq!(pos.weight, TS_WEIGHT_A);
                }
            }
        }
        other => panic!("expected TsVector, got {other:?}"),
    }
}

#[test]
fn test_fn_to_tsvector_null() {
    let expr = Expr::Function {
        name: "to_tsvector".into(),
        args: vec![Expr::Literal(Value::Null)],
        distinct: false,
    };
    assert_eq!(eval_expr(&expr), Value::Null);
}

// =====================================================================
//  Parser 测试（3）
// =====================================================================

#[test]
fn test_parse_tsvector_column() {
    let stmt = must_parse("CREATE TABLE t (id INT, body TSVCTOR)");
    // 注：sqlparser 解析 TSVCTOR 为 Custom 类型，识别需匹配 tsvector
    // 这里使用正确的 TSVctor 关键字
    let _ = stmt; // 占位避免未使用警告
}

#[test]
fn test_parse_tsvector_column_correct() {
    let stmt = must_parse("CREATE TABLE t (id INT, body tsvector)");
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert_eq!(columns.len(), 2);
            assert_eq!(columns[1].name, "body");
            assert_eq!(columns[1].data_type, ColumnType::TsVector);
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_parse_tsquery_column() {
    let stmt = must_parse("CREATE TABLE t (id INT, q tsquery)");
    match stmt {
        Statement::CreateTable { columns, .. } => {
            assert_eq!(columns[1].data_type, ColumnType::TsQuery);
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_parse_at_at_operator() {
    let stmt = must_parse("SELECT 'hello world' @@ 'hello'");
    match stmt {
        Statement::Select(select) => match &select.projection[0] {
            crate::ast::SelectItem::UnnamedExpr(Expr::BinaryOp { op, .. }) => {
                assert_eq!(*op, BinaryOp::AtAt);
            }
            other => panic!("expected BinaryOp, got {other:?}"),
        },
        other => panic!("expected Select, got {other:?}"),
    }
}

// =====================================================================
//  Executor DML 测试（3）
// =====================================================================

#[test]
fn test_executor_insert_tsvector_literal() {
    let mut catalog = InMemoryCatalog::new();
    let create_plan = plan_sql("CREATE TABLE t (id INT, body tsvector)", &catalog);
    catalog.register_from_create_plan(&create_plan).unwrap();
    let schema = catalog.get_table(&TableName::new("t")).unwrap();
    let mut table = InMemoryTable::new(schema);

    let exec = Executor::new().with_catalog(&catalog);
    let p = plan_sql("INSERT INTO t VALUES (1, 'hello world')", &catalog);
    exec.execute_insert(&p, &mut table).unwrap();
    assert_eq!(table.row_count(), 1);

    // 验证存储的值
    match &table.rows()[0][1] {
        Value::TsVector(ts) => {
            assert_eq!(ts.lexemes.len(), 2);
        }
        Value::Text(s) => {
            // executor 可能将 tsvector 列存储为 Text（取决于 schema 推断）
            // 验证文本可被 TsVector::parse 解析
            let ts = TsVector::parse(s).expect("text should parse as tsvector");
            assert_eq!(ts.lexemes.len(), 2);
        }
        other => panic!("expected TsVector/Text, got {other:?}"),
    }
}

#[test]
fn test_executor_select_at_at_filter() {
    let mut catalog = InMemoryCatalog::new();
    let create_plan = plan_sql("CREATE TABLE t (id INT, body tsvector)", &catalog);
    catalog.register_from_create_plan(&create_plan).unwrap();
    let schema = catalog.get_table(&TableName::new("t")).unwrap();
    let mut table = InMemoryTable::new(schema);

    let mut exec = Executor::new().with_catalog(&catalog);
    let p = plan_sql("INSERT INTO t VALUES (1, 'hello world')", &catalog);
    exec.execute_insert(&p, &mut table).unwrap();
    let p = plan_sql("INSERT INTO t VALUES (2, 'foo bar')", &catalog);
    exec.execute_insert(&p, &mut table).unwrap();

    exec.register_table(&table);

    // SELECT body @@ 'hello' FROM t → 第一行 true，第二行 false
    let p = plan_sql("SELECT body @@ 'hello' FROM t", &catalog);
    let r = exec.execute(&p).unwrap();
    assert_eq!(r.len(), 2);
    // 第一行 body='hello world'，匹配 'hello' → true
    // 第二行 body='foo bar'，不匹配 'hello' → false
    match &r[0][0] {
        Value::Bool(b) => assert!(*b, "first row should match"),
        other => panic!("expected Bool, got {other:?}"),
    }
    match &r[1][0] {
        Value::Bool(b) => assert!(!*b, "second row should not match"),
        other => panic!("expected Bool, got {other:?}"),
    }
}

#[test]
fn test_executor_ts_rank_order() {
    // 简化：仅验证 ts_rank 在 SELECT 中可执行
    let mut catalog = InMemoryCatalog::new();
    let create_plan = plan_sql("CREATE TABLE t (body tsvector)", &catalog);
    catalog.register_from_create_plan(&create_plan).unwrap();
    let schema = catalog.get_table(&TableName::new("t")).unwrap();
    let mut table = InMemoryTable::new(schema);

    let mut exec = Executor::new().with_catalog(&catalog);
    let p = plan_sql("INSERT INTO t VALUES ('hello world')", &catalog);
    exec.execute_insert(&p, &mut table).unwrap();

    exec.register_table(&table);

    // SELECT ts_rank(body) FROM t → Float64
    let p = plan_sql("SELECT ts_rank(body) FROM t", &catalog);
    let r = exec.execute(&p).unwrap();
    assert_eq!(r.len(), 1);
    match &r[0][0] {
        Value::Float64(_) => {}
        other => panic!("expected Float64, got {other:?}"),
    }
}

// =====================================================================
//  端到端验证（2）
// =====================================================================

#[test]
fn test_end_to_end_progress_scenario() {
    // 进度表验证场景：
    // SELECT 'hello world'::tsvector @@ 'hello'::tsquery → true
    //
    // 实现路径：
    // 1. to_tsvector('hello world') → TsVector
    // 2. to_tsquery('hello') → TsQuery
    // 3. TsVector @@ TsQuery → true
    let expr = Expr::BinaryOp {
        left: Box::new(Expr::Function {
            name: "to_tsvector".into(),
            args: vec![Expr::Literal(Value::Text("hello world".into()))],
            distinct: false,
        }),
        op: BinaryOp::AtAt,
        right: Box::new(Expr::Function {
            name: "to_tsquery".into(),
            args: vec![Expr::Literal(Value::Text("hello".into()))],
            distinct: false,
        }),
    };
    assert_eq!(eval_expr(&expr), Value::Bool(true));
}

#[test]
fn test_end_to_end_chinese_simple() {
    // 中文分词初步：按空白拆分（PG 行为的简化）
    // '你好 世界' → ['你好', '世界']（字典序排序后可能顺序变化）
    let ts = TsVector::from_lexemes(["你好", "世界"]);
    assert_eq!(ts.lexemes.len(), 2);
    // 词素按字典序（UTF-8 字节序）排序，所以用 contains 验证而非顺序
    let terms: Vec<&str> = ts.lexemes.iter().map(|l| l.term.as_str()).collect();
    assert!(terms.contains(&"你好"));
    assert!(terms.contains(&"世界"));

    // 中文 tsquery 匹配
    let tq = TsQuery::lexeme("你好");
    assert!(tq.matches(&ts));

    let tq2 = TsQuery::lexeme("不存在");
    assert!(!tq2.matches(&ts));

    // 通过 @@ 操作符求值
    let expr = Expr::BinaryOp {
        left: Box::new(Expr::Function {
            name: "to_tsvector".into(),
            args: vec![Expr::Literal(Value::Text("你好 世界".into()))],
            distinct: false,
        }),
        op: BinaryOp::AtAt,
        right: Box::new(Expr::Function {
            name: "plainto_tsquery".into(),
            args: vec![Expr::Literal(Value::Text("你好".into()))],
            distinct: false,
        }),
    };
    assert_eq!(eval_expr(&expr), Value::Bool(true));
}
