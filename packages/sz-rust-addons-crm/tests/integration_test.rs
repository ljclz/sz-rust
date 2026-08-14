//! CRM 集成测试 — 覆盖 Contact / Lead / Deal 模型与控制器
//!
//! 使用 `InMemoryRepository` 作为仓储实现，不依赖真实数据库。

use serde_json::json;
use std::sync::Arc;
use sz_rust_core::orm::repository::{InMemoryRepository, Repository};

use sz_rust_addons_crm::controller::contact::ContactController;
use sz_rust_addons_crm::controller::deal::DealController;
use sz_rust_addons_crm::controller::lead::LeadController;
use sz_rust_addons_crm::model::contact::Contact;
use sz_rust_addons_crm::model::deal::Deal;
use sz_rust_addons_crm::model::lead::Lead;

type ContactRepo = Arc<InMemoryRepository<Contact>>;
type LeadRepo = Arc<InMemoryRepository<Lead>>;
type DealRepo = Arc<InMemoryRepository<Deal>>;

// ============================================================================
// Contact 测试
// ============================================================================

#[test]
fn contact_default_has_empty_name() {
    let c = Contact::default();
    assert!(c.name.is_empty());
    assert_eq!(c.id, 0);
}

#[test]
fn contact_new_returns_default() {
    let c = Contact::new();
    assert_eq!(c, Contact::default());
}

#[test]
fn contact_get_attribute_returns_correct_types() {
    let c = Contact {
        id: 42,
        name: "Alice".to_string(),
        customer_id: 7,
        ..Default::default()
    };
    use sz_rust_core::orm::repository::EntityAttributes;
    assert_eq!(
        c.get_attribute("id"),
        Some(sz_rust_core::orm::Value::I64(42))
    );
    assert_eq!(
        c.get_attribute("name"),
        Some(sz_rust_core::orm::Value::String("Alice".to_string()))
    );
    assert_eq!(c.get_attribute("unknown"), None);
}

#[tokio::test]
async fn contact_create_rejects_empty_name() {
    let repo: ContactRepo = Arc::new(InMemoryRepository::new());
    // name 为空字符串触发 400（id 为必填字段，提供 0）
    let body = json!({"id": 0, "name": "", "phone": "123"});
    let result = ContactController::create(&*repo, body).await;
    assert_eq!(result["code"], 400);
    assert_eq!(result["msg"], "name 必填");
}

#[tokio::test]
async fn contact_create_success() {
    let repo: ContactRepo = Arc::new(InMemoryRepository::new());
    // id 和 name 是必填字段；控制器会将 id 重置为 0（模拟 DB 自增）
    let body = json!({"id": 1, "name": "Alice", "phone": "13800138000"});
    let result = ContactController::create(&*repo, body).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["msg"], "created");
    assert_eq!(result["data"]["name"], "Alice");
    assert_eq!(result["data"]["id"], 0);
}

#[tokio::test]
async fn contact_get_found_and_not_found() {
    let repo: ContactRepo = Arc::new(InMemoryRepository::new());
    let c = Contact {
        id: 10,
        name: "Bob".to_string(),
        ..Default::default()
    };
    repo.save(c.clone()).unwrap();

    // found
    let r = ContactController::get(&*repo, 10).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["name"], "Bob");

    // not found
    let r = ContactController::get(&*repo, 99).await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn contact_update_patches_fields() {
    let repo: ContactRepo = Arc::new(InMemoryRepository::new());
    repo.save(Contact {
        id: 5,
        name: "Original".to_string(),
        phone: "old".to_string(),
        ..Default::default()
    })
    .unwrap();

    let body = json!({"phone": "new_phone", "position": "Manager"});
    let r = ContactController::update(&*repo, 5, body).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["name"], "Original"); // unchanged
    assert_eq!(r["data"]["phone"], "new_phone");
    assert_eq!(r["data"]["position"], "Manager");
}

#[tokio::test]
async fn contact_update_not_found() {
    let repo: ContactRepo = Arc::new(InMemoryRepository::new());
    let r = ContactController::update(&*repo, 999, json!({"name": "x"})).await;
    assert_eq!(r["code"], 404);
}

