//! CRM 插件能力实现模块。
//!
//! 提供 7 个 Capability 实现（4 查询 + 3 写入），对齐 design.md 2.2.2.5 节。

use async_trait::async_trait;
use serde_json::{json, Value};
use sz_rust_capability::{CapError, CapResult, Capability, CapabilitySource};

use crate::controller::contact::ContactController;
use crate::controller::deal::DealController;
use crate::controller::lead::LeadController;
use crate::CrmState;

fn controller_result_to_cap_result(value: Value) -> CapResult<Value> {
    let code = value.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code == 0 {
        Ok(value)
    } else {
        let msg = value
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error")
            .to_string();
        if code == 404 {
            Err(CapError::NotFound(msg))
        } else if code == 422 || code == 400 {
            Err(CapError::ValidationError(msg))
        } else {
            Err(CapError::ExecutionError(msg))
        }
    }
}

// ============================================================================
// 查询类 Capability（4 个）
// ============================================================================

/// 搜索联系人能力。
pub struct SearchContactCapability {
    state: CrmState,
}
impl SearchContactCapability {
    pub fn new(state: CrmState) -> Self {
        Self { state }
    }
}
#[async_trait]
impl Capability for SearchContactCapability {
    fn name(&self) -> &'static str {
        "crm.search_contact"
    }
    fn description(&self) -> &'static str {
        "搜索联系人列表，支持关键词过滤与分页"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "keyword": { "type": "string" },
                "page": { "type": "integer", "minimum": 1, "default": 1 },
                "page_size": { "type": "integer", "minimum": 1, "default": 20 }
            }
        })
    }
    fn tags(&self) -> &[&'static str] {
        &["crm", "contact", "search", "read"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        let page = args.get("page").and_then(|v| v.as_u64()).unwrap_or(1);
        let page_size = args.get("page_size").and_then(|v| v.as_u64()).unwrap_or(20);
        let keyword = args
            .get("keyword")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let result = ContactController::list(&*self.state.contacts, page, page_size, keyword).await;
        controller_result_to_cap_result(result)
    }
}

/// 搜索线索能力。
pub struct SearchLeadCapability {
    state: CrmState,
}
impl SearchLeadCapability {
    pub fn new(state: CrmState) -> Self {
        Self { state }
    }
}
#[async_trait]
impl Capability for SearchLeadCapability {
    fn name(&self) -> &'static str {
        "crm.search_lead"
    }
    fn description(&self) -> &'static str {
        "搜索线索列表，支持关键词、状态过滤与分页"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "keyword": { "type": "string" },
                "status": { "type": "string" },
                "page": { "type": "integer", "minimum": 1, "default": 1 },
                "page_size": { "type": "integer", "minimum": 1, "default": 20 }
            }
        })
    }
    fn tags(&self) -> &[&'static str] {
        &["crm", "lead", "search", "read"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        let page = args.get("page").and_then(|v| v.as_u64()).unwrap_or(1);
        let page_size = args.get("page_size").and_then(|v| v.as_u64()).unwrap_or(20);
        let keyword = args
            .get("keyword")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let status = args
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let result =
            LeadController::list(&*self.state.leads, page, page_size, keyword, status).await;
        controller_result_to_cap_result(result)
    }
}

/// 搜索商机能力。
pub struct SearchDealCapability {
    state: CrmState,
}
impl SearchDealCapability {
    pub fn new(state: CrmState) -> Self {
        Self { state }
    }
}
#[async_trait]
impl Capability for SearchDealCapability {
    fn name(&self) -> &'static str {
        "crm.search_deal"
    }
    fn description(&self) -> &'static str {
        "搜索商机列表，支持关键词、阶段过滤与分页"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "keyword": { "type": "string" },
                "stage": { "type": "string" },
                "page": { "type": "integer", "minimum": 1, "default": 1 },
                "page_size": { "type": "integer", "minimum": 1, "default": 20 }
            }
        })
    }
    fn tags(&self) -> &[&'static str] {
        &["crm", "deal", "search", "read"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        let page = args.get("page").and_then(|v| v.as_u64()).unwrap_or(1);
        let page_size = args.get("page_size").and_then(|v| v.as_u64()).unwrap_or(20);
        let keyword = args
            .get("keyword")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let stage = args
            .get("stage")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let result =
            DealController::list(&*self.state.deals, page, page_size, keyword, stage).await;
        controller_result_to_cap_result(result)
    }
}

