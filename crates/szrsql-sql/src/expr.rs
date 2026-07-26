//! SzRSQL 表达式求值（Phase 3.3）— 火山模型执行器的核心组件。
//!
//! # 设计
//!
//! - **入口**：`ExprEvaluator::eval(expr, ctx) -> Result<Value, EvalError>`
//! - **EvalContext** — 提供列值查找、聚合状态访问、子查询执行能力
//! - **运算符**：算术 / 比较 / 逻辑 / 字符串拼接 / 按位
//! - **函数**：upper / lower / length / abs / round / coalesce / now / count / sum / avg / min / max
//! - **CASE / CAST / IN / BETWEEN / LIKE / IS NULL / EXISTS**：完整支持
//! - **NULL 语义**遵循 SQL 三值逻辑（NULL AND false = false, NULL OR true = true, 其他 = NULL）
//! - **边界处理**：除零返回错误，溢出返回错误，不 panic
//!
//! 对应 `SzRSQL实施进度.md` Phase 3.3。

use crate::ast::*;
use szrsql_types::value::{
    TsQuery, TsVector, Value, TS_WEIGHT_A, TS_WEIGHT_B, TS_WEIGHT_C, TS_WEIGHT_D,
};
use thiserror::Error;

// =====================================================================
//  错误类型
// =====================================================================

/// 表达式求值错误
#[derive(Debug, Clone, PartialEq, Error)]
pub enum EvalError {
    /// 除零
    #[error("division by zero")]
    DivisionByZero,
    /// 整数溢出
    #[error("integer overflow: {0}")]
    IntegerOverflow(String),
    /// 浮点溢出（inf/nan）
    #[error("float overflow: {0}")]
    FloatOverflow(String),
    /// 类型不匹配
    #[error("type mismatch: expected {expected}, got {actual}")]
    TypeMismatch {
        /// 期望类型描述
        expected: &'static str,
        /// 实际值描述
        actual: &'static str,
    },
    /// 列不存在
    #[error("column not found: {0}")]
    ColumnNotFound(String),
    /// 函数不存在
    #[error("function not found: {0}")]
    FunctionNotFound(String),
    /// 函数参数错误
    #[error("invalid function arguments: {0}")]
    InvalidFunctionArgs(String),
    /// CAST 失败
    #[error("cast failed: {0}")]
    CastFailed(String),
    /// 不支持的表达式
    #[error("unsupported expression: {0}")]
    Unsupported(String),
    /// 子查询返回多行（用于非标量子查询上下文）
    #[error("subquery returned more than one row")]
    SubqueryMultipleRows,
    /// LIKE 模式错误
    #[error("invalid LIKE pattern: {0}")]
    InvalidLikePattern(String),
}

// =====================================================================
//  求值上下文
// =====================================================================

/// 求值上下文 — 提供列值查找与外部依赖
pub trait EvalContext {
    /// 按列名查找值（单层标识符）
    fn lookup_column(&self, name: &str) -> Result<Value, EvalError>;

    /// 按限定名查找值（table.column）
    fn lookup_qualified(&self, table: &str, column: &str) -> Result<Value, EvalError> {
        // 默认实现忽略表名前缀，仅按列名查找（适用单表场景）
        let _ = table;
        self.lookup_column(column)
    }

    /// 执行标量子查询，返回单个值
    fn eval_subquery(&self, _select: &Select) -> Result<Value, EvalError> {
        Err(EvalError::Unsupported(
            "subquery evaluation not supported in this context".into(),
        ))
    }

    /// 执行 EXISTS 子查询，返回 bool
    fn eval_exists(&self, _select: &Select) -> Result<bool, EvalError> {
        Err(EvalError::Unsupported(
            "EXISTS evaluation not supported in this context".into(),
        ))
    }
}

/// 简单行上下文 — 一行数据（列名 → 值），用于单行求值
#[derive(Debug, Clone, Default)]
pub struct RowContext {
    /// 列名 → 值（大小写不敏感键存储）
    pub columns: std::collections::HashMap<String, Value>,
}

impl RowContext {
    /// 创建空上下文
    pub fn new() -> Self {
        Self {
            columns: std::collections::HashMap::new(),
        }
    }

    /// 添加列值
    pub fn with(mut self, name: &str, value: Value) -> Self {
        self.columns.insert(name.to_lowercase(), value);
        self
    }

    /// 添加多列
    pub fn with_all<I>(&mut self, pairs: I)
    where
        I: IntoIterator<Item = (String, Value)>,
    {
        for (k, v) in pairs {
            self.columns.insert(k.to_lowercase(), v);
        }
    }
}

impl EvalContext for RowContext {
    fn lookup_column(&self, name: &str) -> Result<Value, EvalError> {
        self.columns
            .get(&name.to_lowercase())
            .cloned()
            .ok_or_else(|| EvalError::ColumnNotFound(name.to_string()))
    }
}

// =====================================================================
//  求值器
// =====================================================================

/// 表达式求值器
pub struct ExprEvaluator;

