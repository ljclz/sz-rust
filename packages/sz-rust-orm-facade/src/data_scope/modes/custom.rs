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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_scope::custom::CustomGeneratorRegistry;
    use crate::data_scope::rule::DataScopeMode;

    #[tokio::test]
    async fn test_custom_mode_missing_generator_name() {
        let registry = Arc::new(CustomGeneratorRegistry::new());
        let mode = CustomMode::new(registry);
        let ctx = DataScopeContext::new(1, 5, false);
        let rule = DataScopeRule::new("order", DataScopeMode::Custom);
        let err = mode.evaluate(&ctx, &rule).await.unwrap_err();
        assert_eq!(err.error_code(), "DATA_SCOPE_INVALID_RULE");
    }

    #[tokio::test]
    async fn test_custom_mode_generator_not_found() {
        let registry = Arc::new(CustomGeneratorRegistry::new());
        let mode = CustomMode::new(registry);
        let ctx = DataScopeContext::new(1, 5, false);
        let rule =
            DataScopeRule::new("order", DataScopeMode::Custom).with_custom_generator("nonexistent");
        let err = mode.evaluate(&ctx, &rule).await.unwrap_err();
        assert_eq!(err.error_code(), "DATA_SCOPE_GENERATOR_NOT_FOUND");
    }
}