/// 查询销售漏斗能力。返回各阶段分组统计。
pub struct QueryPipelineCapability {
    state: CrmState,
}
impl QueryPipelineCapability {
    pub fn new(state: CrmState) -> Self {
        Self { state }
    }
}
#[async_trait]
impl Capability for QueryPipelineCapability {
    fn name(&self) -> &'static str {
        "crm.query_pipeline"
    }
    fn description(&self) -> &'static str {
        "查询销售漏斗，返回各阶段分组统计（数量+金额）"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    fn tags(&self) -> &[&'static str] {
        &["crm", "deal", "pipeline", "read"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, _args: Value) -> CapResult<Value> {
        let result = DealController::pipeline(&*self.state.deals).await;
        controller_result_to_cap_result(result)
    }
}

// ============================================================================
// 写入类 Capability（3 个）
// ============================================================================

/// 创建联系人能力。校验 name 必填。
pub struct CreateContactCapability {
    state: CrmState,
}
impl CreateContactCapability {
    pub fn new(state: CrmState) -> Self {
        Self { state }
    }
}
#[async_trait]
impl Capability for CreateContactCapability {
    fn name(&self) -> &'static str {
        "crm.create_contact"
    }
    fn description(&self) -> &'static str {
        "创建联系人，name 必填"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "minLength": 1 },
                "phone": { "type": "string" },
                "email": { "type": "string" }
            },
            "required": ["name"]
        })
    }
    fn tags(&self) -> &[&'static str] {
        &["crm", "contact", "create", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name.is_empty() {
            return Err(CapError::ValidationError("name 必填".to_string()));
        }
        let mut body = args;
        if body.get("id").is_none() {
            body["id"] = json!(0);
        }
        let result = ContactController::create(&*self.state.contacts, body).await;
        controller_result_to_cap_result(result)
    }
}

/// 线索转化能力。需要人工确认。
pub struct ConvertLeadCapability {
    state: CrmState,
}
impl ConvertLeadCapability {
    pub fn new(state: CrmState) -> Self {
        Self { state }
    }
}
#[async_trait]
impl Capability for ConvertLeadCapability {
    fn name(&self) -> &'static str {
        "crm.convert_lead"
    }
    fn description(&self) -> &'static str {
        "线索转化：创建关联 Contact 和 Deal，三步原子操作"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "description": "线索 ID" }
            },
            "required": ["id"]
        })
    }
    fn tags(&self) -> &[&'static str] {
        &["crm", "lead", "convert", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    fn requires_confirmation(&self) -> bool {
        true
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        let id = args
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CapError::ValidationError("id is required".to_string()))?;
        let result = LeadController::convert(
            &*self.state.leads,
            &*self.state.contacts,
            &*self.state.deals,
            id,
        )
        .await;
        controller_result_to_cap_result(result)
    }
}

/// 商机阶段更新能力。需要人工确认。
pub struct UpdateDealStageCapability {
    state: CrmState,
}
impl UpdateDealStageCapability {
    pub fn new(state: CrmState) -> Self {
        Self { state }
    }
}
#[async_trait]
impl Capability for UpdateDealStageCapability {
    fn name(&self) -> &'static str {
        "crm.update_deal_stage"
    }
    fn description(&self) -> &'static str {
        "更新商机阶段，校验合法流转表"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer" },
                "new_stage": { "type": "string", "enum": ["initial", "requirement_confirmed", "quoted", "negotiating", "won", "lost"] }
            },
            "required": ["id", "new_stage"]
        })
    }
    fn tags(&self) -> &[&'static str] {
        &["crm", "deal", "update", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    fn requires_confirmation(&self) -> bool {
        true
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        let id = args
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CapError::ValidationError("id is required".to_string()))?;
        let new_stage = args
            .get("new_stage")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ValidationError("new_stage is required".to_string()))?;
        let result = DealController::update_stage(&*self.state.deals, id, new_stage).await;
        controller_result_to_cap_result(result)
    }
}

