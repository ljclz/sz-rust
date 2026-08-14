//! CUSTOM 模式 — 自定义条件（通过 CustomConditionGenerator 生成）

use super::ModeEvaluator;
use crate::data_scope::context::DataScopeContext;
use crate::data_scope::custom::CustomGeneratorRegistry;
use crate::data_scope::error::DataScopeError;
use crate::data_scope::rule::DataScopeRule;
use crate::repository::WhereCondition;
use async_trait::async_trait;
use std::sync::Arc;

/// CUSTOM 模式评估器
pub struct CustomMode {
    registry: Arc<CustomGeneratorRegistry>,
}

impl CustomMode {
    pub fn new(registry: Arc<CustomGeneratorRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl ModeEvaluator for CustomMode {
    async fn evaluate(
        &self,
        ctx: &DataScopeContext,
        rule: &DataScopeRule,
    ) -> Result<Vec<WhereCondition>, DataScopeError> {
        let generator_name = rule.custom_generator.as_deref().ok_or_else(|| {
            DataScopeError::InvalidRule("CUSTOM mode requires custom_generator".into())
        })?;
        let generator = self
            .registry
            .get(generator_name)
            .ok_or_else(|| DataScopeError::GeneratorNotFound(generator_name.to_string()))?;
        let conditions = generator.generate(ctx).await?;
        if conditions.is_empty() {
            return Err(DataScopeError::UnsafeCustomCondition(
                "custom generator returned empty conditions".into(),
            ));
        }
        Ok(conditions)
    }
}
