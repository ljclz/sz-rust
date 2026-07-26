//! CHECK 约束运行时校验 — Phase 3.30
//!
//! # 设计
//!
//! - **`CheckConstraintValidator`** — 静态方法集合，校验 INSERT/UPDATE 行不违反 CHECK 约束
//! - **校验语义**（与 PG 一致）：
//!   - 对行求值 CHECK 表达式
//!   - 结果为 `true` 或 `NULL` → 通过
//!   - 结果为 `false` → 报 `CheckViolation`
//!   - 求值错误 → 报 `CheckViolation`（包装原错误）
//! - **NULL 语义**：PG 中 CHECK 表达式求值为 NULL 视为通过（与 WHERE 不同）
//!
//! 对应 `SzRSQL实施进度.md` Phase 3.30。

use crate::ast::Expr;
use crate::executor::{ExecRowContext, ExecutionError, Row};
use crate::expr::{EvalError, ExprEvaluator};
use crate::plan::{CheckConstraint, TableSchema};
use szrsql_types::value::Value;

// =====================================================================
//  CheckConstraintValidator
// =====================================================================

/// CHECK 约束校验器 — Phase 3.30
///
/// 所有方法均为静态方法，接收 schema + 行数据进行校验。
pub struct CheckConstraintValidator;

impl CheckConstraintValidator {
    /// 校验单行不违反所有 CHECK 约束 — Phase 3.30
    ///
    /// 对每个 CHECK 约束：
    /// 1. 在行上下文中求值 `expr`
    /// 2. 结果为 `true` 或 `NULL` → 通过
    /// 3. 结果为 `false` → 报 `CheckViolation`
    /// 4. 求值错误 → 报 `CheckViolation`（包装原错误）
    ///
    /// # 参数
    /// - `schema` — 表 Schema（用于列名解析）
    /// - `row` — 待校验的行
    /// - `checks` — CHECK 约束列表
    pub fn validate_row(
        schema: &TableSchema,
        row: &Row,
        checks: &[CheckConstraint],
    ) -> Result<(), ExecutionError> {
        for check in checks {
            Self::evaluate_check(schema, row, check)?;
        }
        Ok(())
    }

    /// 校验多行不违反 CHECK 约束 — Phase 3.30
    ///
    /// 对每行调用 `validate_row`。任一行违反即立即返回错误。
    pub fn validate_rows(
        schema: &TableSchema,
        rows: &[Row],
        checks: &[CheckConstraint],
    ) -> Result<(), ExecutionError> {
        for row in rows {
            Self::validate_row(schema, row, checks)?;
        }
        Ok(())
    }

    /// 求值单个 CHECK 约束 — Phase 3.30
    fn evaluate_check(
        schema: &TableSchema,
        row: &Row,
        check: &CheckConstraint,
    ) -> Result<(), ExecutionError> {
        let ctx = ExecRowContext::new_proxy(schema, row);
        let result = ExprEvaluator::eval(&check.expr, &ctx).map_err(|e| {
            Self::violation_error(check, schema, row, format!("evaluation error: {e}"))
        })?;

        match result {
            Value::Bool(true) | Value::Null => Ok(()),
            Value::Bool(false) => Err(Self::violation_error(
                check,
                schema,
                row,
                "false".to_string(),
            )),
            other => Err(Self::violation_error(
                check,
                schema,
                row,
                format!("non-boolean result: {other:?}"),
            )),
        }
    }

    /// 构造 CHECK 违反错误 — Phase 3.30
    fn violation_error(
        check: &CheckConstraint,
        schema: &TableSchema,
        row: &Row,
        reason: String,
    ) -> ExecutionError {
        let name = check.name.as_deref().unwrap_or("<unnamed>");
        ExecutionError::CheckViolation(format!(
            "check constraint \"{name}\" on table \"{}\" violated ({reason}); row = {:?}",
            schema.name.qualified_name(),
            row
        ))
    }
}

/// 评估 CHECK 表达式（用于测试）— Phase 3.30
///
/// 在给定行上下文中求值表达式，返回结果。
/// 主要用于单元测试中验证表达式求值正确性。
pub fn evaluate_check_expr(
    schema: &TableSchema,
    row: &Row,
    expr: &Expr,
) -> Result<Value, EvalError> {
    let ctx = ExecRowContext::new_proxy(schema, row);
    ExprEvaluator::eval(expr, &ctx)
}