impl ExprEvaluator {
    /// 求值表达式
    pub fn eval(expr: &Expr, ctx: &dyn EvalContext) -> Result<Value, EvalError> {
        match expr {
            Expr::Literal(value) => Ok(value.clone()),
            Expr::Identifier(parts) => {
                if parts.is_empty() {
                    return Err(EvalError::ColumnNotFound("(empty)".into()));
                }
                if parts.len() == 1 {
                    ctx.lookup_column(&parts[0])
                } else {
                    // table.col → 取最后一部分作为列名
                    let table = &parts[parts.len() - 2];
                    let col = &parts[parts.len() - 1];
                    ctx.lookup_qualified(table, col)
                }
            }
            Expr::BinaryOp { left, op, right } => {
                // 短路求值：AND / OR
                match op {
                    BinaryOp::And => {
                        let l = Self::eval(left, ctx)?;
                        match l {
                            Value::Bool(false) => Ok(Value::Bool(false)),
                            Value::Null => {
                                let r = Self::eval(right, ctx)?;
                                Ok(match r {
                                    Value::Bool(false) => Value::Bool(false),
                                    _ => Value::Null,
                                })
                            }
                            Value::Bool(true) => {
                                let r = Self::eval(right, ctx)?;
                                Self::to_bool_or_null(r)
                            }
                            other => Err(EvalError::TypeMismatch {
                                expected: "bool",
                                actual: value_type_name(&other),
                            }),
                        }
                    }
                    BinaryOp::Or => {
                        let l = Self::eval(left, ctx)?;
                        match l {
                            Value::Bool(true) => Ok(Value::Bool(true)),
                            Value::Null => {
                                let r = Self::eval(right, ctx)?;
                                Ok(match r {
                                    Value::Bool(true) => Value::Bool(true),
                                    _ => Value::Null,
                                })
                            }
                            Value::Bool(false) => {
                                let r = Self::eval(right, ctx)?;
                                Self::to_bool_or_null(r)
                            }
                            other => Err(EvalError::TypeMismatch {
                                expected: "bool",
                                actual: value_type_name(&other),
                            }),
                        }
                    }
                    _ => {
                        let l = Self::eval(left, ctx)?;
                        let r = Self::eval(right, ctx)?;
                        Self::eval_binary_op(*op, l, r)
                    }
                }
            }
            Expr::UnaryOp { op, expr } => {
                let v = Self::eval(expr, ctx)?;
                Self::eval_unary_op(*op, v)
            }
            Expr::Function {
                name,
                args,
                distinct,
            } => Self::eval_function(name, args, *distinct, ctx),
            Expr::Case {
                operand,
                when_then,
                else_expr,
            } => Self::eval_case(operand, when_then, else_expr, ctx),
            Expr::Cast { expr, data_type } => {
                let v = Self::eval(expr, ctx)?;
                v.cast_explicit(data_type)
                    .map_err(|e| EvalError::CastFailed(format!("{e}")))
            }
            Expr::InList {
                expr,
                list,
                negated,
            } => Self::eval_in_list(expr, list, *negated, ctx),
            Expr::InSubquery {
                expr,
                subquery,
                negated,
            } => {
                let v = Self::eval(expr, ctx)?;
                let _ = subquery;
                let _ = negated;
                let _ = v;
                Err(EvalError::Unsupported(
                    "IN subquery not yet supported in evaluator".into(),
                ))
            }
            Expr::Between {
                expr,
                low,
                high,
                negated,
            } => Self::eval_between(expr, low, high, *negated, ctx),
            Expr::Like {
                expr,
                pattern,
                negated,
            } => Self::eval_like(expr, pattern, *negated, ctx),
            Expr::IsNull { expr, negated } => {
                let v = Self::eval(expr, ctx)?;
                let is_null = matches!(v, Value::Null);
                Ok(Value::Bool(if *negated {
                    !is_null
                } else {
                    is_null
                }))
            }
            Expr::Exists { subquery, negated } => {
                let exists = ctx.eval_exists(subquery)?;
                Ok(Value::Bool(if *negated {
                    !exists
                } else {
                    exists
                }))
            }
            Expr::Subquery(select) => ctx.eval_subquery(select),
            Expr::Tuple(_) => Err(EvalError::Unsupported(
                "tuple expression not yet supported in evaluator".into(),
            )),
            Expr::Wildcard => Err(EvalError::Unsupported(
                "wildcard not allowed in this context".into(),
            )),
            // Phase 3.26: 未替换的参数占位符 → 报错（应在 EXECUTE 时被 substitute_parameters 替换）
            Expr::Parameter(idx) => Err(EvalError::Unsupported(format!(
                "unbound parameter ${idx} — must be substituted via EXECUTE before evaluation"
            ))),
            // Phase 3.32: ARRAY[e1, e2, ...] 数组字面量
            Expr::Array(exprs) => {
                let mut values = Vec::with_capacity(exprs.len());
                for e in exprs {
                    values.push(Self::eval(e, ctx)?);
                }
                Ok(Value::Array(values))
            }
            // Phase 3.32: left OP ANY(right) / left OP SOME(right)
            // PG 语义：right 求值为数组；存在任一元素 elem 使 left OP elem 为 true → true
            // 若数组为空 → false；NULL 行为遵循 PG：右操作数为 NULL 时返回 NULL
            Expr::AnyOp { left, op, right } => {
                let l = Self::eval(left, ctx)?;
                let r = Self::eval(right, ctx)?;
                match r {
                    Value::Null => Ok(Value::Null),
                    Value::Array(elems) => {
                        if elems.is_empty() {
                            return Ok(Value::Bool(false));
                        }
                        for elem in elems {
                            // NULL 元素被跳过（PG 语义：NULL 不满足任何比较）
                            if matches!(elem, Value::Null) {
                                continue;
                            }
                            let cmp = Self::eval_binary_op(*op, l.clone(), elem)?;
                            if matches!(cmp, Value::Bool(true)) {
                                return Ok(Value::Bool(true));
                            }
                            // 若 cmp 为 NULL（l 为 NULL 时），继续尝试下一个元素
                        }
                        // 所有非 NULL 元素都不满足 → false
                        Ok(Value::Bool(false))
                    }
                    other => Err(EvalError::TypeMismatch {
                        expected: "array",
                        actual: value_type_name(&other),
                    }),
                }
            }
            // Phase 3.32: left OP ALL(right)
            // PG 语义：right 求值为数组；所有非 NULL 元素 elem 都使 left OP elem 为 true → true
            // 若数组为空 → true；若所有元素都为 NULL → NULL；若 l 为 NULL → NULL
            Expr::AllOp { left, op, right } => {
                let l = Self::eval(left, ctx)?;
                let r = Self::eval(right, ctx)?;
                match r {
                    Value::Null => Ok(Value::Null),
                    Value::Array(elems) => {
                        if elems.is_empty() {
                            return Ok(Value::Bool(true));
                        }
                        let mut saw_non_null = false;
                        let mut saw_null_result = false;
                        for elem in elems {
                            if matches!(elem, Value::Null) {
                                continue;
                            }
                            saw_non_null = true;
                            let cmp = Self::eval_binary_op(*op, l.clone(), elem)?;
                            match cmp {
                                Value::Bool(false) => return Ok(Value::Bool(false)),
                                Value::Null => saw_null_result = true,
                                _ => {}
                            }
                        }
                        if !saw_non_null {
                            // 所有元素都是 NULL → NULL（PG 语义）
                            return Ok(Value::Null);
                        }
                        if saw_null_result {
                            // 至少有一个 NULL 结果且无 false → NULL
                            Ok(Value::Null)
                        } else {
                            Ok(Value::Bool(true))
                        }
                    }
                    other => Err(EvalError::TypeMismatch {
                        expected: "array",
                        actual: value_type_name(&other),
                    }),
                }
            }
            // Phase 6.2: 窗口函数 — 在 Projection 求值前已由 substitute_window_functions
            // 替换为 Expr::Literal，因此此处不应到达；若到达则报错
            Expr::WindowFunction { name, .. } => Err(EvalError::Unsupported(format!(
                "window function '{name}' must be materialized by Window node before evaluation"
            ))),
        }
    }

    // -----------------------------------------------------------------
    //  二元运算
    // -----------------------------------------------------------------

    fn to_bool_or_null(v: Value) -> Result<Value, EvalError> {
        match v {
            Value::Bool(b) => Ok(Value::Bool(b)),
            Value::Null => Ok(Value::Null),
            other => Err(EvalError::TypeMismatch {
                expected: "bool",
                actual: value_type_name(&other),
            }),
        }
    }

    fn eval_binary_op(op: BinaryOp, l: Value, r: Value) -> Result<Value, EvalError> {
        // NULL 处理：所有二元运算（除 IS NULL）任一操作数为 NULL → NULL
        if matches!(l, Value::Null) || matches!(r, Value::Null) {
            return Ok(Value::Null);
        }
        match op {
            BinaryOp::Plus => Self::numeric_binary(&l, &r, i64::checked_add, |a, b| a + b),
            BinaryOp::Minus => Self::numeric_binary(&l, &r, i64::checked_sub, |a, b| a - b),
            BinaryOp::Multiply => Self::numeric_binary(&l, &r, i64::checked_mul, |a, b| a * b),
            BinaryOp::Divide => Self::eval_divide(&l, &r),
            BinaryOp::Modulo => Self::eval_modulo(&l, &r),
            BinaryOp::Eq
            | BinaryOp::NotEq
            | BinaryOp::Lt
            | BinaryOp::LtEq
            | BinaryOp::Gt
            | BinaryOp::GtEq => Self::eval_compare(op, &l, &r),
            BinaryOp::BitAnd => Self::eval_bitwise(&l, &r, |a, b| a & b),
            BinaryOp::BitOr => Self::eval_bitwise(&l, &r, |a, b| a | b),
            BinaryOp::BitXor => Self::eval_bitwise(&l, &r, |a, b| a ^ b),
            BinaryOp::ShiftLeft => Self::eval_bitwise(&l, &r, |a, b| a.wrapping_shl(b as u32)),
            BinaryOp::ShiftRight => Self::eval_bitwise(&l, &r, |a, b| a.wrapping_shr(b as u32)),
            BinaryOp::StringConcat => Self::eval_string_concat(&l, &r),
            BinaryOp::AtAt => Self::eval_ts_match(&l, &r),
            BinaryOp::And | BinaryOp::Or => unreachable!("AND/OR 已在短路求值中处理"),
        }
    }

