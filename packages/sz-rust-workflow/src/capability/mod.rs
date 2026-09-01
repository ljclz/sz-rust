use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use sz_rust_addons_loader::capability_hook::CapabilityHook;
use sz_rust_capability::{CapResult, Capability, CapabilityRegistry, CapabilitySource};

use crate::WorkflowState;

pub const WORKFLOW_CAPABILITY_NAMES: [&str; 3] = [
    "workflow.health_check",
    "workflow.list_definitions",
    "workflow.list_instances",
];

pub struct WorkflowPlugin {
    state: WorkflowState,
}

impl WorkflowPlugin {
    pub fn new(state: WorkflowState) -> Self {
        Self { state }
    }
}

impl CapabilityHook for WorkflowPlugin {
    fn register_capabilities(&self, registry: &CapabilityRegistry) -> CapResult<Vec<String>> {
        let caps: Vec<Arc<dyn Capability>> = vec![
            Arc::new(HealthCheckCapability::new(self.state.clone())),
            Arc::new(ListDefinitionsCapability::new()),
            Arc::new(ListInstancesCapability::new()),
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
        WORKFLOW_CAPABILITY_NAMES
            .iter()
            .map(|s| s.to_string())
            .collect()
    }
}

pub struct HealthCheckCapability {
    state: WorkflowState,
}

impl HealthCheckCapability {
    pub fn new(state: WorkflowState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Capability for HealthCheckCapability {
    fn name(&self) -> &'static str {
        "workflow.health_check"
    }

    fn description(&self) -> &'static str {
        "workflow 引擎健康检查"
    }

    fn schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    fn tags(&self) -> &'static [&'static str] {
        &["workflow", "health", "read"]
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn call(&self, _params: Value) -> CapResult<Value> {
        Ok(json!({
            "code": 1,
            "msg": "success",
            "data": {
                "plugin": "workflow",
                "status": "active",
                "engine": "WorkflowEngine",
                "version": self.state.version
            }
        }))
    }
}

pub struct ListDefinitionsCapability;

impl Default for ListDefinitionsCapability {
    fn default() -> Self {
        Self::new()
    }
}

impl ListDefinitionsCapability {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Capability for ListDefinitionsCapability {
    fn name(&self) -> &'static str {
        "workflow.list_definitions"
    }

    fn description(&self) -> &'static str {
        "列出工作流定义"
    }

    fn schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    fn tags(&self) -> &'static [&'static str] {
        &["workflow", "definition", "read"]
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn call(&self, _params: Value) -> CapResult<Value> {
        Ok(json!({
            "code": 1,
            "msg": "success",
            "data": {
                "definitions": [],
                "total": 0
            }
        }))
    }
}

pub struct ListInstancesCapability;

impl Default for ListInstancesCapability {
    fn default() -> Self {
        Self::new()
    }
}

impl ListInstancesCapability {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Capability for ListInstancesCapability {
    fn name(&self) -> &'static str {
        "workflow.list_instances"
    }

    fn description(&self) -> &'static str {
        "列出工作流实例"
    }

    fn schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    fn tags(&self) -> &'static [&'static str] {
        &["workflow", "instance", "read"]
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn call(&self, _params: Value) -> CapResult<Value> {
        let engine = crate::WorkflowEngine::new(
            crate::WorkflowConfig::default(),
            crate::WorkflowDeps::default_for_test(),
        );
        let page = crate::PageRequest::default();
        let pending_tasks = engine
            .query_tasks("", page)
            .await
            .map(|r| r.total)
            .unwrap_or(0);
        Ok(json!({
            "code": 1,
            "msg": "success",
            "data": {
                "instances": [],
                "total": 0,
                "pending_tasks": pending_tasks
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workflow_capability_names() {
        assert_eq!(WORKFLOW_CAPABILITY_NAMES.len(), 3);
        assert!(WORKFLOW_CAPABILITY_NAMES.contains(&"workflow.health_check"));
        assert!(WORKFLOW_CAPABILITY_NAMES.contains(&"workflow.list_definitions"));
        assert!(WORKFLOW_CAPABILITY_NAMES.contains(&"workflow.list_instances"));
    }

    #[tokio::test]
    async fn test_register_capabilities() {
        let registry = CapabilityRegistry::new();
        let plugin = WorkflowPlugin::new(WorkflowState::default());
        let names = plugin.register_capabilities(&registry).unwrap();
        assert_eq!(names.len(), 3);
    }

    #[tokio::test]
    async fn test_health_check_capability() {
        let cap = HealthCheckCapability::new(WorkflowState::default());
        let result = cap.call(json!({})).await.unwrap();
        assert_eq!(result["code"], 1);
        assert_eq!(result["data"]["plugin"], "workflow");
    }

    #[tokio::test]
    async fn test_list_definitions_capability() {
        let cap = ListDefinitionsCapability::new();
        let result = cap.call(json!({})).await.unwrap();
        assert_eq!(result["code"], 1);
    }

    #[tokio::test]
    async fn test_list_instances_capability() {
        let cap = ListInstancesCapability::new();
        let result = cap.call(json!({})).await.unwrap();
        assert_eq!(result["code"], 1);
    }
}
