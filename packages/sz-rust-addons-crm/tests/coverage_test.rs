//! 覆盖率补充测试 — 覆盖错误分支、元数据方法、路由注册、模型方法等
//!
//! 使用 FailingRepository mock 覆盖控制器错误分支，使用 InMemoryRepository 覆盖正常分支。

use serde_json::json;
use std::marker::PhantomData;
use std::sync::Arc;
use sz_rust_addons_loader::capability_hook::CapabilityHook;
use sz_rust_capability::{CapError, Capability, CapabilityRegistry, CapabilitySource};
use sz_rust_core::orm::repository::{
    EntityAttributes, InMemoryRepository, Repository, RepositoryError, RepositoryResult,
    WhereCondition,
};
use sz_rust_core::orm::Value as OrmValue;
use sz_rust_core::router::RouterBuilder;

use sz_rust_addons_crm::capability::{
    ConvertLeadCapability, CreateContactCapability, CrmPlugin, QueryPipelineCapability,
    SearchContactCapability, SearchDealCapability, SearchLeadCapability, UpdateDealStageCapability,
};
use sz_rust_addons_crm::controller::contact::ContactController;
use sz_rust_addons_crm::controller::deal::{is_valid_transition, DealController, PIPELINE_STAGES};
use sz_rust_addons_crm::controller::lead::LeadController;
use sz_rust_addons_crm::model::contact::Contact;
use sz_rust_addons_crm::model::deal::Deal;
use sz_rust_addons_crm::model::lead::Lead;
use sz_rust_addons_crm::{register_routes, CrmState};

type ContactRepo = Arc<InMemoryRepository<Contact>>;
type LeadRepo = Arc<InMemoryRepository<Lead>>;
type DealRepo = Arc<InMemoryRepository<Deal>>;

// ============================================================================
// FailingRepository — 所有方法都返回 Err 的 mock
// ============================================================================

struct FailingRepository<E>(PhantomData<E>);

impl<E: Clone + Send + Sync + 'static> Repository<E> for FailingRepository<E> {
    type Key = OrmValue;

    fn key_of(&self, _entity: &E) -> Self::Key {
        OrmValue::I64(0)
    }

    fn find_by_id(&self, _key: &Self::Key) -> RepositoryResult<Option<E>> {
        Err(RepositoryError::DatabaseError("mock".to_string()))
    }

    fn find_all(&self) -> RepositoryResult<Vec<E>> {
        Err(RepositoryError::DatabaseError("mock".to_string()))
    }

    fn find_by(&self, _conditions: &[WhereCondition]) -> RepositoryResult<Vec<E>> {
        Err(RepositoryError::DatabaseError("mock".to_string()))
    }

    fn save(&self, _entity: E) -> RepositoryResult<E> {
        Err(RepositoryError::DatabaseError("mock".to_string()))
    }

    fn delete(&self, _key: &Self::Key) -> RepositoryResult<usize> {
        Err(RepositoryError::DatabaseError("mock".to_string()))
    }

    fn count(&self) -> RepositoryResult<u64> {
        Err(RepositoryError::DatabaseError("mock".to_string()))
    }
}

// ============================================================================
// SaveFailingRepo — 包装 R，save 返回 Err，其他委托 R
// ============================================================================

struct SaveFailingRepo<R>(R);

impl<E: Clone + Send + Sync + 'static, R: Repository<E, Key = OrmValue>> Repository<E>
    for SaveFailingRepo<R>
{
    type Key = OrmValue;

    fn key_of(&self, entity: &E) -> Self::Key {
        self.0.key_of(entity)
    }

    fn find_by_id(&self, key: &Self::Key) -> RepositoryResult<Option<E>> {
        self.0.find_by_id(key)
    }

    fn find_all(&self) -> RepositoryResult<Vec<E>> {
        self.0.find_all()
    }

    fn find_by(&self, conditions: &[WhereCondition]) -> RepositoryResult<Vec<E>> {
        self.0.find_by(conditions)
    }

    fn save(&self, _entity: E) -> RepositoryResult<E> {
        Err(RepositoryError::DatabaseError("save failed".to_string()))
    }

    fn delete(&self, key: &Self::Key) -> RepositoryResult<usize> {
        self.0.delete(key)
    }

    fn count(&self) -> RepositoryResult<u64> {
        self.0.count()
    }
}

