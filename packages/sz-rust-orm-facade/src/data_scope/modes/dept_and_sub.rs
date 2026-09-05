// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! DEPT_AND_SUB 模式 — 本部门及子部门（`WHERE dept_id IN (...)`）

use super::ModeEvaluator;
use crate::data_scope::cache::DeptTreeCache;
use crate::data_scope::context::DataScopeContext;
use crate::data_scope::error::DataScopeError;
use crate::data_scope::rule::DataScopeRule;
use crate::repository::{WhereCondition, WhereOp};
use crate::Value;
use async_trait::async_trait;
use std::sync::Arc;

/// DEPT_AND_SUB 模式评估器
pub struct DeptAndSubMode {
    dept_cache: Arc<DeptTreeCache>,
}

impl DeptAndSubMode {
    pub fn new(dept_cache: Arc<DeptTreeCache>) -> Self {
        Self { dept_cache }
    }
}

#[async_trait]
impl ModeEvaluator for DeptAndSubMode {
    async fn evaluate(
        &self,
        ctx: &DataScopeContext,
        rule: &DataScopeRule,
    ) -> Result<Vec<WhereCondition>, DataScopeError> {
        let field = rule.dept_field.as_deref().ok_or_else(|| {
            DataScopeError::InvalidRule("DEPT_AND_SUB mode requires dept_field".into())
        })?;
        let dept_ids = self.dept_cache.get_with_sub(ctx.dept_id).await?;
        let values: Vec<Value> = dept_ids.into_iter().map(Value::I64).collect();
        Ok(vec![WhereCondition::new(
            field,
            WhereOp::In,
            Value::Array(values),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_scope::cache::{DeptTreeCache, DeptTreeProvider};
    use async_trait::async_trait;

    struct MockProvider;

    #[async_trait]
    impl DeptTreeProvider for MockProvider {
        async fn sub_depts(&self, dept_id: i64) -> Result<Vec<i64>, DataScopeError> {
            match dept_id {
                5 => Ok(vec![6, 7]),
                6 => Ok(vec![8]),
                _ => Ok(vec![]),
            }
        }
    }

    #[tokio::test]
    async fn test_dept_and_sub_mode() {
        let cache = Arc::new(DeptTreeCache::new(
            Arc::new(MockProvider),
            std::time::Duration::from_secs(300),
        ));
        let mode = DeptAndSubMode::new(cache);
        let ctx = DataScopeContext::new(1, 5, false);
        let rule = DataScopeRule::new("order", crate::data_scope::rule::DataScopeMode::DeptAndSub)
            .with_dept_field("dept_id");
        let conditions = mode.evaluate(&ctx, &rule).await.unwrap();
        assert_eq!(conditions.len(), 1);
    }

    #[tokio::test]
    async fn test_dept_and_sub_mode_missing_field() {
        let cache = Arc::new(DeptTreeCache::new(
            Arc::new(MockProvider),
            std::time::Duration::from_secs(300),
        ));
        let mode = DeptAndSubMode::new(cache);
        let ctx = DataScopeContext::new(1, 5, false);
        let rule = DataScopeRule::new("order", crate::data_scope::rule::DataScopeMode::DeptAndSub);
        let err = mode.evaluate(&ctx, &rule).await.unwrap_err();
        assert_eq!(err.error_code(), "DATA_SCOPE_INVALID_RULE");
    }
}
