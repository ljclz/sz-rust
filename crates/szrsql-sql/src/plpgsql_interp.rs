//! PL/pgSQL 解释器 — Phase 6.6
//!
//! 遍历 `PlPgSqlBlock` AST 执行 PL/pgSQL 函数体。
//!
//! # 设计
//!
//! - **入口**：`PlPgSqlInterpreter::call(name, args) -> Result<Option<Value>, PlInterpError>`
//! - **环境**：`Environment` 模拟变量作用域栈（block / loop / function）
//! - **控制流**：`ControlFlow` 枚举传递 Return / Exit / Continue 信号
//! - **表达式求值**：内置 `eval_pl_expr` 处理 PL/pgSQL 表达式
//!   （字面量 / 变量 / 算术 / 比较 / 逻辑 / CASE / IN / IS NULL / 函数调用 / 字符串拼接）
//! - **SQL 委托**：`PlSqlExecutor` trait 抽象 SQL 执行
//!   （SELECT INTO / PERFORM / EXECUTE / RETURN QUERY / 裸 SQL）
//! - **栈深度**：`stack_depth` 字段跟踪递归深度，防止栈溢出
//! - **循环保护**：`max_loop_iterations` 限制单次循环最大迭代数
//!
//! # Stress 验收
//!
//! - 递归调用深度 1000 不栈溢出（默认 `max_stack_depth = 2048`）
//! - 循环 10,000,000 次性能合理（`max_loop_iterations = 100_000_000`）
//! - 复杂业务逻辑函数正确执行（IF / CASE / LOOP / WHILE / FOR 嵌套）
//!
//! # 与 Phase 6.5 的衔接
//!
//! Phase 6.5 解析器产出 `PlPgSqlBlock` AST；Phase 6.6 解释器遍历该 AST 执行。
//! 表达式以原始 `String` 形式存储于 AST 中，解释器在执行时通过 `eval_pl_expr` 求值。

use crate::ast::{FunctionArgMode, FunctionParameter};
use crate::plpgsql::{
    parse_function_body, PlPgSqlBlock, PlPgSqlDeclaration, PlPgSqlRaiseLevel, PlPgSqlStatement,
};
use std::collections::{HashMap, HashSet};
use szrsql_types::value::Value;
use thiserror::Error;

// =====================================================================
//  错误类型
// =====================================================================

/// PL/pgSQL 解释器错误
#[derive(Debug, Clone, PartialEq, Error)]
pub enum PlInterpError {
    /// 变量未定义
    #[error("variable not found: {0}")]
    VarNotFound(String),
    /// 常量重新赋值
    #[error("cannot reassign constant: {0}")]
    ConstantReassign(String),
    /// 类型错误
    #[error("type error: {0}")]
    TypeError(String),
    /// 除零
    #[error("division by zero")]
    DivisionByZero,
    /// 整数溢出
    #[error("integer overflow: {0}")]
    IntegerOverflow(String),
    /// 栈溢出
    #[error("stack overflow: depth {depth} exceeds max {max}")]
    StackOverflow { depth: usize, max: usize },
    /// 循环迭代次数超限
    #[error("loop iteration limit exceeded: {0}")]
    LoopLimitExceeded(usize),
    /// 未捕获的异常（RAISE EXCEPTION）
    #[error("uncaught exception: {0}")]
    UncaughtException(String),
    /// SQL 执行错误
    #[error("sql execution error: {0}")]
    SqlError(String),
    /// 解析错误（函数体懒解析失败）
    #[error("parse error: {0}")]
    ParseError(String),
    /// 不支持
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// 函数未找到
    #[error("function not found: {0}")]
    FunctionNotFound(String),
    /// 参数数量不匹配
    #[error("argument count mismatch: expected {expected}, got {got}")]
    ArgCountMismatch { expected: usize, got: usize },
    /// CASE 未匹配且无 ELSE
    #[error("case_not_found")]
    CaseNotFound,
    /// 表达式解析错误
    #[error("expression parse error: {0}")]
    ExprParseError(String),
}

// =====================================================================
//  控制流信号
// =====================================================================

/// 语句执行后的控制流信号
#[derive(Debug, Clone, PartialEq)]
enum ControlFlow {
    /// 正常继续下一条语句
    Normal,
    /// RETURN 触发（携带可选返回值）
    Return(Option<Value>),
    /// EXIT 触发（携带可选标签）
    Exit { label: Option<String> },
    /// CONTINUE 触发（携带可选标签）
    Continue { label: Option<String> },
}

// =====================================================================
//  变量环境
// =====================================================================

/// 变量作用域
#[derive(Debug, Default, Clone)]
struct Scope {
    /// 变量名（小写） → 值
    vars: HashMap<String, Value>,
    /// 常量名集合（小写）
    constants: HashSet<String>,
}

/// 变量环境 — 作用域栈
///
/// - 函数调用时压入新作用域
/// - 进入 BLOCK / LOOP 时压入新作用域
/// - 离开时弹出
/// - `lookup` 从栈顶向下查找
/// - `assign` 从栈顶向下查找并修改（若为常量则报错）
#[derive(Debug, Clone)]
pub struct Environment {
    scopes: Vec<Scope>,
}

impl Default for Environment {
    fn default() -> Self {
        // 保持与 new() 一致：至少含一个顶层作用域
        Self::new()
    }
}

impl Environment {
    /// 创建空环境（含一个顶层作用域）
    pub fn new() -> Self {
        Self {
            scopes: vec![Scope::default()],
        }
    }

    /// 压入新作用域
    fn push(&mut self) {
        self.scopes.push(Scope::default());
    }

    /// 弹出顶层作用域
    fn pop(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// 在当前作用域声明变量
    fn declare(&mut self, name: &str, value: Value, is_constant: bool) {
        let key = name.to_lowercase();
        let scope = self
            .scopes
            .last_mut()
            .expect("environment must have at least one scope");
        scope.vars.insert(key.clone(), value);
        if is_constant {
            scope.constants.insert(key);
        }
    }

    /// 查找变量值（从栈顶向下）
    fn lookup(&self, name: &str) -> Option<&Value> {
        let key = name.to_lowercase();
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.vars.get(&key) {
                return Some(v);
            }
        }
        None
    }

    /// 赋值已有变量（从栈顶向下查找；常量报错）
    ///
    /// 非PG扩展：若变量未声明，自动在当前作用域声明（简化测试与使用）。
    /// PG 严格要求变量在 DECLARE 中声明，否则报 syntax_error。
    fn assign(&mut self, name: &str, value: Value) -> Result<(), PlInterpError> {
        let key = name.to_lowercase();
        for scope in self.scopes.iter_mut().rev() {
            if scope.vars.contains_key(&key) {
                if scope.constants.contains(&key) {
                    return Err(PlInterpError::ConstantReassign(name.into()));
                }
                scope.vars.insert(key, value);
                return Ok(());
            }
        }
        // 自动声明（非PG扩展）：在当前作用域创建新变量
        let scope = self
            .scopes
            .last_mut()
            .expect("environment must have at least one scope");
        scope.vars.insert(key, value);
        Ok(())
    }

    /// 当前作用域深度
    fn depth(&self) -> usize {
        self.scopes.len()
    }
}

// =====================================================================
//  SQL 执行器抽象
// =====================================================================

/// SQL 执行器 trait — 由调用方实现以提供 SQL 执行能力
///
/// Phase 6.6 解释器将以下语句委托给此 trait：
/// - `SELECT INTO ...` — 调用 `execute_query`
/// - `PERFORM query` — 调用 `execute_query`（丢弃结果）
/// - `EXECUTE expr` — 调用 `execute_stmt` 或 `execute_query`
/// - `RETURN QUERY query` — 调用 `execute_query`
/// - 裸 SQL 语句 — 调用 `execute_stmt`
pub trait PlSqlExecutor {
    /// 执行 SQL 查询，返回行列表（每行为 `Vec<Value>`）
    fn execute_query(&mut self, sql: &str) -> Result<Vec<Vec<Value>>, PlInterpError>;

    /// 执行 SQL 语句（INSERT/UPDATE/DELETE 等），返回影响行数
    fn execute_stmt(&mut self, sql: &str) -> Result<usize, PlInterpError>;
}

/// 无操作 SQL 执行器 — 用于纯计算函数（无 SQL 委托需求）的测试
///
/// 调用任何方法均返回 `Unsupported` 错误。
pub struct NoopSqlExecutor;

impl PlSqlExecutor for NoopSqlExecutor {
    fn execute_query(&mut self, _sql: &str) -> Result<Vec<Vec<Value>>, PlInterpError> {
        Err(PlInterpError::Unsupported(
            "SQL execution not available (NoopSqlExecutor)".into(),
        ))
    }

    fn execute_stmt(&mut self, _sql: &str) -> Result<usize, PlInterpError> {
        Err(PlInterpError::Unsupported(
            "SQL execution not available (NoopSqlExecutor)".into(),
        ))
    }
}

// =====================================================================
//  函数定义与注册表
// =====================================================================

/// PL/pgSQL 函数定义
#[derive(Debug, Clone)]
pub struct PlFunction {
    /// 函数名
    pub name: String,
    /// 参数列表
    pub parameters: Vec<FunctionParameter>,
    /// 返回类型原文（如 `integer`、`void`）
    pub return_type: String,
    /// 函数体原文（`$$ ... $$` 内部内容）
    pub body: String,
    /// STRICT（任一参数为 NULL 时直接返回 NULL）
    pub strict: bool,
}

impl PlFunction {
    /// 计算 IN / INOUT 参数数量（用于调用时参数个数校验）
    pub fn in_param_count(&self) -> usize {
        self.parameters
            .iter()
            .filter(|p| {
                matches!(
                    p.mode.unwrap_or(FunctionArgMode::In),
                    FunctionArgMode::In | FunctionArgMode::InOut
                )
            })
            .count()
    }
}

/// 函数注册表 — 存储已定义的 PL/pgSQL 函数
///
/// - `register`：注册函数（CREATE FUNCTION）
/// - `get`：按名查找（用于函数调用）
/// - `remove`：移除函数（DROP FUNCTION）
#[derive(Debug, Default, Clone)]
pub struct FunctionRegistry {
    /// 函数名（小写） → 函数定义
    functions: HashMap<String, PlFunction>,
}

/// PL/pgSQL 函数解析器 trait — 用于在表达式求值时解析用户定义函数调用
///
/// 表达式求值器 `eval_ast` 在遇到 `Expr::FuncCall` 时，先尝试通过此 trait 解析用户定义函数；
/// 若返回 `FunctionNotFound`，则回退到内置函数表 `eval_builtin`。
pub trait PlFuncResolver {
    /// 解析函数调用
    ///
    /// - 返回 `Ok(value)`：函数存在并执行成功
    /// - 返回 `Err(FunctionNotFound(_))`：函数不存在，回退到内置函数
    /// - 返回 `Err(other)`：执行错误
    fn resolve_function(&mut self, name: &str, args: &[Value]) -> Result<Value, PlInterpError>;
}

/// 无操作函数解析器 — 不解析任何用户定义函数（仅用于无函数调用的表达式求值）
pub struct NoopFuncResolver;

impl PlFuncResolver for NoopFuncResolver {
    fn resolve_function(&mut self, name: &str, _args: &[Value]) -> Result<Value, PlInterpError> {
        Err(PlInterpError::FunctionNotFound(name.into()))
    }
}

impl FunctionRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
        }
    }

    /// 注册函数（若同名已存在则覆盖，与 `CREATE OR REPLACE` 一致）
    pub fn register(&mut self, func: PlFunction) {
        self.functions.insert(func.name.to_lowercase(), func);
    }

    /// 按名查找函数
    pub fn get(&self, name: &str) -> Option<&PlFunction> {
        self.functions.get(&name.to_lowercase())
    }

    /// 移除函数
    pub fn remove(&mut self, name: &str) -> Option<PlFunction> {
        self.functions.remove(&name.to_lowercase())
    }

    /// 当前函数数量
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }
}

// =====================================================================
//  主解释器
// =====================================================================

/// PL/pgSQL 解释器
///
/// # 生命周期
///
/// - `new(registry)` 创建解释器，绑定函数注册表
/// - `call(name, args)` 调用函数（递归入口）
/// - `with_sql_executor(executor)` 绑定 SQL 执行器（可选）
/// - `with_max_stack_depth(n)` 配置最大栈深度（默认 2048）
/// - `with_max_loop_iterations(n)` 配置循环最大迭代数（默认 100M）
pub struct PlPgSqlInterpreter<'a> {
    /// 变量环境
    env: Environment,
    /// 函数注册表
    function_registry: &'a FunctionRegistry,
    /// SQL 执行器（可选；None 时 SELECT INTO / PERFORM / EXECUTE 等会报错）
    sql_executor: Option<&'a mut dyn PlSqlExecutor>,
    /// 当前栈深度（函数调用层数）
    stack_depth: usize,
    /// 最大栈深度
    max_stack_depth: usize,
    /// 单次循环最大迭代数
    max_loop_iterations: usize,
}

