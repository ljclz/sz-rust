//! DataScopeEvaluator — 数据范围评估器
//!
//! 根据 DataScopeRule.mode 分发到对应 ModeEvaluator，
//! 超级管理员绕过（记录审计日志），错误上报指标。

use crate::data_scope::cache::DeptTreeCache;
use crate::data_scope::context::DataScopeContext;
use crate::data_scope::custom::CustomGeneratorRegistry;
use crate::data_scope::error::DataScopeError;
use crate::data_scope::metrics::DataScopeMetrics;
use crate::data_scope::modes::all::AllMode;
use crate::data_scope::modes::dept::DeptMode;
use crate::data_scope::modes::self_mode::SelfMode;
use crate::data_scope::modes::ModeEvaluator;
use crate::data_scope::rule::{DataScopeMode, DataScopeRule};
use crate::repository::WhereCondition;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;

/// 数据范围评估器 trait
#[async_trait]
pub trait DataScopeEvaluator: Send + Sync {
    /// 评估数据范围，返回 WHERE 条件列表
    async fn evaluate(
        &self,
        ctx: &DataScopeContext,
        rule: &DataScopeRule,
    ) -> Result<Vec<WhereCondition>, DataScopeError>;
}

/// 默认评估器实现
pub struct DefaultDataScopeEvaluator {
    dept_cache: Arc<DeptTreeCache>,
    custom_registry: Arc<CustomGeneratorRegistry>,
    metrics: Arc<DataScopeMetrics>,
}

impl DefaultDataScopeEvaluator {
    /// 创建评估器
    pub fn new(
        dept_cache: Arc<DeptTreeCache>,
        custom_registry: Arc<CustomGeneratorRegistry>,
        metrics: Arc<DataScopeMetrics>,
    ) -> Self {
        Self {
            dept_cache,
            custom_registry,
            metrics,
        }
    }
}

#[async_trait]
impl DataScopeEvaluator for DefaultDataScopeEvaluator {
    async fn evaluate(
        &self,
        ctx: &DataScopeContext,
        rule: &DataScopeRule,
    ) -> Result<Vec<WhereCondition>, DataScopeError> {
        let start = Instant::now();

        if ctx.is_super {
            self.metrics.record_bypass(ctx.user_id, &rule.target_table);
            return Ok(Vec::new());
        }

        let result = match rule.mode {
            DataScopeMode::All => AllMode.evaluate(ctx, rule).await,
            DataScopeMode::Dept => DeptMode.evaluate(ctx, rule).await,
            DataScopeMode::DeptAndSub => {
                let mode = crate::data_scope::modes::dept_and_sub::DeptAndSubMode::new(
                    self.dept_cache.clone(),
                );
                mode.evaluate(ctx, rule).await
            }
            DataScopeMode::Self_ => SelfMode.evaluate(ctx, rule).await,
            DataScopeMode::Custom => {
                let mode =
                    crate::data_scope::modes::custom::CustomMode::new(self.custom_registry.clone());
                mode.evaluate(ctx, rule).await
            }
        };

        let elapsed_ms = start.elapsed().as_millis() as u64;
        self.metrics.record_eval(elapsed_ms);

        match result {
            Ok(conditions) => {
                self.metrics
                    .record_hit(&rule.target_table, rule.mode.as_str());
                Ok(conditions)
            }
            Err(ref e) => {
                self.metrics
                    .record_reject(e.error_code(), &rule.target_table);
                Err(e.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_scope::cache::{DeptTreeCache, DeptTreeProvider};

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
    async fn test_evaluator_all_mode() {
        let evaluator = make_evaluator();
        let ctx = DataScopeContext::new(1, 5, false);
        let rule = DataScopeRule::new("order", DataScopeMode::All);
        let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
        assert!(conditions.is_empty());
    }

    #[tokio::test]
    async fn test_evaluator_dept_mode() {
        let evaluator = make_evaluator();
        let ctx = DataScopeContext::new(1, 5, false);
        let rule = DataScopeRule::new("order", DataScopeMode::Dept).with_dept_field("dept_id");
        let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
        assert_eq!(conditions.len(), 1);
    }

    #[tokio::test]
    async fn test_evaluator_self_mode() {
        let evaluator = make_evaluator();
        let ctx = DataScopeContext::new(10, 5, false);
        let rule =
            DataScopeRule::new("order", DataScopeMode::Self_).with_creator_field("creator_id");
        let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
        assert_eq!(conditions.len(), 1);
    }

    #[tokio::test]
    async fn test_evaluator_super_bypass() {
        let evaluator = make_evaluator();

        let ctx = DataScopeContext::new(1, 5, true);
        let rule = DataScopeRule::new("order", DataScopeMode::Dept).with_dept_field("dept_id");
        let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
        assert!(conditions.is_empty());
    }

    #[tokio::test]
    async fn test_evaluator_error_propagation() {
        let evaluator = make_evaluator();
        let ctx = DataScopeContext::new(1, 5, false);
        let rule = DataScopeRule::new("order", DataScopeMode::Dept);
        let result = evaluator.evaluate(&ctx, &rule).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().error_code(), "DATA_SCOPE_INVALID_RULE");
    }

    #[tokio::test]
    async fn test_evaluator_dept_and_sub_mode() {
        let evaluator = make_evaluator();
        let ctx = DataScopeContext::new(1, 5, false);
        let rule =
            DataScopeRule::new("order", DataScopeMode::DeptAndSub).with_dept_field("dept_id");
        let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
        assert_eq!(conditions.len(), 1);
    }
}