    /// PG 全文检索匹配 `tsvector @@ tsquery` → bool（Phase 3.33）
    ///
    /// 规则：
    /// - 任一操作数为 NULL → NULL
    /// - 左操作数自动从 Text 解析为 TsVector（若已是 TsVector 则直接使用）
    /// - 右操作数自动从 Text 解析为 TsQuery（若已是 TsQuery 则直接使用）
    /// - 返回 `Value::Bool(q.matches(v))`
    fn eval_ts_match(l: &Value, r: &Value) -> Result<Value, EvalError> {
        if matches!(l, Value::Null) || matches!(r, Value::Null) {
            return Ok(Value::Null);
        }
        let ts: TsVector = match l {
            Value::TsVector(t) => t.clone(),
            Value::Text(s) => TsVector::parse(s)
                .map_err(|e| EvalError::Unsupported(format!("@@ left operand parse error: {e}")))?,
            other => {
                return Err(EvalError::Unsupported(format!(
                    "@@ requires tsvector/text on left, got {}",
                    value_type_name(other)
                )))
            }
        };
        let tq: TsQuery = match r {
            Value::TsQuery(q) => q.clone(),
            Value::Text(s) => TsQuery::parse(s).map_err(|e| {
                EvalError::Unsupported(format!("@@ right operand parse error: {e}"))
            })?,
            other => {
                return Err(EvalError::Unsupported(format!(
                    "@@ requires tsquery/text on right, got {}",
                    value_type_name(other)
                )))
            }
        };
        Ok(Value::Bool(tq.matches(&ts)))
    }

    fn numeric_binary<FInt, FFloat>(
        l: &Value,
        r: &Value,
        f_int: FInt,
        f_float: FFloat,
    ) -> Result<Value, EvalError>
    where
        FInt: Fn(i64, i64) -> Option<i64>,
        FFloat: Fn(f64, f64) -> f64,
    {
        match (l, r) {
            (Value::Int64(a), Value::Int64(b)) => {
                let result = f_int(*a, *b)
                    .ok_or_else(|| EvalError::IntegerOverflow(format!("{a} op {b}")))?;
                Ok(Value::Int64(result))
            }
            (Value::Float64(a), Value::Float64(b)) => {
                let v = f_float(*a, *b);
                if v.is_nan() || v.is_infinite() {
                    return Err(EvalError::FloatOverflow(format!("{v}")));
                }
                Ok(Value::Float64(v))
            }
            (Value::Int64(a), Value::Float64(b)) => {
                let v = f_float(*a as f64, *b);
                if v.is_nan() || v.is_infinite() {
                    return Err(EvalError::FloatOverflow(format!("{v}")));
                }
                Ok(Value::Float64(v))
            }
            (Value::Float64(a), Value::Int64(b)) => {
                let v = f_float(*a, *b as f64);
                if v.is_nan() || v.is_infinite() {
                    return Err(EvalError::FloatOverflow(format!("{v}")));
                }
                Ok(Value::Float64(v))
            }
            _ => Err(EvalError::TypeMismatch {
                expected: "numeric",
                actual: value_type_name(l),
            }),
        }
    }

    fn eval_divide(l: &Value, r: &Value) -> Result<Value, EvalError> {
        match (l, r) {
            (Value::Int64(_), Value::Int64(0)) => Err(EvalError::DivisionByZero),
            (Value::Int64(a), Value::Int64(b)) => {
                // 先检查 i64::MIN / -1 溢出（必须在 div_euclid 之前，否则 panic）
                if *a == i64::MIN && *b == -1 {
                    return Err(EvalError::IntegerOverflow(format!("{a} / {b}")));
                }
                // PG 整数除法向 0 截断
                Ok(Value::Int64(a / b))
            }
            (Value::Float64(_), Value::Float64(b)) if *b == 0.0 => Err(EvalError::DivisionByZero),
            (Value::Float64(a), Value::Float64(b)) => {
                let v = a / b;
                if v.is_infinite() {
                    return Err(EvalError::FloatOverflow(format!("{v}")));
                }
                Ok(Value::Float64(v))
            }
            (Value::Int64(a), Value::Float64(b)) => {
                if *b == 0.0 {
                    return Err(EvalError::DivisionByZero);
                }
                let v = *a as f64 / b;
                if v.is_infinite() {
                    return Err(EvalError::FloatOverflow(format!("{v}")));
                }
                Ok(Value::Float64(v))
            }
            (Value::Float64(a), Value::Int64(b)) => {
                if *b == 0 {
                    return Err(EvalError::DivisionByZero);
                }
                let v = a / *b as f64;
                if v.is_infinite() {
                    return Err(EvalError::FloatOverflow(format!("{v}")));
                }
                Ok(Value::Float64(v))
            }
            _ => Err(EvalError::TypeMismatch {
                expected: "numeric",
                actual: value_type_name(l),
            }),
        }
    }

    fn eval_modulo(l: &Value, r: &Value) -> Result<Value, EvalError> {
        match (l, r) {
            (Value::Int64(_), Value::Int64(0)) => Err(EvalError::DivisionByZero),
            (Value::Int64(a), Value::Int64(b)) => {
                if *a == i64::MIN && *b == -1 {
                    return Ok(Value::Int64(0));
                }
                Ok(Value::Int64(a % b))
            }
            (Value::Float64(_), Value::Float64(b)) if *b == 0.0 => Err(EvalError::DivisionByZero),
            (Value::Float64(a), Value::Float64(b)) => Ok(Value::Float64(a % b)),
            _ => Err(EvalError::TypeMismatch {
                expected: "numeric",
                actual: value_type_name(l),
            }),
        }
    }

    fn eval_compare(op: BinaryOp, l: &Value, r: &Value) -> Result<Value, EvalError> {
        // NULL 已由 eval_binary_op 处理，此处双保险
        if matches!(l, Value::Null) || matches!(r, Value::Null) {
            return Ok(Value::Null);
        }
        match compare_values(l, r) {
            Some(ord) => {
                use std::cmp::Ordering;
                let result = match op {
                    BinaryOp::Eq => ord == Ordering::Equal,
                    BinaryOp::NotEq => ord != Ordering::Equal,
                    BinaryOp::Lt => ord == Ordering::Less,
                    BinaryOp::LtEq => ord != Ordering::Greater,
                    BinaryOp::Gt => ord == Ordering::Greater,
                    BinaryOp::GtEq => ord != Ordering::Less,
                    _ => unreachable!("非比较运算符进入 eval_compare"),
                };
                Ok(Value::Bool(result))
            }
            None => Err(EvalError::TypeMismatch {
                expected: "comparable",
                actual: value_type_name(l),
            }),
        }
    }

