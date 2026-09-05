// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! ALL 模式 — 全部数据（不追加任何条件）

use super::ModeEvaluator;
use crate::data_scope::context::DataScopeContext;
use crate::data_scope::error::DataScopeError;
use crate::data_scope::rule::DataScopeRule;
use crate::repository::WhereCondition;
use async_trait::async_trait;

/// ALL 模式评估器
pub struct AllMode;

#[async_trait]
impl ModeEvaluator for AllMode {
    async fn evaluate(
        &self,
        _ctx: &DataScopeContext,
        _rule: &DataScopeRule,
    ) -> Result<Vec<WhereCondition>, DataScopeError> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_all_mode_returns_empty() {
        let mode = AllMode;
        let ctx = DataScopeContext::new(1, 5, false);
        let rule = DataScopeRule::new("order", crate::data_scope::rule::DataScopeMode::All);
        let conditions = mode.evaluate(&ctx, &rule).await.unwrap();
        assert!(conditions.is_empty());
    }
}