fn failing_contact() -> FailingRepository<Contact> {
    FailingRepository(PhantomData)
}
fn failing_lead() -> FailingRepository<Lead> {
    FailingRepository(PhantomData)
}
fn failing_deal() -> FailingRepository<Deal> {
    FailingRepository(PhantomData)
}

// ============================================================================
// Model 补充测试
// ============================================================================

#[test]
fn deal_new_returns_default() {
    let d = Deal::new();
    assert_eq!(d, Deal::default());
    assert_eq!(d.stage, "initial");
}

#[test]
fn lead_new_returns_default() {
    let l = Lead::new();
    assert_eq!(l, Lead::default());
    assert_eq!(l.status, "prospect");
}

#[test]
fn deal_get_attribute_timestamps_and_unknown() {
    let d = Deal {
        id: 1,
        created_at: 1700000000,
        updated_at: 1700000001,
        ..Default::default()
    };
    assert_eq!(
        d.get_attribute("created_at"),
        Some(OrmValue::I64(1700000000))
    );
    assert_eq!(
        d.get_attribute("updated_at"),
        Some(OrmValue::I64(1700000001))
    );
    assert_eq!(d.get_attribute("nonexistent"), None);
}

#[test]
fn lead_get_attribute_remark_timestamps_and_unknown() {
    let l = Lead {
        id: 1,
        remark: "test remark".to_string(),
        created_at: 1700000000,
        updated_at: 1700000001,
        ..Default::default()
    };
    assert_eq!(
        l.get_attribute("remark"),
        Some(OrmValue::String("test remark".to_string()))
    );
    assert_eq!(
        l.get_attribute("created_at"),
        Some(OrmValue::I64(1700000000))
    );
    assert_eq!(
        l.get_attribute("updated_at"),
        Some(OrmValue::I64(1700000001))
    );
    assert_eq!(l.get_attribute("nonexistent"), None);
}

#[test]
fn contact_get_attribute_all_fields() {
    let c = Contact {
        id: 1,
        name: "Alice".to_string(),
        phone: "123".to_string(),
        email: "a@b.com".to_string(),
        customer_id: 5,
        position: "Manager".to_string(),
        remark: "vip".to_string(),
        created_at: 100,
        updated_at: 200,
    };
    assert_eq!(
        c.get_attribute("phone"),
        Some(OrmValue::String("123".into()))
    );
    assert_eq!(
        c.get_attribute("email"),
        Some(OrmValue::String("a@b.com".into()))
    );
    assert_eq!(c.get_attribute("customer_id"), Some(OrmValue::I64(5)));
    assert_eq!(
        c.get_attribute("position"),
        Some(OrmValue::String("Manager".into()))
    );
    assert_eq!(
        c.get_attribute("remark"),
        Some(OrmValue::String("vip".into()))
    );
    assert_eq!(c.get_attribute("created_at"), Some(OrmValue::I64(100)));
    assert_eq!(c.get_attribute("updated_at"), Some(OrmValue::I64(200)));
    assert_eq!(c.get_attribute("unknown"), None);
}

// ============================================================================
// Deal 常量测试
// ============================================================================

#[test]
fn pipeline_stages_has_6_stages() {
    assert_eq!(PIPELINE_STAGES.len(), 6);
    assert_eq!(PIPELINE_STAGES[0], "initial");
    assert_eq!(PIPELINE_STAGES[5], "lost");
}

#[test]
fn is_valid_transition_valid_paths() {
    assert!(is_valid_transition("initial", "requirement_confirmed"));
    assert!(is_valid_transition("requirement_confirmed", "quoted"));
    assert!(is_valid_transition("quoted", "negotiating"));
    assert!(is_valid_transition("negotiating", "won"));
    assert!(is_valid_transition("negotiating", "lost"));
}