    fn eval_bitwise<F>(l: &Value, r: &Value, f: F) -> Result<Value, EvalError>
    where
        F: Fn(i64, i64) -> i64,
    {
        match (l, r) {
            (Value::Int64(a), Value::Int64(b)) => Ok(Value::Int64(f(*a, *b))),
            _ => Err(EvalError::TypeMismatch {
                expected: "int64",
                actual: value_type_name(l),
            }),
        }
    }

    fn eval_string_concat(l: &Value, r: &Value) -> Result<Value, EvalError> {
        let ls = value_to_text(l);
        let rs = value_to_text(r);
        match (ls, rs) {
            (Some(a), Some(b)) => Ok(Value::Text(a + &b)),
            _ => Ok(Value::Null),
        }
    }

    // -----------------------------------------------------------------
    //  一元运算
    // -----------------------------------------------------------------

    fn eval_unary_op(op: UnaryOp, v: Value) -> Result<Value, EvalError> {
        match op {
            UnaryOp::Plus => match v {
                Value::Int64(_) | Value::Float64(_) => Ok(v),
                Value::Null => Ok(Value::Null),
                other => Err(EvalError::TypeMismatch {
                    expected: "numeric",
                    actual: value_type_name(&other),
                }),
            },
            UnaryOp::Minus => match v {
                Value::Int64(n) => n
                    .checked_neg()
                    .map(Value::Int64)
                    .ok_or_else(|| EvalError::IntegerOverflow(format!("-({n})"))),
                Value::Float64(f) => Ok(Value::Float64(-f)),
                Value::Null => Ok(Value::Null),
                other => Err(EvalError::TypeMismatch {
                    expected: "numeric",
                    actual: value_type_name(&other),
                }),
            },
            UnaryOp::Not => match v {
                Value::Bool(b) => Ok(Value::Bool(!b)),
                Value::Null => Ok(Value::Null),
                other => Err(EvalError::TypeMismatch {
                    expected: "bool",
                    actual: value_type_name(&other),
                }),
            },
            UnaryOp::BitNot => match v {
                Value::Int64(n) => Ok(Value::Int64(!n)),
                Value::Null => Ok(Value::Null),
                other => Err(EvalError::TypeMismatch {
                    expected: "int64",
                    actual: value_type_name(&other),
                }),
            },
        }
    }

    // -----------------------------------------------------------------
    //  CASE 表达式
    // -----------------------------------------------------------------

    fn eval_case(
        operand: &Option<Box<Expr>>,
        when_then: &[(Expr, Expr)],
        else_expr: &Option<Box<Expr>>,
        ctx: &dyn EvalContext,
    ) -> Result<Value, EvalError> {
        if let Some(op_expr) = operand {
            // 简单 CASE：CASE x WHEN v1 THEN r1 WHEN v2 THEN r2 ELSE r END
            let op_val = Self::eval(op_expr, ctx)?;
            for (when, then) in when_then {
                let when_val = Self::eval(when, ctx)?;
                if matches!(values_equal(&op_val, &when_val)?, Value::Bool(true)) {
                    return Self::eval(then, ctx);
                }
            }
        } else {
            // 搜索 CASE：CASE WHEN c1 THEN r1 WHEN c2 THEN r2 ELSE r END
            for (when, then) in when_then {
                let when_val = Self::eval(when, ctx)?;
                if let Value::Bool(true) = when_val {
                    return Self::eval(then, ctx);
                }
            }
        }
        if let Some(e) = else_expr {
            Self::eval(e, ctx)
        } else {
            Ok(Value::Null)
        }
    }

    // -----------------------------------------------------------------
    //  IN / BETWEEN / LIKE
    // -----------------------------------------------------------------

    fn eval_in_list(
        expr: &Expr,
        list: &[Expr],
        negated: bool,
        ctx: &dyn EvalContext,
    ) -> Result<Value, EvalError> {
        let v = Self::eval(expr, ctx)?;
        if matches!(v, Value::Null) {
            return Ok(Value::Null);
        }
        let mut found_null = false;
        for item in list {
            let item_val = Self::eval(item, ctx)?;
            match values_equal(&v, &item_val)? {
                Value::Bool(true) => return Ok(Value::Bool(!negated)),
                Value::Null => found_null = true,
                _ => {}
            }
        }
        if found_null {
            Ok(Value::Null)
        } else {
            Ok(Value::Bool(negated))
        }
    }

    fn eval_between(
        expr: &Expr,
        low: &Expr,
        high: &Expr,
        negated: bool,
        ctx: &dyn EvalContext,
    ) -> Result<Value, EvalError> {
        let v = Self::eval(expr, ctx)?;
        let lo = Self::eval(low, ctx)?;
        let hi = Self::eval(high, ctx)?;
        // 任一 NULL → NULL
        if matches!(v, Value::Null) || matches!(lo, Value::Null) || matches!(hi, Value::Null) {
            return Ok(Value::Null);
        }
        let ge_low = match compare_values(&v, &lo) {
            Some(ord) => ord != std::cmp::Ordering::Less,
            None => {
                return Err(EvalError::TypeMismatch {
                    expected: "comparable",
                    actual: value_type_name(&v),
                });
            }
        };
        let le_high = match compare_values(&v, &hi) {
            Some(ord) => ord != std::cmp::Ordering::Greater,
            None => {
                return Err(EvalError::TypeMismatch {
                    expected: "comparable",
                    actual: value_type_name(&v),
                });
            }
        };
        let in_range = ge_low && le_high;
        Ok(Value::Bool(if negated {
            !in_range
        } else {
            in_range
        }))
    }

    fn eval_like(
        expr: &Expr,
        pattern: &Expr,
        negated: bool,
        ctx: &dyn EvalContext,
    ) -> Result<Value, EvalError> {
        let v = Self::eval(expr, ctx)?;
        let p = Self::eval(pattern, ctx)?;
        match (v, p) {
            (Value::Text(s), Value::Text(pat)) => {
                let regex = like_to_regex(&pat)?;
                let matched = regex.is_match(&s);
                Ok(Value::Bool(if negated {
                    !matched
                } else {
                    matched
                }))
            }
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (other, _) => Err(EvalError::TypeMismatch {
                expected: "text",
                actual: value_type_name(&other),
            }),
        }
    }

    // -----------------------------------------------------------------
    //  函数求值
    // -----------------------------------------------------------------

