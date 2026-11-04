// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! DEPT 模式 — 本部门数据（`WHERE dept_id = ?`）

use super::ModeEvaluator;
use crate::data_scope::context::DataScopeContext;
use crate::data_scope::error::DataScopeError;
use crate::data_scope::rule::DataScopeRule;
use crate::repository::{WhereCondition, WhereOp};
use crate::Value;
use async_trait::async_trait;

/// DEPT 模式评估器
pub struct DeptMode;

#[async_trait]
impl ModeEvaluator for DeptMode {
    async fn evaluate(
        &self,
        ctx: &DataScopeContext,
        rule: &DataScopeRule,
    ) -> Result<Vec<WhereCondition>, DataScopeError> {
        let field = rule
            .dept_field
            .as_deref()
            .ok_or_else(|| DataScopeError::InvalidRule("DEPT mode requires dept_field".into()))?;
        Ok(vec![WhereCondition::new(
            field,
            WhereOp::Eq,
            Value::I64(ctx.dept_id),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dept_mode() {
        let mode = DeptMode;
        let ctx = DataScopeContext::new(1, 5, false);
        let rule = DataScopeRule::new("order", crate::data_scope::rule::DataScopeMode::Dept)
            .with_dept_field("dept_id");
        let conditions = mode.evaluate(&ctx, &rule).await.unwrap();
        assert_eq!(conditions.len(), 1);
    }

    #[tokio::test]
    async fn test_dept_mode_missing_field() {
        let mode = DeptMode;
        let ctx = DataScopeContext::new(1, 5, false);
        let rule = DataScopeRule::new("order", crate::data_scope::rule::DataScopeMode::Dept);
        let result = mode.evaluate(&ctx, &rule).await;
        assert!(result.is_err());
    }
}
