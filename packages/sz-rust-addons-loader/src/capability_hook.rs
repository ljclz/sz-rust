use sz_rust_capability::{CapResult, CapabilityRegistry};

/// 插件能力钩子 trait。
///
/// 插件实现此 trait 后，在激活时由 `AddonLoader` 调用 [`register_capabilities`](CapabilityHook::register_capabilities)
/// 将插件能力注册到全局 [`CapabilityRegistry`]。
///
/// # 命名规范
///
/// 能力 name 应以 `{plugin_name}.` 前缀开头（如 `crm.search_customer`），
/// 以便卸载时按前缀批量注销。
///
/// # 实现示例
///
/// ```ignore
/// use sz_rust_addons_loader::CapabilityHook;
/// use sz_rust_capability::{CapabilityRegistry, CapResult};
///
/// struct CrmPlugin;
///
/// impl CapabilityHook for CrmPlugin {
///     fn register_capabilities(&self, registry: &CapabilityRegistry) -> CapResult<Vec<String>> {
///         // 注册 CRM 插件的能力...
///         Ok(vec!["crm.search_customer".into()])
///     }
///
///     fn capability_names(&self) -> Vec<String> {
///         vec!["crm.search_customer".into()]
///     }
/// }
/// ```
pub trait CapabilityHook: Send + Sync {
    /// 注册插件能力到 Registry，返回已注册的能力名称列表。
    fn register_capabilities(&self, registry: &CapabilityRegistry) -> CapResult<Vec<String>>;

    /// 返回插件声明的能力名称列表（不实际注册）。
    fn capability_names(&self) -> Vec<String>;
}

/// 按插件名前缀批量注销能力。
///
/// 遍历 Registry 中所有能力，对名称以 `{plugin_name}.` 开头的能力调用 `unregister`。
///
/// # 返回值
///
/// 返回已注销的能力名称列表。
pub fn unregister_plugin_capabilities(
    registry: &CapabilityRegistry,
    plugin_name: &str,
) -> Vec<String> {
    let prefix = format!("{plugin_name}.");
    let caps = registry.list_all();
    let to_remove: Vec<String> = caps
        .iter()
        .filter(|cap| cap.name().starts_with(&prefix))
        .map(|cap| cap.name().to_string())
        .collect();

    let mut removed = Vec::with_capacity(to_remove.len());
    for name in &to_remove {
        if registry.unregister(name).is_some() {
            removed.push(name.clone());
        }
    }
    removed
}

/// 校验能力命名规范。
///
/// 能力 name 应以 `{plugin_name}.` 前缀开头。不符则 `tracing::warn`（不拒绝）。
pub fn validate_capability_naming(plugin_name: &str, cap_name: &str) -> bool {
    let prefix = format!("{plugin_name}.");
    if cap_name.starts_with(&prefix) {
        true
    } else {
        tracing::warn!("能力 {cap_name} 不符合命名规范：应以 {prefix} 开头（插件 {plugin_name}）");
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;
    use sz_rust_capability::{Capability, CapabilitySource};

    struct TestCap {
        cap_name: &'static str,
    }

    #[async_trait]
    impl Capability for TestCap {
        fn name(&self) -> &'static str {
            self.cap_name
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
            CapabilitySource::Plugin
        }
        async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
            Ok(args)
        }
    }

    #[test]
    fn test_unregister_plugin_capabilities() {
        let registry = CapabilityRegistry::new();
        registry.register(Arc::new(TestCap {
            cap_name: "crm.search",
        }) as Arc<dyn Capability>);
        registry.register(Arc::new(TestCap {
            cap_name: "crm.create",
        }) as Arc<dyn Capability>);
        registry.register(Arc::new(TestCap {
            cap_name: "erp.export",
        }) as Arc<dyn Capability>);

        let removed = unregister_plugin_capabilities(&registry, "crm");
        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&"crm.search".to_string()));
        assert!(removed.contains(&"crm.create".to_string()));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_unregister_nonexistent_plugin() {
        let registry = CapabilityRegistry::new();
        let removed = unregister_plugin_capabilities(&registry, "nonexistent");
        assert!(removed.is_empty());
    }

    #[test]
    fn test_validate_naming_valid() {
        assert!(validate_capability_naming("crm", "crm.search"));
    }

    #[test]
    fn test_validate_naming_invalid() {
        assert!(!validate_capability_naming("crm", "wrong.search"));
    }
}