    fn eval_function(
        name: &str,
        args: &[Expr],
        distinct: bool,
        ctx: &dyn EvalContext,
    ) -> Result<Value, EvalError> {
        let fname = name.to_lowercase();
        // 聚合函数在非聚合上下文下报错
        if is_aggregate_function(&fname) {
            return Err(EvalError::Unsupported(format!(
                "aggregate function `{fname}` cannot be evaluated in row context"
            )));
        }
        let _ = distinct; // 聚合 DISTINCT 在行内无意义
        let arg_vals: Result<Vec<Value>, EvalError> =
            args.iter().map(|a| Self::eval(a, ctx)).collect();
        let arg_vals = arg_vals?;
        match fname.as_str() {
            "upper" => {
                check_arg_count(&fname, &arg_vals, 1)?;
                match &arg_vals[0] {
                    Value::Text(s) => Ok(Value::Text(s.to_uppercase())),
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "text",
                        actual: value_type_name(other),
                    }),
                }
            }
            "lower" => {
                check_arg_count(&fname, &arg_vals, 1)?;
                match &arg_vals[0] {
                    Value::Text(s) => Ok(Value::Text(s.to_lowercase())),
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "text",
                        actual: value_type_name(other),
                    }),
                }
            }
            "length" | "char_length" | "character_length" => {
                check_arg_count(&fname, &arg_vals, 1)?;
                match &arg_vals[0] {
                    Value::Text(s) => Ok(Value::Int64(s.chars().count() as i64)),
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "text",
                        actual: value_type_name(other),
                    }),
                }
            }
            "octet_length" => {
                check_arg_count(&fname, &arg_vals, 1)?;
                match &arg_vals[0] {
                    Value::Text(s) => Ok(Value::Int64(s.len() as i64)),
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "text",
                        actual: value_type_name(other),
                    }),
                }
            }
            "abs" => {
                check_arg_count(&fname, &arg_vals, 1)?;
                match &arg_vals[0] {
                    Value::Int64(n) => Ok(Value::Int64(n.wrapping_abs())),
                    Value::Float64(f) => Ok(Value::Float64(f.abs())),
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "numeric",
                        actual: value_type_name(other),
                    }),
                }
            }
            "round" => {
                // round(x) 或 round(x, n)
                if arg_vals.is_empty() || arg_vals.len() > 2 {
                    return Err(EvalError::InvalidFunctionArgs(format!(
                        "{fname} expects 1 or 2 args, got {}",
                        arg_vals.len()
                    )));
                }
                let n = if arg_vals.len() == 2 {
                    match &arg_vals[1] {
                        Value::Int64(n) => *n as i32,
                        Value::Null => return Ok(Value::Null),
                        other => {
                            return Err(EvalError::TypeMismatch {
                                expected: "int",
                                actual: value_type_name(other),
                            });
                        }
                    }
                } else {
                    0
                };
                match &arg_vals[0] {
                    Value::Float64(f) => {
                        let factor = 10_f64.powi(n);
                        Ok(Value::Float64((f * factor).round() / factor))
                    }
                    Value::Int64(_) => Ok(arg_vals[0].clone()),
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "numeric",
                        actual: value_type_name(other),
                    }),
                }
            }
            "ceil" | "ceiling" => {
                check_arg_count(&fname, &arg_vals, 1)?;
                match &arg_vals[0] {
                    Value::Float64(f) => Ok(Value::Float64(f.ceil())),
                    Value::Int64(_) => Ok(arg_vals[0].clone()),
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "numeric",
                        actual: value_type_name(other),
                    }),
                }
            }
            "floor" => {
                check_arg_count(&fname, &arg_vals, 1)?;
                match &arg_vals[0] {
                    Value::Float64(f) => Ok(Value::Float64(f.floor())),
                    Value::Int64(_) => Ok(arg_vals[0].clone()),
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "numeric",
                        actual: value_type_name(other),
                    }),
                }
            }
            "coalesce" => {
                if arg_vals.is_empty() {
                    return Err(EvalError::InvalidFunctionArgs(
                        "coalesce requires at least 1 arg".into(),
                    ));
                }
                for v in &arg_vals {
                    if !matches!(v, Value::Null) {
                        return Ok(v.clone());
                    }
                }
                Ok(Value::Null)
            }
            "nullif" => {
                check_arg_count(&fname, &arg_vals, 2)?;
                if values_equal(&arg_vals[0], &arg_vals[1])? == Value::Bool(true) {
                    Ok(Value::Null)
                } else {
                    Ok(arg_vals[0].clone())
                }
            }
            "concat" => {
                let mut s = String::new();
                for v in &arg_vals {
                    if let Some(t) = value_to_text(v) {
                        s.push_str(&t);
                    } else {
                        // NULL 在 concat 中被忽略（与 PG 行为一致）
                    }
                }
                Ok(Value::Text(s))
            }
            "trim" => {
                check_arg_count(&fname, &arg_vals, 1)?;
                match &arg_vals[0] {
                    Value::Text(s) => Ok(Value::Text(s.trim().to_string())),
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "text",
                        actual: value_type_name(other),
                    }),
                }
            }
            "ltrim" => {
                check_arg_count(&fname, &arg_vals, 1)?;
                match &arg_vals[0] {
                    Value::Text(s) => Ok(Value::Text(s.trim_start().to_string())),
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "text",
                        actual: value_type_name(other),
                    }),
                }
            }
            "rtrim" => {
                check_arg_count(&fname, &arg_vals, 1)?;
                match &arg_vals[0] {
                    Value::Text(s) => Ok(Value::Text(s.trim_end().to_string())),
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "text",
                        actual: value_type_name(other),
                    }),
                }
            }
            "replace" => {
                if arg_vals.len() != 3 {
                    return Err(EvalError::InvalidFunctionArgs(format!(
                        "{fname} expects 3 args, got {}",
                        arg_vals.len()
                    )));
                }
                match (&arg_vals[0], &arg_vals[1], &arg_vals[2]) {
                    (Value::Text(s), Value::Text(from), Value::Text(to)) => {
                        Ok(Value::Text(s.replace(from.as_str(), to.as_str())))
                    }
                    (Value::Null, _, _) | (_, Value::Null, _) | (_, _, Value::Null) => {
                        Ok(Value::Null)
                    }
                    _ => Err(EvalError::TypeMismatch {
                        expected: "text",
                        actual: "non-text",
                    }),
                }
            }
            "substring" | "substr" => {
                if arg_vals.is_empty() || arg_vals.len() > 3 {
                    return Err(EvalError::InvalidFunctionArgs(format!(
                        "{fname} expects 2-3 args, got {}",
                        arg_vals.len()
                    )));
                }
                match &arg_vals[0] {
                    Value::Text(s) => {
                        let start = match &arg_vals[1] {
                            Value::Int64(n) => *n,
                            Value::Null => return Ok(Value::Null),
                            other => {
                                return Err(EvalError::TypeMismatch {
                                    expected: "int",
                                    actual: value_type_name(other),
                                });
                            }
                        };
                        let chars: Vec<char> = s.chars().collect();
                        // PG substring is 1-based
                        let start_idx = if start < 1 {
                            0
                        } else {
                            (start - 1) as usize
                        };
                        let end_idx = if arg_vals.len() == 3 {
                            let length = match &arg_vals[2] {
                                Value::Int64(n) => *n,
                                Value::Null => return Ok(Value::Null),
                                other => {
                                    return Err(EvalError::TypeMismatch {
                                        expected: "int",
                                        actual: value_type_name(other),
                                    });
                                }
                            };
                            if length < 0 {
                                return Ok(Value::Text(String::new()));
                            }
                            (start_idx + length as usize).min(chars.len())
                        } else {
                            chars.len()
                        };
                        if start_idx >= chars.len() {
                            Ok(Value::Text(String::new()))
                        } else {
                            let end = end_idx.min(chars.len());
                            Ok(Value::Text(chars[start_idx..end].iter().collect()))
                        }
                    }
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "text",
                        actual: value_type_name(other),
                    }),
                }
            }
            "now" | "current_timestamp" => {
                // 用系统时间，简化测试
                use std::time::{SystemTime, UNIX_EPOCH};
                let secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_micros() as i64)
                    .unwrap_or(0);
                Ok(Value::Timestamp(secs))
            }
            // Phase 3.32: array_length(arr) — 返回数组长度（一维）；NULL 输入返回 NULL
            "array_length" => {
                check_arg_count(&fname, &arg_vals, 1)?;
                match &arg_vals[0] {
                    Value::Array(elems) => Ok(Value::Int64(elems.len() as i64)),
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "array",
                        actual: value_type_name(other),
                    }),
                }
            }
            // Phase 3.32: cardinality(arr) — 返回数组元素总数（含多维展开）
            // 简化实现：仅支持一维；多维递归统计
            "cardinality" => {
                check_arg_count(&fname, &arg_vals, 1)?;
                fn count_elements(v: &Value) -> i64 {
                    match v {
                        Value::Array(elems) => {
                            let mut total = 0_i64;
                            for e in elems {
                                total += count_elements(e);
                            }
                            total
                        }
                        _ => 1,
                    }
                }
                match &arg_vals[0] {
                    Value::Array(elems) => {
                        let mut total = 0_i64;
                        for e in elems {
                            // NULL 元素在 PG 中不计入 cardinality
                            if !matches!(e, Value::Null) {
                                total += count_elements(e);
                            }
                        }
                        Ok(Value::Int64(total))
                    }
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "array",
                        actual: value_type_name(other),
                    }),
                }
            }
            // Phase 3.32: array_to_string(arr, delimiter) — 用 delimiter 拼接数组元素
            // NULL 元素被跳过（PG 语义）
            "array_to_string" => {
                if arg_vals.len() != 2 {
                    return Err(EvalError::InvalidFunctionArgs(format!(
                        "{fname} expects 2 args (array, delimiter), got {}",
                        arg_vals.len()
                    )));
                }
                let delimiter = match &arg_vals[1] {
                    Value::Text(s) => s.clone(),
                    Value::Null => return Ok(Value::Null),
                    other => {
                        return Err(EvalError::TypeMismatch {
                            expected: "text",
                            actual: value_type_name(other),
                        });
                    }
                };
                match &arg_vals[0] {
                    Value::Array(elems) => {
                        let parts: Vec<String> = elems
                            .iter()
                            .filter(|v| !matches!(v, Value::Null))
                            .filter_map(value_to_text)
                            .collect();
                        Ok(Value::Text(parts.join(&delimiter)))
                    }
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "array",
                        actual: value_type_name(other),
                    }),
                }
            }
            // Phase 3.32: array_append(arr, elem) — 返回新数组（追加 elem 到末尾）
            "array_append" => {
                check_arg_count(&fname, &arg_vals, 2)?;
                match &arg_vals[0] {
                    Value::Array(elems) => {
                        let mut new_arr = elems.clone();
                        new_arr.push(arg_vals[1].clone());
                        Ok(Value::Array(new_arr))
                    }
                    Value::Null => Ok(Value::Array(vec![arg_vals[1].clone()])),
                    _ => Err(EvalError::TypeMismatch {
                        expected: "array",
                        actual: value_type_name(&arg_vals[0]),
                    }),
                }
            }
            // Phase 3.32: array_prepend(elem, arr) — 返回新数组（elem 插入到开头）
            "array_prepend" => {
                check_arg_count(&fname, &arg_vals, 2)?;
                match &arg_vals[1] {
                    Value::Array(elems) => {
                        let mut new_arr = Vec::with_capacity(elems.len() + 1);
                        new_arr.push(arg_vals[0].clone());
                        new_arr.extend(elems.iter().cloned());
                        Ok(Value::Array(new_arr))
                    }
                    Value::Null => Ok(Value::Array(vec![arg_vals[0].clone()])),
                    _ => Err(EvalError::TypeMismatch {
                        expected: "array",
                        actual: value_type_name(&arg_vals[1]),
                    }),
                }
            }
            // Phase 3.32: array_cat(arr1, arr2) — 拼接两个数组
            "array_cat" => {
                check_arg_count(&fname, &arg_vals, 2)?;
                let left = match &arg_vals[0] {
                    Value::Array(e) => e.clone(),
                    Value::Null => Vec::new(),
                    _ => {
                        return Err(EvalError::TypeMismatch {
                            expected: "array",
                            actual: value_type_name(&arg_vals[0]),
                        });
                    }
                };
                let right = match &arg_vals[1] {
                    Value::Array(e) => e.clone(),
                    Value::Null => Vec::new(),
                    _ => {
                        return Err(EvalError::TypeMismatch {
                            expected: "array",
                            actual: value_type_name(&arg_vals[1]),
                        });
                    }
                };
                let mut combined = left;
                combined.extend(right);
                Ok(Value::Array(combined))
            }
            // Phase 3.32: array_contains(arr, elem) — 等价于 arr @> ARRAY[elem]
            // 返回 bool；任一参数为 NULL → NULL
            "array_contains" => {
                check_arg_count(&fname, &arg_vals, 2)?;
                if matches!(arg_vals[0], Value::Null) || matches!(arg_vals[1], Value::Null) {
                    return Ok(Value::Null);
                }
                match &arg_vals[0] {
                    Value::Array(elems) => {
                        let target = &arg_vals[1];
                        let found = elems.iter().any(|e| e == target);
                        Ok(Value::Bool(found))
                    }
                    _ => Err(EvalError::TypeMismatch {
                        expected: "array",
                        actual: value_type_name(&arg_vals[0]),
                    }),
                }
            }
            // Phase 3.32: array_position(arr, elem) — 返回元素在数组中的 1-based 位置
            // 未找到 → NULL；arr 为 NULL → NULL
            "array_position" => {
                check_arg_count(&fname, &arg_vals, 2)?;
                if matches!(arg_vals[0], Value::Null) {
                    return Ok(Value::Null);
                }
                match &arg_vals[0] {
                    Value::Array(elems) => {
                        let target = &arg_vals[1];
                        for (i, e) in elems.iter().enumerate() {
                            if e == target {
                                return Ok(Value::Int64((i + 1) as i64));
                            }
                        }
                        Ok(Value::Null)
                    }
                    _ => Err(EvalError::TypeMismatch {
                        expected: "array",
                        actual: value_type_name(&arg_vals[0]),
                    }),
                }
            }
            // Phase 3.32: unnest(arr) — 在行上下文中返回数组的第一个元素（简化实现）
            // 真正的 unnest 是 SRF（返回集合），需要专门的执行器节点支持。
            // 当前实现：将数组转为 Text 形式 '{1,2,3}'，便于 SELECT unnest(arr) FROM t 时
            // 返回数组的字符串表示。完整 SRF 支持留待后续阶段。
            // 注：此处返回 Value::Array 本身，让上层投影按需处理。
            "unnest" => {
                check_arg_count(&fname, &arg_vals, 1)?;
                match &arg_vals[0] {
                    Value::Array(_) => Ok(arg_vals[0].clone()),
                    Value::Null => Ok(Value::Null),
                    other => Err(EvalError::TypeMismatch {
                        expected: "array",
                        actual: value_type_name(other),
                    }),
                }
            }
            // Phase 3.33: to_tsvector(text) — 将文本分词并构造 TsVector
            //
            // PG 语义：`to_tsvector('hello world')` → `'hello:1 world:2'`
            // 简化分词：按 ASCII 空白拆分，小写化，过滤空 token。
            // 已是 TsVector → 直接返回；NULL → NULL。
            "to_tsvector" => {
                check_arg_count_min(&fname, &arg_vals, 1)?;
                if matches!(arg_vals[0], Value::Null) {
                    return Ok(Value::Null);
                }
                let text = match &arg_vals[0] {
                    Value::Text(s) => s.clone(),
                    Value::TsVector(t) => return Ok(Value::TsVector(t.clone())),
                    other => {
                        return Err(EvalError::TypeMismatch {
                            expected: "text",
                            actual: value_type_name(other),
                        })
                    }
                };
                let lexemes = tokenize_simple(&text);
                Ok(Value::TsVector(TsVector::from_lexemes(lexemes)))
            }
            // Phase 3.33: to_tsquery(text) — 将查询字符串构造为 TsQuery
            //
            // PG 语义：`to_tsquery('hello & world')` → 查询树
            // 输入必须是合法 tsquery 语法（含 `&`、`|`、`!`、`<->`）。
            "to_tsquery" => {
                check_arg_count_min(&fname, &arg_vals, 1)?;
                if matches!(arg_vals[0], Value::Null) {
                    return Ok(Value::Null);
                }
                let text = match &arg_vals[0] {
                    Value::Text(s) => s.clone(),
                    Value::TsQuery(q) => return Ok(Value::TsQuery(q.clone())),
                    other => {
                        return Err(EvalError::TypeMismatch {
                            expected: "text",
                            actual: value_type_name(other),
                        })
                    }
                };
                match TsQuery::parse(&text) {
                    Ok(q) => Ok(Value::TsQuery(q)),
                    Err(e) => Err(EvalError::Unsupported(format!(
                        "to_tsquery parse error: {e}"
                    ))),
                }
            }
            // Phase 3.33: plainto_tsquery(text) — 将普通文本转为 TsQuery
            //
            // PG 语义：`plainto_tsquery('hello world')` → `'hello' & 'world'`
            // 不接受 tsquery 语法符号（&、|、! 等），而是将所有词素用 AND 连接。
            "plainto_tsquery" => {
                check_arg_count_min(&fname, &arg_vals, 1)?;
                if matches!(arg_vals[0], Value::Null) {
                    return Ok(Value::Null);
                }
                let text = match &arg_vals[0] {
                    Value::Text(s) => s.clone(),
                    Value::TsQuery(q) => return Ok(Value::TsQuery(q.clone())),
                    other => {
                        return Err(EvalError::TypeMismatch {
                            expected: "text",
                            actual: value_type_name(other),
                        })
                    }
                };
                let lexemes = tokenize_simple(&text);
                let mut iter = lexemes.into_iter();
                let result = match iter.next() {
                    Some(first) => iter.fold(TsQuery::lexeme(first), |acc, term| {
                        acc.and(TsQuery::lexeme(term))
                    }),
                    None => TsQuery::empty(),
                };
                Ok(Value::TsQuery(result))
            }
            // Phase 3.33: ts_rank(tsvector [, tsquery]) — 返回排名分数（Float64）
            //
            // PG 语义：基于词素频率与权重计算的相关性分数。
            // 简化实现：每个词素贡献 1.0，权重 A/B/C/D 分别叠加 1.2 / 0.4 / 0.2 / 0.1。
            // 不带 tsquery 时：所有词素计入；带 tsquery 时：仅匹配 tsquery 的词素计入。
            "ts_rank" | "ts_rank_cd" => {
                check_arg_count_min(&fname, &arg_vals, 1)?;
                if matches!(arg_vals[0], Value::Null) {
                    return Ok(Value::Null);
                }
                let ts = match &arg_vals[0] {
                    Value::TsVector(t) => t.clone(),
                    Value::Text(s) => TsVector::parse(s)
                        .map_err(|e| EvalError::Unsupported(format!("ts_rank parse error: {e}")))?,
                    other => {
                        return Err(EvalError::TypeMismatch {
                            expected: "tsvector",
                            actual: value_type_name(other),
                        })
                    }
                };
                let filter_q: Option<TsQuery> = if arg_vals.len() >= 2 {
                    match &arg_vals[1] {
                        Value::Null => None,
                        Value::TsQuery(q) => Some(q.clone()),
                        Value::Text(s) => Some(TsQuery::parse(s).map_err(|e| {
                            EvalError::Unsupported(format!("ts_rank query parse error: {e}"))
                        })?),
                        other => {
                            return Err(EvalError::TypeMismatch {
                                expected: "tsquery",
                                actual: value_type_name(other),
                            })
                        }
                    }
                } else {
                    None
                };
                let mut rank = 0.0_f64;
                for lex in &ts.lexemes {
                    if let Some(ref q) = filter_q {
                        // 仅计入命中 tsquery 的词素
                        if !q.matches(&ts) {
                            continue;
                        }
                        // 进一步要求词素出现在 tsquery 中（简化：仅 Lexeme 类型命中）
                        if !tsquery_contains_term(q, &lex.term) {
                            continue;
                        }
                    }
                    for pos in &lex.positions {
                        // 基础分 1.0 + 权重加成
                        rank += 1.0;
                        if pos.weight & TS_WEIGHT_A != 0 {
                            rank += 1.2;
                        }
                        if pos.weight & TS_WEIGHT_B != 0 {
                            rank += 0.4;
                        }
                        if pos.weight & TS_WEIGHT_C != 0 {
                            rank += 0.2;
                        }
                        if pos.weight & TS_WEIGHT_D != 0 {
                            rank += 0.1;
                        }
                    }
                }
                Ok(Value::Float64(rank))
            }
            // Phase 3.33: setweight(tsvector, weight_char) — 修改所有词素的权重
            //
            // PG 语义：`setweight('hello:1 world:2'::tsvector, 'A')` → 所有位置权重加上 A
            // weight_char 取值：'A' / 'B' / 'C' / 'D'（不区分大小写）
            "setweight" => {
                check_arg_count(&fname, &arg_vals, 2)?;
                if matches!(arg_vals[0], Value::Null) || matches!(arg_vals[1], Value::Null) {
                    return Ok(Value::Null);
                }
                let mut ts = match &arg_vals[0] {
                    Value::TsVector(t) => t.clone(),
                    Value::Text(s) => TsVector::parse(s).map_err(|e| {
                        EvalError::Unsupported(format!("setweight parse error: {e}"))
                    })?,
                    other => {
                        return Err(EvalError::TypeMismatch {
                            expected: "tsvector",
                            actual: value_type_name(other),
                        })
                    }
                };
                let weight_char = match &arg_vals[1] {
                    Value::Text(s) => s.trim().to_uppercase(),
                    other => {
                        return Err(EvalError::TypeMismatch {
                            expected: "text (A/B/C/D)",
                            actual: value_type_name(other),
                        })
                    }
                };
                let weight_mask = match weight_char.as_str() {
                    "A" => TS_WEIGHT_A,
                    "B" => TS_WEIGHT_B,
                    "C" => TS_WEIGHT_C,
                    "D" => TS_WEIGHT_D,
                    "" => 0,
                    other => {
                        return Err(EvalError::Unsupported(format!(
                            "setweight weight must be A/B/C/D, got '{other}'"
                        )))
                    }
                };
                for lex in &mut ts.lexemes {
                    for pos in &mut lex.positions {
                        pos.weight = weight_mask;
                    }
                }
                Ok(Value::TsVector(ts))
            }
            _ => Err(EvalError::FunctionNotFound(fname)),
        }
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