impl<'a> PlPgSqlInterpreter<'a> {
    /// 创建解释器
    pub fn new(function_registry: &'a FunctionRegistry) -> Self {
        Self {
            env: Environment::new(),
            function_registry,
            sql_executor: None,
            stack_depth: 0,
            max_stack_depth: 2048,
            max_loop_iterations: 100_000_000,
        }
    }

    /// 绑定 SQL 执行器
    pub fn with_sql_executor(mut self, executor: &'a mut dyn PlSqlExecutor) -> Self {
        self.sql_executor = Some(executor);
        self
    }

    /// 配置最大栈深度
    pub fn with_max_stack_depth(mut self, depth: usize) -> Self {
        self.max_stack_depth = depth;
        self
    }

    /// 配置循环最大迭代数
    pub fn with_max_loop_iterations(mut self, n: usize) -> Self {
        self.max_loop_iterations = n;
        self
    }

    /// 调用 PL/pgSQL 函数
    ///
    /// # 参数
    /// - `name`：函数名（大小写不敏感）
    /// - `args`：参数值列表（按 IN / INOUT 参数顺序）
    ///
    /// # 返回
    /// - `Ok(Some(value))`：函数 RETURN 一个值
    /// - `Ok(None)`：函数无 RETURN 或 `RETURN;`（void 函数）
    /// - `Err(_)`：执行错误
    ///
    /// # 栈深度检查
    ///
    /// 调用前检查 `stack_depth < max_stack_depth`，防止无限递归导致栈溢出。
    pub fn call(&mut self, name: &str, args: &[Value]) -> Result<Option<Value>, PlInterpError> {
        // 栈深度检查
        if self.stack_depth >= self.max_stack_depth {
            return Err(PlInterpError::StackOverflow {
                depth: self.stack_depth,
                max: self.max_stack_depth,
            });
        }

        // 查找函数
        let func = self
            .function_registry
            .get(name)
            .ok_or_else(|| PlInterpError::FunctionNotFound(name.into()))?
            .clone();

        // 参数数量校验
        let expected = func.in_param_count();
        if args.len() != expected {
            return Err(PlInterpError::ArgCountMismatch {
                expected,
                got: args.len(),
            });
        }

        // STRICT：任一参数为 NULL 直接返回 NULL
        if func.strict && args.iter().any(|v| matches!(v, Value::Null)) {
            return Ok(Some(Value::Null));
        }

        // 压栈
        self.stack_depth += 1;
        self.env.push();

        // 绑定参数
        let mut arg_idx = 0;
        for param in &func.parameters {
            let mode = param.mode.unwrap_or(FunctionArgMode::In);
            match mode {
                FunctionArgMode::In | FunctionArgMode::InOut => {
                    if let Some(name) = &param.name {
                        let val = args.get(arg_idx).cloned().unwrap_or(Value::Null);
                        self.env.declare(name, val, false);
                    }
                    arg_idx += 1;
                }
                FunctionArgMode::Out | FunctionArgMode::Variadic => {
                    // OUT 参数不接收输入值；VARIADIC 暂不处理
                }
            }
        }

        // 懒解析函数体
        let block = parse_function_body(&func.body)
            .map_err(|e| PlInterpError::ParseError(format!("function {}: {e}", func.name)))?;

        // 执行函数体
        let result = self.exec_block_inner(&block);

        // 弹栈
        self.env.pop();
        self.stack_depth -= 1;

        match result? {
            ControlFlow::Return(v) => Ok(v),
            _ => Ok(None),
        }
    }

    /// 执行 BLOCK（不弹栈，由调用方负责作用域管理）
    ///
    /// 处理 EXCEPTION：若语句执行抛出 `PlInterpError`，匹配 exception_handlers；
    /// 匹配成功则执行对应 handler 语句序列；否则向上传播。
    fn exec_block_inner(&mut self, block: &PlPgSqlBlock) -> Result<ControlFlow, PlInterpError> {
        // 块作用域
        self.env.push();

        // 处理声明
        let mut decl_err: Option<PlInterpError> = None;
        for decl in &block.declarations {
            if let Err(e) = self.exec_declaration(decl) {
                decl_err = Some(e);
                break;
            }
        }

        let result = if let Some(e) = decl_err {
            Err(e)
        } else {
            // 执行语句
            self.exec_stmts(&block.statements)
        };

        let final_result = match result {
            Ok(cf) => Ok(cf),
            Err(e) => {
                // 异常处理
                if block.exception_handlers.is_empty() {
                    Err(e)
                } else {
                    self.handle_exception(&block.exception_handlers, e)
                }
            }
        };

        self.env.pop();
        final_result
    }

    /// 异常处理
    fn handle_exception(
        &mut self,
        handlers: &[crate::plpgsql::PlPgSqlExceptionHandler],
        err: PlInterpError,
    ) -> Result<ControlFlow, PlInterpError> {
        // 构建 SQLSTATE 字符串（简化）
        let sqlstate = pl_error_sqlstate(&err);
        let err_msg = pl_error_message(&err);

        for handler in handlers {
            let matched = handler.conditions.iter().any(|cond| {
                let upper = cond.to_uppercase();
                if upper == "OTHERS" {
                    return true;
                }
                // 匹配 SQLSTATE 或 PG 条件名（如 division_by_zero → 22012）
                upper == sqlstate || condition_name_to_sqlstate(&upper) == Some(sqlstate.as_str())
            });

            if matched {
                // 注入内置变量：SQLSTATE / SQLERRM
                self.env.push();
                self.env
                    .declare("SQLSTATE", Value::Text(sqlstate.clone()), false);
                self.env
                    .declare("SQLERRM", Value::Text(err_msg.clone()), false);

                let result = self.exec_stmts(&handler.statements);

                self.env.pop();
                return result;
            }
        }

        // 无匹配 handler，向上传播
        Err(err)
    }

    /// 执行声明
    fn exec_declaration(&mut self, decl: &PlPgSqlDeclaration) -> Result<(), PlInterpError> {
        match decl {
            PlPgSqlDeclaration::Variable {
                name,
                is_constant,
                data_type,
                default,
                ..
            } => {
                let val = if let Some(expr) = default {
                    let raw = self.eval_expr(expr)?;
                    coerce_to_type(raw, data_type)
                } else {
                    Value::Null
                };
                self.env.declare(name, val, *is_constant);
            }
            PlPgSqlDeclaration::VariableTypeRef {
                name,
                is_constant,
                default,
                ..
            } => {
                let val = if let Some(expr) = default {
                    self.eval_expr(expr)?
                } else {
                    Value::Null
                };
                self.env.declare(name, val, *is_constant);
            }
            PlPgSqlDeclaration::Alias { name, target } => {
                let val = self
                    .env
                    .lookup(target)
                    .cloned()
                    .ok_or_else(|| PlInterpError::VarNotFound(target.clone()))?;
                self.env.declare(name, val, false);
            }
        }
        Ok(())
    }

    /// 执行语句序列
    fn exec_stmts(&mut self, stmts: &[PlPgSqlStatement]) -> Result<ControlFlow, PlInterpError> {
        for stmt in stmts {
            match self.exec_stmt(stmt)? {
                ControlFlow::Normal => continue,
                other => return Ok(other),
            }
        }
        Ok(ControlFlow::Normal)
    }

