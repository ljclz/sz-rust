//! 5 种数据范围模式评估器
//!
//! 每种模式将 `DataScopeRule` + `DataScopeContext` 转换为 `Vec<WhereCondition>`。

pub mod all;
pub mod custom;
pub mod dept;
pub mod dept_and_sub;
pub mod self_mode;

use crate::data_scope::context::DataScopeContext;
use crate::data_scope::error::DataScopeError;
use crate::data_scope::rule::DataScopeRule;
use crate::repository::WhereCondition;
use async_trait::async_trait;

/// 模式评估器 trait
#[async_trait]
pub trait ModeEvaluator: Send + Sync {
    /// 评估数据范围，返回 WHERE 条件列表
    async fn evaluate(
        &self,
        ctx: &DataScopeContext,
        rule: &DataScopeRule,
    ) -> Result<Vec<WhereCondition>, DataScopeError>;
}