fn check_arg_count(fname: &str, args: &[Value], expected: usize) -> Result<(), EvalError> {
    if args.len() != expected {
        Err(EvalError::InvalidFunctionArgs(format!(
            "{fname} expects {expected} arg(s), got {}",
            args.len()
        )))
    } else {
        Ok(())
    }
}

/// 检查函数参数数量 ≥ min_count（用于可选参数场景）
fn check_arg_count_min(fname: &str, args: &[Value], min_count: usize) -> Result<(), EvalError> {
    if args.len() < min_count {
        Err(EvalError::InvalidFunctionArgs(format!(
            "{fname} expects at least {min_count} arg(s), got {}",
            args.len()
        )))
    } else {
        Ok(())
    }
}

/// 简单分词器：按 ASCII 空白拆分，小写化，过滤空 token（Phase 3.33）
///
/// 这是 PG 全文检索分词的简化实现：
/// - 不支持 stemming / thesaurus / stop words
/// - 不支持中文分词（CJK 字符按空白拆分后整体保留）
/// - 仅按空白（含 tab/newline）拆分
fn tokenize_simple(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|s| s.to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 检查 tsquery 是否包含某个词素（递归遍历表达式树，Phase 3.33）
///
/// 仅匹配 Lexeme 节点，不处理 FollowedBy 等组合节点的位置语义。
fn tsquery_contains_term(q: &TsQuery, term: &str) -> bool {
    match q {
        TsQuery::Lexeme { term: t, .. } => t == term,
        TsQuery::And(l, r) | TsQuery::Or(l, r) => {
            tsquery_contains_term(l, term) || tsquery_contains_term(r, term)
        }
        TsQuery::Not(inner) => tsquery_contains_term(inner, term),
        TsQuery::FollowedBy { left, right, .. } => {
            tsquery_contains_term(left, term) || tsquery_contains_term(right, term)
        }
        TsQuery::Empty => false,
    }
}

fn is_aggregate_function(name: &str) -> bool {
    matches!(
        name,
        "count" | "sum" | "avg" | "min" | "max" | "array_agg" | "string_agg"
    )
}

fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Int64(_) => "int64",
        Value::Float64(_) => "float64",
        Value::Text(_) => "text",
        Value::Blob(_) => "blob",
        Value::Bool(_) => "bool",
        Value::Date(_) => "date",
        Value::Timestamp(_) => "timestamp",
        Value::Decimal(_, _) => "decimal",
        Value::Array(_) => "array",
        Value::Enum(_) => "enum",
        Value::Range(_) => "range",
        Value::Json(_) => "json",
        Value::TsVector(_) => "tsvector",
        Value::TsQuery(_) => "tsquery",
    }
}