    /// 执行单条语句
    fn exec_stmt(&mut self, stmt: &PlPgSqlStatement) -> Result<ControlFlow, PlInterpError> {
        match stmt {
            PlPgSqlStatement::Assignment { target, value } => {
                let val = self.eval_expr(value)?;
                self.env.assign(target, val)?;
                Ok(ControlFlow::Normal)
            }
            PlPgSqlStatement::Return { value } => {
                let val = if let Some(expr) = value {
                    Some(self.eval_expr(expr)?)
                } else {
                    None
                };
                Ok(ControlFlow::Return(val))
            }
            PlPgSqlStatement::Null => Ok(ControlFlow::Normal),
            PlPgSqlStatement::If {
                branches,
                else_branch,
            } => {
                for branch in branches {
                    let cond = self.eval_expr(&branch.cond)?;
                    if is_truthy(&cond)? {
                        return self.exec_stmts(&branch.statements);
                    }
                }
                if let Some(else_stmts) = else_branch {
                    return self.exec_stmts(else_stmts);
                }
                Ok(ControlFlow::Normal)
            }
            PlPgSqlStatement::Case {
                selector,
                branches,
                else_branch,
            } => {
                let sel_val = if let Some(s) = selector {
                    Some(self.eval_expr(s)?)
                } else {
                    None
                };
                for branch in branches {
                    let cond_val = self.eval_expr(&branch.cond)?;
                    let matched = match &sel_val {
                        Some(s) => values_equal(s, &cond_val),
                        None => is_truthy(&cond_val)?,
                    };
                    if matched {
                        return self.exec_stmts(&branch.statements);
                    }
                }
                if let Some(else_stmts) = else_branch {
                    return self.exec_stmts(else_stmts);
                }
                // PG 语义：CASE 无匹配且无 ELSE 时抛出 case_not_found
                Err(PlInterpError::CaseNotFound)
            }
            PlPgSqlStatement::Loop { body, .. } => {
                let mut iter_count = 0usize;
                loop {
                    if iter_count >= self.max_loop_iterations {
                        return Err(PlInterpError::LoopLimitExceeded(iter_count));
                    }
                    iter_count += 1;

                    self.env.push();
                    let cf = self.exec_stmts(body)?;
                    self.env.pop();

                    match cf {
                        ControlFlow::Normal => continue,
                        ControlFlow::Return(v) => return Ok(ControlFlow::Return(v)),
                        ControlFlow::Exit { .. } => return Ok(ControlFlow::Normal),
                        ControlFlow::Continue { .. } => continue,
                    }
                }
            }
            PlPgSqlStatement::While { cond, body, .. } => {
                let mut iter_count = 0usize;
                loop {
                    if iter_count >= self.max_loop_iterations {
                        return Err(PlInterpError::LoopLimitExceeded(iter_count));
                    }
                    iter_count += 1;

                    let c = self.eval_expr(cond)?;
                    if !is_truthy(&c)? {
                        break;
                    }

                    self.env.push();
                    let cf = self.exec_stmts(body)?;
                    self.env.pop();

                    match cf {
                        ControlFlow::Normal => continue,
                        ControlFlow::Return(v) => return Ok(ControlFlow::Return(v)),
                        ControlFlow::Exit { .. } => return Ok(ControlFlow::Normal),
                        ControlFlow::Continue { .. } => continue,
                    }
                }
                Ok(ControlFlow::Normal)
            }
            PlPgSqlStatement::For {
                var,
                reverse,
                lower,
                upper,
                step,
                body,
                ..
            } => {
                // PG 语义：lower 为起始值，upper 为结束值
                // - 正向 `FOR i IN 1..10`：i 从 1 递增到 10
                // - 反向 `FOR i IN REVERSE 10..1`：i 从 10 递减到 1
                let start = self.eval_int(lower)?;
                let end = self.eval_int(upper)?;
                let st: i64 = match step {
                    Some(s) => {
                        let v = self.eval_int(s)?;
                        if v <= 0 {
                            return Err(PlInterpError::TypeError(format!(
                                "FOR step must be positive, got {v}"
                            )));
                        }
                        v
                    }
                    None => 1,
                };

                self.env.push();
                self.env.declare(var, Value::Null, false);

                let mut iter_count = 0usize;
                let mut i = start;
                loop {
                    if iter_count >= self.max_loop_iterations {
                        self.env.pop();
                        return Err(PlInterpError::LoopLimitExceeded(iter_count));
                    }
                    iter_count += 1;

                    let cond = if *reverse {
                        i >= end
                    } else {
                        i <= end
                    };
                    if !cond {
                        break;
                    }

                    self.env.assign(var, Value::Int64(i))?;

                    match self.exec_stmts(body)? {
                        ControlFlow::Normal => {}
                        ControlFlow::Return(v) => {
                            self.env.pop();
                            return Ok(ControlFlow::Return(v));
                        }
                        ControlFlow::Exit { .. } => {
                            self.env.pop();
                            return Ok(ControlFlow::Normal);
                        }
                        ControlFlow::Continue { .. } => {}
                    }

                    if *reverse {
                        i = match i.checked_sub(st) {
                            Some(v) => v,
                            None => break,
                        };
                    } else {
                        i = match i.checked_add(st) {
                            Some(v) => v,
                            None => break,
                        };
                    }
                }

                self.env.pop();
                Ok(ControlFlow::Normal)
            }
            PlPgSqlStatement::Exit { label, cond } => {
                if let Some(c) = cond {
                    let v = self.eval_expr(c)?;
                    if !is_truthy(&v)? {
                        return Ok(ControlFlow::Normal);
                    }
                }
                Ok(ControlFlow::Exit {
                    label: label.clone(),
                })
            }
            PlPgSqlStatement::Continue { label, cond } => {
                if let Some(c) = cond {
                    let v = self.eval_expr(c)?;
                    if !is_truthy(&v)? {
                        return Ok(ControlFlow::Normal);
                    }
                }
                Ok(ControlFlow::Continue {
                    label: label.clone(),
                })
            }
            PlPgSqlStatement::Block(block) => self.exec_block_inner(block),
            PlPgSqlStatement::Raise {
                level,
                format,
                args,
                ..
            } => {
                let msg = if let Some(fmt) = format {
                    let mut s = fmt.clone();
                    for arg in args {
                        let v = self.eval_expr(arg)?;
                        let v_str = value_to_display_string(&v);
                        // 替换第一个 % 占位符
                        if let Some(idx) = s.find('%') {
                            s.replace_range(idx..idx + 1, &v_str);
                        }
                    }
                    s
                } else {
                    String::new()
                };

                if *level == PlPgSqlRaiseLevel::Exception {
                    return Err(PlInterpError::UncaughtException(msg));
                }
                // 其他级别（DEBUG / LOG / INFO / NOTICE / WARNING）仅记录，不中断
                Ok(ControlFlow::Normal)
            }
            PlPgSqlStatement::ReturnNext { value } => {
                // 简化：仅求值，不累积（SETOF 返回值留待后续）
                let _v = self.eval_expr(value)?;
                Ok(ControlFlow::Normal)
            }
            PlPgSqlStatement::ReturnQuery { query } => {
                if let Some(executor) = self.sql_executor.as_deref_mut() {
                    let _rows = executor.execute_query(query)?;
                }
                Ok(ControlFlow::Normal)
            }
            PlPgSqlStatement::SelectInto { targets, query } => {
                if let Some(executor) = self.sql_executor.as_deref_mut() {
                    let rows = executor.execute_query(query)?;
                    if let Some(row) = rows.first() {
                        for (i, target) in targets.iter().enumerate() {
                            let val = row.get(i).cloned().unwrap_or(Value::Null);
                            self.env.assign(target, val)?;
                        }
                    }
                } else {
                    return Err(PlInterpError::Unsupported(
                        "SELECT INTO requires SQL executor".into(),
                    ));
                }
                Ok(ControlFlow::Normal)
            }
            PlPgSqlStatement::Perform { query } => {
                if let Some(executor) = self.sql_executor.as_deref_mut() {
                    let _ = executor.execute_query(query)?;
                }
                Ok(ControlFlow::Normal)
            }
            PlPgSqlStatement::Execute { query, into, using } => {
                // 求值 query 表达式得到动态 SQL 字符串
                let sql_text = self.eval_expr(query)?;
                let sql_str = match sql_text {
                    Value::Text(s) => s,
                    other => {
                        return Err(PlInterpError::TypeError(format!(
                            "EXECUTE expects text query, got {:?}",
                            other.column_type()
                        )))
                    }
                };

                // 求值 USING 参数（当前简化：仅求值，不传递）
                let _args: Vec<Value> = using
                    .iter()
                    .map(|e| self.eval_expr(e))
                    .collect::<Result<_, _>>()?;

                if let Some(executor) = self.sql_executor.as_deref_mut() {
                    if into.is_empty() {
                        executor.execute_stmt(&sql_str)?;
                    } else {
                        let rows = executor.execute_query(&sql_str)?;
                        if let Some(row) = rows.first() {
                            for (i, target) in into.iter().enumerate() {
                                let val = row.get(i).cloned().unwrap_or(Value::Null);
                                self.env.assign(target, val)?;
                            }
                        }
                    }
                }
                Ok(ControlFlow::Normal)
            }
            PlPgSqlStatement::Goto { .. } => {
                Err(PlInterpError::Unsupported("GOTO not supported".into()))
            }
            PlPgSqlStatement::SqlStatement { sql } => {
                if let Some(executor) = self.sql_executor.as_deref_mut() {
                    executor.execute_stmt(sql)?;
                }
                Ok(ControlFlow::Normal)
            }
            PlPgSqlStatement::ForQuery {
                var, query, body, ..
            } => {
                if let Some(executor) = self.sql_executor.as_deref_mut() {
                    let rows = executor.execute_query(query)?;
                    self.env.push();
                    self.env.declare(var, Value::Null, false);

                    for row in rows {
                        // 简化：取第一列赋给变量（完整 row 类型留待后续）
                        let val = row.first().cloned().unwrap_or(Value::Null);
                        self.env.assign(var, val)?;

                        match self.exec_stmts(body)? {
                            ControlFlow::Normal => continue,
                            ControlFlow::Return(v) => {
                                self.env.pop();
                                return Ok(ControlFlow::Return(v));
                            }
                            ControlFlow::Exit { .. } => {
                                self.env.pop();
                                return Ok(ControlFlow::Normal);
                            }
                            ControlFlow::Continue { .. } => continue,
                        }
                    }
                    self.env.pop();
                }
                Ok(ControlFlow::Normal)
            }
            PlPgSqlStatement::ForEach { var, body, .. } => {
                // 简化：从变量读取数组并迭代
                let arr = self.env.lookup(var).cloned().unwrap_or(Value::Null);
                self.env.push();

                if let Value::Array(items) = arr {
                    for item in items {
                        self.env.assign(var, item)?;
                        match self.exec_stmts(body)? {
                            ControlFlow::Normal => continue,
                            ControlFlow::Return(v) => {
                                self.env.pop();
                                return Ok(ControlFlow::Return(v));
                            }
                            ControlFlow::Exit { .. } => break,
                            ControlFlow::Continue { .. } => continue,
                        }
                    }
                }

                self.env.pop();
                Ok(ControlFlow::Normal)
            }
        }
    }

    /// 求值表达式（原始字符串）
    ///
    /// 使用 `std::mem::take` 临时将 `env` 移出 self，避免 `&self.env` 与 `&mut self`
    /// （作为 `PlFuncResolver`）的借用冲突。求值完成后立即恢复。
    fn eval_expr(&mut self, expr: &str) -> Result<Value, PlInterpError> {
        let env = std::mem::take(&mut self.env);
        let result = eval_pl_expr(expr, &env, self);
        self.env = env;
        result
    }

    /// 求值为整数
    fn eval_int(&mut self, expr: &str) -> Result<i64, PlInterpError> {
        let v = self.eval_expr(expr)?;
        match v {
            Value::Int64(n) => Ok(n),
            Value::Float64(f) => {
                if f.is_finite() {
                    Ok(f as i64)
                } else {
                    Err(PlInterpError::TypeError(format!(
                        "cannot convert non-finite float {f} to integer"
                    )))
                }
            }
            Value::Bool(b) => Ok(if b {
                1
            } else {
                0
            }),
            Value::Decimal(n, scale) => {
                let divisor = 10_i128.pow(u32::from(scale));
                Ok((n / divisor) as i64)
            }
            other => Err(PlInterpError::TypeError(format!(
                "expected integer, got {:?}",
                other.column_type()
            ))),
        }
    }

    /// 当前环境（仅供测试）
    #[cfg(test)]
    pub fn env_depth(&self) -> usize {
        self.env.depth()
    }
}

impl<'a> PlFuncResolver for PlPgSqlInterpreter<'a> {
    fn resolve_function(&mut self, name: &str, args: &[Value]) -> Result<Value, PlInterpError> {
        if self.function_registry.get(name).is_some() {
            // 用户定义函数：递归调用
            self.call(name, args).map(|v| v.unwrap_or(Value::Null))
        } else {
            // 不是用户定义函数：返回 FunctionNotFound，让表达式求值器回退到内置函数
            Err(PlInterpError::FunctionNotFound(name.into()))
        }
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 根据声明的类型名将 Value 进行隐式类型转换
///
/// 例如：`DECLARE x numeric := 3` 会将 `Int64(3)` 转为 `Decimal(3, 0)`。
/// 若类型名不被识别或无需转换，原样返回。
fn coerce_to_type(val: Value, type_name: &str) -> Value {
    let lower = type_name.to_lowercase();
    match (val, lower.as_str()) {
        // numeric / decimal → Decimal
        (Value::Int64(n), "numeric" | "decimal" | "numeric(") => Value::Decimal(n as i128, 0),
        (Value::Float64(f), "numeric" | "decimal") => {
            // 近似转换：保留 6 位小数
            let scaled = (f * 1_000_000.0).round() as i128;
            Value::Decimal(scaled, 6)
        }
        // int / integer / bigint → Int64
        (Value::Decimal(n, scale), "int" | "integer" | "bigint" | "int4" | "int8") => {
            let divisor = 10_i128.pow(u32::from(scale));
            Value::Int64((n / divisor) as i64)
        }
        (Value::Float64(f), "int" | "integer" | "bigint" | "int4" | "int8") => {
            Value::Int64(f as i64)
        }
        // float / double → Float64
        (Value::Int64(n), "float" | "double" | "real" | "float8" | "double precision") => {
            Value::Float64(n as f64)
        }
        (Value::Decimal(n, scale), "float" | "double" | "real" | "float8" | "double precision") => {
            let divisor = 10_f64.powi(i32::from(scale));
            Value::Float64(n as f64 / divisor)
        }
        // text / varchar → Text
        (Value::Int64(n), "text" | "varchar" | "char" | "string") => Value::Text(n.to_string()),
        (Value::Bool(b), "text" | "varchar" | "char" | "string") => Value::Text(b.to_string()),
        // bool / boolean → Bool
        (Value::Int64(n), "bool" | "boolean") => Value::Bool(n != 0),
        // 其他情况：不转换
        (v, _) => v,
    }
}

/// 判断值是否为"真"（PG 语义：NULL 为假，布尔直接判断，数值非零为真）
fn is_truthy(v: &Value) -> Result<bool, PlInterpError> {
    match v {
        Value::Null => Ok(false),
        Value::Bool(b) => Ok(*b),
        Value::Int64(n) => Ok(*n != 0),
        Value::Float64(f) => Ok(*f != 0.0),
        Value::Decimal(n, _) => Ok(*n != 0),
        Value::Text(s) => Ok(!s.is_empty()),
        _ => Err(PlInterpError::TypeError(format!(
            "cannot evaluate truthiness of {:?}",
            v.column_type()
        ))),
    }
}

/// 判断两个值是否相等（PG 语义：类型不同则尝试隐式转换后比较）
fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true, // CASE 中 NULL = NULL 视为匹配（PG 行为）
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::Int64(x), Value::Int64(y)) => x == y,
        (Value::Float64(x), Value::Float64(y)) => x == y,
        (Value::Int64(x), Value::Float64(y)) => (*x as f64) == *y,
        (Value::Float64(x), Value::Int64(y)) => *x == (*y as f64),
        (Value::Text(x), Value::Text(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int64(x), Value::Bool(y)) => (*x != 0) == *y,
        (Value::Bool(x), Value::Int64(y)) => *x == (*y != 0),
        _ => a == b,
    }
}

/// 将值转为显示字符串（用于 RAISE 格式化）
fn value_to_display_string(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Int64(n) => n.to_string(),
        Value::Float64(f) => f.to_string(),
        Value::Text(s) => s.clone(),
        Value::Bool(b) => {
            if *b {
                "t".into()
            } else {
                "f".into()
            }
        }
        Value::Decimal(n, scale) => {
            if *scale == 0 {
                n.to_string()
            } else {
                let divisor = 10_i128.pow(u32::from(*scale));
                let int_part = n / divisor;
                let frac_part = (n % divisor).abs();
                format!("{int_part}.{frac_part:0>scale$}", scale = *scale as usize)
            }
        }
        other => format!("{other:?}"),
    }
}

/// 将 PlInterpError 映射为 SQLSTATE（简化版，仅常见错误）
fn pl_error_sqlstate(e: &PlInterpError) -> String {
    match e {
        PlInterpError::DivisionByZero => "22012".into(),
        PlInterpError::ConstantReassign(_) => "55006".into(),
        PlInterpError::StackOverflow { .. } => "54001".into(),
        PlInterpError::CaseNotFound => "20000".into(),
        PlInterpError::UncaughtException(msg) => {
            // 如果是嵌套异常（RAISE EXCEPTION），尝试从消息中提取 SQLSTATE
            // 格式：'message' 或 'message (SQLSTATE)'
            if let Some(start) = msg.rfind("SQLSTATE ") {
                let rest = &msg[start + "SQLSTATE ".len()..];
                // ADV-BUG-003 修复：使用 get(..5) 避免跨越 UTF-8 字符边界 panic
                // SQLSTATE 是 5 位 ASCII 数字/字母，但消息可能含 Unicode，需防御性切片
                if let Some(prefix) = rest.get(..5) {
                    return prefix.to_string();
                }
            }
            "P0001".into() // 默认 RAISE_EXCEPTION
        }
        PlInterpError::VarNotFound(_) => "42701".into(),
        PlInterpError::TypeError(_) => "42804".into(),
        PlInterpError::IntegerOverflow(_) => "22003".into(),
        PlInterpError::LoopLimitExceeded(_) => "P0001".into(),
        PlInterpError::SqlError(_) => "P0001".into(),
        PlInterpError::ParseError(_) => "42601".into(),
        PlInterpError::Unsupported(_) => "0A000".into(),
        PlInterpError::FunctionNotFound(_) => "42883".into(),
        PlInterpError::ArgCountMismatch { .. } => "42883".into(),
        PlInterpError::ExprParseError(_) => "42601".into(),
    }
}

