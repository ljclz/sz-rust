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
    /// 正则表达式错误
    #[error("invalid regex pattern: {0}")]
    InvalidRegex(String),
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

    /// 尝试调用 UDF（用户定义函数）— P0-SQL-8 修复
    ///
    /// 当 `ExprEvaluator` 在内建函数表中找不到匹配项时，调用此方法
    /// 查询 UDF 注册系统（`UdfRegistry`）。默认实现查询当前线程的
    /// `current_udf_registry`（由 `Executor` 在 SQL 执行入口设置），
    /// 这样无需修改每个 `EvalContext` 实现即可让表达式求值器感知 UDF。
    ///
    /// # 参数
    /// - `name`：函数名（大小写不敏感）
    /// - `args`：已求值的参数值列表
    ///
    /// # 返回
    /// - `Some(Ok(Value))`：UDF 存在且调用成功
    /// - `Some(Err(EvalError))`：UDF 存在但调用失败
    /// - `None`：UDF 不存在（调用方应回退到 `FunctionNotFound`）
    fn try_call_udf(&self, name: &str, args: &[Value]) -> Option<Result<Value, EvalError>> {
        // 1. 先查 UDF 注册表（Rust 原生函数）
        let udf_result = current_udf_registry::with(|opt| {
            let reg = opt.as_ref()?;
            let ctx = crate::udf::UdfContext::default();
            match reg.call(name, args, &ctx) {
                Ok(v) => Some(Ok(v)),
                Err(crate::udf::UdfError::NotFound(_)) => None,
                Err(e) => Some(Err(EvalError::Unsupported(format!(
                    "UDF '{name}' error: {e}"
                )))),
            }
        });
        if udf_result.is_some() {
            return udf_result;
        }
        // 2. 回退：查 SQL 函数注册表（CREATE FUNCTION 创建的函数）— P0-FN 修复
        current_sql_functions::with(|opt| {
            let funcs = opt.as_ref()?;
            let func_defs = funcs.get(&name.to_lowercase())?;
            let def = func_defs
                .iter()
                .find(|f| f.parameters.len() == args.len())?;
            // P0-3 修复：LANGUAGE plpgsql 函数体走 plpgsql_interp 解释器执行
            if def.language.to_lowercase() == "plpgsql" {
                return call_plpgsql_function(name, def, args);
            }
            evaluate_sql_function(def, args)
        })
    }
}

