use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use sz_rust_addons_loader::capability_hook::CapabilityHook;
use sz_rust_capability::{CapResult, Capability, CapabilityRegistry, CapabilitySource};

use crate::OperateState;

pub const OPERATE_CAPABILITY_NAMES: [&str; 2] = ["operate.list_models", "operate.health_check"];

pub struct OperatePlugin {
    state: OperateState,
}

impl OperatePlugin {
    pub fn new(state: OperateState) -> Self {
        Self { state }
    }
}

impl CapabilityHook for OperatePlugin {
    fn register_capabilities(&self, registry: &CapabilityRegistry) -> CapResult<Vec<String>> {
        let caps: Vec<Arc<dyn Capability>> = vec![
            Arc::new(ListModelsCapability::new()),
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
        OPERATE_CAPABILITY_NAMES
            .iter()
            .map(|s| s.to_string())
            .collect()
    }
}

pub struct ListModelsCapability;

impl Default for ListModelsCapability {
    fn default() -> Self {
        Self::new()
    }
}

impl ListModelsCapability {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Capability for ListModelsCapability {
    fn name(&self) -> &'static str {
        "operate.list_models"
    }

    fn description(&self) -> &'static str {
        "列出 operate 插件所有模型及字段元数据"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    fn tags(&self) -> &'static [&'static str] {
        &["operate", "models", "read"]
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn call(&self, _params: Value) -> CapResult<Value> {
        Ok(json!({
            "code": 1,
            "msg": "success",
            "data": {
                "plugin": "operate",
                "models": [
                    {"name": "Customer", "table": "customer", "fields": ["id", "name", "phone", "rentarea_ids", "level_id", "store_id", "company_id", "create_time", "update_time"]},
                    {"name": "Contract", "table": "contract", "fields": ["id", "contract_no", "customer_id", "product_id", "amount", "pay_detail", "start_date", "end_date", "status", "create_time"]},
                    {"name": "Category", "table": "category", "fields": ["id", "name", "pid", "sort", "status"]},
                    {"name": "Rentarea", "table": "rentarea", "fields": ["id", "name", "code", "pid"]},
                    {"name": "Dept", "table": "dept", "fields": ["id", "name", "pid", "sort"]},
                    {"name": "Company", "table": "company", "fields": ["id", "name", "code", "legal_person", "contact_phone"]},
                    {"name": "Store", "table": "store", "fields": ["id", "name", "company_id", "address", "phone"]},
                    {"name": "Level", "table": "level", "fields": ["id", "name", "sort", "discount"]}
                ]
            }
        }))
    }
}

pub struct HealthCheckCapability {
    state: OperateState,
}

impl HealthCheckCapability {
    pub fn new(state: OperateState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Capability for HealthCheckCapability {
    fn name(&self) -> &'static str {
        "operate.health_check"
    }

    fn description(&self) -> &'static str {
        "operate 插件健康检查（实例化模型验证链接）"
    }

    fn schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    fn tags(&self) -> &'static [&'static str] {
        &["operate", "health", "read"]
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn call(&self, _params: Value) -> CapResult<Value> {
        let _customer = crate::Customer::new();
        let _contract = crate::Contract::new();
        let _category = crate::Category::new();
        Ok(json!({
            "code": 1,
            "msg": "success",
            "data": {
                "plugin": "operate",
                "status": "active",
                "models_loaded": self.state.models.len(),
                "version": self.state.version
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operate_capability_names() {
        assert_eq!(OPERATE_CAPABILITY_NAMES.len(), 2);
        assert!(OPERATE_CAPABILITY_NAMES.contains(&"operate.list_models"));
        assert!(OPERATE_CAPABILITY_NAMES.contains(&"operate.health_check"));
    }

    #[tokio::test]
    async fn test_register_capabilities() {
        let registry = CapabilityRegistry::new();
        let plugin = OperatePlugin::new(OperateState::default());
        let names = plugin.register_capabilities(&registry).unwrap();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"operate.list_models".to_string()));
        assert!(names.contains(&"operate.health_check".to_string()));
    }

    #[tokio::test]
    async fn test_list_models_capability() {
        let cap = ListModelsCapability::new();
        let result = cap.call(json!({})).await.unwrap();
        assert_eq!(result["code"], 1);
        let models = result["data"]["models"].as_array().unwrap();
        assert!(models.len() >= 5);
    }

    #[tokio::test]
    async fn test_health_check_capability() {
        let cap = HealthCheckCapability::new(OperateState::default());
        let result = cap.call(json!({})).await.unwrap();
        assert_eq!(result["code"], 1);
        assert_eq!(result["data"]["plugin"], "operate");
        assert_eq!(result["data"]["status"], "active");
    }
}
