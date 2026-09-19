// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use std::sync::{Arc, OnceLock};

use crate::capability::Capability;
use crate::error::{CapError, CapResult};
use crate::metrics::CapMetrics;
use crate::permission::PermissionChecker;
use crate::registry::CapabilityRegistry;
use crate::source::CapabilitySource;

struct CapInstance {
    registry: Arc<CapabilityRegistry>,
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
    /// 使用外部 registry 初始化全局 facade（与调用方共享同一实例）
    ///
    /// 业务应用（如 sz300）持有自己的 `Arc<CapabilityRegistry>` 用于注入
    /// `AppState` 时，应使用本方法而非 [`Cap::init`]——否则全局 facade 与
    /// 应用局部 registry 是**两个独立实例**，`Cap::register` 注册的能力
    /// 无法被业务 handler 访问（2026-08-15 双实例缺陷修复）。
    pub fn init_with(registry: Arc<CapabilityRegistry>) -> CapResult<()> {
        GLOBAL
            .set(CapInstance { registry })
            .map_err(|_| CapError::NotInitialized)
    }

    pub fn init() -> CapResult<()> {
        Self::init_with(Arc::new(CapabilityRegistry::new()))
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

    struct PluginCapability;

    #[async_trait]
    impl Capability for PluginCapability {
        fn name(&self) -> &'static str {
            "plugin_cap"
        }
        fn description(&self) -> &'static str {
            "插件能力"
        }
        fn schema(&self) -> serde_json::Value {
            json!({})
        }
        fn tags(&self) -> &[&'static str] {
            &["plugin", "data"]
        }
        fn source(&self) -> CapabilitySource {
            CapabilitySource::Plugin
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

    #[test]
    fn test_is_initialized() {
        Cap::init().ok();
        assert!(Cap::is_initialized());
    }

    #[test]
    fn test_unregister() {
        Cap::init().ok();
        let cap = Arc::new(TestCapability) as Arc<dyn Capability>;
        Cap::register(cap).ok();
        let removed = Cap::unregister("test_cap").unwrap();
        assert!(removed.is_some());
        assert!(Cap::get("test_cap").unwrap().is_none());
        let again = Cap::unregister("test_cap").unwrap();
        assert!(again.is_none());
    }

    #[test]
    fn test_find_by_tags() {
        Cap::init().ok();
        let cap = Arc::new(PluginCapability) as Arc<dyn Capability>;
        Cap::register(cap).ok();
        let results = Cap::find_by_tags(&["plugin"], None).unwrap();
        assert!(!results.is_empty());
        let filtered = Cap::find_by_tags(&["plugin"], Some(CapabilitySource::Plugin)).unwrap();
        assert!(!filtered.is_empty());
        let none = Cap::find_by_tags(&["nonexistent_tag"], None).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn test_search() {
        Cap::init().ok();
        let cap = Arc::new(PluginCapability) as Arc<dyn Capability>;
        Cap::register(cap).ok();
        let results = Cap::search("plugin").unwrap();
        assert!(!results.is_empty());
        let empty = Cap::search("zzz_no_match_zzz").unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn test_list_all() {
        Cap::init().ok();
        let cap = Arc::new(PluginCapability) as Arc<dyn Capability>;
        Cap::register(cap).ok();
        let all = Cap::list_all().unwrap();
        assert!(!all.is_empty());
    }

    #[test]
    fn test_list_by_source() {
        Cap::init().ok();
        let cap = Arc::new(PluginCapability) as Arc<dyn Capability>;
        Cap::register(cap).ok();
        let plugins = Cap::list_by_source(CapabilitySource::Plugin).unwrap();
        assert!(!plugins.is_empty());
        let services = Cap::list_by_source(CapabilitySource::Service).unwrap();
        assert!(services.is_empty());
    }

    #[test]
    fn test_is_empty_and_len() {
        Cap::init().ok();
        let cap = Arc::new(PluginCapability) as Arc<dyn Capability>;
        Cap::register(cap).ok();
        assert!(!Cap::is_empty().unwrap());
        assert!(Cap::len().unwrap() >= 1);
    }

    #[test]
    fn test_metrics() {
        Cap::init().ok();
        let _metrics = Cap::metrics().unwrap();
    }

    #[test]
    fn test_set_permission_checker() {
        Cap::init().ok();
        let checker = Arc::new(crate::permission::AllowAll) as Arc<dyn PermissionChecker>;
        assert!(Cap::set_permission_checker(checker).is_ok());
    }

    #[tokio::test]
    async fn test_call_with_tenant() {
        Cap::init().ok();
        let cap = Arc::new(PluginCapability) as Arc<dyn Capability>;
        Cap::register(cap).ok();
        let checker = Arc::new(crate::permission::AllowAll) as Arc<dyn PermissionChecker>;
        Cap::set_permission_checker(checker).ok();
        let result = Cap::call_with_tenant("plugin_cap", json!({"x": 1}), 42).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), json!({"x": 1}));
    }

    #[test]
    fn test_init_with_already_initialized_fails() {
        Cap::init().ok();
        let registry = Arc::new(CapabilityRegistry::new());
        let result = Cap::init_with(registry);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_nonexistent() {
        Cap::init().ok();
        assert!(Cap::get("zzz_nonexistent_zzz").unwrap().is_none());
    }
}
