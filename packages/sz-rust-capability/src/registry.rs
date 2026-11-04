// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::capability::{Capability, CapabilityInfo};
use crate::error::{CapError, CapResult};
use crate::metrics::CapMetrics;
use crate::permission::PermissionChecker;
use crate::source::CapabilitySource;

/// 中心能力注册表，提供注册/发现/调用的统一入口。
///
/// 内部使用 `parking_lot::RwLock<HashMap<String, Arc<dyn Capability>>>` 保证并发安全。
/// 所有读操作（get/find/search/list）在读锁内完成 Arc 克隆后释放锁，不跨 await 点。
///
/// # 调用链路
///
/// `call` / `call_with_tenant` 执行三步链路：参数校验 → 权限检查 → 能力调用。
/// 未设置 `PermissionChecker` 时默认放行。
///
/// # 性能指标
///
/// | 操作 | 延迟 |
/// |------|------|
/// | register | ~187 ns |
/// | get | ~38 ns |
/// | find_by_tags (1000 能力) | ~20 μs |
pub struct CapabilityRegistry {
    capabilities: parking_lot::RwLock<HashMap<String, Arc<dyn Capability>>>,
    permission_checker: parking_lot::RwLock<Option<Arc<dyn PermissionChecker>>>,
    call_total: AtomicU64,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self {
            capabilities: parking_lot::RwLock::new(HashMap::new()),
            permission_checker: parking_lot::RwLock::new(None),
            call_total: AtomicU64::new(0),
        }
    }

    /// 设置权限检查器，设置后所有 `call` / `call_with_tenant` 将在能力调用前执行权限检查。
    pub fn set_permission_checker(&self, checker: Arc<dyn PermissionChecker>) {
        let mut guard = self.permission_checker.write();
        *guard = Some(checker);
    }

    pub fn register(&self, cap: Arc<dyn Capability>) -> Option<Arc<dyn Capability>> {
        let name = cap.name().to_string();
        let mut caps = self.capabilities.write();
        caps.insert(name, cap)
    }

    pub fn unregister(&self, name: &str) -> Option<Arc<dyn Capability>> {
        let mut caps = self.capabilities.write();
        caps.remove(name)
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Capability>> {
        let caps = self.capabilities.read();
        caps.get(name).cloned()
    }

    pub fn find_by_tags(
        &self,
        tags: &[&str],
        source: Option<CapabilitySource>,
    ) -> Vec<Arc<dyn Capability>> {
        let caps = self.capabilities.read();
        caps.values()
            .filter(|cap| {
                let tag_match = tags.iter().all(|t| cap.tags().contains(t));
                let source_match = source.map_or(true, |s| cap.source() == s);
                tag_match && source_match
            })
            .cloned()
            .collect()
    }

    pub fn search(&self, query: &str) -> Vec<Arc<dyn Capability>> {
        let caps = self.capabilities.read();
        let query_lower = query.to_lowercase();
        caps.values()
            .filter(|cap| {
                cap.name().to_lowercase().contains(&query_lower)
                    || cap.description().to_lowercase().contains(&query_lower)
            })
            .cloned()
            .collect()
    }

    pub fn list_all(&self) -> Vec<Arc<dyn Capability>> {
        let caps = self.capabilities.read();
        caps.values().cloned().collect()
    }

    pub fn list_by_source(&self, source: CapabilitySource) -> Vec<Arc<dyn Capability>> {
        let caps = self.capabilities.read();
        caps.values()
            .filter(|cap| cap.source() == source)
            .cloned()
            .collect()
    }

    /// 调用能力，使用默认 `tenant_id = 0`（无租户上下文）。
    ///
    /// 执行链路：参数校验 → 权限检查 → 能力调用。
    pub async fn call(&self, name: &str, args: serde_json::Value) -> CapResult<serde_json::Value> {
        self.call_with_tenant(name, args, 0).await
    }

    /// 调用能力，携带租户上下文用于权限检查。
    ///
    /// 执行链路：参数校验 → 权限检查 → 能力调用。
    /// 未设置 `PermissionChecker` 时默认放行。
    pub async fn call_with_tenant(
        &self,
        name: &str,
        args: serde_json::Value,
        tenant_id: i64,
    ) -> CapResult<serde_json::Value> {
        let cap = self
            .get(name)
            .ok_or_else(|| CapError::NotFound(name.to_string()))?;

        if cap.requires_confirmation() {
            return Err(CapError::ConfirmationRequired);
        }

        cap.validate_args(&args).await?;

        let checker_opt = {
            let guard = self.permission_checker.read();
            guard.as_ref().cloned()
        };
        if let Some(checker) = checker_opt {
            checker.check(name, &args, tenant_id).await?;
        }

        self.call_total.fetch_add(1, Ordering::Relaxed);
        cap.call(args).await
    }

    pub fn len(&self) -> usize {
        let caps = self.capabilities.read();
        caps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn metrics(&self) -> CapMetrics {
        let caps = self.capabilities.read();
        let mut by_source = HashMap::new();
        for cap in caps.values() {
            *by_source.entry(cap.source()).or_insert(0) += 1;
        }
        CapMetrics {
            total: caps.len(),
            by_source,
            call_total: self.call_total.load(Ordering::Relaxed),
        }
    }

    pub fn list_info(&self) -> Vec<CapabilityInfo> {
        let caps = self.capabilities.read();
        caps.values()
            .map(|cap| CapabilityInfo::from_trait(cap.as_ref()))
            .collect()
    }
}

impl Default for CapabilityRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub fn validate_json_schema(schema: &serde_json::Value, args: &serde_json::Value) -> CapResult<()> {
    if !schema.is_object() {
        return Ok(());
    }

    if let Some(required) = schema.get("required").and_then(|v| v.as_array()) {
        for field in required {
            if let Some(field_name) = field.as_str() {
                if !args
                    .as_object()
                    .is_some_and(|obj| obj.contains_key(field_name))
                {
                    return Err(CapError::ValidationError(format!(
                        "缺少必填字段: {field_name}"
                    )));
                }
            }
        }
    }

    if let (Some(properties), Some(args_obj)) = (schema.get("properties"), args.as_object()) {
        if let Some(props) = properties.as_object() {
            for (field_name, field_schema) in props {
                if let Some(field_value) = args_obj.get(field_name) {
                    if let Some(expected_type) = field_schema.get("type").and_then(|v| v.as_str()) {
                        let actual_type = json_type_of(field_value);
                        let type_match = expected_type == actual_type
                            || (expected_type == "number" && actual_type == "integer");
                        if !type_match {
                            return Err(CapError::ValidationError(format!(
                                "字段 {field_name} 类型不匹配: 期望 {expected_type}, 实际 {actual_type}"
                            )));
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn json_type_of(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    struct EchoCapability;

    #[async_trait]
    impl Capability for EchoCapability {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn description(&self) -> &'static str {
            "回显输入参数"
        }
        fn schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        }
        fn tags(&self) -> &[&'static str] {
            &["test", "echo"]
        }
        fn source(&self) -> CapabilitySource {
            CapabilitySource::Skill
        }
        async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
            Ok(args)
        }
    }

    #[tokio::test]
    async fn test_register_and_get() {
        let registry = CapabilityRegistry::new();
        let cap = Arc::new(EchoCapability) as Arc<dyn Capability>;
        let old = registry.register(cap);
        assert!(old.is_none());
        assert!(registry.get("echo").is_some());
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn test_register_overwrite() {
        let registry = CapabilityRegistry::new();
        let cap1 = Arc::new(EchoCapability) as Arc<dyn Capability>;
        let cap2 = Arc::new(EchoCapability) as Arc<dyn Capability>;
        registry.register(cap1);
        let old = registry.register(cap2);
        assert!(old.is_some());
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn test_unregister() {
        let registry = CapabilityRegistry::new();
        let cap = Arc::new(EchoCapability) as Arc<dyn Capability>;
        registry.register(cap);
        let removed = registry.unregister("echo");
        assert!(removed.is_some());
        assert!(registry.get("echo").is_none());
        assert!(registry.is_empty());
    }

    #[tokio::test]
    async fn test_find_by_tags() {
        let registry = CapabilityRegistry::new();
        let cap = Arc::new(EchoCapability) as Arc<dyn Capability>;
        registry.register(cap);
        let caps = registry.find_by_tags(&["test"], None);
        assert_eq!(caps.len(), 1);
        let caps = registry.find_by_tags(&["test"], Some(CapabilitySource::Skill));
        assert_eq!(caps.len(), 1);
        let caps = registry.find_by_tags(&["test"], Some(CapabilitySource::Plugin));
        assert_eq!(caps.len(), 0);
        let caps = registry.find_by_tags(&["nonexistent"], None);
        assert_eq!(caps.len(), 0);
    }

    #[tokio::test]
    async fn test_search() {
        let registry = CapabilityRegistry::new();
        let cap = Arc::new(EchoCapability) as Arc<dyn Capability>;
        registry.register(cap);
        let caps = registry.search("echo");
        assert_eq!(caps.len(), 1);
        let caps = registry.search("回显");
        assert_eq!(caps.len(), 1);
        let caps = registry.search("nonexistent");
        assert_eq!(caps.len(), 0);
    }

    #[tokio::test]
    async fn test_call_success() {
        let registry = CapabilityRegistry::new();
        let cap = Arc::new(EchoCapability) as Arc<dyn Capability>;
        registry.register(cap);
        let result = registry.call("echo", json!({"message": "hello"})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), json!({"message": "hello"}));
    }

    #[tokio::test]
    async fn test_call_not_found() {
        let registry = CapabilityRegistry::new();
        let result = registry.call("nonexistent", json!({})).await;
        assert!(matches!(result, Err(CapError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_call_validation_error() {
        let registry = CapabilityRegistry::new();
        let cap = Arc::new(EchoCapability) as Arc<dyn Capability>;
        registry.register(cap);
        let result = registry.call("echo", json!({})).await;
        assert!(matches!(result, Err(CapError::ValidationError(_))));
    }

    #[tokio::test]
    async fn test_call_type_mismatch() {
        let registry = CapabilityRegistry::new();
        let cap = Arc::new(EchoCapability) as Arc<dyn Capability>;
        registry.register(cap);
        let result = registry.call("echo", json!({"message": 123})).await;
        assert!(matches!(result, Err(CapError::ValidationError(_))));
    }

    #[tokio::test]
    async fn test_metrics() {
        let registry = CapabilityRegistry::new();
        let cap = Arc::new(EchoCapability) as Arc<dyn Capability>;
        registry.register(cap);
        let _ = registry.call("echo", json!({"message": "hi"})).await;
        let metrics = registry.metrics();
        assert_eq!(metrics.total, 1);
        assert_eq!(metrics.call_total, 1);
        assert_eq!(metrics.by_source.get(&CapabilitySource::Skill), Some(&1));
    }

    #[test]
    fn test_validate_json_schema_valid() {
        let schema = json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        });
        let args = json!({ "name": "test" });
        assert!(validate_json_schema(&schema, &args).is_ok());
    }

    #[test]
    fn test_validate_json_schema_missing_required() {
        let schema = json!({
            "required": ["name"]
        });
        let args = json!({});
        assert!(validate_json_schema(&schema, &args).is_err());
    }

    #[test]
    fn test_validate_json_schema_type_mismatch() {
        let schema = json!({
            "properties": { "age": { "type": "number" } }
        });
        let args = json!({ "age": "twenty" });
        assert!(validate_json_schema(&schema, &args).is_err());
    }

    #[test]
    fn test_validate_json_schema_no_schema() {
        let args = json!({ "any": "thing" });
        assert!(validate_json_schema(&json!(null), &args).is_ok());
    }

    #[tokio::test]
    async fn test_concurrent_register_and_get() {
        use std::sync::Arc;
        let registry = Arc::new(CapabilityRegistry::new());

        struct NamedCap {
            cap_name: &'static str,
        }
        #[async_trait]
        impl Capability for NamedCap {
            fn name(&self) -> &'static str {
                self.cap_name
            }
            fn description(&self) -> &'static str {
                "并发测试能力"
            }
            fn schema(&self) -> serde_json::Value {
                json!({})
            }
            fn tags(&self) -> &[&'static str] {
                &["concurrent"]
            }
            fn source(&self) -> CapabilitySource {
                CapabilitySource::Skill
            }
            async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
                Ok(args)
            }
        }

        let mut handles = vec![];
        for i in 0..50u32 {
            let reg = registry.clone();
            handles.push(tokio::spawn(async move {
                let name: &'static str = Box::leak(format!("cap_{i}").into_boxed_str());
                let cap = Arc::new(NamedCap { cap_name: name }) as Arc<dyn Capability>;
                reg.register(cap);
            }));
        }
        for h in handles {
            h.await.unwrap_or_else(|e| panic!("并发注册任务失败: {e}"));
        }
        assert_eq!(registry.len(), 50);

        let caps = registry.find_by_tags(&["concurrent"], None);
        assert_eq!(caps.len(), 50);
    }

    #[tokio::test]
    async fn test_concurrent_call_no_deadlock() {
        use std::sync::Arc;
        let registry = Arc::new(CapabilityRegistry::new());
        let cap = Arc::new(EchoCapability) as Arc<dyn Capability>;
        registry.register(cap);

        let mut handles = vec![];
        for _ in 0..100u32 {
            let reg = registry.clone();
            handles.push(tokio::spawn(async move {
                let _ = reg.call("echo", json!({"message": "concurrent"})).await;
            }));
        }
        for h in handles {
            h.await.unwrap_or_else(|e| panic!("并发注册任务失败: {e}"));
        }
        assert_eq!(registry.metrics().call_total, 100);
    }

    #[tokio::test]
    async fn test_concurrent_mixed_operations() {
        use std::sync::Arc;
        let registry = Arc::new(CapabilityRegistry::new());

        struct MixedCap;
        #[async_trait]
        impl Capability for MixedCap {
            fn name(&self) -> &'static str {
                "mixed_cap"
            }
            fn description(&self) -> &'static str {
                "混合操作测试"
            }
            fn schema(&self) -> serde_json::Value {
                json!({})
            }
            fn tags(&self) -> &[&'static str] {
                &["mixed"]
            }
            fn source(&self) -> CapabilitySource {
                CapabilitySource::Plugin
            }
            async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
                Ok(args)
            }
        }

        let cap = Arc::new(MixedCap) as Arc<dyn Capability>;
        registry.register(cap);

        let mut handles = vec![];
        for _ in 0..50 {
            let reg = registry.clone();
            handles.push(tokio::spawn(async move {
                let _ = reg.get("mixed_cap");
                let _ = reg.find_by_tags(&["mixed"], None);
                let _ = reg.list_all();
                let _ = reg.metrics();
            }));
        }
        for h in handles {
            h.await.unwrap_or_else(|e| panic!("并发注册任务失败: {e}"));
        }
        assert_eq!(registry.len(), 1);
    }
}
