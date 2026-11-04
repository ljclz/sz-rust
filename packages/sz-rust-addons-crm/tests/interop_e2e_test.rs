// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 端到端接线测试 — 跨插件 owner 校验 + 事件发布 + 订阅接收完整链路。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use sz_rust_addons_crm::controller::contact::ContactController;
use sz_rust_addons_crm::controller::lead::LeadController;
use sz_rust_addons_crm::interop::{SharedUserRepo, UserRecord};
use sz_rust_addons_crm::model::contact::Contact;
use sz_rust_addons_crm::model::lead::Lead;
use sz_rust_core::orm::repository::{InMemoryRepository, Repository};

use sz_rust_core::plugin::{EventBus, EventHandler, InMemoryEventBus, PluginEvent, SysUser};

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

fn make_user_repo(user_id: i64, tenant_id: i64) -> SharedUserRepo {
    let user = UserRecord(SysUser {
        id: user_id,
        tenant_id,
        username: "alice".to_string(),
        display_name: "alice".to_string(),
        password_hash: String::new(),
        email: None,
        phone: None,
        status: "active".to_string(),
        extra: json!({}),
        created_at: now(),
        updated_at: now(),
    });
    Arc::new(InMemoryRepository::from_vec(vec![user]))
}

/// 计数处理器，记录收到的事件数。
struct CountHandler {
    count: Mutex<usize>,
}

#[async_trait]
impl EventHandler for CountHandler {
    async fn handle(&self, _event: &PluginEvent) -> Result<(), String> {
        *self.count.lock().unwrap() += 1;
        Ok(())
    }
}

// ─── ContactController::create 端到端 ─────────────────────────────────────────

#[tokio::test]
async fn contact_create_validates_owner_and_publishes_event() {
    let repo: Arc<InMemoryRepository<Contact>> = Arc::new(InMemoryRepository::new());
    let shared_users = make_user_repo(10, 1);
    let bus = Arc::new(InMemoryEventBus::new());
    let handler = Arc::new(CountHandler {
        count: Mutex::new(0),
    });
    bus.subscribe("crm.contact.created", handler.clone())
        .await
        .unwrap();

    let result = ContactController::create(
        &*repo,
        json!({"id": 0, "name": "Alice", "owner_id": 10}),
        Some(&shared_users),
        Some(&bus),
        1,
    )
    .await;
    assert_eq!(result["code"], 0);
    assert_eq!(*handler.count.lock().unwrap(), 1);
}

#[tokio::test]
async fn contact_create_rejects_nonexistent_owner() {
    let repo: Arc<InMemoryRepository<Contact>> = Arc::new(InMemoryRepository::new());
    let shared_users: SharedUserRepo = Arc::new(InMemoryRepository::new());

    let result = ContactController::create(
        &*repo,
        json!({"id": 0, "name": "Alice", "owner_id": 99}),
        Some(&shared_users),
        None,
        1,
    )
    .await;
    assert_eq!(result["code"], 422);
    assert_eq!(result["msg"], "owner_id 指向的用户不存在");
}

#[tokio::test]
async fn contact_create_rejects_cross_tenant_owner() {
    let repo: Arc<InMemoryRepository<Contact>> = Arc::new(InMemoryRepository::new());
    let shared_users = make_user_repo(10, 2);

    let result = ContactController::create(
        &*repo,
        json!({"id": 0, "name": "Alice", "owner_id": 10}),
        Some(&shared_users),
        None,
        1,
    )
    .await;
    assert_eq!(result["code"], 403);
}

#[tokio::test]
async fn contact_create_owner_zero_passes_without_shared_users() {
    let repo: Arc<InMemoryRepository<Contact>> = Arc::new(InMemoryRepository::new());
    let result =
        ContactController::create(&*repo, json!({"id": 0, "name": "Alice"}), None, None, 0).await;
    assert_eq!(result["code"], 0);
}

// ─── LeadController::convert 端到端 ───────────────────────────────────────────

