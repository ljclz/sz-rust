use std::sync::{Arc, OnceLock};

use crate::capability::Capability;
use crate::error::{CapError, CapResult};
use crate::metrics::CapMetrics;
use crate::permission::PermissionChecker;
use crate::registry::CapabilityRegistry;
use crate::source::CapabilitySource;

struct CapInstance {
    registry: CapabilityRegistry,
}

static GLOBAL: OnceLock<CapInstance> = OnceLock::new();

/// Capability Registry 全局 facade，对齐 `Ai` facade 模式。
///
/// 使用 `OnceLock<CapInstance>` 全局单例，所有静态方法通过 `instance()` 获取后委托给内部 [`CapabilityRegistry`]。
///
/// # 使用方式
///
/// ```no_run
/// use sz_rust_capability::Cap;
///
/// Cap::init().ok(); // 初始化（仅需一次）
/// let metrics = Cap::metrics().unwrap();
/// ```
pub struct Cap;

impl Cap {
    pub fn init() -> CapResult<()> {
        GLOBAL
            .set(CapInstance {
                registry: CapabilityRegistry::new(),
            })
            .map_err(|_| CapError::NotInitialized)
    }

    pub fn is_initialized() -> bool {
        GLOBAL.get().is_some()
    }

    fn instance() -> CapResult<&'static CapInstance> {
        GLOBAL.get().ok_or(CapError::NotInitialized)
    }

    pub fn register(cap: Arc<dyn Capability>) -> CapResult<Option<Arc<dyn Capability>>> {
        Ok(Self::instance()?.registry.register(cap))
    }

    pub fn unregister(name: &str) -> CapResult<Option<Arc<dyn Capability>>> {
        Ok(Self::instance()?.registry.unregister(name))
    }

    pub fn get(name: &str) -> CapResult<Option<Arc<dyn Capability>>> {
        Ok(Self::instance()?.registry.get(name))
    }

    pub fn find_by_tags(
        tags: &[&str],
        source: Option<CapabilitySource>,
    ) -> CapResult<Vec<Arc<dyn Capability>>> {
        Ok(Self::instance()?.registry.find_by_tags(tags, source))
    }

    pub fn search(query: &str) -> CapResult<Vec<Arc<dyn Capability>>> {
        Ok(Self::instance()?.registry.search(query))
    }

    pub fn list_all() -> CapResult<Vec<Arc<dyn Capability>>> {
        Ok(Self::instance()?.registry.list_all())
    }

    pub fn list_by_source(source: CapabilitySource) -> CapResult<Vec<Arc<dyn Capability>>> {
        Ok(Self::instance()?.registry.list_by_source(source))
    }

    pub async fn call(name: &str, args: serde_json::Value) -> CapResult<serde_json::Value> {
        Self::instance()?.registry.call(name, args).await
    }

    /// 调用能力，携带租户上下文用于权限检查。
    pub async fn call_with_tenant(
        name: &str,
        args: serde_json::Value,
        tenant_id: i64,
    ) -> CapResult<serde_json::Value> {
        Self::instance()?
            .registry
            .call_with_tenant(name, args, tenant_id)
            .await
    }

    /// 设置权限检查器。
    pub fn set_permission_checker(checker: Arc<dyn PermissionChecker>) -> CapResult<()> {
        Self::instance()?.registry.set_permission_checker(checker);
        Ok(())
    }

    pub fn metrics() -> CapResult<CapMetrics> {
        Ok(Self::instance()?.registry.metrics())
    }

    pub fn len() -> CapResult<usize> {
        Ok(Self::instance()?.registry.len())
    }

    pub fn is_empty() -> CapResult<bool> {
        Ok(Self::instance()?.registry.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    struct TestCapability;

    #[async_trait]
    impl Capability for TestCapability {
        fn name(&self) -> &'static str {
            "test_cap"
        }
        fn description(&self) -> &'static str {
            "测试能力"
        }
        fn schema(&self) -> serde_json::Value {
            json!({})
        }
        fn tags(&self) -> &[&'static str] {
            &["test"]
        }
        fn source(&self) -> CapabilitySource {
            CapabilitySource::Skill
        }
        async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
            Ok(args)
        }
    }

    #[test]
    fn test_facade_lifecycle() {
        Cap::init().ok();
        let cap = Arc::new(TestCapability) as Arc<dyn Capability>;
        Cap::register(cap).unwrap();
        assert!(Cap::get("test_cap").unwrap().is_some());
        assert!(Cap::len().unwrap() >= 1);
    }

    #[tokio::test]
    async fn test_call_through_facade() {
        Cap::init().ok();
        let cap = Arc::new(TestCapability) as Arc<dyn Capability>;
        Cap::register(cap).ok();
        let result = Cap::call("test_cap", json!({"hello": "world"})).await;
        assert!(result.is_ok());
    }
}
