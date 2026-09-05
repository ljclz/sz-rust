// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! SELF 模式 — 仅本人创建的数据（`WHERE creator_id = ?`）

use super::ModeEvaluator;
use crate::data_scope::context::DataScopeContext;
use crate::data_scope::error::DataScopeError;
use crate::data_scope::rule::DataScopeRule;
use crate::repository::{WhereCondition, WhereOp};
use crate::Value;
use async_trait::async_trait;

/// SELF 模式评估器
pub struct SelfMode;

#[async_trait]
impl ModeEvaluator for SelfMode {
    async fn evaluate(
        &self,
        ctx: &DataScopeContext,
        rule: &DataScopeRule,
    ) -> Result<Vec<WhereCondition>, DataScopeError> {
        let field = rule.creator_field.as_deref().ok_or_else(|| {
            DataScopeError::InvalidRule("SELF mode requires creator_field".into())
        })?;
        Ok(vec![WhereCondition::new(
            field,
            WhereOp::Eq,
            Value::I64(ctx.user_id),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_self_mode() {
        let mode = SelfMode;
        let ctx = DataScopeContext::new(10, 5, false);
        let rule = DataScopeRule::new("order", crate::data_scope::rule::DataScopeMode::Self_)
            .with_creator_field("creator_id");
        let conditions = mode.evaluate(&ctx, &rule).await.unwrap();
        assert_eq!(conditions.len(), 1);
    }

    #[tokio::test]
    async fn test_self_mode_missing_field() {
        let mode = SelfMode;
        let ctx = DataScopeContext::new(10, 5, false);
        let rule = DataScopeRule::new("order", crate::data_scope::rule::DataScopeMode::Self_);
        let result = mode.evaluate(&ctx, &rule).await;
        assert!(result.is_err());
    }
}