// ============================================================================
// CrmPlugin — CapabilityHook 实现
// ============================================================================

use std::sync::Arc;
use sz_rust_addons_loader::CapabilityHook;
use sz_rust_capability::CapabilityRegistry;

/// CRM 插件 CapabilityHook 实现。
pub struct CrmPlugin {
    state: CrmState,
}
impl CrmPlugin {
    pub fn new(state: CrmState) -> Self {
        Self { state }
    }
}

pub const CRM_CAPABILITY_NAMES: [&str; 7] = [
    "crm.search_contact",
    "crm.create_contact",
    "crm.search_lead",
    "crm.convert_lead",
    "crm.search_deal",
    "crm.update_deal_stage",
    "crm.query_pipeline",
];

impl CapabilityHook for CrmPlugin {
    fn register_capabilities(&self, registry: &CapabilityRegistry) -> CapResult<Vec<String>> {
        let caps: Vec<Arc<dyn Capability>> = vec![
            Arc::new(SearchContactCapability::new(self.state.clone())),
            Arc::new(CreateContactCapability::new(self.state.clone())),
            Arc::new(SearchLeadCapability::new(self.state.clone())),
            Arc::new(ConvertLeadCapability::new(self.state.clone())),
            Arc::new(SearchDealCapability::new(self.state.clone())),
            Arc::new(UpdateDealStageCapability::new(self.state.clone())),
            Arc::new(QueryPipelineCapability::new(self.state.clone())),
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
        CRM_CAPABILITY_NAMES.iter().map(|s| s.to_string()).collect()
    }
}

// ============================================================================
// 测试模块
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::contact::Contact;
    use crate::model::deal::Deal;
    use crate::model::lead::Lead;
    use sz_rust_core::orm::repository::Repository;

    fn test_state() -> CrmState {
        CrmState::default()
    }

    // --- 查询类测试 ---

