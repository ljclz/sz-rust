//! Phase 3.3 表达式求值单元测试 — 共 100 个用例。
//!
//! 覆盖类别：
//! - 算术运算（15）：加减乘除、模、混合类型、字符串拼接
//! - 比较运算（12）：=、<>、<、<=、>、>=、跨类型、文本字典序
//! - 逻辑运算（10）：AND/OR/NOT 三值逻辑、短路求值
//! - 位运算（6）：&、|、^、<<、>>、~
//! - 函数（25）：upper/lower/length/abs/round/ceil/floor/coalesce/nullif/concat/trim/replace/substring/now
//! - CASE（8）：简单 CASE、搜索 CASE、ELSE、NULL 处理
//! - CAST（8）：类型转换、失败处理、NULL 保持
//! - IN/BETWEEN/LIKE/IS NULL（10）
//! - 边界与错误（6）：除零、溢出、空 IN、空标识符

// 测试中使用的 3.14 等值并非数学常数 PI，仅作为普通测试输入
#![allow(clippy::approx_constant)]

use super::expr::{EvalError, ExprEvaluator, RowContext};
use crate::ast::{BinaryOp, Expr, UnaryOp};
use szrsql_types::value::{ColumnType, Value, VectorValue};

// =====================================================================
//  辅助函数
// =====================================================================

fn lit_i64(n: i64) -> Expr {
    Expr::Literal(Value::Int64(n))
}

fn lit_f64(f: f64) -> Expr {
    Expr::Literal(Value::Float64(f))
}

fn lit_text(s: &str) -> Expr {
    Expr::Literal(Value::Text(s.to_string()))
}

fn lit_bool(b: bool) -> Expr {
    Expr::Literal(Value::Bool(b))
}

fn lit_null() -> Expr {
    Expr::Literal(Value::Null)
}

fn lit_vector(data: Vec<f64>) -> Expr {
    Expr::Literal(Value::Vector(VectorValue::new(data)))
}

fn binary(left: Expr, op: BinaryOp, right: Expr) -> Expr {
    Expr::BinaryOp {
        left: Box::new(left),
        op,
        right: Box::new(right),
    }
}

fn unary(op: UnaryOp, expr: Expr) -> Expr {
    Expr::UnaryOp {
        op,
        expr: Box::new(expr),
    }
}

fn func(name: &str, args: Vec<Expr>) -> Expr {
    Expr::Function {
        name: name.to_string(),
        args,
        distinct: false,
    }
}

fn col(name: &str) -> Expr {
    Expr::Identifier(vec![name.to_string()])
}

fn eval(expr: &Expr) -> Result<Value, EvalError> {
    ExprEvaluator::eval(expr, &RowContext::new())
}

fn eval_with(expr: &Expr, ctx: &RowContext) -> Result<Value, EvalError> {
    ExprEvaluator::eval(expr, ctx)
}

// =====================================================================
//  算术运算（15）
// =====================================================================

#[test]
fn test_arith_01_int_add() {
    let e = binary(lit_i64(2), BinaryOp::Plus, lit_i64(3));
    assert_eq!(eval(&e).unwrap(), Value::Int64(5));
}

#[test]
fn test_arith_02_float_add() {
    let e = binary(lit_f64(1.5), BinaryOp::Plus, lit_f64(2.5));
    assert_eq!(eval(&e).unwrap(), Value::Float64(4.0));
}

#[test]
fn test_arith_03_mixed_add_int_float() {
    let e = binary(lit_i64(2), BinaryOp::Plus, lit_f64(0.5));
    assert_eq!(eval(&e).unwrap(), Value::Float64(2.5));
}

#[test]
fn test_arith_04_mixed_add_float_int() {
    let e = binary(lit_f64(1.5), BinaryOp::Plus, lit_i64(2));
    assert_eq!(eval(&e).unwrap(), Value::Float64(3.5));
}

#[test]
fn test_arith_05_int_sub() {
    let e = binary(lit_i64(10), BinaryOp::Minus, lit_i64(3));
    assert_eq!(eval(&e).unwrap(), Value::Int64(7));
}

#[test]
fn test_arith_06_int_mul() {
    let e = binary(lit_i64(4), BinaryOp::Multiply, lit_i64(7));
    assert_eq!(eval(&e).unwrap(), Value::Int64(28));
}

#[test]
fn test_arith_07_int_div() {
    let e = binary(lit_i64(20), BinaryOp::Divide, lit_i64(4));
    assert_eq!(eval(&e).unwrap(), Value::Int64(5));
}

#[test]
fn test_arith_08_int_div_by_zero() {
    let e = binary(lit_i64(10), BinaryOp::Divide, lit_i64(0));
    assert_eq!(eval(&e), Err(EvalError::DivisionByZero));
}

#[test]
fn test_arith_09_float_div_by_zero() {
    let e = binary(lit_f64(1.0), BinaryOp::Divide, lit_f64(0.0));
    assert_eq!(eval(&e), Err(EvalError::DivisionByZero));
}

#[test]
fn test_arith_10_modulo() {
    let e = binary(lit_i64(17), BinaryOp::Modulo, lit_i64(5));
    assert_eq!(eval(&e).unwrap(), Value::Int64(2));
}