fn value_to_text(v: &Value) -> Option<String> {
    match v {
        Value::Text(s) => Some(s.clone()),
        Value::Int64(n) => Some(n.to_string()),
        Value::Float64(f) => Some(f.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Date(d) => Some(format!("date({d})")),
        Value::Timestamp(t) => Some(format!("ts({t})")),
        Value::Decimal(v, s) => Some(format!("dec({v},{s})")),
        Value::Enum(s) => Some(s.clone()),
        Value::Blob(b) => Some(format!("blob({b:?})")),
        Value::TsVector(t) => Some(t.to_pg_string()),
        Value::TsQuery(q) => Some(q.to_pg_string()),
        Value::Null => None,
        Value::Array(_) | Value::Range(_) | Value::Json(_) => None,
    }
}

fn values_equal(l: &Value, r: &Value) -> Result<Value, EvalError> {
    if matches!(l, Value::Null) || matches!(r, Value::Null) {
        return Ok(Value::Null);
    }
    Ok(Value::Bool(l == r))
}

/// 类型感知的值比较 — 返回 `Option<Ordering>`，不可比较时返回 `None`
///
/// 支持的跨类型提升：
/// - Int64 ↔ Float64（提升为 f64 比较）
/// - Date ↔ Timestamp（Date 提升为微秒）
/// - Decimal ↔ Decimal（统一到较大 scale 后比较）
///
/// Phase 5.1：改为 `pub` 以供 szrsql-optimizer 统计信息收集 min/max 使用。
/// 不可排序类型（Blob/Array/Range/Json/TsVector/TsQuery）返回 `None`。
/// NULL 与任何值比较返回 `None`（符合 SQL 三值逻辑）。
pub fn compare_values(l: &Value, r: &Value) -> Option<std::cmp::Ordering> {
    match (l, r) {
        (Value::Int64(a), Value::Int64(b)) => Some(a.cmp(b)),
        (Value::Float64(a), Value::Float64(b)) => a.partial_cmp(b),
        (Value::Int64(a), Value::Float64(b)) => (*a as f64).partial_cmp(b),
        (Value::Float64(a), Value::Int64(b)) => a.partial_cmp(&(*b as f64)),
        (Value::Text(a), Value::Text(b)) => Some(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        (Value::Date(a), Value::Date(b)) => Some(a.cmp(b)),
        (Value::Timestamp(a), Value::Timestamp(b)) => Some(a.cmp(b)),
        (Value::Date(a), Value::Timestamp(b)) => {
            // Date(days) → microseconds
            let a_us = i64::from(*a).checked_mul(86_400_000_000)?;
            Some(a_us.cmp(b))
        }
        (Value::Timestamp(a), Value::Date(b)) => {
            let b_us = i64::from(*b).checked_mul(86_400_000_000)?;
            Some(a.cmp(&b_us))
        }
        (Value::Enum(a), Value::Enum(b)) => Some(a.cmp(b)),
        (Value::Decimal(av, as_), Value::Decimal(bv, bs)) => {
            let max_scale = (*as_).max(*bs);
            let a_scaled = scale_decimal(*av, *as_, max_scale)?;
            let b_scaled = scale_decimal(*bv, *bs, max_scale)?;
            Some(a_scaled.cmp(&b_scaled))
        }
        // Blob / Array / Range / Json 暂不支持排序比较
        _ => None,
    }
}

/// 将 Decimal 提升到目标 scale（仅允许从小 scale 到大 scale）
fn scale_decimal(v: i128, from_scale: u8, to_scale: u8) -> Option<i128> {
    if to_scale < from_scale {
        return None;
    }
    let diff = to_scale - from_scale;
    let factor = 10_i128.checked_pow(u32::from(diff))?;
    v.checked_mul(factor)
}

/// LIKE 模式转正则表达式
fn like_to_regex(pattern: &str) -> Result<regex::Regex, EvalError> {
    let mut regex_str = String::from("^");
    for c in pattern.chars() {
        match c {
            '%' => regex_str.push_str(".*"),
            '_' => regex_str.push('.'),
            // 正则元字符转义
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' => {
                regex_str.push('\\');
                regex_str.push(c);
            }
            _ => regex_str.push(c),
        }
    }
    regex_str.push('$');
    regex::Regex::new(&regex_str).map_err(|e| EvalError::InvalidLikePattern(e.to_string()))
}