/// 执行 SQL/PLpgSQL 函数体 — P0-FN 修复
///
/// 支持：
/// - `LANGUAGE sql`：body 为 `SELECT expr`，提取 expr 求值
/// - PL/pgSQL：body 含 `RETURN expr`，提取 expr 求值
/// - MySQL `BEGIN RETURN expr; END`：同上
fn evaluate_sql_function(
    def: &crate::plan::FunctionDefinition,
    args: &[Value],
) -> Option<Result<Value, EvalError>> {
    // 构建参数名 → 值的映射
    let mut param_map: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for (i, param) in def.parameters.iter().enumerate() {
        let val = args.get(i).cloned().unwrap_or(Value::Null);
        // PG 允许匿名参数，有名字才建立命名映射
        if let Some(name) = &param.name {
            param_map.insert(name.to_lowercase(), val.clone());
        }
        // PG 风格 $1, $2 也支持
        param_map.insert(format!("${}", i + 1), val);
    }

    let body = def.body.trim();
    let language = def.language.to_lowercase();

    // 提取返回表达式
    let expr_str = if language == "sql" {
        // LANGUAGE sql: body 形如 "SELECT expr" 或 "SELECT expr FROM ..."
        let upper = body.to_uppercase();
        if upper.starts_with("SELECT ") {
            // 取 SELECT 后到 FROM/; 之间的部分
            let after_select = &body[7..]; // len("SELECT ") = 7
            let end = if let Some(pos) = after_select.to_uppercase().find(" FROM ") {
                pos
            } else if let Some(pos) = after_select.find(';') {
                pos
            } else {
                after_select.len()
            };
            after_select[..end].trim()
        } else {
            // 直接作为表达式
            body.trim_end_matches(';')
        }
    } else {
        // PL/pgSQL 或 MySQL BEGIN...END：提取 RETURN 后的表达式
        let upper = body.to_uppercase();
        // clippy question_mark 误报：两个分支搜索不同模式（"RETURN " vs "RETURN"），
        // 分支 1 未命中时必须继续尝试分支 2，无法用 ? 重写
        #[allow(clippy::question_mark)]
        if let Some(pos) = upper.find("RETURN ") {
            let after_return = &body[pos + 7..]; // len("RETURN ") = 7
                                                 // 去掉尾部的 ; END 等
            let end = after_return
                .to_uppercase()
                .find("END")
                .unwrap_or_else(|| after_return.find(';').unwrap_or(after_return.len()));
            after_return[..end].trim()
        } else if let Some(pos) = upper.find("RETURN") {
            let after_return = &body[pos + 6..];
            let end = after_return
                .to_uppercase()
                .find("END")
                .unwrap_or_else(|| after_return.find(';').unwrap_or(after_return.len()));
            after_return[..end].trim()
        } else {
            // 无法解析函数体，返回 None
            return None;
        }
    };

    if expr_str.is_empty() {
        return None;
    }

    // 解析表达式并求值
    // 使用 SQL 解析器解析表达式
    let parse_sql = format!("SELECT {}", expr_str);
    match crate::parser::parse_sql(&parse_sql) {
        Ok(stmts) => {
            if let Some(crate::ast::Statement::Select(select)) = stmts.into_iter().next() {
                // 提取投影表达式
                if let Some(crate::ast::SelectItem::UnnamedExpr(expr)) = select.projection.first() {
                    // 创建参数上下文求值
                    let ctx = FunctionArgContext { params: param_map };
                    Some(ExprEvaluator::eval(expr, &ctx))
                } else {
                    None
                }
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

/// P0-3 辅助：通过 plpgsql_interp 解释器执行 LANGUAGE plpgsql 函数。
///
/// 从 `current_plpgsql_interp` thread_local 获取函数注册表，
/// 创建 `PlPgSqlInterpreter` 并调用函数。
fn call_plpgsql_function(
    name: &str,
    _def: &crate::plan::FunctionDefinition,
    args: &[Value],
) -> Option<Result<Value, EvalError>> {
    current_plpgsql_interp::with(|opt| {
        let registry_arc = opt.as_ref()?;
        let registry = registry_arc
            .lock()
            .map_err(|e| EvalError::Unsupported(format!("plpgsql registry lock poisoned: {e}")))
            .ok()?;
        let mut interp = crate::plpgsql_interp::PlPgSqlInterpreter::new(&registry);
        match interp.call(name, args) {
            Ok(Some(v)) => Some(Ok(v)),
            Ok(None) => Some(Ok(Value::Null)), // void 函数
            Err(crate::plpgsql_interp::PlInterpError::FunctionNotFound(_)) => None,
            Err(e) => Some(Err(EvalError::Unsupported(format!(
                "plpgsql function '{name}' error: {e}"
            )))),
        }
    })
}

/// 函数参数求值上下文 — 将函数参数名映射为值
struct FunctionArgContext {
    params: std::collections::HashMap<String, Value>,
}

impl EvalContext for FunctionArgContext {
    fn lookup_column(&self, name: &str) -> Result<Value, EvalError> {
        // 函数参数上下文中找不到列名应返回错误（而非 None），便于上层捕获
        match self.params.get(&name.to_lowercase()) {
            Some(v) => Ok(v.clone()),
            None => Err(EvalError::ColumnNotFound(name.to_string())),
        }
    }

    fn try_call_udf(&self, name: &str, args: &[Value]) -> Option<Result<Value, EvalError>> {
        // 嵌套函数调用：复用默认实现
        current_udf_registry::with(|opt| {
            let reg = opt.as_ref()?;
            let ctx = crate::udf::UdfContext::default();
            match reg.call(name, args, &ctx) {
                Ok(v) => Some(Ok(v)),
                Err(crate::udf::UdfError::NotFound(_)) => None,
                Err(e) => Some(Err(EvalError::Unsupported(format!(
                    "UDF '{name}' error: {e}"
                )))),
            }
        })
    }
}

// =====================================================================
//  线程局部 UDF 注册表 — P0-SQL-8 修复
// =====================================================================

/// 当前线程绑定的 UDF 注册表。
///
/// `ExprEvaluator` 在内建函数表未命中时通过 `current_udf_registry::with`
/// 查询此注册表。由 `Executor` 在执行 SQL 期间通过
/// `current_udf_registry::set` / `current_udf_registry::clear` 维护。
///
/// 设计动机：避免修改 23+ 处 `ExecRowContext::new` 调用点和 6+ 个
/// `EvalContext` 实现来传递 `&UdfRegistry` 引用。thread_local 方案
/// 最小侵入式接入，且 `UdfRegistry::call` 仅需 `&self`。
///
/// 使用 `Arc<UdfRegistry>` 而非 `&'static` 以避免静态生命周期约束；
/// 每次 `set` 克隆一次 `Arc`（仅在 SQL 执行入口，开销可忽略）。
pub mod current_udf_registry {
    use crate::udf::UdfRegistry;
    use std::cell::RefCell;
    use std::sync::Arc;

    thread_local! {
        static CURRENT: RefCell<Option<Arc<UdfRegistry>>> = const { RefCell::new(None) };
        /// 嵌套深度计数器：仅最外层 set/clear 实际操作 thread_local
        static DEPTH: RefCell<u32> = const { RefCell::new(0) };
    }

    /// 在闭包中访问当前线程的 UDF 注册表
    pub fn with<F, R>(f: F) -> R
    where
        F: FnOnce(&Option<Arc<UdfRegistry>>) -> R,
    {
        CURRENT.with(|cell| f(&cell.borrow()))
    }

    /// 设置当前线程的 UDF 注册表（若未设置），返回 RAII guard
    ///
    /// guard 析构时自动清理（仅最外层 guard 实际执行清理）。
    /// 嵌套调用安全：内层 guard 不重复设置/清理。
    pub fn guard(registry: Arc<UdfRegistry>) -> UdfGuard {
        let was_set = CURRENT.with(|cell| cell.borrow().is_some());
        if !was_set {
            CURRENT.with(|cell| {
                *cell.borrow_mut() = Some(registry);
            });
        }
        DEPTH.with(|d| *d.borrow_mut() += 1);
        UdfGuard {
            was_set, // 当前调用是否真正执行了 set
        }
    }

    /// RAII guard — 析构时清理 thread_local UDF 注册表
    pub struct UdfGuard {
        was_set: bool,
    }

    impl Drop for UdfGuard {
        fn drop(&mut self) {
            DEPTH.with(|d| {
                let mut depth = d.borrow_mut();
                *depth = depth.saturating_sub(1);
                // 仅当深度归零且当前 guard 是最初设置者（was_set=false）时清理
                // was_set=true 表示 thread_local 已被外层 guard 设置，本 guard 不应清理
                if *depth == 0 && !self.was_set {
                    CURRENT.with(|cell| {
                        *cell.borrow_mut() = None;
                    });
                }
            });
        }
    }
}

// =====================================================================
//  线程局部 SQL 函数注册表 — P0-FN 修复
// =====================================================================

/// 当前线程绑定的 SQL 函数定义注册表。
///
/// `ExprEvaluator` 在内建函数表和 UDF 注册表未命中时，通过
/// `current_sql_functions::with` 查询此注册表，执行 `CREATE FUNCTION`
/// 创建的 SQL/PLpgSQL 函数体。
///
/// 由 `Executor` 在执行 SQL 期间通过 `current_sql_functions::guard` 维护。
pub mod current_sql_functions {
    use crate::plan::FunctionDefinition;
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static CURRENT: RefCell<Option<HashMap<String, Vec<FunctionDefinition>>>> =
            const { RefCell::new(None) };
        /// 嵌套深度计数器：仅最外层 guard 实际设置/清理 thread_local
        static DEPTH: RefCell<u32> = const { RefCell::new(0) };
    }

    /// 在闭包中访问当前线程的 SQL 函数注册表
    pub fn with<F, R>(f: F) -> R
    where
        F: FnOnce(&Option<HashMap<String, Vec<FunctionDefinition>>>) -> R,
    {
        CURRENT.with(|cell| f(&cell.borrow()))
    }

    /// 设置当前线程的 SQL 函数注册表（若未设置），返回 RAII guard
    ///
    /// guard 析构时自动清理（仅最外层 guard 实际执行清理）。
    /// 嵌套调用安全：内层 guard 不重复设置/清理。
    pub fn guard(functions: HashMap<String, Vec<FunctionDefinition>>) -> SqlFuncGuard {
        let was_set = CURRENT.with(|cell| cell.borrow().is_some());
        if !was_set {
            CURRENT.with(|cell| {
                *cell.borrow_mut() = Some(functions);
            });
        }
        DEPTH.with(|d| *d.borrow_mut() += 1);
        SqlFuncGuard { was_set }
    }

    /// RAII guard — 析构时清理 thread_local SQL 函数注册表
    pub struct SqlFuncGuard {
        was_set: bool,
    }

    impl Drop for SqlFuncGuard {
        fn drop(&mut self) {
            DEPTH.with(|d| {
                let mut depth = d.borrow_mut();
                *depth = depth.saturating_sub(1);
                // 仅当深度归零且当前 guard 是最初设置者（was_set=false）时清理
                // was_set=true 表示 thread_local 已被外层 guard 设置，本 guard 不应清理
                if *depth == 0 && !self.was_set {
                    CURRENT.with(|cell| {
                        *cell.borrow_mut() = None;
                    });
                }
            });
        }
    }
}

/// 当前线程绑定的 PL/pgSQL 函数注册表。
///
/// `ExprEvaluator::try_call_udf` 在查询 SQL 函数注册表时，若发现函数语言为
/// `LANGUAGE plpgsql`，则通过此 thread_local 获取 `FunctionRegistry`，
/// 创建 `PlPgSqlInterpreter` 并执行函数体。
///
/// 由 `Executor` 在执行 SQL 期间通过 `current_plpgsql_interp::guard` 维护。
/// 存储 `Arc<Mutex<FunctionRegistry>>`：Arc 使 guard 可共享，Mutex 提供
/// `&mut FunctionRegistry`（PlPgSqlInterpreter::call 需要可变引用）。
pub mod current_plpgsql_interp {
    use crate::plpgsql_interp::FunctionRegistry;
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};

    thread_local! {
        static CURRENT: RefCell<Option<Arc<Mutex<FunctionRegistry>>>> =
            const { RefCell::new(None) };
    }

    /// 在闭包中访问当前线程的 PL/pgSQL 函数注册表
    pub fn with<F, R>(f: F) -> R
    where
        F: FnOnce(&Option<Arc<Mutex<FunctionRegistry>>>) -> R,
    {
        CURRENT.with(|cell| f(&cell.borrow()))
    }

    /// 设置当前线程的 PL/pgSQL 函数注册表（返回 RAII guard）
    ///
    /// guard 析构时自动清理 thread_local。
    pub fn guard(registry: Arc<Mutex<FunctionRegistry>>) -> PlPgGuard {
        CURRENT.with(|cell| {
            *cell.borrow_mut() = Some(registry);
        });
        PlPgGuard
    }

    /// RAII guard — 析构时清理 thread_local PL/pgSQL 注册表
    pub struct PlPgGuard;

    impl Drop for PlPgGuard {
        fn drop(&mut self) {
            CURRENT.with(|cell| {
                *cell.borrow_mut() = None;
            });
        }
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
                case_insensitive,
            } => Self::eval_like(expr, pattern, *negated, *case_insensitive, ctx),
            Expr::IsNull { expr, negated } => {
                let v = Self::eval(expr, ctx)?;
                let is_null = matches!(v, Value::Null);
                Ok(Value::Bool(if *negated {
                    !is_null
                } else {
                    is_null
                }))
            }
            // PG IS DISTINCT FROM / IS NOT DISTINCT FROM — Phase F-9
            // NULL 安全比较：将 NULL 视为一个可比较的"标记值"
            Expr::IsDistinctFrom { left, right, not } => {
                Self::eval_is_distinct_from(left, right, *not, ctx)
            }
            // PG SIMILAR TO — Phase F-9
            Expr::SimilarTo {
                expr,
                pattern,
                negated,
            } => Self::eval_similar_to(expr, pattern, *negated, ctx),
            // PG SUBSTRING(expr [FROM start] [FOR len]) — Phase F-9
            Expr::Substring {
                expr,
                from,
                for_len,
            } => Self::eval_substring(expr, from, for_len, ctx),
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
            // PG 正则匹配：`~` / `~*` / `!~` / `!~*`
            BinaryOp::RegexMatch => Self::eval_regex(&l, &r, false, false),
            BinaryOp::RegexIMatch => Self::eval_regex(&l, &r, true, false),
            BinaryOp::RegexNotMatch => Self::eval_regex(&l, &r, false, true),
            BinaryOp::RegexNotIMatch => Self::eval_regex(&l, &r, true, true),
            // PG JSON/JSONB 操作符：-> / ->> / #> / #>> / @> / <@
            // 当前实现为简化版：解析 JSON 后用 serde_json::Value 进行路径访问
            BinaryOp::JsonArrow => Self::eval_json_arrow(&l, &r, false),
            BinaryOp::JsonLongArrow => Self::eval_json_arrow(&l, &r, true),
            BinaryOp::JsonHashArrow => Self::eval_json_hash_arrow(&l, &r, false),
            BinaryOp::JsonHashLongArrow => Self::eval_json_hash_arrow(&l, &r, true),
            BinaryOp::JsonAtArrow => Self::eval_json_contains(&l, &r),
            BinaryOp::JsonArrowAt => Self::eval_json_contains(&r, &l),
            BinaryOp::And | BinaryOp::Or => unreachable!("AND/OR 已在短路求值中处理"),
        }
    }

    /// PG JSON/JSONB 路径访问：`json -> 'key'` / `json -> 1`
    ///
    /// - `as_text=true`：返回 text（`->>` 语义），JSON 标量转为文本，复合 JSON 序列化为字符串
    /// - `as_text=false`：返回 json（`->` 语义），保留 JSON 类型
    ///
    /// 规则：
    /// - 任一操作数为 NULL → NULL
    /// - 左操作数：Json 或 Text（自动解析为 JSON）
    /// - 右操作数：Text（键名）或 Integer（数组索引，支持负数）
    fn eval_json_arrow(l: &Value, r: &Value, as_text: bool) -> Result<Value, EvalError> {
        if matches!(l, Value::Null) || matches!(r, Value::Null) {
            return Ok(Value::Null);
        }
        let json_val = match l {
            Value::Json(j) => j.clone(),
            Value::Text(s) => serde_json::from_str(s)
                .map_err(|e| EvalError::CastFailed(format!("invalid JSON: {e}")))?,
            other => {
                return Err(EvalError::TypeMismatch {
                    expected: "json/text",
                    actual: value_type_name(other),
                })
            }
        };
        let result = match r {
            Value::Text(key) => json_val.get(key).cloned(),
            Value::Int64(idx) => {
                let i = *idx;
                if i >= 0 {
                    json_val.get(i as usize).cloned()
                } else {
                    // 负索引：从末尾计算
                    json_val.as_array().and_then(|arr| {
                        let len = arr.len() as i64;
                        let real_idx = len + i;
                        if real_idx >= 0 {
                            arr.get(real_idx as usize).cloned()
                        } else {
                            None
                        }
                    })
                }
            }
            other => {
                return Err(EvalError::TypeMismatch {
                    expected: "text/int",
                    actual: value_type_name(other),
                })
            }
        };
        match result {
            Some(serde_json::Value::Null) | None => Ok(Value::Null),
            Some(v) if as_text => {
                let s = match v {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                Ok(Value::Text(s))
            }
            Some(v) => Ok(Value::Json(v)),
        }
    }

    /// PG JSON/JSONB 路径数组访问：`json #> '{a,b}'` / `json #>> '{a,b}'`
    ///
    /// - `as_text=true`：返回 text（`#>>` 语义）
    /// - `as_text=false`：返回 json（`#>` 语义）
    ///
    /// 规则：
    /// - 右操作数为 PG 数组字面量字符串 `'{a,b}'`，按逗号分割为路径
    fn eval_json_hash_arrow(l: &Value, r: &Value, as_text: bool) -> Result<Value, EvalError> {
        if matches!(l, Value::Null) || matches!(r, Value::Null) {
            return Ok(Value::Null);
        }
        let mut json_val = match l {
            Value::Json(j) => j.clone(),
            Value::Text(s) => serde_json::from_str(s)
                .map_err(|e| EvalError::CastFailed(format!("invalid JSON: {e}")))?,
            other => {
                return Err(EvalError::TypeMismatch {
                    expected: "json/text",
                    actual: value_type_name(other),
                })
            }
        };
        let path_str = match r {
            Value::Text(s) => s.trim_start_matches('{').trim_end_matches('}'),
            other => {
                return Err(EvalError::TypeMismatch {
                    expected: "text",
                    actual: value_type_name(other),
                })
            }
        };
        for seg in path_str.split(',') {
            let seg = seg.trim();
            if seg.is_empty() {
                continue;
            }
            // 尝试作为数组索引，否则作为对象键
            json_val = if let Ok(idx) = seg.parse::<i64>() {
                if idx >= 0 {
                    json_val
                        .get(idx as usize)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null)
                } else {
                    json_val
                        .as_array()
                        .and_then(|arr| {
                            let len = arr.len() as i64;
                            let real = len + idx;
                            if real >= 0 {
                                arr.get(real as usize).cloned()
                            } else {
                                None
                            }
                        })
                        .unwrap_or(serde_json::Value::Null)
                }
            } else {
                json_val
                    .get(seg)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null)
            };
        }
        if json_val.is_null() {
            return Ok(Value::Null);
        }
        if as_text {
            let s = match json_val {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            Ok(Value::Text(s))
        } else {
            Ok(Value::Json(json_val))
        }
    }

    /// PG JSON/JSONB 包含：`json @> json` → bool
    ///
    /// 规则：
    /// - 左侧 JSON 是否包含右侧 JSON（递归子集匹配）
    /// - 数组：右侧所有元素都在左侧数组中
    /// - 对象：右侧所有键值对都在左侧对象中
    /// - 标量：相等比较
    fn eval_json_contains(l: &Value, r: &Value) -> Result<Value, EvalError> {
        if matches!(l, Value::Null) || matches!(r, Value::Null) {
            return Ok(Value::Null);
        }
        let l_json = match l {
            Value::Json(j) => j.clone(),
            Value::Text(s) => serde_json::from_str(s)
                .map_err(|e| EvalError::CastFailed(format!("invalid JSON: {e}")))?,
            other => {
                return Err(EvalError::TypeMismatch {
                    expected: "json/text",
                    actual: value_type_name(other),
                })
            }
        };
        let r_json = match r {
            Value::Json(j) => j.clone(),
            Value::Text(s) => serde_json::from_str(s)
                .map_err(|e| EvalError::CastFailed(format!("invalid JSON: {e}")))?,
            other => {
                return Err(EvalError::TypeMismatch {
                    expected: "json/text",
                    actual: value_type_name(other),
                })
            }
        };
        Ok(Value::Bool(json_contains(&l_json, &r_json)))
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

    /// PG POSIX 正则匹配：`text ~ pattern` → bool
    ///
    /// # 参数
    /// - `l`: 左操作数（待匹配文本，应为 Text）
    /// - `r`: 右操作数（POSIX 正则模式，应为 Text）
    /// - `case_insensitive`: true 表示 `~*` / `!~*`（大小写不敏感）
    /// - `negated`: true 表示 `!~` / `!~*`（取反）
    ///
    /// # 语义
    /// - 任一操作数为 NULL → NULL
    /// - 左/右操作数非 Text → TypeMismatch
    /// - 正则编译失败 → InvalidRegex
    /// - 返回 `Bool(matched ^ negated)`
    ///
    /// # 注意
    /// PG 使用 POSIX ERE（Extended Regular Expression），Rust `regex` crate 默认即 ERE，
    /// 但 PG 还支持一些反向引用等 PCRE 特性，本实现不涵盖（POSIX ERE 子集已覆盖 99% 用例）。
    fn eval_regex(
        l: &Value,
        r: &Value,
        case_insensitive: bool,
        negated: bool,
    ) -> Result<Value, EvalError> {
        if matches!(l, Value::Null) || matches!(r, Value::Null) {
            return Ok(Value::Null);
        }
        let s = match l {
            Value::Text(s) => s.as_str(),
            other => {
                return Err(EvalError::TypeMismatch {
                    expected: "text",
                    actual: value_type_name(other),
                })
            }
        };
        let pat = match r {
            Value::Text(s) => s.as_str(),
            other => {
                return Err(EvalError::TypeMismatch {
                    expected: "text",
                    actual: value_type_name(other),
                })
            }
        };
        // 构建正则：case_insensitive 时添加 (?i) 内联标志
        let pattern = if case_insensitive {
            format!("(?i){pat}")
        } else {
            pat.to_string()
        };
        let re = regex::Regex::new(&pattern)
            .map_err(|e| EvalError::InvalidRegex(format!("'{pat}': {e}")))?;
        // PG 的 ~ 默认是搜索（部分匹配），不是完全匹配，等价于 regex.is_match
        let matched = re.is_match(s);
        Ok(Value::Bool(matched ^ negated))
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
        case_insensitive: bool,
        ctx: &dyn EvalContext,
    ) -> Result<Value, EvalError> {
        let v = Self::eval(expr, ctx)?;
        let p = Self::eval(pattern, ctx)?;
        match (v, p) {
            (Value::Text(s), Value::Text(pat)) => {
                // ILIKE：对两端文本调用 lower() 后再 LIKE（PG 语义）
                let (s_cmp, pat_cmp) = if case_insensitive {
                    (s.to_lowercase(), pat.to_lowercase())
                } else {
                    (s, pat)
                };
                let regex = like_to_regex(&pat_cmp)?;
                let matched = regex.is_match(&s_cmp);
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

    /// PG IS DISTINCT FROM / IS NOT DISTINCT FROM — Phase F-9
    ///
    /// # 语义
    /// - `IS DISTINCT FROM`：NULL 与非 NULL 视为不同 → true；两非 NULL 值不等 → true；其余 → false
    /// - `IS NOT DISTINCT FROM`：NULL 与 NULL 视为相同 → true；两非 NULL 值相等 → true；其余 → false
    ///
    /// # 等价表达式
    /// - `a IS DISTINCT FROM b` ≡ `(a IS NOT NULL AND b IS NULL) OR (a IS NULL AND b IS NOT NULL) OR (a <> b)`
    /// - `a IS NOT DISTINCT FROM b` ≡ `(a IS NULL AND b IS NULL) OR (a = b)`
    fn eval_is_distinct_from(
        left: &Expr,
        right: &Expr,
        not: bool,
        ctx: &dyn EvalContext,
    ) -> Result<Value, EvalError> {
        let l = Self::eval(left, ctx)?;
        let r = Self::eval(right, ctx)?;
        // 判定两值是否"相同"（NULL-safe）
        let same = match (l, r) {
            (Value::Null, Value::Null) => true,
            (Value::Null, _) | (_, Value::Null) => false,
            (a, b) => a == b,
        };
        // IS DISTINCT FROM：!same；IS NOT DISTINCT FROM：same
        let result = if not {
            same
        } else {
            !same
        };
        Ok(Value::Bool(result))
    }

    /// PG SIMILAR TO — Phase F-9
    ///
    /// # 语义
    /// - 使用 SQL 正则语法（介于 LIKE 与 POSIX ~ 之间）
    /// - 必须完全匹配整个字符串（与 ~ 部分匹配不同）
    /// - 支持元字符：`|`、`*`、`+`、`?`、`[...]`、`(...)`、`_`（LIKE 风格单字符）
    ///
    /// # 转换策略
    /// 将 SQL 正则转换为 POSIX 正则后用 regex crate 匹配
    fn eval_similar_to(
        expr: &Expr,
        pattern: &Expr,
        negated: bool,
        ctx: &dyn EvalContext,
    ) -> Result<Value, EvalError> {
        let v = Self::eval(expr, ctx)?;
        let p = Self::eval(pattern, ctx)?;
        match (v, p) {
            (Value::Text(s), Value::Text(pat)) => {
                let regex = similar_to_regex(&pat)?;
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

    /// PG SUBSTRING(expr [FROM start] [FOR len]) — Phase F-9
    ///
    /// # 语义
    /// - 1-based 索引截取子串
    /// - `start` 为 NULL → NULL；`start <= 0` 调整为 1（PG 特殊语义：start=0 时长度-1）
    /// - `len` 为 NULL → 截到末尾；`len < 0` → NULL
    /// - 字符级而非字节级（与 length() 一致）
    fn eval_substring(
        expr: &Expr,
        from: &Option<Box<Expr>>,
        for_len: &Option<Box<Expr>>,
        ctx: &dyn EvalContext,
    ) -> Result<Value, EvalError> {
        let v = Self::eval(expr, ctx)?;
        let s = match v {
            Value::Text(s) => s,
            Value::Null => return Ok(Value::Null),
            other => {
                return Err(EvalError::TypeMismatch {
                    expected: "text",
                    actual: value_type_name(&other),
                })
            }
        };
        // from 子句
        let start_val = match from {
            Some(e) => Self::eval(e, ctx)?,
            None => Value::Int64(1),
        };
        let mut start: i64 = match start_val {
            Value::Int64(n) => n,
            Value::Null => return Ok(Value::Null),
            other => {
                return Err(EvalError::TypeMismatch {
                    expected: "int",
                    actual: value_type_name(&other),
                })
            }
        };
        // for len 子句
        let len_val = match for_len {
            Some(e) => Some(Self::eval(e, ctx)?),
            None => None,
        };
        let mut length: Option<i64> = match len_val {
            Some(Value::Int64(n)) => Some(n),
            Some(Value::Null) => return Ok(Value::Null),
            None => None,
            Some(other) => {
                return Err(EvalError::TypeMismatch {
                    expected: "int",
                    actual: value_type_name(&other),
                })
            }
        };

        // PG 特殊语义：start <= 0 时实际从 1 开始，长度相应调整
        if start < 1 {
            // 例：SUBSTRING('hello' FROM 0 FOR 3) → 'he'（实际 start=1, length=2）
            if let Some(len) = length.as_mut() {
                *len += start - 1; // start=0 → len-1, start=-1 → len-2
                if *len < 0 {
                    return Ok(Value::Text(String::new()));
                }
            }
            start = 1;
        }

        if let Some(len) = length {
            if len < 0 {
                return Ok(Value::Text(String::new()));
            }
        }

        let chars: Vec<char> = s.chars().collect();
        let start_idx = (start - 1) as usize;
        if start_idx >= chars.len() {
            return Ok(Value::Text(String::new()));
        }
        let end_idx = match length {
            Some(len) => (start_idx + len as usize).min(chars.len()),
            None => chars.len(),
        };
        Ok(Value::Text(chars[start_idx..end_idx].iter().collect()))
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
            // Phase TDengine-P3: time_bucket — 时序分析函数（对标 TimescaleDB）
            // time_bucket(bucket_width_text, timestamp) → timestamp（桶起点）
            // 支持 '1 second' / '1 minute' / '1 hour' / '1 day' / '1 week' / '1 month' / '1 year'
            "time_bucket" => {
                check_arg_count("time_bucket", &arg_vals, 2)?;
                let bucket_str = match &arg_vals[0] {
                    Value::Text(s) => s.as_str(),
                    other => {
                        return Err(EvalError::TypeMismatch {
                            expected: "text",
                            actual: value_type_name(other),
                        })
                    }
                };
                let ts_us = match arg_vals[1] {
                    Value::Timestamp(us) => us,
                    Value::Null => return Ok(Value::Null),
                    ref other => {
                        return Err(EvalError::TypeMismatch {
                            expected: "timestamp",
                            actual: value_type_name(other),
                        })
                    }
                };
                let bucket_us = parse_bucket_width(bucket_str)?;
                // 使用 div_euclid 确保负数时间戳（1970 年前）也能正确对齐
                let bucket_start = ts_us.div_euclid(bucket_us) * bucket_us;
                Ok(Value::Timestamp(bucket_start))
            }
            _ => {
                // P0-SQL-8 修复：内建函数表中未命中时，回退到 UDF 注册系统查询。
                // try_call_udf 返回 None 表示 UDF 也不存在，此时按原逻辑返回 FunctionNotFound；
                // 返回 Some(result) 表示 UDF 存在（无论成功或失败均透传）。
                match ctx.try_call_udf(fname.as_str(), &arg_vals) {
                    Some(result) => result,
                    None => Err(EvalError::FunctionNotFound(fname)),
                }
            }
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

/// 解析 time_bucket 的桶宽度字符串为微秒数
/// 支持格式：'1 second' / '5 minutes' / '1 hour' / '1 day' / '1 week' / '1 month' / '1 year'
/// 月和年使用固定近似值（30 天/月，365 天/年），与 TimescaleDB 行为一致
fn parse_bucket_width(s: &str) -> Result<i64, EvalError> {
    let s = s.trim().to_lowercase();
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 2 {
        return Err(EvalError::InvalidFunctionArgs(format!(
            "time_bucket bucket_width must be '<number> <unit>', got '{s}'"
        )));
    }
    let n: i64 = parts[0].parse().map_err(|_| {
        EvalError::InvalidFunctionArgs(format!(
            "time_bucket bucket_width invalid number: '{}'",
            parts[0]
        ))
    })?;
    let unit = parts[1];
    // 去掉复数 s
    let unit = unit.trim_end_matches('s');
    let multiplier = match unit {
        "second" => 1_000_000,         // 1 秒 = 1,000,000 微秒
        "minute" => 60_000_000,        // 1 分钟 = 60,000,000 微秒
        "hour" => 3_600_000_000,       // 1 小时 = 3,600,000,000 微秒
        "day" => 86_400_000_000,       // 1 天 = 86,400,000,000 微秒
        "week" => 604_800_000_000,     // 1 周 = 604,800,000,000 微秒
        "month" => 2_592_000_000_000,  // 30 天 ≈ 2,592,000,000,000 微秒
        "year" => 31_536_000_000_000,  // 365 天 ≈ 31,536,000,000,000 微秒
        other => return Err(EvalError::InvalidFunctionArgs(format!(
            "time_bucket unsupported unit: '{other}' (supported: second/minute/hour/day/week/month/year)"
        ))),
    };
    // 使用 checked_mul 防止溢出（铁律第 4 条）
    n.checked_mul(multiplier).ok_or_else(|| {
        EvalError::InvalidFunctionArgs(format!("time_bucket bucket_width overflow: {n} {unit}"))
    })
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

/// JSON 包含判断：左侧 JSON 是否包含右侧 JSON（PG `@>` 语义）
///
/// - 数组包含：右侧所有元素都在左侧数组中（顺序无关）
/// - 对象包含：右侧所有键值对都在左侧对象中（递归）
/// - 标量相等：直接比较
fn json_contains(container: &serde_json::Value, contained: &serde_json::Value) -> bool {
    match (container, contained) {
        (serde_json::Value::Array(arr_l), serde_json::Value::Array(arr_r)) => arr_r
            .iter()
            .all(|r| arr_l.iter().any(|l| json_contains(l, r))),
        (serde_json::Value::Object(obj_l), serde_json::Value::Object(obj_r)) => obj_r
            .iter()
            .all(|(k, v)| obj_l.get(k).is_some_and(|l| json_contains(l, v))),
        _ => container == contained,
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

/// SIMILAR TO 模式 → POSIX 正则 — Phase F-9
///
/// # 转换规则
/// - `_` → `.`（单字符通配）
/// - `%` → `.*`（多字符通配，PG 文档明确支持）
/// - `*`、`+`、`?` → 保留为 POSIX 量词
/// - `|` → 保留为 POSIX 选择
/// - `(...)` → 保留为 POSIX 分组
/// - `[...]` → 保留为 POSIX 字符类
/// - 其他正则元字符（`.`、`^`、`$`、`{`、`}`、`\\`）→ 转义
/// - 整体加 `^...$` 强制完全匹配（PG SIMILAR TO 是完全匹配）
fn similar_to_regex(pattern: &str) -> Result<regex::Regex, EvalError> {
    let mut regex_str = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '_' => regex_str.push('.'),
            '%' => regex_str.push_str(".*"),
            '*' | '+' | '?' | '|' | '(' | ')' | '[' | ']' => regex_str.push(c),
            '\\' => {
                // 转义下一个字符
                if let Some(next) = chars.next() {
                    regex_str.push('\\');
                    regex_str.push(next);
                }
            }
            // 需要转义的 POSIX 元字符
            '.' | '^' | '$' | '{' | '}' => {
                regex_str.push('\\');
                regex_str.push(c);
            }
            _ => regex_str.push(c),
        }
    }
    regex_str.push('$');
    regex::Regex::new(&regex_str).map_err(|e| EvalError::InvalidRegex(format!("'{pattern}': {e}")))
}

// =====================================================================
//  测试 — Phase TDengine-P3: time_bucket
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Expr;
    use crate::expr::{EvalContext, ExprEvaluator};

    /// 简单求值上下文（空行，用于无列引用的函数求值）
    struct EmptyContext;
    impl EvalContext for EmptyContext {
        fn lookup_column(&self, _name: &str) -> Result<Value, EvalError> {
            Err(EvalError::Unsupported(format!("column not found: {_name}")))
        }
        fn lookup_qualified(&self, table: &str, col: &str) -> Result<Value, EvalError> {
            Err(EvalError::Unsupported(format!(
                "column not found: {table}.{col}"
            )))
        }
    }

    #[test]
    fn test_time_bucket_1_hour() {
        // 2024-01-01 10:30:00 UTC = 1704105000 秒 = 1704105000000000 微秒
        let ts = 1704105000000000i64;
        let expr = Expr::Function {
            name: "time_bucket".to_string(),
            args: vec![
                Expr::Literal(Value::Text("1 hour".to_string())),
                Expr::Literal(Value::Timestamp(ts)),
            ],
            distinct: false,
        };
        let ctx = EmptyContext;
        let result = ExprEvaluator::eval(&expr, &ctx).unwrap();
        // 桶起点 = 10:00:00 = 1704103200 秒 = 1704103200000000 微秒
        match result {
            Value::Timestamp(bucket_start) => {
                assert_eq!(bucket_start, 1704103200000000);
            }
            other => panic!("expected Timestamp, got {other:?}"),
        }
    }

    #[test]
    fn test_time_bucket_5_minutes() {
        // 10:12:30 UTC = 1704103950 秒 = 1704103950000000 微秒 → 5 分钟桶 → 10:10:00
        let ts = 1704103950000000i64;
        let expr = Expr::Function {
            name: "time_bucket".to_string(),
            args: vec![
                Expr::Literal(Value::Text("5 minutes".to_string())),
                Expr::Literal(Value::Timestamp(ts)),
            ],
            distinct: false,
        };
        let ctx = EmptyContext;
        let result = ExprEvaluator::eval(&expr, &ctx).unwrap();
        // 10:10:00 = 1704103800 秒 = 1704103800000000 微秒
        match result {
            Value::Timestamp(bucket_start) => {
                assert_eq!(bucket_start, 1704103800000000);
            }
            other => panic!("expected Timestamp, got {other:?}"),
        }
    }

    #[test]
    fn test_time_bucket_1_day() {
        // 2024-01-01 10:30:00 → 1 天桶 → 2024-01-01 00:00:00
        let ts = 1704105000000000i64;
        let expr = Expr::Function {
            name: "time_bucket".to_string(),
            args: vec![
                Expr::Literal(Value::Text("1 day".to_string())),
                Expr::Literal(Value::Timestamp(ts)),
            ],
            distinct: false,
        };
        let ctx = EmptyContext;
        let result = ExprEvaluator::eval(&expr, &ctx).unwrap();
        // 2024-01-01 00:00:00 = 1704067200 秒 = 1704067200000000 微秒
        match result {
            Value::Timestamp(bucket_start) => {
                assert_eq!(bucket_start, 1704067200000000);
            }
            other => panic!("expected Timestamp, got {other:?}"),
        }
    }

    #[test]
    fn test_time_bucket_null() {
        let expr = Expr::Function {
            name: "time_bucket".to_string(),
            args: vec![
                Expr::Literal(Value::Text("1 hour".to_string())),
                Expr::Literal(Value::Null),
            ],
            distinct: false,
        };
        let ctx = EmptyContext;
        let result = ExprEvaluator::eval(&expr, &ctx).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn test_time_bucket_invalid_unit() {
        let ts = 1704109800000000i64;
        let expr = Expr::Function {
            name: "time_bucket".to_string(),
            args: vec![
                Expr::Literal(Value::Text("1 millennium".to_string())),
                Expr::Literal(Value::Timestamp(ts)),
            ],
            distinct: false,
        };
        let ctx = EmptyContext;
        let result = ExprEvaluator::eval(&expr, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_time_bucket_invalid_arg_type() {
        let expr = Expr::Function {
            name: "time_bucket".to_string(),
            args: vec![
                Expr::Literal(Value::Int64(1)),
                Expr::Literal(Value::Timestamp(0)),
            ],
            distinct: false,
        };
        let ctx = EmptyContext;
        let result = ExprEvaluator::eval(&expr, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_bucket_width_units() {
        assert_eq!(parse_bucket_width("1 second").unwrap(), 1_000_000);
        assert_eq!(parse_bucket_width("1 minute").unwrap(), 60_000_000);
        assert_eq!(parse_bucket_width("1 hour").unwrap(), 3_600_000_000);
        assert_eq!(parse_bucket_width("1 day").unwrap(), 86_400_000_000);
        assert_eq!(parse_bucket_width("1 week").unwrap(), 604_800_000_000);
        assert_eq!(parse_bucket_width("1 month").unwrap(), 2_592_000_000_000);
        assert_eq!(parse_bucket_width("1 year").unwrap(), 31_536_000_000_000);
        // 复数形式
        assert_eq!(parse_bucket_width("3 hours").unwrap(), 3 * 3_600_000_000);
        assert_eq!(parse_bucket_width("10 minutes").unwrap(), 10 * 60_000_000);
    }

    #[test]
    fn test_parse_bucket_width_invalid() {
        assert!(parse_bucket_width("invalid").is_err());
        assert!(parse_bucket_width("1").is_err());
        assert!(parse_bucket_width("1 lightyear").is_err());
        assert!(parse_bucket_width("abc hour").is_err());
    }

    // =====================================================================
    //  P0-SQL-8 端到端测试：ExprEvaluator 通过 thread_local 调用 UDF
    // =====================================================================

    use crate::udf::{UdfContext, UdfFunction, UdfRegistry};
    use std::sync::Arc;

    /// 测试用 UDF：my_double(x) → x * 2
    struct DoubleUdf;
    impl UdfFunction for DoubleUdf {
        fn signature(
            &self,
        ) -> (
            &'static str,
            &'static [(&'static str, &'static str)],
            &'static str,
        ) {
            ("my_double", &[("x", "integer")], "integer")
        }
        fn call(&self, args: &[Value], _ctx: &UdfContext) -> Result<Value, crate::udf::UdfError> {
            match &args[0] {
                Value::Int64(n) => Ok(Value::Int64(n * 2)),
                other => Err(crate::udf::UdfError::TypeError(format!(
                    "expected integer, got {}",
                    value_type_name(other)
                ))),
            }
        }
    }

    /// 测试 ExprEvaluator 在内建函数表未命中时回退到 UDF 注册表
    #[test]
    fn test_p0_sql8_udf_fallback_through_thread_local() {
        // 1. 准备 UDF 注册表
        let mut registry = UdfRegistry::new();
        registry.register(Arc::new(DoubleUdf));
        let registry_arc = Arc::new(registry);

        // 2. 设置 thread_local UDF 注册表（RAII guard）
        let _guard = current_udf_registry::guard(registry_arc.clone());

        // 3. 求值 my_double(21) — 内建函数表无此函数，应回退到 UDF
        let ctx = EmptyContext;
        let expr = Expr::Function {
            name: "my_double".to_string(),
            args: vec![Expr::Literal(Value::Int64(21))],
            distinct: false,
        };
        let result = ExprEvaluator::eval(&expr, &ctx).unwrap();
        assert_eq!(result, Value::Int64(42));
    }

    /// 测试未注册 UDF 仍返回 FunctionNotFound（不假装成功）
    #[test]
    fn test_p0_sql8_unknown_udf_returns_function_not_found() {
        // 空 UDF 注册表
        let registry = UdfRegistry::new();
        let _guard = current_udf_registry::guard(Arc::new(registry));

        let ctx = EmptyContext;
        let expr = Expr::Function {
            name: "nonexistent_udf".to_string(),
            args: vec![Expr::Literal(Value::Int64(1))],
            distinct: false,
        };
        let err = ExprEvaluator::eval(&expr, &ctx).unwrap_err();
        assert!(matches!(err, EvalError::FunctionNotFound(_)));
    }

    /// 测试无 UDF 注册表时（thread_local 未设置）仍返回 FunctionNotFound
    #[test]
    fn test_p0_sql8_no_registry_returns_function_not_found() {
        // 不设置 thread_local UDF 注册表
        let ctx = EmptyContext;
        let expr = Expr::Function {
            name: "any_udf".to_string(),
            args: vec![Expr::Literal(Value::Int64(1))],
            distinct: false,
        };
        let err = ExprEvaluator::eval(&expr, &ctx).unwrap_err();
        assert!(matches!(err, EvalError::FunctionNotFound(_)));
    }
}
