// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use async_trait::async_trait;
use serde::Serialize;

use crate::error::CapResult;
use crate::source::CapabilitySource;

/// 统一能力抽象 trait。
///
/// Skills（AI 内置能力）和 Plugins（业务插件）都实现此 trait，
/// 通过 [`CapabilityRegistry`](crate::CapabilityRegistry) 统一注册、发现和调用。
///
/// # 实现示例
///
/// ```
/// use async_trait::async_trait;
/// use serde_json::{json, Value};
/// use sz_rust_capability::{Capability, CapabilitySource, CapResult};
///
/// struct SearchCustomerCapability;
///
/// #[async_trait]
/// impl Capability for SearchCustomerCapability {
///     fn name(&self) -> &'static str { "crm.search_customer" }
///     fn description(&self) -> &'static str { "搜索客户" }
///     fn schema(&self) -> Value {
///         json!({
///             "type": "object",
///             "properties": { "keyword": { "type": "string" } },
///             "required": ["keyword"]
///         })
///     }
///     fn tags(&self) -> &[&'static str] { &["crm", "search", "read"] }
///     fn source(&self) -> CapabilitySource { CapabilitySource::Plugin }
///     async fn call(&self, args: Value) -> CapResult<Value> {
///         let keyword = args.get("keyword").and_then(|v| v.as_str()).unwrap_or("");
///         Ok(json!({ "results": [keyword] }))
///     }
/// }
/// ```
#[async_trait]
pub trait Capability: Send + Sync + 'static {
    /// 能力名称，全局唯一，格式建议 `{source_prefix}.{capability_name}`。
    fn name(&self) -> &'static str;

    /// 人类可读的能力描述。
    fn description(&self) -> &'static str;

    /// 参数 JSON Schema，描述 `call` 方法的输入参数格式。
    fn schema(&self) -> serde_json::Value;

    /// 能力标签，用于 `find_by_tags` 搜索。多标签 AND 逻辑。
    fn tags(&self) -> &[&'static str];

    /// 能力来源类型（Skill/Plugin/Service）。
    fn source(&self) -> CapabilitySource;

    /// 执行能力，接受 JSON 参数，返回 JSON 结果。
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value>;

    /// 能力版本，默认 "1.0.0"。
    fn version(&self) -> &'static str {
        "1.0.0"
    }

    /// 是否需要人工确认（HITL），默认 false。
    fn requires_confirmation(&self) -> bool {
        false
    }

    /// 参数校验，默认实现委托 [`validate_json_schema`](crate::registry::validate_json_schema) 做轻量校验。
    /// 能力可覆盖此方法做完整 JSON Schema 校验。
    async fn validate_args(&self, args: &serde_json::Value) -> CapResult<()> {
        crate::registry::validate_json_schema(&self.schema(), args)
    }
}

/// 能力元信息快照，用于列表/搜索返回。
#[derive(Debug, Clone, Serialize)]
pub struct CapabilityInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub tags: Vec<&'static str>,
    pub source: CapabilitySource,
    pub version: &'static str,
    pub requires_confirmation: bool,
}

impl CapabilityInfo {
    /// 从 Capability trait 对象提取元信息快照。
    pub fn from_trait(cap: &dyn Capability) -> Self {
        Self {
            name: cap.name(),
            description: cap.description(),
            tags: cap.tags().to_vec(),
            source: cap.source(),
            version: cap.version(),
            requires_confirmation: cap.requires_confirmation(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    struct TestCap;

    #[async_trait]
    impl Capability for TestCap {
        fn name(&self) -> &'static str {
            "test.cap"
        }
        fn description(&self) -> &'static str {
            "测试能力"
        }
        fn schema(&self) -> serde_json::Value {
            json!({"type": "object"})
        }
        fn tags(&self) -> &[&'static str] {
            &["test", "demo"]
        }
        fn source(&self) -> CapabilitySource {
            CapabilitySource::Skill
        }
        async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
            Ok(args)
        }
    }

    struct CustomVersionCap;

    #[async_trait]
    impl Capability for CustomVersionCap {
        fn name(&self) -> &'static str {
            "custom.version"
        }
        fn description(&self) -> &'static str {
            "自定义版本能力"
        }
        fn schema(&self) -> serde_json::Value {
            json!({})
        }
        fn tags(&self) -> &[&'static str] {
            &[]
        }
        fn source(&self) -> CapabilitySource {
            CapabilitySource::Plugin
        }
        fn version(&self) -> &'static str {
            "2.0.0"
        }
        fn requires_confirmation(&self) -> bool {
            true
        }
        async fn call(&self, _: serde_json::Value) -> CapResult<serde_json::Value> {
            Ok(json!({}))
        }
    }

    #[test]
    fn test_capability_info_from_trait_defaults() {
        let cap = TestCap;
        let info = CapabilityInfo::from_trait(&cap);
        assert_eq!(info.name, "test.cap");
        assert_eq!(info.description, "测试能力");
        assert_eq!(info.tags, vec!["test", "demo"]);
        assert_eq!(info.source, CapabilitySource::Skill);
        assert_eq!(info.version, "1.0.0");
        assert!(!info.requires_confirmation);
    }

    #[test]
    fn test_capability_info_from_trait_custom() {
        let cap = CustomVersionCap;
        let info = CapabilityInfo::from_trait(&cap);
        assert_eq!(info.name, "custom.version");
        assert_eq!(info.version, "2.0.0");
        assert!(info.requires_confirmation);
        assert_eq!(info.source, CapabilitySource::Plugin);
        assert!(info.tags.is_empty());
    }

    #[tokio::test]
    async fn test_capability_default_validate_args() {
        let cap = TestCap;
        let result = cap.validate_args(&json!({"key": "value"})).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_capability_info_serialize() {
        let cap = TestCap;
        let info = CapabilityInfo::from_trait(&cap);
        let json = serde_json::to_string(&info).expect("序列化失败");
        assert!(json.contains("test.cap"));
        assert!(json.contains("测试能力"));
    }
}