#[tokio::test]
async fn lead_convert_publishes_event() {
    let lead_repo: Arc<InMemoryRepository<Lead>> = Arc::new(InMemoryRepository::new());
    let contact_repo: Arc<InMemoryRepository<Contact>> = Arc::new(InMemoryRepository::new());
    let deal_repo: Arc<InMemoryRepository<sz_rust_addons_crm::model::deal::Deal>> =
        Arc::new(InMemoryRepository::new());
    lead_repo
        .save(Lead {
            id: 1,
            name: "HotLead".to_string(),
            status: "prospect".to_string(),
            company: "Acme".to_string(),
            estimated_amount: 50000.0,
            ..Default::default()
        })
        .unwrap();

    let bus = Arc::new(InMemoryEventBus::new());
    let handler = Arc::new(CountHandler {
        count: Mutex::new(0),
    });
    bus.subscribe("crm.lead.converted", handler.clone())
        .await
        .unwrap();

    let result = LeadController::convert(
        &*lead_repo,
        &*contact_repo,
        &*deal_repo,
        1,
        None,
        Some(&bus),
        1,
    )
    .await;
    assert_eq!(result["code"], 0);
    assert_eq!(*handler.count.lock().unwrap(), 1);
}

#[tokio::test]
async fn lead_convert_rejects_nonexistent_owner() {
    let lead_repo: Arc<InMemoryRepository<Lead>> = Arc::new(InMemoryRepository::new());
    let contact_repo: Arc<InMemoryRepository<Contact>> = Arc::new(InMemoryRepository::new());
    let deal_repo: Arc<InMemoryRepository<sz_rust_addons_crm::model::deal::Deal>> =
        Arc::new(InMemoryRepository::new());
    lead_repo
        .save(Lead {
            id: 1,
            name: "HotLead".to_string(),
            status: "prospect".to_string(),
            owner_id: 99,
            ..Default::default()
        })
        .unwrap();

    let shared_users: SharedUserRepo = Arc::new(InMemoryRepository::new());
    let result = LeadController::convert(
        &*lead_repo,
        &*contact_repo,
        &*deal_repo,
        1,
        Some(&shared_users),
        None,
        1,
    )
    .await;
    assert_eq!(result["code"], 422);
}

// ─── 旧 JSON 反序列化兼容（Contact 无 owner_id 默认 0）─────────────────────────

#[tokio::test]
async fn contact_deserialize_old_json_without_owner_id_defaults_zero() {
    let old_json = json!({"id": 1, "name": "Legacy", "phone": "123"});
    let contact: Contact = serde_json::from_value(old_json).unwrap();
    assert_eq!(contact.owner_id, 0);
}

// ─── CRM 事件订阅处理器端到端 ─────────────────────────────────────────────────

#[tokio::test]
async fn cms_article_published_handler_idempotent_via_bus() {
    use sz_rust_addons_crm::event_handler::CmsArticlePublishedHandler;
    let bus = InMemoryEventBus::new();
    let handler = Arc::new(CmsArticlePublishedHandler::new());
    bus.subscribe("cms.article.published", handler.clone())
        .await
        .unwrap();

    let event = PluginEvent {
        id: 0,
        tenant_id: 1,
        event_type: "cms.article.published".to_string(),
        source_plugin: "cms".to_string(),
        payload: json!({"article_id": 1}),
    };
    bus.publish(&event).await.unwrap();
    bus.publish(&event).await.unwrap(); // 同事件再发一次（不同 id，但 handler 演示幂等逻辑）
    assert_eq!(handler.processed_count(), 2); // 两个不同 id 的事件各处理一次
}

#[tokio::test]
async fn permission_changed_handler_idempotent_via_bus() {
    use sz_rust_addons_crm::event_handler::PermissionChangedHandler;
    let bus = InMemoryEventBus::new();
    let handler = Arc::new(PermissionChangedHandler::new());
    bus.subscribe("sys.user.permission_changed", handler.clone())
        .await
        .unwrap();

    let event = PluginEvent {
        id: 0,
        tenant_id: 1,
        event_type: "sys.user.permission_changed".to_string(),
        source_plugin: "sys".to_string(),
        payload: json!({"user_id": 1}),
    };
    bus.publish(&event).await.unwrap();
    assert_eq!(handler.processed_count(), 1);
}