#[tokio::test]
async fn contact_delete_found_and_not_found() {
    let repo: ContactRepo = Arc::new(InMemoryRepository::new());
    repo.save(Contact {
        id: 7,
        name: "ToDelete".to_string(),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(ContactController::delete(&*repo, 7).await["code"], 0);
    assert_eq!(ContactController::delete(&*repo, 7).await["code"], 404);
}

#[tokio::test]
async fn contact_list_pagination_and_keyword() {
    let repo: ContactRepo = Arc::new(InMemoryRepository::new());
    // 控制器使用 WhereOp::Like 但不自动添加 % 通配符，因此是精确匹配
    for i in 1..=5 {
        repo.save(Contact {
            id: i,
            name: format!("User{}", i),
            ..Default::default()
        })
        .unwrap();
    }
    repo.save(Contact {
        id: 10,
        name: "AliceWonder".to_string(),
        ..Default::default()
    })
    .unwrap();

    // All (no keyword)
    let r = ContactController::list(&*repo, 1, 10, None).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["total"], 6);

    // Keyword filter — Like 精确匹配（控制器未添加 % 通配符）
    let r = ContactController::list(&*repo, 1, 10, Some("AliceWonder".to_string())).await;
    assert_eq!(r["data"]["total"], 1);
    assert_eq!(r["data"]["list"][0]["name"], "AliceWonder");

    // Pagination
    let r = ContactController::list(&*repo, 1, 2, None).await;
    assert_eq!(r["data"]["page"], 1);
    assert_eq!(r["data"]["page_size"], 2);
    assert_eq!(r["data"]["list"].as_array().unwrap().len(), 2);
}

// ============================================================================
// Lead 测试
// ============================================================================

#[test]
fn lead_default_status_is_prospect() {
    let l = Lead::default();
    assert_eq!(l.status, "prospect");
    assert_eq!(l.id, 0);
}

#[test]
fn lead_get_attribute_includes_estimated_amount() {
    use sz_rust_core::orm::repository::EntityAttributes;
    let l = Lead {
        id: 3,
        estimated_amount: 9999.5,
        owner_id: 42,
        ..Default::default()
    };
    assert_eq!(
        l.get_attribute("id"),
        Some(sz_rust_core::orm::Value::I64(3))
    );
    assert_eq!(
        l.get_attribute("estimated_amount"),
        Some(sz_rust_core::orm::Value::F64(9999.5))
    );
    assert_eq!(
        l.get_attribute("owner_id"),
        Some(sz_rust_core::orm::Value::I64(42))
    );
}

#[tokio::test]
async fn lead_create_rejects_empty_name() {
    let repo: LeadRepo = Arc::new(InMemoryRepository::new());
    let r = LeadController::create(&*repo, json!({"name": ""})).await;
    assert_eq!(r["code"], 400);
}

#[tokio::test]
async fn lead_create_and_convert() {
    let repo: LeadRepo = Arc::new(InMemoryRepository::new());
    let contact_repo: ContactRepo = Arc::new(InMemoryRepository::new());
    let deal_repo: DealRepo = Arc::new(InMemoryRepository::new());
    // id 和 name 是必填字段；控制器将 id 重置为 0
    let body = json!({"id": 0, "name": "HotLead", "status": "qualified"});
    let r = LeadController::create(&*repo, body).await;
    assert_eq!(r["code"], 0);

    // 控制器将 id 重置为 0，convert 使用 id=0
    let r = LeadController::convert(&*repo, &*contact_repo, &*deal_repo, 0).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["msg"], "converted");
    assert_eq!(r["data"]["lead"]["status"], "converted");

    // Status filter in list
    let r = LeadController::list(&*repo, 1, 10, None, Some("converted".to_string())).await;
    assert_eq!(r["data"]["total"], 1);
}

#[tokio::test]
async fn lead_update_patches_estimated_amount() {
    let repo: LeadRepo = Arc::new(InMemoryRepository::new());
    repo.save(Lead {
        id: 2,
        name: "L".to_string(),
        estimated_amount: 100.0,
        ..Default::default()
    })
    .unwrap();
    let r = LeadController::update(&*repo, 2, json!({"estimated_amount": 5000.0})).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["estimated_amount"], 5000.0);
}

#[tokio::test]
async fn lead_delete_not_found_returns_404() {
    let repo: LeadRepo = Arc::new(InMemoryRepository::new());
    assert_eq!(LeadController::delete(&*repo, 999).await["code"], 404);
}

