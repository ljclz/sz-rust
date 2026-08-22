//! DataScopeExt — 查询构建器扩展 trait
//!
//! 为 sz-orm 查询构建器提供 `data_scope()` 链式方法，
//! 自动注入数据范围 WHERE 条件。

use crate::data_scope::context::DataScopeContext;
use crate::data_scope::error::DataScopeError;
use crate::data_scope::evaluator::DataScopeEvaluator;
use crate::data_scope::rule::DataScopeRule;
use crate::repository::WhereCondition;

/// 数据范围扩展 trait
///
/// 为查询构建器实现此 trait 后，可链式调用 `.data_scope_async(ctx, rule, evaluator)`
/// 自动注入数据范围 WHERE 条件。
#[async_trait::async_trait]
pub trait DataScopeExt: Sized {
    /// 同步设置数据范围（需预先评估好的条件）
    fn with_data_scope_conditions(self, conditions: &[WhereCondition]) -> Self;

    /// 异步评估并注入数据范围
    ///
    /// 调用 `evaluator.evaluate(ctx, rule)` 获取条件列表，
    /// 逐个追加到查询构建器。
    async fn data_scope_async(
        self,
        ctx: &DataScopeContext,
        rule: &DataScopeRule,
        evaluator: &dyn DataScopeEvaluator,
    ) -> Result<Self, DataScopeError> {
        let conditions = evaluator.evaluate(ctx, rule).await?;
        Ok(self.with_data_scope_conditions(&conditions))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_scope::cache::{DeptTreeCache, DeptTreeProvider};
    use crate::data_scope::custom::CustomGeneratorRegistry;
    use crate::data_scope::evaluator::DefaultDataScopeEvaluator;
    use crate::data_scope::metrics::DataScopeMetrics;
    use crate::data_scope::rule::DataScopeMode;
    use crate::repository::WhereCondition;
    use async_trait::async_trait;
    use std::sync::Arc;

    struct MockQueryBuilder {
        conditions: Vec<WhereCondition>,
    }

    #[async_trait]
    impl DataScopeExt for MockQueryBuilder {
        fn with_data_scope_conditions(mut self, conditions: &[WhereCondition]) -> Self {
            self.conditions = conditions.to_vec();
            self
        }
    }

    struct MockDeptProvider;

    #[async_trait]
    impl DeptTreeProvider for MockDeptProvider {
        async fn sub_depts(&self, _dept_id: i64) -> Result<Vec<i64>, DataScopeError> {
            Ok(vec![])
        }
    }

    fn make_evaluator() -> DefaultDataScopeEvaluator {
        DefaultDataScopeEvaluator::new(
            Arc::new(DeptTreeCache::new(
                Arc::new(MockDeptProvider),
                std::time::Duration::from_secs(300),
            )),
            Arc::new(CustomGeneratorRegistry::new()),
            Arc::new(DataScopeMetrics::new()),
        )
    }

    #[tokio::test]
    async fn test_data_scope_async_injects_conditions() {
        let evaluator = make_evaluator();
        let ctx = DataScopeContext::new(1, 5, false);
        let rule = DataScopeRule::new("order", DataScopeMode::Dept).with_dept_field("dept_id");
        let builder = MockQueryBuilder { conditions: vec![] };
        let result = builder
            .data_scope_async(&ctx, &rule, &evaluator)
            .await
            .unwrap();
        assert_eq!(result.conditions.len(), 1);
    }

    #[tokio::test]
    async fn test_data_scope_async_super_bypass_empty() {
        let evaluator = make_evaluator();
        let ctx = DataScopeContext::new(1, 5, true);
        let rule = DataScopeRule::new("order", DataScopeMode::Dept).with_dept_field("dept_id");
        let builder = MockQueryBuilder { conditions: vec![] };
        let result = builder
            .data_scope_async(&ctx, &rule, &evaluator)
            .await
            .unwrap();
        assert!(result.conditions.is_empty());
    }

    #[tokio::test]
    async fn test_data_scope_async_dept_and_sub() {
        let evaluator = make_evaluator();
        let ctx = DataScopeContext::new(1, 5, false);
        let rule =
            DataScopeRule::new("order", DataScopeMode::DeptAndSub).with_dept_field("dept_id");
        let builder = MockQueryBuilder { conditions: vec![] };
        let result = builder
            .data_scope_async(&ctx, &rule, &evaluator)
            .await
            .unwrap();
        assert_eq!(result.conditions.len(), 1);
    }
}