#[test]
fn test_arith_11_modulo_by_zero() {
    let e = binary(lit_i64(17), BinaryOp::Modulo, lit_i64(0));
    assert_eq!(eval(&e), Err(EvalError::DivisionByZero));
}

#[test]
fn test_arith_12_float_mul() {
    let e = binary(lit_f64(2.5), BinaryOp::Multiply, lit_f64(4.0));
    assert_eq!(eval(&e).unwrap(), Value::Float64(10.0));
}

#[test]
fn test_arith_13_string_concat() {
    let e = binary(lit_text("foo"), BinaryOp::StringConcat, lit_text("bar"));
    assert_eq!(eval(&e).unwrap(), Value::Text("foobar".to_string()));
}

#[test]
fn test_arith_14_string_concat_with_null() {
    let e = binary(lit_text("foo"), BinaryOp::StringConcat, lit_null());
    assert_eq!(eval(&e).unwrap(), Value::Null);
}

#[test]
fn test_arith_15_negative_arithmetic() {
    let e = binary(lit_i64(-5), BinaryOp::Multiply, lit_i64(-3));
    assert_eq!(eval(&e).unwrap(), Value::Int64(15));
}

// =====================================================================
//  比较运算（12）
// =====================================================================

#[test]
fn test_cmp_01_eq_true() {
    let e = binary(lit_i64(5), BinaryOp::Eq, lit_i64(5));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_cmp_02_eq_false() {
    let e = binary(lit_i64(5), BinaryOp::Eq, lit_i64(6));
    assert_eq!(eval(&e).unwrap(), Value::Bool(false));
}

#[test]
fn test_cmp_03_neq() {
    let e = binary(lit_i64(5), BinaryOp::NotEq, lit_i64(6));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_cmp_04_lt() {
    let e = binary(lit_i64(3), BinaryOp::Lt, lit_i64(5));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_cmp_05_lteq() {
    let e = binary(lit_i64(5), BinaryOp::LtEq, lit_i64(5));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_cmp_06_gt() {
    let e = binary(lit_i64(7), BinaryOp::Gt, lit_i64(5));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_cmp_07_gteq() {
    let e = binary(lit_i64(5), BinaryOp::GtEq, lit_i64(5));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_cmp_08_cross_type_int_float() {
    // 1 = 1.0 应为 true（SQL 语义，跨类型提升）
    let e = binary(lit_i64(1), BinaryOp::Eq, lit_f64(1.0));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_cmp_09_float_comparison() {
    let e = binary(lit_f64(3.14), BinaryOp::Lt, lit_f64(3.15));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_cmp_10_text_eq() {
    let e = binary(lit_text("abc"), BinaryOp::Eq, lit_text("abc"));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_cmp_11_text_lexicographic() {
    // "apple" < "banana"
    let e = binary(lit_text("apple"), BinaryOp::Lt, lit_text("banana"));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_cmp_12_bool_eq() {
    let e = binary(lit_bool(true), BinaryOp::Eq, lit_bool(true));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

// =====================================================================
//  逻辑运算（10）— SQL 三值逻辑
// =====================================================================

#[test]
fn test_logic_01_and_true_true() {
    let e = binary(lit_bool(true), BinaryOp::And, lit_bool(true));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_logic_02_and_true_false() {
    let e = binary(lit_bool(true), BinaryOp::And, lit_bool(false));
    assert_eq!(eval(&e).unwrap(), Value::Bool(false));
}

#[test]
fn test_logic_03_and_false_false() {
    let e = binary(lit_bool(false), BinaryOp::And, lit_bool(false));
    assert_eq!(eval(&e).unwrap(), Value::Bool(false));
}

#[test]
fn test_logic_04_or_true_false() {
    let e = binary(lit_bool(true), BinaryOp::Or, lit_bool(false));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_logic_05_or_false_false() {
    let e = binary(lit_bool(false), BinaryOp::Or, lit_bool(false));
    assert_eq!(eval(&e).unwrap(), Value::Bool(false));
}

#[test]
fn test_logic_06_not_true() {
    let e = unary(UnaryOp::Not, lit_bool(true));
    assert_eq!(eval(&e).unwrap(), Value::Bool(false));
}

#[test]
fn test_logic_07_null_and_true() {
    let e = binary(lit_null(), BinaryOp::And, lit_bool(true));
    assert_eq!(eval(&e).unwrap(), Value::Null);
}

#[test]
fn test_logic_08_null_and_false() {
    // NULL AND false = false（短路）
    let e = binary(lit_null(), BinaryOp::And, lit_bool(false));
    assert_eq!(eval(&e).unwrap(), Value::Bool(false));
}

#[test]
fn test_logic_09_null_or_true() {
    // NULL OR true = true（短路）
    let e = binary(lit_null(), BinaryOp::Or, lit_bool(true));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_logic_10_null_or_false() {
    let e = binary(lit_null(), BinaryOp::Or, lit_bool(false));
    assert_eq!(eval(&e).unwrap(), Value::Null);
}

// =====================================================================
//  位运算（6）
// =====================================================================

#[test]
fn test_bit_01_bitand() {
    // 5 & 3 = 1
    let e = binary(lit_i64(5), BinaryOp::BitAnd, lit_i64(3));
    assert_eq!(eval(&e).unwrap(), Value::Int64(1));
}

#[test]
fn test_bit_02_bitor() {
    // 5 | 2 = 7
    let e = binary(lit_i64(5), BinaryOp::BitOr, lit_i64(2));
    assert_eq!(eval(&e).unwrap(), Value::Int64(7));
}

#[test]
fn test_bit_03_bitxor() {
    // 5 ^ 1 = 4
    let e = binary(lit_i64(5), BinaryOp::BitXor, lit_i64(1));
    assert_eq!(eval(&e).unwrap(), Value::Int64(4));
}

#[test]
fn test_bit_04_shift_left() {
    // 1 << 4 = 16
    let e = binary(lit_i64(1), BinaryOp::ShiftLeft, lit_i64(4));
    assert_eq!(eval(&e).unwrap(), Value::Int64(16));
}

#[test]
fn test_bit_05_shift_right() {
    // 256 >> 4 = 16
    let e = binary(lit_i64(256), BinaryOp::ShiftRight, lit_i64(4));
    assert_eq!(eval(&e).unwrap(), Value::Int64(16));
}

#[test]
fn test_bit_06_bitnot() {
    // ~5 = -6
    let e = unary(UnaryOp::BitNot, lit_i64(5));
    assert_eq!(eval(&e).unwrap(), Value::Int64(!5));
}

// =====================================================================
//  函数（25）
// =====================================================================

#[test]
fn test_func_01_upper() {
    let e = func("upper", vec![lit_text("abc")]);
    assert_eq!(eval(&e).unwrap(), Value::Text("ABC".to_string()));
}

#[test]
fn test_func_02_lower() {
    let e = func("lower", vec![lit_text("ABC")]);
    assert_eq!(eval(&e).unwrap(), Value::Text("abc".to_string()));
}

#[test]
fn test_func_03_length() {
    let e = func("length", vec![lit_text("hello")]);
    assert_eq!(eval(&e).unwrap(), Value::Int64(5));
}

#[test]
fn test_func_04_char_length() {
    let e = func("char_length", vec![lit_text("hi")]);
    assert_eq!(eval(&e).unwrap(), Value::Int64(2));
}

#[test]
fn test_func_05_octet_length() {
    // "hello" = 5 bytes
    let e = func("octet_length", vec![lit_text("hello")]);
    assert_eq!(eval(&e).unwrap(), Value::Int64(5));
}

#[test]
fn test_func_06_octet_length_unicode() {
    // "你好" = 6 UTF-8 bytes, 2 chars
    let e = func("octet_length", vec![lit_text("你好")]);
    assert_eq!(eval(&e).unwrap(), Value::Int64(6));
}

#[test]
fn test_func_07_abs_int() {
    let e = func("abs", vec![lit_i64(-5)]);
    assert_eq!(eval(&e).unwrap(), Value::Int64(5));
}

#[test]
fn test_func_08_abs_positive() {
    let e = func("abs", vec![lit_i64(5)]);
    assert_eq!(eval(&e).unwrap(), Value::Int64(5));
}

#[test]
fn test_func_09_abs_float() {
    let e = func("abs", vec![lit_f64(-3.14)]);
    assert_eq!(eval(&e).unwrap(), Value::Float64(3.14));
}

#[test]
fn test_func_10_round_no_scale() {
    let e = func("round", vec![lit_f64(3.14159)]);
    assert_eq!(eval(&e).unwrap(), Value::Float64(3.0));
}

#[test]
fn test_func_11_round_with_scale() {
    let e = func("round", vec![lit_f64(3.14159), lit_i64(2)]);
    assert_eq!(eval(&e).unwrap(), Value::Float64(3.14));
}

#[test]
fn test_func_12_ceil() {
    let e = func("ceil", vec![lit_f64(3.14)]);
    assert_eq!(eval(&e).unwrap(), Value::Float64(4.0));
}

#[test]
fn test_func_13_floor() {
    let e = func("floor", vec![lit_f64(3.99)]);
    assert_eq!(eval(&e).unwrap(), Value::Float64(3.0));
}

#[test]
fn test_func_14_coalesce_first() {
    let e = func("coalesce", vec![lit_i64(1), lit_i64(2)]);
    assert_eq!(eval(&e).unwrap(), Value::Int64(1));
}

#[test]
fn test_func_15_coalesce_skip_nulls() {
    let e = func("coalesce", vec![lit_null(), lit_null(), lit_i64(5)]);
    assert_eq!(eval(&e).unwrap(), Value::Int64(5));
}

#[test]
fn test_func_16_coalesce_all_null() {
    let e = func("coalesce", vec![lit_null(), lit_null()]);
    assert_eq!(eval(&e).unwrap(), Value::Null);
}

#[test]
fn test_func_17_nullif_equal() {
    // nullif(5, 5) → NULL
    let e = func("nullif", vec![lit_i64(5), lit_i64(5)]);
    assert_eq!(eval(&e).unwrap(), Value::Null);
}

#[test]
fn test_func_18_nullif_not_equal() {
    let e = func("nullif", vec![lit_i64(5), lit_i64(3)]);
    assert_eq!(eval(&e).unwrap(), Value::Int64(5));
}

#[test]
fn test_func_19_concat_strings() {
    let e = func("concat", vec![lit_text("a"), lit_text("b"), lit_text("c")]);
    assert_eq!(eval(&e).unwrap(), Value::Text("abc".to_string()));
}

#[test]
fn test_func_20_concat_skips_null() {
    // concat 中 NULL 被忽略
    let e = func("concat", vec![lit_text("a"), lit_null(), lit_text("b")]);
    assert_eq!(eval(&e).unwrap(), Value::Text("ab".to_string()));
}

#[test]
fn test_func_21_trim() {
    let e = func("trim", vec![lit_text("  hi  ")]);
    assert_eq!(eval(&e).unwrap(), Value::Text("hi".to_string()));
}

#[test]
fn test_func_22_ltrim_rtrim() {
    let l = eval(&func("ltrim", vec![lit_text("  hi  ")])).unwrap();
    assert_eq!(l, Value::Text("hi  ".to_string()));
    let r = eval(&func("rtrim", vec![lit_text("  hi  ")])).unwrap();
    assert_eq!(r, Value::Text("  hi".to_string()));
}

#[test]
fn test_func_23_replace() {
    let e = func(
        "replace",
        vec![lit_text("hello"), lit_text("l"), lit_text("L")],
    );
    assert_eq!(eval(&e).unwrap(), Value::Text("heLLo".to_string()));
}

#[test]
fn test_func_24_substring_with_length() {
    // substring('hello', 2, 3) = "ell"
    let e = func("substring", vec![lit_text("hello"), lit_i64(2), lit_i64(3)]);
    assert_eq!(eval(&e).unwrap(), Value::Text("ell".to_string()));
}

#[test]
fn test_func_25_substring_no_length() {
    // substring('hello', 2) = "ello"
    let e = func("substring", vec![lit_text("hello"), lit_i64(2)]);
    assert_eq!(eval(&e).unwrap(), Value::Text("ello".to_string()));
}

// =====================================================================
//  CASE 表达式（8）
// =====================================================================

#[test]
fn test_case_01_simple_match() {
    // CASE 1 WHEN 1 THEN 'one' END
    let e = Expr::Case {
        operand: Some(Box::new(lit_i64(1))),
        when_then: vec![(lit_i64(1), lit_text("one"))],
        else_expr: None,
    };
    assert_eq!(eval(&e).unwrap(), Value::Text("one".to_string()));
}

#[test]
fn test_case_02_simple_no_match() {
    // CASE 5 WHEN 1 THEN 'one' END → NULL
    let e = Expr::Case {
        operand: Some(Box::new(lit_i64(5))),
        when_then: vec![(lit_i64(1), lit_text("one"))],
        else_expr: None,
    };
    assert_eq!(eval(&e).unwrap(), Value::Null);
}

#[test]
fn test_case_03_simple_with_else() {
    // CASE 5 WHEN 1 THEN 'one' ELSE 'other' END
    let e = Expr::Case {
        operand: Some(Box::new(lit_i64(5))),
        when_then: vec![(lit_i64(1), lit_text("one"))],
        else_expr: Some(Box::new(lit_text("other"))),
    };
    assert_eq!(eval(&e).unwrap(), Value::Text("other".to_string()));
}

#[test]
fn test_case_04_searched_true() {
    // CASE WHEN 1 = 1 THEN 'yes' END
    let cond = binary(lit_i64(1), BinaryOp::Eq, lit_i64(1));
    let e = Expr::Case {
        operand: None,
        when_then: vec![(cond, lit_text("yes"))],
        else_expr: None,
    };
    assert_eq!(eval(&e).unwrap(), Value::Text("yes".to_string()));
}

#[test]
fn test_case_05_searched_all_false() {
    // CASE WHEN 1 = 2 THEN 'a' WHEN 3 = 4 THEN 'b' END → NULL
    let c1 = binary(lit_i64(1), BinaryOp::Eq, lit_i64(2));
    let c2 = binary(lit_i64(3), BinaryOp::Eq, lit_i64(4));
    let e = Expr::Case {
        operand: None,
        when_then: vec![(c1, lit_text("a")), (c2, lit_text("b"))],
        else_expr: None,
    };
    assert_eq!(eval(&e).unwrap(), Value::Null);
}

#[test]
fn test_case_06_searched_with_else() {
    // CASE WHEN 1 = 2 THEN 'a' ELSE 'default' END
    let c1 = binary(lit_i64(1), BinaryOp::Eq, lit_i64(2));
    let e = Expr::Case {
        operand: None,
        when_then: vec![(c1, lit_text("a"))],
        else_expr: Some(Box::new(lit_text("default"))),
    };
    assert_eq!(eval(&e).unwrap(), Value::Text("default".to_string()));
}

#[test]
fn test_case_07_null_operand() {
    // CASE NULL WHEN 1 THEN 'one' ELSE 'other' END → 'other'
    let e = Expr::Case {
        operand: Some(Box::new(lit_null())),
        when_then: vec![(lit_i64(1), lit_text("one"))],
        else_expr: Some(Box::new(lit_text("other"))),
    };
    assert_eq!(eval(&e).unwrap(), Value::Text("other".to_string()));
}

#[test]
fn test_case_08_multiple_when() {
    // CASE x WHEN 1 THEN 'a' WHEN 2 THEN 'b' WHEN 3 THEN 'c' ELSE 'd' END
    let ctx = RowContext::new().with("x", Value::Int64(2));
    let e = Expr::Case {
        operand: Some(Box::new(col("x"))),
        when_then: vec![
            (lit_i64(1), lit_text("a")),
            (lit_i64(2), lit_text("b")),
            (lit_i64(3), lit_text("c")),
        ],
        else_expr: Some(Box::new(lit_text("d"))),
    };
    assert_eq!(eval_with(&e, &ctx).unwrap(), Value::Text("b".to_string()));
}

// =====================================================================
//  CAST（8）
// =====================================================================

#[test]
fn test_cast_01_text_to_int() {
    let e = Expr::Cast {
        expr: Box::new(lit_text("123")),
        data_type: ColumnType::Int64,
    };
    assert_eq!(eval(&e).unwrap(), Value::Int64(123));
}

#[test]
fn test_cast_02_int_to_text() {
    let e = Expr::Cast {
        expr: Box::new(lit_i64(123)),
        data_type: ColumnType::Text,
    };
    assert_eq!(eval(&e).unwrap(), Value::Text("123".to_string()));
}

#[test]
fn test_cast_03_text_to_float() {
    let e = Expr::Cast {
        expr: Box::new(lit_text("3.14")),
        data_type: ColumnType::Float64,
    };
    assert_eq!(eval(&e).unwrap(), Value::Float64(3.14));
}

#[test]
fn test_cast_04_int_to_bool() {
    // CAST(1 AS BOOL) → true
    let e = Expr::Cast {
        expr: Box::new(lit_i64(1)),
        data_type: ColumnType::Bool,
    };
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_cast_05_text_to_bool() {
    // CAST('true' AS BOOL)
    let e = Expr::Cast {
        expr: Box::new(lit_text("true")),
        data_type: ColumnType::Bool,
    };
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_cast_06_null_preserved() {
    // CAST(NULL AS INT64) → NULL
    let e = Expr::Cast {
        expr: Box::new(lit_null()),
        data_type: ColumnType::Int64,
    };
    assert_eq!(eval(&e).unwrap(), Value::Null);
}

#[test]
fn test_cast_07_failure() {
    // CAST('abc' AS INT64) → error
    let e = Expr::Cast {
        expr: Box::new(lit_text("abc")),
        data_type: ColumnType::Int64,
    };
    assert!(matches!(eval(&e), Err(EvalError::CastFailed(_))));
}

#[test]
fn test_cast_08_int_to_float() {
    // 隐式：Int64 → Float64
    let e = Expr::Cast {
        expr: Box::new(lit_i64(42)),
        data_type: ColumnType::Float64,
    };
    assert_eq!(eval(&e).unwrap(), Value::Float64(42.0));
}

// =====================================================================
//  IN / BETWEEN / LIKE / IS NULL（10）
// =====================================================================

#[test]
fn test_misc_01_in_list_found() {
    // 2 IN (1, 2, 3) → true
    let e = Expr::InList {
        expr: Box::new(lit_i64(2)),
        list: vec![lit_i64(1), lit_i64(2), lit_i64(3)],
        negated: false,
    };
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_misc_02_in_list_not_found() {
    let e = Expr::InList {
        expr: Box::new(lit_i64(5)),
        list: vec![lit_i64(1), lit_i64(2), lit_i64(3)],
        negated: false,
    };
    assert_eq!(eval(&e).unwrap(), Value::Bool(false));
}

#[test]
fn test_misc_03_not_in_list() {
    let e = Expr::InList {
        expr: Box::new(lit_i64(5)),
        list: vec![lit_i64(1), lit_i64(2), lit_i64(3)],
        negated: true,
    };
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_misc_04_in_list_with_null() {
    // 5 IN (1, NULL, 3) → NULL（既不在列表中，又遇到 NULL）
    let e = Expr::InList {
        expr: Box::new(lit_i64(5)),
        list: vec![lit_i64(1), lit_null(), lit_i64(3)],
        negated: false,
    };
    assert_eq!(eval(&e).unwrap(), Value::Null);
}

#[test]
fn test_misc_05_between_in_range() {
    // 5 BETWEEN 1 AND 10 → true
    let e = Expr::Between {
        expr: Box::new(lit_i64(5)),
        low: Box::new(lit_i64(1)),
        high: Box::new(lit_i64(10)),
        negated: false,
    };
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_misc_06_between_out_of_range() {
    let e = Expr::Between {
        expr: Box::new(lit_i64(20)),
        low: Box::new(lit_i64(1)),
        high: Box::new(lit_i64(10)),
        negated: false,
    };
    assert_eq!(eval(&e).unwrap(), Value::Bool(false));
}

// =====================================================================
// PG 正则匹配运算符测试（P0-PG 兼容性修复）
// =====================================================================

#[test]
fn test_pg_regex_match_basic() {
    // 'abc' ~ '^a' → true
    let e = binary(lit_text("abc"), BinaryOp::RegexMatch, lit_text("^a"));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
    // 'abc' ~ '^A' → false（大小写敏感）
    let e = binary(lit_text("abc"), BinaryOp::RegexMatch, lit_text("^A"));
    assert_eq!(eval(&e).unwrap(), Value::Bool(false));
}

#[test]
fn test_pg_regex_imatch_basic() {
    // 'abc' ~* '^A' → true（大小写不敏感）
    let e = binary(lit_text("abc"), BinaryOp::RegexIMatch, lit_text("^A"));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_pg_regex_not_match_basic() {
    // 'abc' !~ '^A' → true（大小写敏感，不匹配）
    let e = binary(lit_text("abc"), BinaryOp::RegexNotMatch, lit_text("^A"));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
    // 'abc' !~ '^a' → false（匹配，取反后 false）
    let e = binary(lit_text("abc"), BinaryOp::RegexNotMatch, lit_text("^a"));
    assert_eq!(eval(&e).unwrap(), Value::Bool(false));
}

#[test]
fn test_pg_regex_not_imatch_basic() {
    // 'abc' !~* '^A' → false（大小写不敏感，匹配，取反后 false）
    let e = binary(lit_text("abc"), BinaryOp::RegexNotIMatch, lit_text("^A"));
    assert_eq!(eval(&e).unwrap(), Value::Bool(false));
}

#[test]
fn test_pg_regex_partial_match() {
    // PG ~ 默认是部分匹配（搜索），不是完全匹配
    // 'foobar' ~ 'bar' → true
    let e = binary(lit_text("foobar"), BinaryOp::RegexMatch, lit_text("bar"));
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_pg_regex_null_handling() {
    // NULL ~ 'a' → NULL
    let e = binary(
        Expr::Literal(Value::Null),
        BinaryOp::RegexMatch,
        lit_text("a"),
    );
    assert_eq!(eval(&e).unwrap(), Value::Null);
    // 'a' ~ NULL → NULL
    let e = binary(
        lit_text("a"),
        BinaryOp::RegexMatch,
        Expr::Literal(Value::Null),
    );
    assert_eq!(eval(&e).unwrap(), Value::Null);
}

#[test]
fn test_pg_regex_type_mismatch() {
    // 数字 ~ 'a' → TypeMismatch
    let e = binary(lit_i64(123), BinaryOp::RegexMatch, lit_text("a"));
    assert!(matches!(eval(&e), Err(EvalError::TypeMismatch { .. })));
}

#[test]
fn test_pg_regex_invalid_pattern() {
    // 无效正则 → InvalidRegex
    let e = binary(lit_text("abc"), BinaryOp::RegexMatch, lit_text("["));
    assert!(matches!(eval(&e), Err(EvalError::InvalidRegex(_))));
}

#[test]
fn test_misc_07_not_between() {
    let e = Expr::Between {
        expr: Box::new(lit_i64(20)),
        low: Box::new(lit_i64(1)),
        high: Box::new(lit_i64(10)),
        negated: true,
    };
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_misc_08_like_percent() {
    // 'hello' LIKE '%ello' → true
    let e = Expr::Like {
        expr: Box::new(lit_text("hello")),
        pattern: Box::new(lit_text("%ello")),
        negated: false,
        case_insensitive: false,
    };
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_misc_09_like_underscore() {
    // 'hello' LIKE '_ello' → true
    let e = Expr::Like {
        expr: Box::new(lit_text("hello")),
        pattern: Box::new(lit_text("_ello")),
        negated: false,
        case_insensitive: false,
    };
    assert_eq!(eval(&e).unwrap(), Value::Bool(true));
}

#[test]
fn test_misc_10_is_null_and_is_not_null() {
    let is_null = Expr::IsNull {
        expr: Box::new(lit_null()),
        negated: false,
    };
    assert_eq!(eval(&is_null).unwrap(), Value::Bool(true));

    let is_not_null = Expr::IsNull {
        expr: Box::new(lit_i64(5)),
        negated: true,
    };
    assert_eq!(eval(&is_not_null).unwrap(), Value::Bool(true));
}

// =====================================================================
//  边界与错误（6）
// =====================================================================

#[test]
fn test_edge_01_int_min_negation_overflow() {
    // -i64::MIN 应溢出
    let e = unary(UnaryOp::Minus, lit_i64(i64::MIN));
    assert!(matches!(eval(&e), Err(EvalError::IntegerOverflow(_))));
}

#[test]
fn test_edge_02_int_min_div_neg_one_overflow() {
    // i64::MIN / -1 应溢出
    let e = binary(lit_i64(i64::MIN), BinaryOp::Divide, lit_i64(-1));
    assert!(matches!(eval(&e), Err(EvalError::IntegerOverflow(_))));
}

#[test]
fn test_edge_03_empty_in_list() {
    // 5 IN () → false（空列表，未找到）
    let e = Expr::InList {
        expr: Box::new(lit_i64(5)),
        list: vec![],
        negated: false,
    };
    assert_eq!(eval(&e).unwrap(), Value::Bool(false));
}

#[test]
fn test_edge_04_empty_identifier() {
    // 空标识符 → ColumnNotFound 错误
    let e = Expr::Identifier(vec![]);
    assert!(matches!(eval(&e), Err(EvalError::ColumnNotFound(_))));
}

#[test]
fn test_edge_05_nested_arithmetic() {
    // (1 + 2) * 3 = 9
    let inner = binary(lit_i64(1), BinaryOp::Plus, lit_i64(2));
    let outer = binary(inner, BinaryOp::Multiply, lit_i64(3));
    assert_eq!(eval(&outer).unwrap(), Value::Int64(9));
}

#[test]
fn test_edge_06_column_lookup() {
    // 通过 RowContext 查找列
    let ctx = RowContext::new().with("age", Value::Int64(30));
    let e = col("age");
    assert_eq!(eval_with(&e, &ctx).unwrap(), Value::Int64(30));
}

// =====================================================================
//  补充测试（用于覆盖关键路径，但不计入 100 个核心用例）
// =====================================================================

#[test]
fn test_arithmetic_no_panic_on_overflow_add() {
    // 加法溢出应返回错误而非 panic
    let e = binary(lit_i64(i64::MAX), BinaryOp::Plus, lit_i64(1));
    let result = eval(&e);
    assert!(result.is_err(), "expected overflow error, got {result:?}");
}

#[test]
fn test_arithmetic_no_panic_on_overflow_mul() {
    // 乘法溢出应返回错误而非 panic
    let e = binary(lit_i64(i64::MAX), BinaryOp::Multiply, lit_i64(2));
    let result = eval(&e);
    assert!(result.is_err(), "expected overflow error, got {result:?}");
}

// =====================================================================
//  P3-7 JSON_ARRAY / JSON_OBJECT
// =====================================================================

fn lit_json(s: &str) -> Expr {
    let v: serde_json::Value = serde_json::from_str(s).expect("valid JSON literal");
    Expr::Literal(Value::Json(v))
}

#[test]
fn test_json_array_empty() {
    // JSON_ARRAY() → []
    let e = func("json_array", vec![]);
    let r = eval(&e).unwrap();
    assert_eq!(r, Value::Json(serde_json::json!([])));
}

#[test]
fn test_json_array_single_value() {
    // JSON_ARRAY(1) → [1]
    let e = func("json_array", vec![lit_i64(1)]);
    let r = eval(&e).unwrap();
    assert_eq!(r, Value::Json(serde_json::json!([1])));
}

#[test]
fn test_json_array_multiple_values() {
    // JSON_ARRAY(1, 'hello', true) → [1, "hello", true]
    let e = func("json_array", vec![lit_i64(1), lit_text("hello"), lit_bool(true)]);
    let r = eval(&e).unwrap();
    assert_eq!(r, Value::Json(serde_json::json!([1, "hello", true])));
}

#[test]
fn test_json_array_null_absent_on_null() {
    // 默认 ABSENT ON NULL：NULL 元素被跳过
    let e = func("json_array", vec![lit_i64(1), lit_null(), lit_text("x")]);
    let r = eval(&e).unwrap();
    assert_eq!(r, Value::Json(serde_json::json!([1, "x"])));
}

#[test]
fn test_json_array_null_on_null_returns_null() {
    // NULL ON NULL：任一参数为 NULL → 整体返回 NULL
    let e = func("json_array", vec![
        lit_i64(1),
        lit_null(),
        lit_text("__NULL_ON_NULL__"),
    ]);
    let r = eval(&e).unwrap();
    assert_eq!(r, Value::Null);
}

#[test]
fn test_json_array_nested_json() {
    // JSON_ARRAY 嵌套 JSON 对象
    let e = func("json_array", vec![lit_json(r#"{"a":1}"#)]);
    let r = eval(&e).unwrap();
    assert_eq!(r, Value::Json(serde_json::json!([{"a": 1}])));
}

#[test]
fn test_json_object_basic() {
    // JSON_OBJECT('name', 'alice', 'age', 30) → {"name":"alice","age":30}
    let e = func("json_object", vec![
        lit_text("name"), lit_text("alice"),
        lit_text("age"), lit_i64(30),
    ]);
    let r = eval(&e).unwrap();
    assert_eq!(r, Value::Json(serde_json::json!({"name": "alice", "age": 30})));
}

#[test]
fn test_json_object_odd_args_errors() {
    // 奇数个参数 → 错误
    let e = func("json_object", vec![lit_text("k"), lit_text("v"), lit_text("orphan")]);
    let r = eval(&e);
    assert!(r.is_err(), "expected error for odd args, got {r:?}");
}

#[test]
fn test_json_object_absent_on_null() {
    // 默认 ABSENT ON NULL：值为 NULL 的 key 被跳过
    let e = func("json_object", vec![
        lit_text("a"), lit_i64(1),
        lit_text("b"), lit_null(),
        lit_text("c"), lit_text("x"),
    ]);
    let r = eval(&e).unwrap();
    assert_eq!(r, Value::Json(serde_json::json!({"a": 1, "c": "x"})));
}

#[test]
fn test_json_object_null_on_null_returns_null() {
    // NULL ON NULL：任一 value 为 NULL → 整体返回 NULL
    let e = func("json_object", vec![
        lit_text("a"), lit_i64(1),
        lit_text("b"), lit_null(),
        lit_text("__NULL_ON_NULL__"),
    ]);
    let r = eval(&e).unwrap();
    assert_eq!(r, Value::Null);
}

#[test]
fn test_json_object_non_text_key_errors() {
    // key 非 Text → 类型错误
    let e = func("json_object", vec![lit_i64(1), lit_text("v")]);
    let r = eval(&e);
    assert!(r.is_err(), "expected type error for non-text key, got {r:?}");
}

// =====================================================================
//  向量类型与距离函数（P4-5）
// =====================================================================

#[test]
fn test_vector_value_parse() {
    let v = VectorValue::parse("[1.0, 2.0, 3.0]").unwrap();
    assert_eq!(v.dims(), 3);
    assert_eq!(v.data, vec![1.0, 2.0, 3.0]);
}

#[test]
fn test_vector_value_to_string() {
    let v = VectorValue::new(vec![1.5, 2.5]);
    assert_eq!(v.to_string(), "[1.5, 2.5]");
}

#[test]
fn test_vector_cosine_distance() {
    // 相同向量 → 余弦距离 0
    let a = VectorValue::new(vec![1.0, 0.0]);
    let b = VectorValue::new(vec![1.0, 0.0]);
    assert!((a.cosine_distance(&b) - 0.0).abs() < 1e-9);

    // 正交向量 → 余弦距离 1
    let a = VectorValue::new(vec![1.0, 0.0]);
    let b = VectorValue::new(vec![0.0, 1.0]);
    assert!((a.cosine_distance(&b) - 1.0).abs() < 1e-9);

    // 反向向量 → 余弦距离 2
    let a = VectorValue::new(vec![1.0, 0.0]);
    let b = VectorValue::new(vec![-1.0, 0.0]);
    assert!((a.cosine_distance(&b) - 2.0).abs() < 1e-9);
}

#[test]
fn test_vector_l2_distance() {
    let a = VectorValue::new(vec![0.0, 0.0]);
    let b = VectorValue::new(vec![3.0, 4.0]);
    assert!((a.l2_distance(&b) - 5.0).abs() < 1e-9);

    // 相同向量 → 0
    let a = VectorValue::new(vec![1.0, 2.0]);
    assert!((a.l2_distance(&a) - 0.0).abs() < 1e-9);
}

#[test]
fn test_vector_dot_product() {
    let a = VectorValue::new(vec![1.0, 2.0, 3.0]);
    let b = VectorValue::new(vec![4.0, 5.0, 6.0]);
    assert!((a.dot_product(&b) - 32.0).abs() < 1e-9); // 1*4+2*5+3*6=32
}

#[test]
fn test_vector_cast_text_to_vector() {
    let e = Expr::Cast {
        expr: Box::new(lit_text("[1.0, 2.0, 3.0]")),
        data_type: ColumnType::Vector(3),
    };
    match eval(&e) {
        Ok(Value::Vector(v)) => assert_eq!(v.data, vec![1.0, 2.0, 3.0]),
        other => panic!("expected Vector, got {other:?}"),
    }
}

#[test]
fn test_vector_cast_vector_to_text() {
    let e = Expr::Cast {
        expr: Box::new(lit_vector(vec![1.0, 2.0])),
        data_type: ColumnType::Text,
    };
    match eval(&e) {
        Ok(Value::Text(s)) => assert_eq!(s, "[1, 2]"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn test_vector_column_type() {
    let v = Value::Vector(VectorValue::new(vec![1.0, 2.0, 3.0]));
    assert_eq!(v.column_type(), ColumnType::Vector(3));
}

#[test]
fn test_cosine_distance_function() {
    let _a = VectorValue::new(vec![1.0, 0.0]);
    let _b = VectorValue::new(vec![0.0, 1.0]);
    let e = func("cosine_distance", vec![lit_vector(vec![1.0, 0.0]), lit_vector(vec![0.0, 1.0])]);
    if let Value::Float64(f) = eval(&e).unwrap() {
        assert!((f - 1.0).abs() < 1e-9, "got {f}");
    } else { panic!("expected Float64"); }
}

#[test]
fn test_l2_distance_function() {
    let e = func("l2_distance", vec![lit_vector(vec![0.0, 0.0]), lit_vector(vec![3.0, 4.0])]);
    if let Value::Float64(f) = eval(&e).unwrap() {
        assert!((f - 5.0).abs() < 1e-9, "got {f}");
    } else { panic!("expected Float64"); }
}

#[test]
fn test_dot_product_function() {
    let e = func("dot_product", vec![lit_vector(vec![1.0, 2.0]), lit_vector(vec![3.0, 4.0])]);
    if let Value::Float64(f) = eval(&e).unwrap() {
        assert!((f - 11.0).abs() < 1e-9, "got {f}");
    } else { panic!("expected Float64"); } // 1*3+2*4=11
}

#[test]
fn test_vector_distance_null_propagation() {
    // 任一 NULL → NULL
    let e = func("cosine_distance", vec![lit_null(), lit_vector(vec![1.0, 0.0])]);
    assert_eq!(eval(&e).unwrap(), Value::Null);

    let e = func("l2_distance", vec![lit_vector(vec![1.0, 0.0]), lit_null()]);
    assert_eq!(eval(&e).unwrap(), Value::Null);

    let e = func("dot_product", vec![lit_null(), lit_null()]);
    assert_eq!(eval(&e).unwrap(), Value::Null);
}

#[test]
fn test_vector_distance_type_mismatch() {
    // 非向量参数 → TypeMismatch
    let e = func("cosine_distance", vec![lit_i64(1), lit_i64(2)]);
    assert!(matches!(eval(&e), Err(EvalError::TypeMismatch { .. })));

    let e = func("l2_distance", vec![lit_text("a"), lit_text("b")]);
    assert!(matches!(eval(&e), Err(EvalError::TypeMismatch { .. })));
}
