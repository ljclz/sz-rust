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
