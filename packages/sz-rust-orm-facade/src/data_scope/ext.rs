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
