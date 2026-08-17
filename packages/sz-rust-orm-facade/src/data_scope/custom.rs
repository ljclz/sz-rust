//! 自定义条件生成器注册表 — CustomGeneratorRegistry

use crate::data_scope::context::DataScopeContext;
use crate::data_scope::error::DataScopeError;
use crate::repository::WhereCondition;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// 自定义条件生成器 trait
#[async_trait]
pub trait CustomConditionGenerator: Send + Sync {
    /// 生成器名称
    fn name(&self) -> &str;

    /// 生成 WHERE 条件列表
    async fn generate(&self, ctx: &DataScopeContext)
        -> Result<Vec<WhereCondition>, DataScopeError>;
}

/// 自定义条件生成器注册表
pub struct CustomGeneratorRegistry {
    generators: HashMap<String, Arc<dyn CustomConditionGenerator>>,
}

impl CustomGeneratorRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            generators: HashMap::new(),
        }
    }

    /// 注册生成器
    pub fn register(&mut self, gen: Arc<dyn CustomConditionGenerator>) {
        self.generators.insert(gen.name().to_string(), gen);
    }

    /// 查询生成器
    pub fn get(&self, name: &str) -> Option<Arc<dyn CustomConditionGenerator>> {
        self.generators.get(name).cloned()
    }
}

impl Default for CustomGeneratorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::WhereOp;
    use crate::Value;

    struct RegionGenerator;

    #[async_trait]
    impl CustomConditionGenerator for RegionGenerator {
        fn name(&self) -> &str {
            "region_filter"
        }

        async fn generate(
            &self,
            _ctx: &DataScopeContext,
        ) -> Result<Vec<WhereCondition>, DataScopeError> {
            Ok(vec![WhereCondition::new(
                "region",
                WhereOp::Eq,
                Value::String("CN".into()),
            )])
        }
    }

    #[test]
    fn test_register_and_get() {
        let mut registry = CustomGeneratorRegistry::new();
        registry.register(Arc::new(RegionGenerator));
        assert!(registry.get("region_filter").is_some());
    }

    #[test]
    fn test_get_not_found() {
        let registry = CustomGeneratorRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_default_is_empty() {
        let registry = CustomGeneratorRegistry::default();
        assert!(registry.get("any").is_none());
    }

    #[tokio::test]
    async fn test_generate_produces_condition() {
        let mut registry = CustomGeneratorRegistry::new();
        registry.register(Arc::new(RegionGenerator));
        let gen = registry.get("region_filter").unwrap();
        let ctx = DataScopeContext::new(1, 5, false);
        let conditions = gen.generate(&ctx).await.unwrap();
        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].field, "region");
    }
}