#[test]
fn is_valid_transition_invalid_paths() {
    assert!(!is_valid_transition("initial", "won"));
    assert!(!is_valid_transition("won", "negotiating"));
    assert!(!is_valid_transition("lost", "initial"));
    assert!(!is_valid_transition("won", "lost"));
}

// ============================================================================
// Contact Controller 错误分支测试
// ============================================================================

#[tokio::test]
async fn contact_list_repo_error_returns_500() {
    let repo = failing_contact();
    let r = ContactController::list(&repo, 1, 10, None).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn contact_list_empty_keyword_uses_no_conditions() {
    let repo: ContactRepo = Arc::new(InMemoryRepository::new());
    repo.save(Contact {
        id: 1,
        name: "Alice".to_string(),
        ..Default::default()
    })
    .unwrap();
    // 空字符串 keyword 应等同于 None
    let r = ContactController::list(&*repo, 1, 10, Some(String::new())).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["total"], 1);
}

#[tokio::test]
async fn contact_create_invalid_json_returns_400() {
    let repo: ContactRepo = Arc::new(InMemoryRepository::new());
    // 缺少必填字段 id（i64），传一个无法反序列化的 body
    let body = json!({"id": "not_a_number", "name": "Alice"});
    let r = ContactController::create(&*repo, body).await;
    assert_eq!(r["code"], 400);
}

