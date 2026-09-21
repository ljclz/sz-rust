// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use sz_rust_addons_loader::capability_hook::CapabilityHook;
use sz_rust_capability::{CapResult, Capability, CapabilityRegistry, CapabilitySource};

use crate::{Span, SzTracer, Tracer, TracingState};

pub const TRACING_CAPABILITY_NAMES: [&str; 3] = [
    "tracing.list_spans",
    "tracing.create_span",
    "tracing.health_check",
];

pub struct TracingPlugin {
    state: TracingState,
}

impl TracingPlugin {
    pub fn new(state: TracingState) -> Self {
        Self { state }
    }
}

impl CapabilityHook for TracingPlugin {
    fn register_capabilities(&self, registry: &CapabilityRegistry) -> CapResult<Vec<String>> {
        let caps: Vec<Arc<dyn Capability>> = vec![
            Arc::new(ListSpansCapability::new(self.state.clone())),
            Arc::new(CreateSpanCapability::new(self.state.clone())),
            Arc::new(HealthCheckCapability::new(self.state.clone())),
        ];
        let mut names = Vec::with_capacity(caps.len());
        for cap in caps {
            let name = cap.name().to_string();
            registry.register(cap);
            names.push(name);
        }
        Ok(names)
    }

    fn capability_names(&self) -> Vec<String> {
        TRACING_CAPABILITY_NAMES
            .iter()
            .map(|s| s.to_string())
            .collect()
    }
}

pub struct ListSpansCapability {
    state: TracingState,
}

impl ListSpansCapability {
    pub fn new(state: TracingState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Capability for ListSpansCapability {
    fn name(&self) -> &'static str {
        "tracing.list_spans"
    }

    fn description(&self) -> &'static str {
        "列出最近的追踪 Span"
    }

    fn schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    fn tags(&self) -> &'static [&'static str] {
        &["tracing", "span", "read"]
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn call(&self, _params: Value) -> CapResult<Value> {
        let spans = self.state.tracer.inner().get_spans();
        Ok(json!({
            "code": 1,
            "msg": "success",
            "data": {
                "spans": spans.iter().map(|s| json!({
                    "trace_id": s.trace_id,
                    "span_id": s.span_id,
                    "operation_name": s.operation_name,
                    "service_name": s.service_name,
                })).collect::<Vec<_>>(),
                "total": spans.len()
            }
        }))
    }
}

pub struct CreateSpanCapability {
    state: TracingState,
}

impl CreateSpanCapability {
    pub fn new(state: TracingState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Capability for CreateSpanCapability {
    fn name(&self) -> &'static str {
        "tracing.create_span"
    }

    fn description(&self) -> &'static str {
        "创建新的追踪 Span"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation_name": {"type": "string"},
                "service_name": {"type": "string"},
                "tags": {"type": "object"}
            },
            "required": ["operation_name"]
        })
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    fn tags(&self) -> &'static [&'static str] {
        &["tracing", "span", "write"]
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn call(&self, params: Value) -> CapResult<Value> {
        let operation_name = params
            .get("operation_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                sz_rust_capability::CapError::ValidationError("operation_name is required".into())
            })?;
        let service_name = params
            .get("service_name")
            .and_then(|v| v.as_str())
            .unwrap_or("sz300");
        let trace_id = SzTracer::generate_trace_id();
        let span_id = SzTracer::generate_span_id();
        let mut span = Span::new(&trace_id, &span_id, operation_name).with_service(service_name);
        if let Some(tags) = params.get("tags").and_then(|v| v.as_object()) {
            for (k, v) in tags {
                if let Some(s) = v.as_str() {
                    span = span.with_tag(k, s);
                }
            }
        }
        span.finish();
        let resp = json!({
            "trace_id": span.trace_id,
            "span_id": span.span_id,
            "operation_name": span.operation_name,
            "service_name": span.service_name,
        });
        self.state.tracer.end_span(span);
        Ok(json!({
            "code": 1,
            "msg": "success",
            "data": resp
        }))
    }
}

pub struct HealthCheckCapability {
    state: TracingState,
}

impl HealthCheckCapability {
    pub fn new(state: TracingState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Capability for HealthCheckCapability {
    fn name(&self) -> &'static str {
        "tracing.health_check"
    }

    fn description(&self) -> &'static str {
        "tracing 服务健康检查"
    }

    fn schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    fn tags(&self) -> &'static [&'static str] {
        &["tracing", "health", "read"]
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn call(&self, _params: Value) -> CapResult<Value> {
        let spans = self.state.tracer.inner().get_spans();
        Ok(json!({
            "code": 1,
            "msg": "success",
            "data": {
                "plugin": "tracing",
                "status": "active",
                "spans_recorded": spans.len(),
                "tracer_type": "InMemoryTracer",
                "version": self.state.version
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracing_capability_names() {
        assert_eq!(TRACING_CAPABILITY_NAMES.len(), 3);
        assert!(TRACING_CAPABILITY_NAMES.contains(&"tracing.list_spans"));
        assert!(TRACING_CAPABILITY_NAMES.contains(&"tracing.create_span"));
        assert!(TRACING_CAPABILITY_NAMES.contains(&"tracing.health_check"));
    }

    #[tokio::test]
    async fn test_register_capabilities() {
        let registry = CapabilityRegistry::new();
        let plugin = TracingPlugin::new(TracingState::default());
        let names = plugin.register_capabilities(&registry).unwrap();
        assert_eq!(names.len(), 3);
    }

    #[tokio::test]
    async fn test_list_spans_capability() {
        let cap = ListSpansCapability::new(TracingState::default());
        let result = cap.call(json!({})).await.unwrap();
        assert_eq!(result["code"], 1);
    }

    #[tokio::test]
    async fn test_create_span_capability() {
        let cap = CreateSpanCapability::new(TracingState::default());
        let result = cap
            .call(json!({"operation_name": "test", "service_name": "svc"}))
            .await
            .unwrap();
        assert_eq!(result["code"], 1);
        assert!(!result["data"]["trace_id"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_health_check_capability() {
        let cap = HealthCheckCapability::new(TracingState::default());
        let result = cap.call(json!({})).await.unwrap();
        assert_eq!(result["code"], 1);
        assert_eq!(result["data"]["plugin"], "tracing");
    }
}