/// PG 标准条件名 → SQLSTATE 映射
///
/// 参考：https://www.postgresql.org/docs/current/errcodes-appendix.html
fn condition_name_to_sqlstate(name: &str) -> Option<&str> {
    match name {
        // 数据异常类 (Class 22)
        "DIVISION_BY_ZERO" => Some("22012"),
        "INTEGER_OVERFLOW" | "NUMERIC_VALUE_OUT_OF_RANGE" => Some("22003"),
        "INVALID_TEXT_REPRESENTATION" => Some("22P02"),
        "DATETIME_FIELD_OVERFLOW" => Some("22008"),
        "NULL_VALUE_NOT_ALLOWED" => Some("22004"),
        "STRING_DATA_RIGHT_TRUNCATION" => Some("22001"),
        "INVALID_PARAMETER_VALUE" => Some("22023"),
        // 类 23 — 完整性约束违反
        "UNIQUE_VIOLATION" => Some("23505"),
        "FOREIGN_KEY_VIOLATION" => Some("23503"),
        "NOT_NULL_VIOLATION" => Some("23502"),
        "CHECK_VIOLATION" => Some("23514"),
        // 类 42 — 语法错误或访问规则违反
        "SYNTAX_ERROR" => Some("42601"),
        "UNDEFINED_COLUMN" => Some("42703"),
        "UNDEFINED_TABLE" => Some("42P01"),
        "UNDEFINED_FUNCTION" => Some("42883"),
        "DATATYPE_MISMATCH" => Some("42804"),
        // 类 54 — 程序限制
        "PROGRAM_LIMIT_EXCEEDED" => Some("54001"),
        // 类 55 — 对象不在先决条件状态
        "OBJECT_NOT_IN_PREREQUISITE_STATE" => Some("55006"),
        // PL/pgSQL 错误
        "RAISE_EXCEPTION" => Some("P0001"),
        "CASE_NOT_FOUND" => Some("20000"),
        // 特殊
        "SUCCESSFUL_COMPLETION" => Some("00000"),
        "TRANSACTION_ROLLBACK" => Some("40001"),
        "SERIALIZATION_FAILURE" => Some("40001"),
        "DEADLOCK_DETECTED" => Some("40P01"),
        _ => None,
    }
}

/// 将 PlInterpError 转为消息字符串
fn pl_error_message(e: &PlInterpError) -> String {
    format!("{e}")
}

// =====================================================================
//  PL/pgSQL 表达式求值器
// =====================================================================
//
// 独立的轻量级表达式求值器，覆盖 PL/pgSQL 常用表达式：
//
// - 字面量：整数 / 浮点 / 字符串 / TRUE / FALSE / NULL
// - 变量引用：标识符
// - 二元运算：+ - * / % = <> != < > <= >= AND OR ||
// - 一元运算：- NOT
// - 括号分组
// - 函数调用：abs / length / lower / upper / coalesce / now / sqrt / round / ceil / floor 等
// - CASE WHEN ... THEN ... ELSE ... END
// - IS NULL / IS NOT NULL
// - IN (val1, val2, ...)
// - BETWEEN low AND high
//
// 不支持的语法返回 `ExprParseError`。

mod pl_expr {
    use super::*;

    /// 表达式 token
    #[derive(Debug, Clone, PartialEq)]
    enum Token {
        Int(i64),
        Float(f64),
        Str(String),
        Ident(String),
        True,
        False,
        Null,
        Plus,
        Minus,
        Star,
        Slash,
        Percent,
        Eq,
        NotEq,
        NotEqAlt, // <>
        Lt,
        Gt,
        LtEq,
        GtEq,
        And,
        Or,
        Not,
        Concat, // ||
        LParen,
        RParen,
        Comma,
        Case,
        When,
        Then,
        Else,
        End,
        Is,
        In,
        Between,
        Like,
        Dot,
        Eof,
    }