// ============================================================================
// Deal 测试
// ============================================================================

#[test]
fn deal_default_values() {
    let d = Deal::default();
    assert_eq!(d.id, 0);
    assert!(d.name.is_empty());
    assert_eq!(d.stage, "initial");
}

#[test]
fn deal_get_attribute_all_fields() {
    use sz_rust_core::orm::repository::EntityAttributes;
    let d = Deal {
        id: 8,
        name: "BigDeal".to_string(),
        stage: "negotiating".to_string(),
        amount: 100000.0,
        probability: 80,
        contact_id: 3,
        lead_id: 5,
        owner_id: 7,
        ..Default::default()
    };
    assert_eq!(
        d.get_attribute("id"),
        Some(sz_rust_core::orm::Value::I64(8))
    );
    assert_eq!(
        d.get_attribute("name"),
        Some(sz_rust_core::orm::Value::String("BigDeal".to_string()))
    );
    assert_eq!(
        d.get_attribute("amount"),
        Some(sz_rust_core::orm::Value::F64(100000.0))
    );
    assert_eq!(
        d.get_attribute("probability"),
        Some(sz_rust_core::orm::Value::U8(80))
    );
}

#[tokio::test]
async fn deal_pipeline_aggregation() {
    let repo: DealRepo = Arc::new(InMemoryRepository::new());
    repo.save(Deal {
        id: 1,
        name: "D1".to_string(),
        stage: "initial".to_string(),
        amount: 1000.0,
        ..Default::default()
    })
    .unwrap();
    repo.save(Deal {
        id: 2,
        name: "D2".to_string(),
        stage: "initial".to_string(),
        amount: 2000.0,
        ..Default::default()
    })
    .unwrap();
    repo.save(Deal {
        id: 3,
        name: "D3".to_string(),
        stage: "negotiating".to_string(),
        amount: 5000.0,
        ..Default::default()
    })
    .unwrap();

    let r = DealController::pipeline(&*repo).await;
    assert_eq!(r["code"], 0);
    let pipeline = r["data"]["pipeline"].as_array().unwrap();
    // initial stage
    let initial = pipeline.iter().find(|p| p["stage"] == "initial").unwrap();
    assert_eq!(initial["count"], 2);
    assert_eq!(initial["total_amount"], 3000.0);
    // negotiating stage
    let neg = pipeline
        .iter()
        .find(|p| p["stage"] == "negotiating")
        .unwrap();
    assert_eq!(neg["count"], 1);
    assert_eq!(neg["total_amount"], 5000.0);
    // won should have 0
    let won = pipeline.iter().find(|p| p["stage"] == "won").unwrap();
    assert_eq!(won["count"], 0);
}

#[tokio::test]
async fn deal_create_update_delete_lifecycle() {
    let repo: DealRepo = Arc::new(InMemoryRepository::new());

    // Create — id 和 name 是必填字段；控制器将 id 重置为 0
    let r = DealController::create(
        &*repo,
        json!({"id": 0, "name": "NewDeal", "stage": "quoted", "amount": 50000.0}),
    )
    .await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["id"], 0);

    // Update probability
    let r = DealController::update(&*repo, 0, json!({"probability": 75})).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["probability"], 75);

    // Get
    let r = DealController::get(&*repo, 0).await;
    assert_eq!(r["code"], 0);
    assert_eq!(r["data"]["name"], "NewDeal");

    // Delete
    assert_eq!(DealController::delete(&*repo, 0).await["code"], 0);
    assert_eq!(DealController::get(&*repo, 0).await["code"], 404);
}

#[tokio::test]
async fn deal_list_filter_by_stage() {
    let repo: DealRepo = Arc::new(InMemoryRepository::new());
    for (i, stage) in ["initial", "quoted", "won"].iter().enumerate() {
        repo.save(Deal {
            id: (i + 1) as i64,
            name: format!("D{}", i),
            stage: stage.to_string(),
            ..Default::default()
        })
        .unwrap();
    }
    let r = DealController::list(&*repo, 1, 10, None, Some("quoted".to_string())).await;
    assert_eq!(r["data"]["total"], 1);
    assert_eq!(r["data"]["list"][0]["name"], "D1");
}
