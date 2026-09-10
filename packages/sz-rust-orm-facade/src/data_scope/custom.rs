// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 自定义条件生成器注册表 — CustomGeneratorRegistry
//!
//! 使用 DashMap 实现并发安全，register 采用 &self 签名。

use crate::data_scope::context::DataScopeContext;
use crate::data_scope::error::DataScopeError;
use crate::repository::WhereCondition;
use async_trait::async_trait;
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

/// 自定义条件生成器注册表（并发安全）
pub struct CustomGeneratorRegistry {
    generators: dashmap::DashMap<String, Arc<dyn CustomConditionGenerator>>,
}

impl CustomGeneratorRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            generators: dashmap::DashMap::new(),
        }
    }

    /// 注册生成器（并发安全，&self 签名）
    pub fn register(&self, gen: Arc<dyn CustomConditionGenerator>) {
        self.generators.insert(gen.name().to_string(), gen);
    }

    /// 旧版注册（&mut self），已废弃，内部委托并发安全版本
    ///
    /// 注：Rust 不允许同名方法以不同 self 接收器重载（E0592），
    /// 故旧方法命名为 `register_mut`，调用方应迁移至 `register(&self, gen)`。
    #[deprecated(note = "use register(&self, gen) instead")]
    pub fn register_mut(&mut self, gen: Arc<dyn CustomConditionGenerator>) {
        self.register(gen);
    }

    /// 查询生成器
    pub fn get(&self, name: &str) -> Option<Arc<dyn CustomConditionGenerator>> {
        self.generators.get(name).map(|e| Arc::clone(e.value()))
    }

    /// 返回所有已注册生成器名称
    pub fn list_names(&self) -> Vec<String> {
        self.generators.iter().map(|e| e.key().clone()).collect()
    }

    /// 是否已注册指定名称的生成器
    pub fn contains(&self, name: &str) -> bool {
        self.generators.contains_key(name)
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
        let registry = CustomGeneratorRegistry::new();
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
        let registry = CustomGeneratorRegistry::new();
        registry.register(Arc::new(RegionGenerator));
        let gen = registry.get("region_filter").unwrap();
        let ctx = DataScopeContext::new(1, 5, false);
        let conditions = gen.generate(&ctx).await.unwrap();
        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].field, "region");
    }

    #[test]
    fn test_contains_and_list_names() {
        let registry = CustomGeneratorRegistry::new();
        assert!(!registry.contains("region_filter"));
        registry.register(Arc::new(RegionGenerator));
        assert!(registry.contains("region_filter"));
        assert!(!registry.contains("nonexistent"));
        let names = registry.list_names();
        assert_eq!(names, vec!["region_filter".to_string()]);
    }

    #[test]
    fn test_concurrent_register() {
        use std::thread;
        let registry = Arc::new(CustomGeneratorRegistry::new());
        let mut handles = vec![];
        for _ in 0..4 {
            let r = registry.clone();
            handles.push(thread::spawn(move || {
                r.register(Arc::new(RegionGenerator));
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // 并发注册同名生成器，最终仅一个条目
        assert!(registry.contains("region_filter"));
        assert_eq!(registry.list_names().len(), 1);
    }
}