    /// 词法分析器
    struct Lexer<'a> {
        src: &'a [u8],
        pos: usize,
    }

    impl<'a> Lexer<'a> {
        fn new(src: &'a str) -> Self {
            Self {
                src: src.as_bytes(),
                pos: 0,
            }
        }

        fn peek(&self) -> Option<u8> {
            self.src.get(self.pos).copied()
        }

        fn peek_at(&self, n: usize) -> Option<u8> {
            self.src.get(self.pos + n).copied()
        }

        fn advance(&mut self) -> Option<u8> {
            let c = self.src.get(self.pos).copied()?;
            self.pos += 1;
            Some(c)
        }

        fn skip_whitespace(&mut self) {
            while let Some(c) = self.peek() {
                if c.is_ascii_whitespace() {
                    self.advance();
                } else if c == b'-' && self.peek_at(1) == Some(b'-') {
                    // 行注释
                    while let Some(c) = self.peek() {
                        if c == b'\n' {
                            break;
                        }
                        self.advance();
                    }
                } else if c == b'/' && self.peek_at(1) == Some(b'*') {
                    // 块注释
                    self.advance();
                    self.advance();
                    let mut depth = 1;
                    while depth > 0 {
                        match self.peek() {
                            None => break,
                            Some(b'/') if self.peek_at(1) == Some(b'*') => {
                                self.advance();
                                self.advance();
                                depth += 1;
                            }
                            Some(b'*') if self.peek_at(1) == Some(b'/') => {
                                self.advance();
                                self.advance();
                                depth -= 1;
                            }
                            Some(_) => {
                                self.advance();
                            }
                        }
                    }
                } else {
                    break;
                }
            }
        }

        fn read_string(&mut self) -> Result<Token, PlInterpError> {
            self.advance(); // 消费开头的 '
            let mut result = String::new();
            loop {
                match self.peek() {
                    None => {
                        return Err(PlInterpError::ExprParseError(
                            "unterminated string literal".into(),
                        ))
                    }
                    Some(b'\'') if self.peek_at(1) == Some(b'\'') => {
                        result.push('\'');
                        self.advance();
                        self.advance();
                    }
                    Some(b'\'') => {
                        self.advance();
                        break;
                    }
                    Some(c) => {
                        result.push(c as char);
                        self.advance();
                    }
                }
            }
            Ok(Token::Str(result))
        }

        fn read_number(&mut self) -> Result<Token, PlInterpError> {
            let start = self.pos;
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.advance();
                } else {
                    break;
                }
            }
            // 浮点：. 后跟数字
            if self.peek() == Some(b'.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
                self.advance(); // .
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() {
                        self.advance();
                    } else {
                        break;
                    }
                }
                // 指数 e[+-]?[0-9]+
                if self.peek() == Some(b'e') || self.peek() == Some(b'E') {
                    self.advance();
                    if self.peek() == Some(b'+') || self.peek() == Some(b'-') {
                        self.advance();
                    }
                    while let Some(c) = self.peek() {
                        if c.is_ascii_digit() {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                }
                let text = std::str::from_utf8(&self.src[start..self.pos])
                    .map_err(|_| PlInterpError::ExprParseError("invalid UTF-8 in number".into()))?;
                let f: f64 = text.parse().map_err(|_| {
                    PlInterpError::ExprParseError(format!("invalid float literal: {text}"))
                })?;
                Ok(Token::Float(f))
            } else {
                let text = std::str::from_utf8(&self.src[start..self.pos])
                    .map_err(|_| PlInterpError::ExprParseError("invalid UTF-8 in number".into()))?;
                let n: i64 = text.parse().map_err(|_| {
                    PlInterpError::ExprParseError(format!("invalid integer literal: {text}"))
                })?;
                Ok(Token::Int(n))
            }
        }

        fn read_ident(&mut self) -> Token {
            let start = self.pos;
            while let Some(c) = self.peek() {
                if c.is_ascii_alphanumeric() || c == b'_' {
                    self.advance();
                } else {
                    break;
                }
            }
            let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("");
            match text.to_uppercase().as_str() {
                "TRUE" => Token::True,
                "FALSE" => Token::False,
                "NULL" => Token::Null,
                "AND" => Token::And,
                "OR" => Token::Or,
                "NOT" => Token::Not,
                "CASE" => Token::Case,
                "WHEN" => Token::When,
                "THEN" => Token::Then,
                "ELSE" => Token::Else,
                "END" => Token::End,
                "IS" => Token::Is,
                "IN" => Token::In,
                "BETWEEN" => Token::Between,
                "LIKE" => Token::Like,
                _ => Token::Ident(text.to_string()),
            }
        }

        fn next_token(&mut self) -> Result<Token, PlInterpError> {
            self.skip_whitespace();
            match self.peek() {
                None => Ok(Token::Eof),
                Some(c) => match c {
                    b'\'' => self.read_string(),
                    b'0'..=b'9' => self.read_number(),
                    b'a'..=b'z' | b'A'..=b'Z' | b'_' => Ok(self.read_ident()),
                    b'+' => {
                        self.advance();
                        Ok(Token::Plus)
                    }
                    b'-' => {
                        self.advance();
                        Ok(Token::Minus)
                    }
                    b'*' => {
                        self.advance();
                        Ok(Token::Star)
                    }
                    b'/' => {
                        self.advance();
                        Ok(Token::Slash)
                    }
                    b'%' => {
                        self.advance();
                        Ok(Token::Percent)
                    }
                    b'=' => {
                        self.advance();
                        Ok(Token::Eq)
                    }
                    b'!' if self.peek_at(1) == Some(b'=') => {
                        self.advance();
                        self.advance();
                        Ok(Token::NotEq)
                    }
                    b'<' => {
                        self.advance();
                        if self.peek() == Some(b'>') {
                            self.advance();
                            Ok(Token::NotEqAlt)
                        } else if self.peek() == Some(b'=') {
                            self.advance();
                            Ok(Token::LtEq)
                        } else {
                            Ok(Token::Lt)
                        }
                    }
                    b'>' => {
                        self.advance();
                        if self.peek() == Some(b'=') {
                            self.advance();
                            Ok(Token::GtEq)
                        } else {
                            Ok(Token::Gt)
                        }
                    }
                    b'|' if self.peek_at(1) == Some(b'|') => {
                        self.advance();
                        self.advance();
                        Ok(Token::Concat)
                    }
                    b'(' => {
                        self.advance();
                        Ok(Token::LParen)
                    }
                    b')' => {
                        self.advance();
                        Ok(Token::RParen)
                    }
                    b',' => {
                        self.advance();
                        Ok(Token::Comma)
                    }
                    b'.' => {
                        // 检查是否是浮点数字面量（.5 等）
                        if self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
                            self.read_number()
                        } else {
                            self.advance();
                            Ok(Token::Dot)
                        }
                    }
                    b':' if self.peek_at(1) == Some(b':') => {
                        // :: 类型转换运算符 — 简化为跳过类型
                        self.advance();
                        self.advance();
                        // 跳过类型名（标识符）
                        self.skip_whitespace();
                        while let Some(c) = self.peek() {
                            if c.is_ascii_alphanumeric() || c == b'_' || c == b'(' || c == b')' {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                        // 返回下一个 token
                        self.next_token()
                    }
                    other => Err(PlInterpError::ExprParseError(format!(
                        "unexpected character: '{}'",
                        other as char
                    ))),
                },
            }
        }
    }

    /// 表达式 AST
    #[derive(Debug, Clone)]
    enum Expr {
        Lit(Value),
        Var(String),
        UnaryNeg(Box<Expr>),
        Not(Box<Expr>),
        Binary {
            op: BinOp,
            left: Box<Expr>,
            right: Box<Expr>,
        },
        FuncCall {
            name: String,
            args: Vec<Expr>,
        },
        Case {
            operand: Option<Box<Expr>>,
            when_then: Vec<(Expr, Expr)>,
            else_expr: Option<Box<Expr>>,
        },
        IsNull(Box<Expr>, bool),                        // true = IS NOT NULL
        InList(Box<Expr>, Vec<Expr>, bool),             // true = NOT IN
        Between(Box<Expr>, Box<Expr>, Box<Expr>, bool), // true = NOT BETWEEN
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum BinOp {
        Add,
        Sub,
        Mul,
        Div,
        Mod,
        Eq,
        NotEq,
        Lt,
        Gt,
        LtEq,
        GtEq,
        And,
        Or,
        Concat,
    }

    /// Parser
    struct Parser<'a> {
        tokens: Vec<Token>,
        pos: usize,
        _src: &'a str,
    }

    impl<'a> Parser<'a> {
        fn new(src: &'a str) -> Result<Self, PlInterpError> {
            let mut lexer = Lexer::new(src);
            let mut tokens = Vec::new();
            loop {
                let tok = lexer.next_token()?;
                if matches!(tok, Token::Eof) {
                    tokens.push(tok);
                    break;
                }
                tokens.push(tok);
            }
            Ok(Self {
                tokens,
                pos: 0,
                _src: src,
            })
        }

        fn peek(&self) -> &Token {
            &self.tokens[self.pos]
        }

        fn advance(&mut self) -> Token {
            let t = self.tokens[self.pos].clone();
            if self.pos + 1 < self.tokens.len() {
                self.pos += 1;
            }
            t
        }

        fn match_tok(&mut self, expected: &Token) -> bool {
            if self.peek() == expected {
                self.advance();
                true
            } else {
                false
            }
        }

        fn expect(&mut self, expected: &Token, ctx: &str) -> Result<(), PlInterpError> {
            if self.peek() == expected {
                self.advance();
                Ok(())
            } else {
                Err(PlInterpError::ExprParseError(format!(
                    "expected {ctx}, got {:?}",
                    self.peek()
                )))
            }
        }

        /// 入口：解析整个表达式
        fn parse(&mut self) -> Result<Expr, PlInterpError> {
            let expr = self.parse_or()?;
            if !matches!(self.peek(), Token::Eof) {
                return Err(PlInterpError::ExprParseError(format!(
                    "unexpected token after expression: {:?}",
                    self.peek()
                )));
            }
            Ok(expr)
        }

        /// OR 优先级最低
        fn parse_or(&mut self) -> Result<Expr, PlInterpError> {
            let mut left = self.parse_and()?;
            while matches!(self.peek(), Token::Or) {
                self.advance();
                let right = self.parse_and()?;
                left = Expr::Binary {
                    op: BinOp::Or,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }

        fn parse_and(&mut self) -> Result<Expr, PlInterpError> {
            let mut left = self.parse_not()?;
            while matches!(self.peek(), Token::And) {
                self.advance();
                let right = self.parse_not()?;
                left = Expr::Binary {
                    op: BinOp::And,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }

        fn parse_not(&mut self) -> Result<Expr, PlInterpError> {
            if matches!(self.peek(), Token::Not) {
                self.advance();
                let inner = self.parse_not()?;
                return Ok(Expr::Not(Box::new(inner)));
            }
            self.parse_comparison()
        }

        fn parse_comparison(&mut self) -> Result<Expr, PlInterpError> {
            let mut left = self.parse_between_in()?;
            loop {
                let op = match self.peek() {
                    Token::Eq => BinOp::Eq,
                    Token::NotEq => BinOp::NotEq,
                    Token::NotEqAlt => BinOp::NotEq,
                    Token::Lt => BinOp::Lt,
                    Token::Gt => BinOp::Gt,
                    Token::LtEq => BinOp::LtEq,
                    Token::GtEq => BinOp::GtEq,
                    _ => break,
                };
                self.advance();
                let right = self.parse_between_in()?;
                left = Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }

        /// 处理 IS NULL / IN / BETWEEN / LIKE
        fn parse_between_in(&mut self) -> Result<Expr, PlInterpError> {
            let mut left = self.parse_concat()?;

            loop {
                match self.peek() {
                    Token::Is => {
                        self.advance();
                        let negated = self.match_tok(&Token::Not);
                        self.expect(&Token::Null, "NULL after IS [NOT]")?;
                        left = Expr::IsNull(Box::new(left), negated);
                    }
                    Token::Not => {
                        // NOT IN / NOT BETWEEN / NOT LIKE
                        self.advance();
                        match self.peek() {
                            Token::In => {
                                self.advance();
                                let list = self.parse_in_list()?;
                                left = Expr::InList(Box::new(left), list, true);
                            }
                            Token::Between => {
                                self.advance();
                                let low = self.parse_concat()?;
                                self.expect(&Token::And, "AND in BETWEEN")?;
                                let high = self.parse_concat()?;
                                left = Expr::Between(
                                    Box::new(left),
                                    Box::new(low),
                                    Box::new(high),
                                    true,
                                );
                            }
                            Token::Like => {
                                self.advance();
                                let pattern = self.parse_concat()?;
                                // 简化：转换为字符串比较
                                left = Expr::FuncCall {
                                    name: "__not_like".into(),
                                    args: vec![left, pattern],
                                };
                            }
                            _ => {
                                return Err(PlInterpError::ExprParseError(format!(
                                    "expected IN/BETWEEN/LIKE after NOT, got {:?}",
                                    self.peek()
                                )))
                            }
                        }
                    }
                    Token::In => {
                        self.advance();
                        let list = self.parse_in_list()?;
                        left = Expr::InList(Box::new(left), list, false);
                    }
                    Token::Between => {
                        self.advance();
                        let low = self.parse_concat()?;
                        self.expect(&Token::And, "AND in BETWEEN")?;
                        let high = self.parse_concat()?;
                        left = Expr::Between(Box::new(left), Box::new(low), Box::new(high), false);
                    }
                    Token::Like => {
                        self.advance();
                        let pattern = self.parse_concat()?;
                        left = Expr::FuncCall {
                            name: "__like".into(),
                            args: vec![left, pattern],
                        };
                    }
                    _ => break,
                }
            }
            Ok(left)
        }

        fn parse_in_list(&mut self) -> Result<Vec<Expr>, PlInterpError> {
            self.expect(&Token::LParen, "'(' in IN list")?;
            let mut list = Vec::new();
            if !matches!(self.peek(), Token::RParen) {
                list.push(self.parse_or()?);
                while matches!(self.peek(), Token::Comma) {
                    self.advance();
                    list.push(self.parse_or()?);
                }
            }
            self.expect(&Token::RParen, "')' in IN list")?;
            Ok(list)
        }

        /// 字符串拼接 ||
        fn parse_concat(&mut self) -> Result<Expr, PlInterpError> {
            let mut left = self.parse_additive()?;
            while matches!(self.peek(), Token::Concat) {
                self.advance();
                let right = self.parse_additive()?;
                left = Expr::Binary {
                    op: BinOp::Concat,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }

        fn parse_additive(&mut self) -> Result<Expr, PlInterpError> {
            let mut left = self.parse_multiplicative()?;
            loop {
                let op = match self.peek() {
                    Token::Plus => BinOp::Add,
                    Token::Minus => BinOp::Sub,
                    _ => break,
                };
                self.advance();
                let right = self.parse_multiplicative()?;
                left = Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }

        fn parse_multiplicative(&mut self) -> Result<Expr, PlInterpError> {
            let mut left = self.parse_unary()?;
            loop {
                let op = match self.peek() {
                    Token::Star => BinOp::Mul,
                    Token::Slash => BinOp::Div,
                    Token::Percent => BinOp::Mod,
                    _ => break,
                };
                self.advance();
                let right = self.parse_unary()?;
                left = Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            }
            Ok(left)
        }

        fn parse_unary(&mut self) -> Result<Expr, PlInterpError> {
            if matches!(self.peek(), Token::Minus) {
                self.advance();
                let inner = self.parse_unary()?;
                return Ok(Expr::UnaryNeg(Box::new(inner)));
            }
            if matches!(self.peek(), Token::Plus) {
                self.advance();
                return self.parse_unary();
            }
            self.parse_primary()
        }

        fn parse_primary(&mut self) -> Result<Expr, PlInterpError> {
            match self.peek().clone() {
                Token::Int(n) => {
                    self.advance();
                    Ok(Expr::Lit(Value::Int64(n)))
                }
                Token::Float(f) => {
                    self.advance();
                    Ok(Expr::Lit(Value::Float64(f)))
                }
                Token::Str(s) => {
                    self.advance();
                    Ok(Expr::Lit(Value::Text(s)))
                }
                Token::True => {
                    self.advance();
                    Ok(Expr::Lit(Value::Bool(true)))
                }
                Token::False => {
                    self.advance();
                    Ok(Expr::Lit(Value::Bool(false)))
                }
                Token::Null => {
                    self.advance();
                    Ok(Expr::Lit(Value::Null))
                }
                Token::Ident(name) => {
                    self.advance();
                    // 函数调用？
                    if matches!(self.peek(), Token::LParen) {
                        self.advance();
                        let mut args = Vec::new();
                        if !matches!(self.peek(), Token::RParen) {
                            args.push(self.parse_or()?);
                            while matches!(self.peek(), Token::Comma) {
                                self.advance();
                                args.push(self.parse_or()?);
                            }
                        }
                        self.expect(&Token::RParen, "')' in function call")?;
                        Ok(Expr::FuncCall { name, args })
                    } else {
                        Ok(Expr::Var(name))
                    }
                }
                Token::LParen => {
                    self.advance();
                    let expr = self.parse_or()?;
                    self.expect(&Token::RParen, "')'")?;
                    Ok(expr)
                }
                Token::Case => self.parse_case(),
                other => Err(PlInterpError::ExprParseError(format!(
                    "unexpected token: {other:?}"
                ))),
            }
        }

        fn parse_case(&mut self) -> Result<Expr, PlInterpError> {
            self.advance(); // CASE

            // 可选 selector
            let mut operand = None;
            if !matches!(self.peek(), Token::When) {
                operand = Some(Box::new(self.parse_or()?));
            }

            let mut when_then = Vec::new();
            while matches!(self.peek(), Token::When) {
                self.advance();
                let cond = self.parse_or()?;
                self.expect(&Token::Then, "THEN")?;
                let result = self.parse_or()?;
                when_then.push((cond, result));
            }

            let mut else_expr = None;
            if matches!(self.peek(), Token::Else) {
                self.advance();
                else_expr = Some(Box::new(self.parse_or()?));
            }

            self.expect(&Token::End, "END")?;
            Ok(Expr::Case {
                operand,
                when_then,
                else_expr,
            })
        }
    }

    /// 求值 AST
    fn eval_ast(
        expr: &Expr,
        env: &Environment,
        resolver: &mut dyn PlFuncResolver,
    ) -> Result<Value, PlInterpError> {
        match expr {
            Expr::Lit(v) => Ok(v.clone()),
            Expr::Var(name) => env
                .lookup(name)
                .cloned()
                .ok_or_else(|| PlInterpError::VarNotFound(name.clone())),
            Expr::UnaryNeg(inner) => {
                let v = eval_ast(inner, env, resolver)?;
                match v {
                    Value::Null => Ok(Value::Null),
                    Value::Int64(n) => n
                        .checked_neg()
                        .map(Value::Int64)
                        .ok_or_else(|| PlInterpError::IntegerOverflow(format!("-{n}"))),
                    Value::Float64(f) => Ok(Value::Float64(-f)),
                    Value::Decimal(n, scale) => n
                        .checked_neg()
                        .map(|v| Value::Decimal(v, scale))
                        .ok_or_else(|| PlInterpError::IntegerOverflow(format!("-{n}"))),
                    other => Err(PlInterpError::TypeError(format!(
                        "cannot negate {:?}",
                        other.column_type()
                    ))),
                }
            }
            Expr::Not(inner) => {
                let v = eval_ast(inner, env, resolver)?;
                match v {
                    Value::Null => Ok(Value::Null),
                    Value::Bool(b) => Ok(Value::Bool(!b)),
                    Value::Int64(n) => Ok(Value::Bool(n == 0)),
                    _ => Err(PlInterpError::TypeError(format!(
                        "cannot NOT {:?}",
                        v.column_type()
                    ))),
                }
            }
            Expr::Binary { op, left, right } => {
                let l = eval_ast(left, env, resolver)?;
                let r = eval_ast(right, env, resolver)?;
                eval_binary(*op, &l, &r)
            }
            Expr::FuncCall { name, args } => {
                let name_lower = name.to_lowercase();
                let mut vals = Vec::with_capacity(args.len());
                for arg in args {
                    vals.push(eval_ast(arg, env, resolver)?);
                }
                // 先尝试用户定义函数，若不存在则回退到内置函数
                match resolver.resolve_function(&name_lower, &vals) {
                    Ok(v) => Ok(v),
                    Err(PlInterpError::FunctionNotFound(_)) => eval_builtin(&name_lower, &vals),
                    Err(e) => Err(e),
                }
            }
            Expr::Case {
                operand,
                when_then,
                else_expr,
            } => {
                let sel = if let Some(op) = operand {
                    Some(eval_ast(op, env, resolver)?)
                } else {
                    None
                };
                for (cond, result) in when_then {
                    let matched = if let Some(s) = &sel {
                        let cond_val = eval_ast(cond, env, resolver)?;
                        values_equal(s, &cond_val)
                    } else {
                        let cond_val = eval_ast(cond, env, resolver)?;
                        is_truthy(&cond_val)?
                    };
                    if matched {
                        return eval_ast(result, env, resolver);
                    }
                }
                if let Some(e) = else_expr {
                    eval_ast(e, env, resolver)
                } else {
                    Ok(Value::Null)
                }
            }
            Expr::IsNull(inner, negated) => {
                let v = eval_ast(inner, env, resolver)?;
                let is_null = matches!(v, Value::Null);
                Ok(Value::Bool(if *negated {
                    !is_null
                } else {
                    is_null
                }))
            }
            Expr::InList(expr, list, negated) => {
                let v = eval_ast(expr, env, resolver)?;
                if matches!(v, Value::Null) {
                    return Ok(Value::Null);
                }
                let mut found = false;
                let mut has_null = false;
                for item_expr in list {
                    let item_val = eval_ast(item_expr, env, resolver)?;
                    if matches!(item_val, Value::Null) {
                        has_null = true;
                    } else if values_equal(&v, &item_val) {
                        found = true;
                        break;
                    }
                }
                if found {
                    Ok(Value::Bool(!*negated))
                } else if has_null {
                    Ok(Value::Null)
                } else {
                    Ok(Value::Bool(*negated))
                }
            }
            Expr::Between(expr, low, high, negated) => {
                let v = eval_ast(expr, env, resolver)?;
                if matches!(v, Value::Null) {
                    return Ok(Value::Null);
                }
                let lo = eval_ast(low, env, resolver)?;
                let hi = eval_ast(high, env, resolver)?;
                let ge_low = compare_values(&v, &lo).is_some_and(|o| o.is_ge());
                let le_high = compare_values(&v, &hi).is_some_and(|o| o.is_le());
                let in_range = ge_low && le_high;
                Ok(Value::Bool(if *negated {
                    !in_range
                } else {
                    in_range
                }))
            }
        }
    }

    /// 二元运算求值
    fn eval_binary(op: BinOp, l: &Value, r: &Value) -> Result<Value, PlInterpError> {
        // NULL 处理：除了 IS NULL/IS NOT NULL（在外部处理），所有运算遇到 NULL 返回 NULL
        // 但 AND/OR 有三值逻辑：NULL AND false = false; NULL OR true = true
        match op {
            BinOp::And => {
                if matches!(l, Value::Bool(false)) || matches!(r, Value::Bool(false)) {
                    return Ok(Value::Bool(false));
                }
                if matches!(l, Value::Null) || matches!(r, Value::Null) {
                    return Ok(Value::Null);
                }
                let lb = is_truthy(l)?;
                let rb = is_truthy(r)?;
                Ok(Value::Bool(lb && rb))
            }
            BinOp::Or => {
                if matches!(l, Value::Bool(true)) || matches!(r, Value::Bool(true)) {
                    return Ok(Value::Bool(true));
                }
                if matches!(l, Value::Null) || matches!(r, Value::Null) {
                    return Ok(Value::Null);
                }
                let lb = is_truthy(l)?;
                let rb = is_truthy(r)?;
                Ok(Value::Bool(lb || rb))
            }
            _ => {
                if matches!(l, Value::Null) || matches!(r, Value::Null) {
                    return Ok(Value::Null);
                }
                match op {
                    BinOp::Add => arith_op(l, r, |a, b| a.checked_add(b), |a, b| a + b),
                    BinOp::Sub => arith_op(l, r, |a, b| a.checked_sub(b), |a, b| a - b),
                    BinOp::Mul => arith_op(l, r, |a, b| a.checked_mul(b), |a, b| a * b),
                    BinOp::Div => {
                        // 整数除零检查
                        match (l, r) {
                            (Value::Int64(_), Value::Int64(0)) => {
                                Err(PlInterpError::DivisionByZero)
                            }
                            (Value::Int64(a), Value::Int64(b)) => a
                                .checked_div(*b)
                                .map(Value::Int64)
                                .ok_or(PlInterpError::DivisionByZero),
                            (Value::Float64(_), Value::Float64(b)) if *b == 0.0 => {
                                Err(PlInterpError::DivisionByZero)
                            }
                            (Value::Float64(a), Value::Float64(b)) => Ok(Value::Float64(a / b)),
                            _ => Err(PlInterpError::TypeError(format!(
                                "cannot divide {:?} by {:?}",
                                l.column_type(),
                                r.column_type()
                            ))),
                        }
                    }
                    BinOp::Mod => match (l, r) {
                        (Value::Int64(_), Value::Int64(0)) => Err(PlInterpError::DivisionByZero),
                        (Value::Int64(a), Value::Int64(b)) => Ok(Value::Int64(a % b)),
                        (Value::Float64(a), Value::Float64(b)) => Ok(Value::Float64(a % b)),
                        _ => Err(PlInterpError::TypeError(format!(
                            "cannot mod {:?} by {:?}",
                            l.column_type(),
                            r.column_type()
                        ))),
                    },
                    BinOp::Eq => Ok(Value::Bool(values_equal(l, r))),
                    BinOp::NotEq => Ok(Value::Bool(!values_equal(l, r))),
                    BinOp::Lt => Ok(Value::Bool(compare_values(l, r).is_some_and(|o| o.is_lt()))),
                    BinOp::Gt => Ok(Value::Bool(compare_values(l, r).is_some_and(|o| o.is_gt()))),
                    BinOp::LtEq => Ok(Value::Bool(compare_values(l, r).is_some_and(|o| o.is_le()))),
                    BinOp::GtEq => Ok(Value::Bool(compare_values(l, r).is_some_and(|o| o.is_ge()))),
                    BinOp::Concat => {
                        let ls = value_to_display_string(l);
                        let rs = value_to_display_string(r);
                        Ok(Value::Text(format!("{ls}{rs}")))
                    }
                    BinOp::And | BinOp::Or => unreachable!(),
                }
            }
        }
    }

    /// 算术运算辅助（Int64 / Float64 混合）
    fn arith_op(
        l: &Value,
        r: &Value,
        int_op: impl Fn(i64, i64) -> Option<i64>,
        float_op: impl Fn(f64, f64) -> f64,
    ) -> Result<Value, PlInterpError> {
        match (l, r) {
            (Value::Int64(a), Value::Int64(b)) => int_op(*a, *b)
                .map(Value::Int64)
                .ok_or_else(|| PlInterpError::IntegerOverflow(format!("{l:?} op {r:?}"))),
            (Value::Float64(a), Value::Float64(b)) => Ok(Value::Float64(float_op(*a, *b))),
            (Value::Int64(a), Value::Float64(b)) => Ok(Value::Float64(float_op(*a as f64, *b))),
            (Value::Float64(a), Value::Int64(b)) => Ok(Value::Float64(float_op(*a, *b as f64))),
            (Value::Bool(a), Value::Bool(b)) => {
                let ai: i64 = if *a {
                    1
                } else {
                    0
                };
                let bi: i64 = if *b {
                    1
                } else {
                    0
                };
                int_op(ai, bi)
                    .map(Value::Int64)
                    .ok_or_else(|| PlInterpError::IntegerOverflow("bool overflow".into()))
            }
            _ => Err(PlInterpError::TypeError(format!(
                "cannot arith op {:?} and {:?}",
                l.column_type(),
                r.column_type()
            ))),
        }
    }

    /// 值比较（PG 语义）
    fn compare_values(l: &Value, r: &Value) -> Option<std::cmp::Ordering> {
        match (l, r) {
            (Value::Null, _) | (_, Value::Null) => None,
            (Value::Int64(a), Value::Int64(b)) => Some(a.cmp(b)),
            (Value::Float64(a), Value::Float64(b)) => a.partial_cmp(b),
            (Value::Int64(a), Value::Float64(b)) => (*a as f64).partial_cmp(b),
            (Value::Float64(a), Value::Int64(b)) => a.partial_cmp(&(*b as f64)),
            (Value::Text(a), Value::Text(b)) => Some(a.cmp(b)),
            (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
            (Value::Int64(a), Value::Bool(b)) => Some((*a != 0).cmp(b)),
            (Value::Bool(a), Value::Int64(b)) => Some(a.cmp(&(*b != 0))),
            (Value::Decimal(a, _), Value::Decimal(b, _)) => a.partial_cmp(b),
            _ => None,
        }
    }

    /// 扩展 Ordering 辅助
    trait OrderingExt {
        fn is_lt(&self) -> bool;
        fn is_le(&self) -> bool;
        fn is_gt(&self) -> bool;
        fn is_ge(&self) -> bool;
    }

    impl OrderingExt for std::cmp::Ordering {
        fn is_lt(&self) -> bool {
            matches!(self, std::cmp::Ordering::Less)
        }
        fn is_le(&self) -> bool {
            !matches!(self, std::cmp::Ordering::Greater)
        }
        fn is_gt(&self) -> bool {
            matches!(self, std::cmp::Ordering::Greater)
        }
        fn is_ge(&self) -> bool {
            !matches!(self, std::cmp::Ordering::Less)
        }
    }

    /// 内置函数求值
    fn eval_builtin(name: &str, args: &[Value]) -> Result<Value, PlInterpError> {
        match name {
            "abs" => {
                if args.len() != 1 {
                    return Err(PlInterpError::TypeError("abs() expects 1 argument".into()));
                }
                match &args[0] {
                    Value::Null => Ok(Value::Null),
                    Value::Int64(n) => Ok(Value::Int64(n.wrapping_abs())),
                    Value::Float64(f) => Ok(Value::Float64(f.abs())),
                    _ => Err(PlInterpError::TypeError(format!(
                        "abs() expects numeric, got {:?}",
                        args[0].column_type()
                    ))),
                }
            }
            "length" | "char_length" | "character_length" => {
                if args.len() != 1 {
                    return Err(PlInterpError::TypeError(
                        "length() expects 1 argument".into(),
                    ));
                }
                match &args[0] {
                    Value::Null => Ok(Value::Null),
                    Value::Text(s) => Ok(Value::Int64(s.chars().count() as i64)),
                    _ => Err(PlInterpError::TypeError(format!(
                        "length() expects text, got {:?}",
                        args[0].column_type()
                    ))),
                }
            }
            "lower" => match &args[0] {
                Value::Null => Ok(Value::Null),
                Value::Text(s) => Ok(Value::Text(s.to_lowercase())),
                _ => Err(PlInterpError::TypeError(format!(
                    "lower() expects text, got {:?}",
                    args[0].column_type()
                ))),
            },
            "upper" => match &args[0] {
                Value::Null => Ok(Value::Null),
                Value::Text(s) => Ok(Value::Text(s.to_uppercase())),
                _ => Err(PlInterpError::TypeError(format!(
                    "upper() expects text, got {:?}",
                    args[0].column_type()
                ))),
            },
            "coalesce" => {
                for v in args {
                    if !matches!(v, Value::Null) {
                        return Ok(v.clone());
                    }
                }
                Ok(Value::Null)
            }
            "greatest" => {
                if args.is_empty() {
                    return Ok(Value::Null);
                }
                let mut max = &args[0];
                for v in &args[1..] {
                    if matches!(v, Value::Null) {
                        continue;
                    }
                    if matches!(max, Value::Null)
                        || compare_values(max, v).is_some_and(|o| o.is_lt())
                    {
                        max = v;
                    }
                }
                Ok(max.clone())
            }
            "least" => {
                if args.is_empty() {
                    return Ok(Value::Null);
                }
                let mut min = &args[0];
                for v in &args[1..] {
                    if matches!(v, Value::Null) {
                        continue;
                    }
                    if matches!(min, Value::Null)
                        || compare_values(min, v).is_some_and(|o| o.is_gt())
                    {
                        min = v;
                    }
                }
                Ok(min.clone())
            }
            "nullif" => {
                if args.len() != 2 {
                    return Err(PlInterpError::TypeError(
                        "nullif() expects 2 arguments".into(),
                    ));
                }
                if values_equal(&args[0], &args[1]) {
                    Ok(Value::Null)
                } else {
                    Ok(args[0].clone())
                }
            }
            "round" => {
                if args.is_empty() {
                    return Err(PlInterpError::TypeError("round() expects 1+ args".into()));
                }
                match &args[0] {
                    Value::Null => Ok(Value::Null),
                    Value::Int64(n) => Ok(Value::Int64(*n)),
                    Value::Float64(f) => Ok(Value::Float64(f.round())),
                    _ => Err(PlInterpError::TypeError(format!(
                        "round() expects numeric, got {:?}",
                        args[0].column_type()
                    ))),
                }
            }
            "ceil" | "ceiling" => match &args[0] {
                Value::Null => Ok(Value::Null),
                Value::Int64(n) => Ok(Value::Int64(*n)),
                Value::Float64(f) => Ok(Value::Float64(f.ceil())),
                _ => Err(PlInterpError::TypeError("ceil() expects numeric".into())),
            },
            "floor" => match &args[0] {
                Value::Null => Ok(Value::Null),
                Value::Int64(n) => Ok(Value::Int64(*n)),
                Value::Float64(f) => Ok(Value::Float64(f.floor())),
                _ => Err(PlInterpError::TypeError("floor() expects numeric".into())),
            },
            "sqrt" => match &args[0] {
                Value::Null => Ok(Value::Null),
                Value::Int64(n) => Ok(Value::Float64((*n as f64).sqrt())),
                Value::Float64(f) => Ok(Value::Float64(f.sqrt())),
                _ => Err(PlInterpError::TypeError("sqrt() expects numeric".into())),
            },
            "power" | "pow" => {
                if args.len() != 2 {
                    return Err(PlInterpError::TypeError("pow() expects 2 args".into()));
                }
                let base = match &args[0] {
                    Value::Null => return Ok(Value::Null),
                    Value::Int64(n) => *n as f64,
                    Value::Float64(f) => *f,
                    _ => {
                        return Err(PlInterpError::TypeError(
                            "pow() expects numeric base".into(),
                        ))
                    }
                };
                let exp = match &args[1] {
                    Value::Null => return Ok(Value::Null),
                    Value::Int64(n) => *n as f64,
                    Value::Float64(f) => *f,
                    _ => {
                        return Err(PlInterpError::TypeError(
                            "pow() expects numeric exponent".into(),
                        ))
                    }
                };
                Ok(Value::Float64(base.powf(exp)))
            }
            "mod" => {
                if args.len() != 2 {
                    return Err(PlInterpError::TypeError("mod() expects 2 args".into()));
                }
                match (&args[0], &args[1]) {
                    (Value::Int64(a), Value::Int64(b)) => {
                        if *b == 0 {
                            Err(PlInterpError::DivisionByZero)
                        } else {
                            Ok(Value::Int64(a % b))
                        }
                    }
                    _ => Err(PlInterpError::TypeError("mod() expects integers".into())),
                }
            }
            "trim" | "btrim" => match &args[0] {
                Value::Null => Ok(Value::Null),
                Value::Text(s) => Ok(Value::Text(s.trim().to_string())),
                _ => Err(PlInterpError::TypeError("trim() expects text".into())),
            },
            "concat" => {
                let mut s = String::new();
                for v in args {
                    if !matches!(v, Value::Null) {
                        s.push_str(&value_to_display_string(v));
                    }
                }
                Ok(Value::Text(s))
            }
            "substr" | "substring" => {
                if args.len() < 2 {
                    return Err(PlInterpError::TypeError("substr() expects 2-3 args".into()));
                }
                match &args[0] {
                    Value::Null => Ok(Value::Null),
                    Value::Text(s) => {
                        let start = match &args[1] {
                            Value::Int64(n) => *n as usize,
                            _ => {
                                return Err(PlInterpError::TypeError(
                                    "substr() start must be integer".into(),
                                ))
                            }
                        };
                        let chars: Vec<char> = s.chars().collect();
                        let start_idx = if start == 0 {
                            0
                        } else {
                            start - 1
                        };
                        if args.len() >= 3 {
                            let len = match &args[2] {
                                Value::Int64(n) => *n as usize,
                                _ => {
                                    return Err(PlInterpError::TypeError(
                                        "substr() length must be integer".into(),
                                    ))
                                }
                            };
                            let end_idx = (start_idx + len).min(chars.len());
                            if start_idx >= chars.len() {
                                Ok(Value::Text(String::new()))
                            } else {
                                Ok(Value::Text(chars[start_idx..end_idx].iter().collect()))
                            }
                        } else if start_idx >= chars.len() {
                            Ok(Value::Text(String::new()))
                        } else {
                            Ok(Value::Text(chars[start_idx..].iter().collect()))
                        }
                    }
                    _ => Err(PlInterpError::TypeError("substr() expects text".into())),
                }
            }
            "now" | "current_timestamp" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let micros = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_micros() as i64)
                    .unwrap_or(0);
                Ok(Value::Timestamp(micros))
            }
            "__like" => {
                // LIKE 简化实现：仅 % 和 _ 通配符
                if args.len() != 2 {
                    return Err(PlInterpError::TypeError("LIKE expects 2 args".into()));
                }
                match (&args[0], &args[1]) {
                    (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                    (Value::Text(s), Value::Text(p)) => Ok(Value::Bool(like_match(s, p))),
                    _ => Err(PlInterpError::TypeError("LIKE expects text".into())),
                }
            }
            "__not_like" => {
                if args.len() != 2 {
                    return Err(PlInterpError::TypeError("NOT LIKE expects 2 args".into()));
                }
                match (&args[0], &args[1]) {
                    (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                    (Value::Text(s), Value::Text(p)) => Ok(Value::Bool(!like_match(s, p))),
                    _ => Err(PlInterpError::TypeError("NOT LIKE expects text".into())),
                }
            }
            _ => Err(PlInterpError::Unsupported(format!(
                "unknown function: {name}"
            ))),
        }
    }

    /// LIKE 模式匹配（% 任意字符序列，_ 单个字符）
    fn like_match(s: &str, pattern: &str) -> bool {
        let s_chars: Vec<char> = s.chars().collect();
        let p_chars: Vec<char> = pattern.chars().collect();
        like_match_recursive(&s_chars, 0, &p_chars, 0)
    }

    fn like_match_recursive(s: &[char], si: usize, p: &[char], pi: usize) -> bool {
        if pi == p.len() {
            return si == s.len();
        }
        match p[pi] {
            '%' => {
                // % 匹配 0 个或多个字符
                for i in si..=s.len() {
                    if like_match_recursive(s, i, p, pi + 1) {
                        return true;
                    }
                }
                false
            }
            '_' => {
                // _ 匹配 1 个字符
                if si < s.len() {
                    like_match_recursive(s, si + 1, p, pi + 1)
                } else {
                    false
                }
            }
            c => {
                if si < s.len() && s[si] == c {
                    like_match_recursive(s, si + 1, p, pi + 1)
                } else {
                    false
                }
            }
        }
    }

    /// 公开入口：求值 PL/pgSQL 表达式字符串
    pub fn eval_pl_expr(
        expr: &str,
        env: &Environment,
        resolver: &mut dyn PlFuncResolver,
    ) -> Result<Value, PlInterpError> {
        let mut parser = Parser::new(expr)?;
        let ast = parser.parse()?;
        eval_ast(&ast, env, resolver)
    }
}

// 重导出公开入口
pub use pl_expr::eval_pl_expr;

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_function(name: &str, params: &[(&str, &str)], body: &str) -> PlFunction {
        PlFunction {
            name: name.into(),
            parameters: params
                .iter()
                .map(|(n, t)| FunctionParameter {
                    mode: Some(FunctionArgMode::In),
                    name: Some((*n).into()),
                    data_type: (*t).into(),
                    default_expr: None,
                })
                .collect(),
            return_type: "integer".into(),
            body: body.into(),
            strict: false,
        }
    }

    fn make_registry(funcs: Vec<PlFunction>) -> FunctionRegistry {
        let mut reg = FunctionRegistry::new();
        for f in funcs {
            reg.register(f);
        }
        reg
    }

    /// 在大栈线程中运行闭包，用于深度递归 Stress 测试。
    ///
    /// PL/pgSQL 每层递归对应 ~12 层 Rust 调用栈（call → exec_block_inner →
    /// exec_stmts → exec_stmt → eval_expr → eval_pl_expr → eval_ast →
    /// resolve_function → call ...），debug 构建下每层 Rust 帧约 8-12KB，
    /// 1000 层 PL/pgSQL 递归 ≈ 12000 层 Rust 帧 ≈ 100MB。
    ///
    /// 默认线程栈 1MB（Windows）仅够 ~100 层 PL/pgSQL 递归；此处使用 256MB
    /// 大栈线程以确保 Stress 测试（深度 1000+）不触发 C 栈溢出。
    fn run_with_large_stack<F, R>(f: F) -> R
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let handle = std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn(f)
            .expect("failed to spawn large-stack thread");
        handle.join().expect("large-stack thread panicked")
    }

    #[test]
    fn test_simple_return() {
        let reg = make_registry(vec![make_function("ret", &[], "BEGIN RETURN 42; END")]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("ret", &[]).unwrap();
        assert_eq!(result, Some(Value::Int64(42)));
    }

    #[test]
    fn test_variable_assignment() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN x := 10; RETURN x; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Int64(10)));
    }

    #[test]
    fn test_arithmetic() {
        let reg = make_registry(vec![make_function("f", &[], "BEGIN RETURN 2 + 3 * 4; END")]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Int64(14)));
    }

    #[test]
    fn test_parameters() {
        let reg = make_registry(vec![make_function(
            "add",
            &[("a", "integer"), ("b", "integer")],
            "BEGIN RETURN a + b; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp
            .call("add", &[Value::Int64(3), Value::Int64(4)])
            .unwrap();
        assert_eq!(result, Some(Value::Int64(7)));
    }

    #[test]
    fn test_if_branch() {
        let reg = make_registry(vec![make_function(
            "f",
            &[("n", "integer")],
            "BEGIN IF n > 0 THEN RETURN 1; ELSE RETURN -1; END IF; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        assert_eq!(
            interp.call("f", &[Value::Int64(5)]).unwrap(),
            Some(Value::Int64(1))
        );
        assert_eq!(
            interp.call("f", &[Value::Int64(-5)]).unwrap(),
            Some(Value::Int64(-1))
        );
    }

    #[test]
    fn test_while_loop() {
        let reg = make_registry(vec![make_function(
            "sum",
            &[("n", "integer")],
            "BEGIN s := 0; i := 1; WHILE i <= n LOOP s := s + i; i := i + 1; END LOOP; RETURN s; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("sum", &[Value::Int64(100)]).unwrap();
        assert_eq!(result, Some(Value::Int64(5050)));
    }

    #[test]
    fn test_for_loop() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN s := 0; FOR i IN 1..10 LOOP s := s + i; END LOOP; RETURN s; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Int64(55)));
    }

    #[test]
    fn test_for_loop_reverse() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN s := 0; FOR i IN REVERSE 10..1 LOOP s := s + i; END LOOP; RETURN s; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Int64(55)));
    }

    #[test]
    fn test_for_loop_step() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN s := 0; FOR i IN 1..10 BY 2 LOOP s := s + i; END LOOP; RETURN s; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        // 1 + 3 + 5 + 7 + 9 = 25
        assert_eq!(result, Some(Value::Int64(25)));
    }

    #[test]
    fn test_exit_with_when() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN i := 0; LOOP i := i + 1; EXIT WHEN i >= 5; END LOOP; RETURN i; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Int64(5)));
    }

    #[test]
    fn test_continue() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN s := 0; FOR i IN 1..5 LOOP CONTINUE WHEN i = 3; s := s + i; END LOOP; RETURN s; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        // 1 + 2 + 4 + 5 = 12
        assert_eq!(result, Some(Value::Int64(12)));
    }

    #[test]
    fn test_case_simple() {
        let reg = make_registry(vec![make_function(
            "f",
            &[("n", "integer")],
            "BEGIN RETURN CASE WHEN n > 0 THEN 1 WHEN n < 0 THEN -1 ELSE 0 END; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        assert_eq!(
            interp.call("f", &[Value::Int64(5)]).unwrap(),
            Some(Value::Int64(1))
        );
        assert_eq!(
            interp.call("f", &[Value::Int64(-5)]).unwrap(),
            Some(Value::Int64(-1))
        );
        assert_eq!(
            interp.call("f", &[Value::Int64(0)]).unwrap(),
            Some(Value::Int64(0))
        );
    }

    #[test]
    fn test_case_with_selector() {
        let reg = make_registry(vec![make_function(
            "f",
            &[("n", "integer")],
            "BEGIN RETURN CASE n WHEN 1 THEN 10 WHEN 2 THEN 20 ELSE 99 END; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        assert_eq!(
            interp.call("f", &[Value::Int64(1)]).unwrap(),
            Some(Value::Int64(10))
        );
        assert_eq!(
            interp.call("f", &[Value::Int64(2)]).unwrap(),
            Some(Value::Int64(20))
        );
        assert_eq!(
            interp.call("f", &[Value::Int64(99)]).unwrap(),
            Some(Value::Int64(99))
        );
    }

    #[test]
    fn test_string_concat() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN RETURN 'hello' || ' ' || 'world'; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Text("hello world".into())));
    }

    #[test]
    fn test_is_null() {
        let reg = make_registry(vec![make_function(
            "f",
            &[("x", "integer")],
            "BEGIN IF x IS NULL THEN RETURN 1; ELSE RETURN 0; END IF; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        assert_eq!(
            interp.call("f", &[Value::Null]).unwrap(),
            Some(Value::Int64(1))
        );
        assert_eq!(
            interp.call("f", &[Value::Int64(42)]).unwrap(),
            Some(Value::Int64(0))
        );
    }

    #[test]
    fn test_in_list() {
        let reg = make_registry(vec![make_function(
            "f",
            &[("x", "integer")],
            "BEGIN IF x IN (1, 2, 3) THEN RETURN 1; ELSE RETURN 0; END IF; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        assert_eq!(
            interp.call("f", &[Value::Int64(2)]).unwrap(),
            Some(Value::Int64(1))
        );
        assert_eq!(
            interp.call("f", &[Value::Int64(5)]).unwrap(),
            Some(Value::Int64(0))
        );
    }

    #[test]
    fn test_between() {
        let reg = make_registry(vec![make_function(
            "f",
            &[("x", "integer")],
            "BEGIN IF x BETWEEN 1 AND 10 THEN RETURN 1; ELSE RETURN 0; END IF; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        assert_eq!(
            interp.call("f", &[Value::Int64(5)]).unwrap(),
            Some(Value::Int64(1))
        );
        assert_eq!(
            interp.call("f", &[Value::Int64(50)]).unwrap(),
            Some(Value::Int64(0))
        );
    }

    #[test]
    fn test_function_call_builtin() {
        let reg = make_registry(vec![make_function("f", &[], "BEGIN RETURN abs(-10); END")]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Int64(10)));
    }

    #[test]
    fn test_nested_blocks() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN x := 1; BEGIN y := 2; x := x + y; END; RETURN x; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Int64(3)));
    }

    #[test]
    fn test_constant() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "DECLARE pi CONSTANT numeric := 3; BEGIN RETURN pi; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Decimal(3, 0)));
    }

    #[test]
    fn test_constant_reassign_error() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "DECLARE pi CONSTANT integer := 3; BEGIN pi := 4; RETURN pi; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]);
        assert!(matches!(result, Err(PlInterpError::ConstantReassign(_))));
    }

    #[test]
    fn test_raise_exception() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN RAISE EXCEPTION 'custom error'; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]);
        assert!(matches!(result, Err(PlInterpError::UncaughtException(_))));
    }

    #[test]
    fn test_exception_caught() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN RAISE EXCEPTION 'oops'; EXCEPTION WHEN OTHERS THEN RETURN 99; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Int64(99)));
    }

    #[test]
    fn test_exception_sqlstate() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN x := 1 / 0; EXCEPTION WHEN division_by_zero THEN RETURN 1; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Int64(1)));
    }

    #[test]
    fn test_recursion_factorial() {
        let body = "BEGIN IF n <= 1 THEN RETURN 1; ELSE RETURN n * fact(n - 1); END IF; END";
        let mut func = make_function("fact", &[("n", "integer")], body);
        func.return_type = "integer".into();
        let reg = make_registry(vec![func]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("fact", &[Value::Int64(10)]).unwrap();
        assert_eq!(result, Some(Value::Int64(3628800)));
    }

    #[test]
    fn test_recursion_depth_100() {
        // 递归求和 sum_to(n) = n + sum_to(n-1), sum_to(0) = 0
        // 在大栈线程中运行，避免 C 栈溢出（详见 run_with_large_stack 注释）
        let result = run_with_large_stack(|| {
            let body = "BEGIN IF n = 0 THEN RETURN 0; ELSE RETURN n + sum_to(n - 1); END IF; END";
            let func = make_function("sum_to", &[("n", "integer")], body);
            let reg = make_registry(vec![func]);
            let mut interp = PlPgSqlInterpreter::new(&reg);
            interp.call("sum_to", &[Value::Int64(100)]).unwrap()
        });
        assert_eq!(result, Some(Value::Int64(5050)));
    }

    #[test]
    fn test_recursion_depth_1000() {
        // Stress：递归调用深度 1000 不栈溢出
        // 在大栈线程中运行，避免 C 栈溢出（详见 run_with_large_stack 注释）
        let result = run_with_large_stack(|| {
            let body = "BEGIN IF n = 0 THEN RETURN 0; ELSE RETURN 1 + count_n(n - 1); END IF; END";
            let func = make_function("count_n", &[("n", "integer")], body);
            let reg = make_registry(vec![func]);
            let mut interp = PlPgSqlInterpreter::new(&reg).with_max_stack_depth(2048);
            interp.call("count_n", &[Value::Int64(1000)]).unwrap()
        });
        assert_eq!(result, Some(Value::Int64(1000)));
    }

    #[test]
    fn test_stack_overflow_protection() {
        // 无限递归应被栈深度保护拦截
        // 在大栈线程中运行，避免 C 栈溢出（详见 run_with_large_stack 注释）
        let result = run_with_large_stack(|| {
            let body = "BEGIN RETURN recurse(n + 1); END";
            let func = make_function("recurse", &[("n", "integer")], body);
            let reg = make_registry(vec![func]);
            let mut interp = PlPgSqlInterpreter::new(&reg).with_max_stack_depth(100);
            interp.call("recurse", &[Value::Int64(0)])
        });
        assert!(matches!(result, Err(PlInterpError::StackOverflow { .. })));
    }

    #[test]
    fn test_loop_1million() {
        // Stress：循环 1,000,000 次性能合理（debug 构建 < 20s，含系统负载波动容错）
        let body = "BEGIN s := 0; i := 0; WHILE i < 1000000 LOOP s := s + 1; i := i + 1; END LOOP; RETURN s; END";
        let func = make_function("f", &[], body);
        let reg = make_registry(vec![func]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let start = std::time::Instant::now();
        let result = interp.call("f", &[]).unwrap();
        let elapsed = start.elapsed();
        assert_eq!(result, Some(Value::Int64(1_000_000)));
        assert!(
            elapsed.as_secs() < 20,
            "1M iterations took {:?}, expected < 20s",
            elapsed
        );
    }

    #[test]
    #[ignore = "Stress 测试：10M 次循环，debug 构建约 70s；建议在 release 构建下运行 `cargo test --release -- --ignored`"]
    fn test_loop_10million() {
        // Stress：循环 10,000,000 次性能合理（release < 60s，debug < 120s）
        let body = "BEGIN s := 0; i := 0; WHILE i < 10000000 LOOP s := s + 1; i := i + 1; END LOOP; RETURN s; END";
        let func = make_function("f", &[], body);
        let reg = make_registry(vec![func]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let start = std::time::Instant::now();
        let result = interp.call("f", &[]).unwrap();
        let elapsed = start.elapsed();
        assert_eq!(result, Some(Value::Int64(10_000_000)));
        // release 构建约 5-10s；debug 构建约 70-80s
        assert!(
            elapsed.as_secs() < 120,
            "10M iterations took {:?}, expected < 120s",
            elapsed
        );
    }

    #[test]
    fn test_complex_business_logic() {
        // 复杂业务逻辑：模拟订单折扣计算
        // - amount > 1000: 20% off
        // - amount > 500: 10% off
        // - amount > 100: 5% off
        // - else: no discount
        let body = "BEGIN
            IF amount > 1000 THEN
                discount := amount * 20 / 100;
            ELSIF amount > 500 THEN
                discount := amount * 10 / 100;
            ELSIF amount > 100 THEN
                discount := amount * 5 / 100;
            ELSE
                discount := 0;
            END IF;
            final_amount := amount - discount;
            RETURN final_amount;
        END";
        let func = make_function("calc_discount", &[("amount", "integer")], body);
        let reg = make_registry(vec![func]);
        let mut interp = PlPgSqlInterpreter::new(&reg);

        // 2000 → 1600 (20% off)
        assert_eq!(
            interp.call("calc_discount", &[Value::Int64(2000)]).unwrap(),
            Some(Value::Int64(1600))
        );
        // 600 → 540 (10% off)
        assert_eq!(
            interp.call("calc_discount", &[Value::Int64(600)]).unwrap(),
            Some(Value::Int64(540))
        );
        // 200 → 190 (5% off)
        assert_eq!(
            interp.call("calc_discount", &[Value::Int64(200)]).unwrap(),
            Some(Value::Int64(190))
        );
        // 50 → 50 (no discount)
        assert_eq!(
            interp.call("calc_discount", &[Value::Int64(50)]).unwrap(),
            Some(Value::Int64(50))
        );
    }

    #[test]
    fn test_complex_fibonacci() {
        // 复杂业务逻辑：斐波那契数列（迭代版）
        let body = "BEGIN
            IF n <= 1 THEN RETURN n; END IF;
            a := 0;
            b := 1;
            i := 2;
            WHILE i <= n LOOP
                c := a + b;
                a := b;
                b := c;
                i := i + 1;
            END LOOP;
            RETURN b;
        END";
        let func = make_function("fib", &[("n", "integer")], body);
        let reg = make_registry(vec![func]);
        let mut interp = PlPgSqlInterpreter::new(&reg);

        assert_eq!(
            interp.call("fib", &[Value::Int64(0)]).unwrap(),
            Some(Value::Int64(0))
        );
        assert_eq!(
            interp.call("fib", &[Value::Int64(1)]).unwrap(),
            Some(Value::Int64(1))
        );
        assert_eq!(
            interp.call("fib", &[Value::Int64(10)]).unwrap(),
            Some(Value::Int64(55))
        );
        assert_eq!(
            interp.call("fib", &[Value::Int64(20)]).unwrap(),
            Some(Value::Int64(6765))
        );
    }

    #[test]
    fn test_greatest_least() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN RETURN greatest(1, 2, 3); END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Int64(3)));
    }

    #[test]
    fn test_coalesce() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN RETURN coalesce(NULL, NULL, 42); END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Int64(42)));
    }

    #[test]
    fn test_nullif() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN RETURN nullif(5, 5); END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Null));
    }

    #[test]
    fn test_like_pattern() {
        let reg = make_registry(vec![make_function(
            "f",
            &[("s", "text")],
            "BEGIN IF s LIKE 'hello%' THEN RETURN 1; ELSE RETURN 0; END IF; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        assert_eq!(
            interp
                .call("f", &[Value::Text("hello world".into())])
                .unwrap(),
            Some(Value::Int64(1))
        );
        assert_eq!(
            interp.call("f", &[Value::Text("world".into())]).unwrap(),
            Some(Value::Int64(0))
        );
    }

    #[test]
    fn test_strict_function() {
        let mut func = make_function("f", &[("x", "integer")], "BEGIN RETURN x + 1; END");
        func.strict = true;
        let reg = make_registry(vec![func]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        // 严格函数：任一参数为 NULL 直接返回 NULL
        let result = interp.call("f", &[Value::Null]).unwrap();
        assert_eq!(result, Some(Value::Null));
    }

    #[test]
    fn test_function_not_found() {
        let reg = make_registry(vec![]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("nonexistent", &[]);
        assert!(matches!(result, Err(PlInterpError::FunctionNotFound(_))));
    }

    #[test]
    fn test_arg_count_mismatch() {
        let reg = make_registry(vec![make_function(
            "f",
            &[("a", "integer"), ("b", "integer")],
            "BEGIN RETURN a + b; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[Value::Int64(1)]);
        assert!(matches!(
            result,
            Err(PlInterpError::ArgCountMismatch { .. })
        ));
    }

    #[test]
    fn test_nested_function_calls() {
        let reg = make_registry(vec![
            make_function("double", &[("x", "integer")], "BEGIN RETURN x * 2; END"),
            make_function(
                "quad",
                &[("x", "integer")],
                "BEGIN RETURN double(double(x)); END",
            ),
        ]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("quad", &[Value::Int64(5)]).unwrap();
        assert_eq!(result, Some(Value::Int64(20)));
    }

    #[test]
    fn test_raise_notice_no_interrupt() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN RAISE NOTICE 'hello'; RETURN 42; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]).unwrap();
        assert_eq!(result, Some(Value::Int64(42)));
    }

    #[test]
    fn test_raise_with_format() {
        let reg = make_registry(vec![make_function(
            "f",
            &[("n", "integer")],
            "BEGIN RAISE EXCEPTION 'value is %', n; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[Value::Int64(42)]);
        if let Err(PlInterpError::UncaughtException(msg)) = result {
            assert!(msg.contains("42"));
        } else {
            panic!("expected UncaughtException");
        }
    }

    #[test]
    fn test_division_by_zero() {
        let reg = make_registry(vec![make_function("f", &[], "BEGIN RETURN 1 / 0; END")]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]);
        assert!(matches!(result, Err(PlInterpError::DivisionByZero)));
    }

    #[test]
    fn test_integer_overflow() {
        let reg = make_registry(vec![make_function(
            "f",
            &[],
            "BEGIN RETURN 9223372036854775807 + 1; END",
        )]);
        let mut interp = PlPgSqlInterpreter::new(&reg);
        let result = interp.call("f", &[]);
        assert!(matches!(result, Err(PlInterpError::IntegerOverflow(_))));
    }
}