    #[tokio::test]
    async fn search_contact_capability_returns_results() {
        let state = test_state();
        state
            .contacts
            .save(Contact {
                id: 1,
                name: "Alice".to_string(),
                ..Default::default()
            })
            .unwrap();
        let cap = SearchContactCapability::new(state.clone());
        let result = cap.call(json!({})).await.unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["total"], 1);
    }

    #[tokio::test]
    async fn create_contact_capability_validates_required() {
        let state = test_state();
        let cap = CreateContactCapability::new(state);
        let result = cap.call(json!({"name": ""})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn search_lead_capability_filters_by_status() {
        let state = test_state();
        state
            .leads
            .save(Lead {
                id: 1,
                name: "L1".to_string(),
                status: "prospect".to_string(),
                ..Default::default()
            })
            .unwrap();
        state
            .leads
            .save(Lead {
                id: 2,
                name: "L2".to_string(),
                status: "converted".to_string(),
                ..Default::default()
            })
            .unwrap();
        let cap = SearchLeadCapability::new(state.clone());
        let result = cap.call(json!({"status": "converted"})).await.unwrap();
        assert_eq!(result["code"], 0);
    }

    #[tokio::test]
    async fn search_deal_capability_filters_by_stage() {
        let state = test_state();
        state
            .deals
            .save(Deal {
                id: 1,
                name: "D1".to_string(),
                stage: "initial".to_string(),
                ..Default::default()
            })
            .unwrap();
        state
            .deals
            .save(Deal {
                id: 2,
                name: "D2".to_string(),
                stage: "won".to_string(),
                ..Default::default()
            })
            .unwrap();
        let cap = SearchDealCapability::new(state.clone());
        let result = cap.call(json!({"stage": "won"})).await.unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["total"], 1);
    }

    #[tokio::test]
    async fn query_pipeline_capability_returns_grouped_data() {
        let state = test_state();
        state
            .deals
            .save(Deal {
                id: 1,
                name: "D1".to_string(),
                stage: "initial".to_string(),
                amount: 1000.0,
                ..Default::default()
            })
            .unwrap();
        let cap = QueryPipelineCapability::new(state.clone());
        let result = cap.call(json!({})).await.unwrap();
        assert_eq!(result["code"], 0);
        let pipeline = result["data"]["pipeline"].as_array().unwrap();
        assert_eq!(pipeline.len(), 6);
    }

    // --- 转化类测试 ---

    #[tokio::test]
    async fn convert_lead_capability_creates_contact_and_deal() {
        let state = test_state();
        state
            .leads
            .save(Lead {
                id: 1,
                name: "HotLead".to_string(),
                status: "prospect".to_string(),
                phone: "13800138000".to_string(),
                company: "Acme".to_string(),
                estimated_amount: 50000.0,
                ..Default::default()
            })
            .unwrap();
        let cap = ConvertLeadCapability::new(state.clone());
        let result = cap.call(json!({"id": 1})).await.unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["lead"]["status"], "converted");
        assert_eq!(result["data"]["contact"]["name"], "HotLead");
        assert!(result["data"]["deal"]["name"]
            .as_str()
            .unwrap()
            .contains("Acme"));
    }

    #[tokio::test]
    async fn convert_lead_capability_rejects_nonexistent() {
        let state = test_state();
        let cap = ConvertLeadCapability::new(state);
        let result = cap.call(json!({"id": 999})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn convert_lead_capability_rejects_already_converted() {
        let state = test_state();
        state
            .leads
            .save(Lead {
                id: 1,
                name: "L".to_string(),
                status: "converted".to_string(),
                ..Default::default()
            })
            .unwrap();
        let cap = ConvertLeadCapability::new(state);
        let result = cap.call(json!({"id": 1})).await;
        assert!(result.is_err());
    }

    #[test]
    fn convert_lead_requires_confirmation_true() {
        let state = test_state();
        let cap = ConvertLeadCapability::new(state);
        assert!(cap.requires_confirmation());
    }

    // --- 阶段更新类测试 ---

    #[tokio::test]
    async fn update_deal_stage_capability_validates_transition() {
        let state = test_state();
        state
            .deals
            .save(Deal {
                id: 1,
                name: "D".to_string(),
                stage: "initial".to_string(),
                ..Default::default()
            })
            .unwrap();
        let cap = UpdateDealStageCapability::new(state.clone());
        let result = cap
            .call(json!({"id": 1, "new_stage": "requirement_confirmed"}))
            .await
            .unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(result["data"]["stage"], "requirement_confirmed");
    }

    #[tokio::test]
    async fn update_deal_stage_rejects_backward_from_won() {
        let state = test_state();
        state
            .deals
            .save(Deal {
                id: 1,
                name: "D".to_string(),
                stage: "won".to_string(),
                ..Default::default()
            })
            .unwrap();
        let cap = UpdateDealStageCapability::new(state);
        let result = cap.call(json!({"id": 1, "new_stage": "negotiating"})).await;
        assert!(result.is_err());
    }

    #[test]
    fn update_deal_stage_requires_confirmation_true() {
        let state = test_state();
        let cap = UpdateDealStageCapability::new(state);
        assert!(cap.requires_confirmation());
    }

    // --- Hook 类测试 ---

    #[test]
    fn crm_plugin_registers_7_capabilities() {
        let state = test_state();
        let plugin = CrmPlugin::new(state);
        let registry = CapabilityRegistry::new();
        let names = plugin.register_capabilities(&registry).unwrap();
        assert_eq!(names.len(), 7);
        assert_eq!(registry.len(), 7);
    }

    #[test]
    fn crm_capabilities_have_correct_prefix() {
        let state = test_state();
        let plugin = CrmPlugin::new(state);
        let names = plugin.capability_names();
        for name in &names {
            assert!(name.starts_with("crm."), "能力名 {name} 不以 crm. 开头");
        }
    }
}