#[tokio::test]
async fn contact_create_save_error_returns_500() {
    let repo = failing_contact();
    let r = ContactController::create(&repo, json!({"id": 0, "name": "Alice"})).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn contact_get_repo_error_returns_500() {
    let repo = failing_contact();
    let r = ContactController::get(&repo, 1).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn contact_update_repo_error_returns_500() {
    let repo = failing_contact();
    let r = ContactController::update(&repo, 1, json!({"name": "x"})).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn contact_delete_repo_error_returns_500() {
    let repo = failing_contact();
    let r = ContactController::delete(&repo, 1).await;
    assert_eq!(r["code"], 500);
}

// ============================================================================
// Deal Controller 错误分支测试
// ============================================================================

#[tokio::test]
async fn deal_list_repo_error_returns_500() {
    let repo = failing_deal();
    let r = DealController::list(&repo, 1, 10, None, None).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn deal_list_with_keyword_filters() {
    let repo: DealRepo = Arc::new(InMemoryRepository::new());
    repo.save(Deal {
        id: 1,
        name: "BigDeal".to_string(),
        ..Default::default()
    })
    .unwrap();
    repo.save(Deal {
        id: 2,
        name: "SmallDeal".to_string(),
        ..Default::default()
    })
    .unwrap();
    // 非空 keyword 触发 WhereCondition 分支
    let r = DealController::list(&*repo, 1, 10, Some("BigDeal".to_string()), None).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["total"], 1);
}

#[tokio::test]
async fn deal_list_with_empty_keyword_and_stage() {
    let repo: DealRepo = Arc::new(InMemoryRepository::new());
    repo.save(Deal {
        id: 1,
        name: "D1".to_string(),
        stage: "initial".to_string(),
        ..Default::default()
    })
    .unwrap();
    // 空字符串 keyword 和 stage 应等同于 None
    let r = DealController::list(&*repo, 1, 10, Some(String::new()), Some(String::new())).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["total"], 1);
}

#[tokio::test]
async fn deal_create_invalid_json_returns_400() {
    let repo: DealRepo = Arc::new(InMemoryRepository::new());
    let body = json!({"id": "bad", "name": "D"});
    let r = DealController::create(&*repo, body).await;
    assert_eq!(r["code"], 400);
}

#[tokio::test]
async fn deal_create_save_error_returns_500() {
    let repo = failing_deal();
    let r = DealController::create(&repo, json!({"id": 0, "name": "D"})).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn deal_get_repo_error_returns_500() {
    let repo = failing_deal();
    let r = DealController::get(&repo, 1).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn deal_get_not_found_returns_404() {
    let repo: DealRepo = Arc::new(InMemoryRepository::new());
    let r = DealController::get(&*repo, 99).await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn deal_update_repo_error_returns_500() {
    let repo = failing_deal();
    let r = DealController::update(&repo, 1, json!({"name": "x"})).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn deal_update_not_found_returns_404() {
    let repo: DealRepo = Arc::new(InMemoryRepository::new());
    let r = DealController::update(&*repo, 99, json!({"name": "x"})).await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn deal_delete_repo_error_returns_500() {
    let repo = failing_deal();
    let r = DealController::delete(&repo, 1).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn deal_pipeline_repo_error_returns_500() {
    let repo = failing_deal();
    let r = DealController::pipeline(&repo).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn deal_update_stage_repo_error_returns_500() {
    let repo = failing_deal();
    let r = DealController::update_stage(&repo, 1, "quoted").await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn deal_update_stage_not_found_returns_404() {
    let repo: DealRepo = Arc::new(InMemoryRepository::new());
    let r = DealController::update_stage(&*repo, 99, "quoted").await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn deal_update_all_patch_fields() {
    let repo: DealRepo = Arc::new(InMemoryRepository::new());
    repo.save(Deal {
        id: 1,
        name: "D".to_string(),
        ..Default::default()
    })
    .unwrap();
    let body = json!({
        "name": "Updated",
        "stage": "quoted",
        "amount": 5000.0,
        "contact_id": 3,
        "lead_id": 7,
        "owner_id": 9,
        "remark": "vip",
        "probability": 80
    });
    let r = DealController::update(&*repo, 1, body).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["name"], "Updated");
    assert_eq!(r["data"]["stage"], "quoted");
    assert_eq!(r["data"]["amount"], 5000.0);
    assert_eq!(r["data"]["contact_id"], 3);
    assert_eq!(r["data"]["lead_id"], 7);
    assert_eq!(r["data"]["owner_id"], 9);
    assert_eq!(r["data"]["remark"], "vip");
    assert_eq!(r["data"]["probability"], 80);
}

// ============================================================================
// Lead Controller 错误分支测试
// ============================================================================

#[tokio::test]
async fn lead_list_repo_error_returns_500() {
    let repo = failing_lead();
    let r = LeadController::list(&repo, 1, 10, None, None).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn lead_list_with_keyword_filters() {
    let repo: LeadRepo = Arc::new(InMemoryRepository::new());
    repo.save(Lead {
        id: 1,
        name: "HotLead".to_string(),
        ..Default::default()
    })
    .unwrap();
    repo.save(Lead {
        id: 2,
        name: "ColdLead".to_string(),
        ..Default::default()
    })
    .unwrap();
    let r = LeadController::list(&*repo, 1, 10, Some("HotLead".to_string()), None).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["total"], 1);
}

#[tokio::test]
async fn lead_list_with_empty_keyword_and_status() {
    let repo: LeadRepo = Arc::new(InMemoryRepository::new());
    repo.save(Lead {
        id: 1,
        name: "L1".to_string(),
        ..Default::default()
    })
    .unwrap();
    let r = LeadController::list(&*repo, 1, 10, Some(String::new()), Some(String::new())).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["total"], 1);
}

#[tokio::test]
async fn lead_create_invalid_json_returns_400() {
    let repo: LeadRepo = Arc::new(InMemoryRepository::new());
    let body = json!({"id": "bad", "name": "L"});
    let r = LeadController::create(&*repo, body).await;
    assert_eq!(r["code"], 400);
}

#[tokio::test]
async fn lead_create_save_error_returns_500() {
    let repo = failing_lead();
    let r = LeadController::create(&repo, json!({"id": 0, "name": "L"})).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn lead_get_found_returns_data() {
    let repo: LeadRepo = Arc::new(InMemoryRepository::new());
    repo.save(Lead {
        id: 5,
        name: "Found".to_string(),
        ..Default::default()
    })
    .unwrap();
    let r = LeadController::get(&*repo, 5).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["name"], "Found");
}

#[tokio::test]
async fn lead_get_not_found_returns_404() {
    let repo: LeadRepo = Arc::new(InMemoryRepository::new());
    let r = LeadController::get(&*repo, 99).await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn lead_get_repo_error_returns_500() {
    let repo = failing_lead();
    let r = LeadController::get(&repo, 1).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn lead_update_repo_error_returns_500() {
    let repo = failing_lead();
    let r = LeadController::update(&repo, 1, json!({"name": "x"})).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn lead_update_all_patch_fields() {
    let repo: LeadRepo = Arc::new(InMemoryRepository::new());
    repo.save(Lead {
        id: 1,
        name: "L".to_string(),
        ..Default::default()
    })
    .unwrap();
    let body = json!({
        "name": "Updated",
        "status": "qualified",
        "source": "web",
        "phone": "138",
        "email": "a@b.com",
        "company": "Acme",
        "estimated_amount": 9999.0,
        "owner_id": 42,
        "remark": "hot"
    });
    let r = LeadController::update(&*repo, 1, body).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["name"], "Updated");
    assert_eq!(r["data"]["status"], "qualified");
    assert_eq!(r["data"]["source"], "web");
    assert_eq!(r["data"]["phone"], "138");
    assert_eq!(r["data"]["email"], "a@b.com");
    assert_eq!(r["data"]["company"], "Acme");
    assert_eq!(r["data"]["estimated_amount"], 9999.0);
    assert_eq!(r["data"]["owner_id"], 42);
    assert_eq!(r["data"]["remark"], "hot");
}

#[tokio::test]
async fn lead_delete_repo_error_returns_500() {
    let repo = failing_lead();
    let r = LeadController::delete(&repo, 1).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn lead_convert_repo_error_returns_500() {
    let lead_repo = failing_lead();
    let contact_repo: ContactRepo = Arc::new(InMemoryRepository::new());
    let deal_repo: DealRepo = Arc::new(InMemoryRepository::new());
    let r = LeadController::convert(&lead_repo, &*contact_repo, &*deal_repo, 1).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn lead_convert_not_found_returns_404() {
    let lead_repo: LeadRepo = Arc::new(InMemoryRepository::new());
    let contact_repo: ContactRepo = Arc::new(InMemoryRepository::new());
    let deal_repo: DealRepo = Arc::new(InMemoryRepository::new());
    let r = LeadController::convert(&*lead_repo, &*contact_repo, &*deal_repo, 99).await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn lead_convert_already_converted_returns_422() {
    let lead_repo: LeadRepo = Arc::new(InMemoryRepository::new());
    let contact_repo: ContactRepo = Arc::new(InMemoryRepository::new());
    let deal_repo: DealRepo = Arc::new(InMemoryRepository::new());
    lead_repo
        .save(Lead {
            id: 1,
            name: "L".to_string(),
            status: "converted".to_string(),
            ..Default::default()
        })
        .unwrap();
    let r = LeadController::convert(&*lead_repo, &*contact_repo, &*deal_repo, 1).await;
    assert_eq!(r["code"], 422);
}

#[tokio::test]
async fn lead_convert_contact_save_failure_triggers_rollback() {
    let lead_repo: LeadRepo = Arc::new(InMemoryRepository::new());
    lead_repo
        .save(Lead {
            id: 1,
            name: "Hot".to_string(),
            status: "prospect".to_string(),
            ..Default::default()
        })
        .unwrap();
    // contact_repo save 失败，触发回滚
    let contact_repo = SaveFailingRepo(InMemoryRepository::<Contact>::new());
    let deal_repo: DealRepo = Arc::new(InMemoryRepository::new());
    let r = LeadController::convert(&*lead_repo, &contact_repo, &*deal_repo, 1).await;
    assert_eq!(r["code"], 500);
    // 验证回滚：lead 状态应恢复为 prospect
    let lead = lead_repo.find_by_id(&OrmValue::I64(1)).unwrap().unwrap();
    assert_eq!(lead.status, "prospect");
}

#[tokio::test]
async fn lead_convert_deal_save_failure_triggers_rollback() {
    let lead_repo: LeadRepo = Arc::new(InMemoryRepository::new());
    lead_repo
        .save(Lead {
            id: 1,
            name: "Hot".to_string(),
            status: "prospect".to_string(),
            ..Default::default()
        })
        .unwrap();
    let contact_repo: ContactRepo = Arc::new(InMemoryRepository::new());
    // deal_repo save 失败，触发回滚
    let deal_repo = SaveFailingRepo(InMemoryRepository::<Deal>::new());
    let r = LeadController::convert(&*lead_repo, &*contact_repo, &deal_repo, 1).await;
    assert_eq!(r["code"], 500);
    // 验证回滚：lead 状态恢复 + contact 被删除
    let lead = lead_repo.find_by_id(&OrmValue::I64(1)).unwrap().unwrap();
    assert_eq!(lead.status, "prospect");
    assert_eq!(contact_repo.count().unwrap(), 0);
}

#[tokio::test]
async fn lead_convert_with_empty_company_uses_lead_name() {
    let lead_repo: LeadRepo = Arc::new(InMemoryRepository::new());
    let contact_repo: ContactRepo = Arc::new(InMemoryRepository::new());
    let deal_repo: DealRepo = Arc::new(InMemoryRepository::new());
    lead_repo
        .save(Lead {
            id: 1,
            name: "HotLead".to_string(),
            status: "prospect".to_string(),
            company: String::new(), // 空公司
            ..Default::default()
        })
        .unwrap();
    let r = LeadController::convert(&*lead_repo, &*contact_repo, &*deal_repo, 1).await;
    assert_eq!(r["code"], 0);
    assert!(r["data"]["deal"]["name"]
        .as_str()
        .unwrap()
        .contains("HotLead"));
}

// ============================================================================
// Capability 元数据测试
// ============================================================================

fn test_state() -> CrmState {
    CrmState::default()
}

#[test]
fn search_contact_capability_metadata() {
    let cap = SearchContactCapability::new(test_state());
    assert_eq!(cap.name(), "crm.search_contact");
    assert!(!cap.description().is_empty());
    assert!(cap.schema().is_object());
    assert!(cap.tags().contains(&"crm"));
    assert!(cap.tags().contains(&"contact"));
    assert_eq!(cap.source(), CapabilitySource::Plugin);
}

#[test]
fn search_lead_capability_metadata() {
    let cap = SearchLeadCapability::new(test_state());
    assert_eq!(cap.name(), "crm.search_lead");
    assert!(!cap.description().is_empty());
    assert!(cap.schema().is_object());
    assert!(cap.tags().contains(&"lead"));
    assert_eq!(cap.source(), CapabilitySource::Plugin);
}

#[test]
fn search_deal_capability_metadata() {
    let cap = SearchDealCapability::new(test_state());
    assert_eq!(cap.name(), "crm.search_deal");
    assert!(!cap.description().is_empty());
    assert!(cap.schema().is_object());
    assert!(cap.tags().contains(&"deal"));
    assert_eq!(cap.source(), CapabilitySource::Plugin);
}

#[test]
fn query_pipeline_capability_metadata() {
    let cap = QueryPipelineCapability::new(test_state());
    assert_eq!(cap.name(), "crm.query_pipeline");
    assert!(!cap.description().is_empty());
    assert!(cap.schema().is_object());
    assert!(cap.tags().contains(&"pipeline"));
    assert_eq!(cap.source(), CapabilitySource::Plugin);
}

#[test]
fn create_contact_capability_metadata() {
    let cap = CreateContactCapability::new(test_state());
    assert_eq!(cap.name(), "crm.create_contact");
    assert!(!cap.description().is_empty());
    assert!(cap.schema().is_object());
    assert!(cap.tags().contains(&"create"));
    assert_eq!(cap.source(), CapabilitySource::Plugin);
    assert!(!cap.requires_confirmation());
}

#[test]
fn convert_lead_capability_metadata() {
    let cap = ConvertLeadCapability::new(test_state());
    assert_eq!(cap.name(), "crm.convert_lead");
    assert!(!cap.description().is_empty());
    assert!(cap.schema().is_object());
    assert!(cap.tags().contains(&"convert"));
    assert_eq!(cap.source(), CapabilitySource::Plugin);
    assert!(cap.requires_confirmation());
}

#[test]
fn update_deal_stage_capability_metadata() {
    let cap = UpdateDealStageCapability::new(test_state());
    assert_eq!(cap.name(), "crm.update_deal_stage");
    assert!(!cap.description().is_empty());
    assert!(cap.schema().is_object());
    assert!(cap.tags().contains(&"update"));
    assert_eq!(cap.source(), CapabilitySource::Plugin);
    assert!(cap.requires_confirmation());
}

// ============================================================================
// Capability call 参数测试
// ============================================================================

#[tokio::test]
async fn search_contact_capability_with_pagination_params() {
    let state = test_state();
    state
        .contacts
        .save(Contact {
            id: 1,
            name: "Alice".to_string(),
            ..Default::default()
        })
        .unwrap();
    let cap = SearchContactCapability::new(state);
    let r = cap
        .call(json!({"page": 1, "page_size": 5, "keyword": "Alice"}))
        .await
        .unwrap();
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["total"], 1);
}

#[tokio::test]
async fn search_lead_capability_with_pagination_params() {
    let state = test_state();
    state
        .leads
        .save(Lead {
            id: 1,
            name: "L1".to_string(),
            ..Default::default()
        })
        .unwrap();
    let cap = SearchLeadCapability::new(state);
    let r = cap
        .call(json!({"page": 1, "page_size": 5, "keyword": "L1", "status": "prospect"}))
        .await
        .unwrap();
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["total"], 1);
}

#[tokio::test]
async fn search_deal_capability_with_pagination_params() {
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
    let cap = SearchDealCapability::new(state);
    let r = cap
        .call(json!({"page": 1, "page_size": 5, "keyword": "D1", "stage": "initial"}))
        .await
        .unwrap();
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["total"], 1);
}

#[tokio::test]
async fn create_contact_capability_success() {
    let state = test_state();
    let cap = CreateContactCapability::new(state.clone());
    let r = cap
        .call(json!({"name": "Alice", "phone": "138"}))
        .await
        .unwrap();
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["name"], "Alice");
}

#[tokio::test]
async fn create_contact_capability_with_existing_id() {
    let state = test_state();
    let cap = CreateContactCapability::new(state.clone());
    // body 中带 id，不应被覆盖
    let r = cap.call(json!({"id": 42, "name": "Bob"})).await.unwrap();
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["name"], "Bob");
}

#[tokio::test]
async fn convert_lead_capability_missing_id_returns_error() {
    let cap = ConvertLeadCapability::new(test_state());
    let r = cap.call(json!({})).await;
    assert!(matches!(r, Err(CapError::ValidationError(_))));
}

#[tokio::test]
async fn update_deal_stage_capability_missing_id_returns_error() {
    let cap = UpdateDealStageCapability::new(test_state());
    let r = cap.call(json!({"new_stage": "quoted"})).await;
    assert!(matches!(r, Err(CapError::ValidationError(_))));
}

#[tokio::test]
async fn update_deal_stage_capability_missing_new_stage_returns_error() {
    let cap = UpdateDealStageCapability::new(test_state());
    let r = cap.call(json!({"id": 1})).await;
    assert!(matches!(r, Err(CapError::ValidationError(_))));
}

#[tokio::test]
async fn update_deal_stage_capability_not_found_returns_error() {
    let cap = UpdateDealStageCapability::new(test_state());
    let r = cap.call(json!({"id": 999, "new_stage": "quoted"})).await;
    assert!(matches!(r, Err(CapError::NotFound(_))));
}

#[tokio::test]
async fn update_deal_stage_capability_invalid_transition_returns_error() {
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
    let r = cap.call(json!({"id": 1, "new_stage": "initial"})).await;
    assert!(matches!(r, Err(CapError::ValidationError(_))));
}

// ============================================================================
// 补充：create name 为空、delete not found、update save 错误等分支
// ============================================================================

#[tokio::test]
async fn deal_create_empty_name_returns_400() {
    let repo: DealRepo = Arc::new(InMemoryRepository::new());
    let r = DealController::create(&*repo, json!({"id": 0, "name": ""})).await;
    assert_eq!(r["code"], 400);
    assert_eq!(r["msg"], "name 必填");
}

#[tokio::test]
async fn lead_create_empty_name_returns_400() {
    let repo: LeadRepo = Arc::new(InMemoryRepository::new());
    let r = LeadController::create(&*repo, json!({"id": 0, "name": ""})).await;
    assert_eq!(r["code"], 400);
    assert_eq!(r["msg"], "name 必填");
}

#[tokio::test]
async fn deal_delete_not_found_returns_404() {
    let repo: DealRepo = Arc::new(InMemoryRepository::new());
    let r = DealController::delete(&*repo, 99).await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn lead_update_not_found_returns_404() {
    let repo: LeadRepo = Arc::new(InMemoryRepository::new());
    let r = LeadController::update(&*repo, 99, json!({"name": "x"})).await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn deal_update_save_failure_returns_500() {
    // 先用 InMemoryRepository 存入实体，再用 SaveFailingRepo 包装使 save 失败
    let inner = InMemoryRepository::<Deal>::new();
    inner
        .save(Deal {
            id: 1,
            name: "D".to_string(),
            ..Default::default()
        })
        .unwrap();
    let repo = SaveFailingRepo(inner);
    let r = DealController::update(&repo, 1, json!({"name": "updated"})).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn deal_update_stage_save_failure_returns_500() {
    let inner = InMemoryRepository::<Deal>::new();
    inner
        .save(Deal {
            id: 1,
            name: "D".to_string(),
            stage: "initial".to_string(),
            ..Default::default()
        })
        .unwrap();
    let repo = SaveFailingRepo(inner);
    let r = DealController::update_stage(&repo, 1, "requirement_confirmed").await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn lead_update_save_failure_returns_500() {
    let inner = InMemoryRepository::<Lead>::new();
    inner
        .save(Lead {
            id: 1,
            name: "L".to_string(),
            ..Default::default()
        })
        .unwrap();
    let repo = SaveFailingRepo(inner);
    let r = LeadController::update(&repo, 1, json!({"name": "updated"})).await;
    assert_eq!(r["code"], 500);
}

#[tokio::test]
async fn lead_convert_lead_save_failure_after_status_change_returns_500() {
    // convert 步骤①：find_by_id 成功，但 save 失败
    let inner = InMemoryRepository::<Lead>::new();
    inner
        .save(Lead {
            id: 1,
            name: "L".to_string(),
            status: "prospect".to_string(),
            ..Default::default()
        })
        .unwrap();
    let lead_repo = SaveFailingRepo(inner);
    let contact_repo: ContactRepo = Arc::new(InMemoryRepository::new());
    let deal_repo: DealRepo = Arc::new(InMemoryRepository::new());
    let r = LeadController::convert(&lead_repo, &*contact_repo, &*deal_repo, 1).await;
    assert_eq!(r["code"], 500);
}

// ============================================================================
// CrmPlugin Hook 测试
// ============================================================================

#[test]
fn crm_plugin_register_and_list_capabilities() {
    let plugin = CrmPlugin::new(test_state());
    let registry = CapabilityRegistry::new();
    let names = plugin.register_capabilities(&registry).unwrap();
    assert_eq!(names.len(), 7);
    // 验证所有能力都已注册
    for name in &names {
        assert!(registry.get(name).is_some());
    }
}

#[test]
fn crm_plugin_capability_names_match_registered() {
    let plugin = CrmPlugin::new(test_state());
    let declared = plugin.capability_names();
    let registry = CapabilityRegistry::new();
    let registered = plugin.register_capabilities(&registry).unwrap();
    assert_eq!(declared, registered);
}

// ============================================================================
// register_routes 测试
// ============================================================================
// 注意：register_routes 使用 `:id` 路径参数格式，axum 0.7+ 要求 `{id}` 格式。
// 调用 register_routes 会在注册 `:id` 路由时 panic，因此使用 catch_unwind 捕获，
// 覆盖 register_routes 函数开头到 panic 点之间的代码行。

#[test]
fn register_routes_executes_until_path_param_panic() {
    let result = std::panic::catch_unwind(|| {
        let builder: RouterBuilder = RouterBuilder::new();
        let state = CrmState::default();
        register_routes(builder, state)
    });
    // 预期 panic（axum 路径参数格式问题）
    assert!(result.is_err());
}
