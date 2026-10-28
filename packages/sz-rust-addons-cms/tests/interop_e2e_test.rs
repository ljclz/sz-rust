// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 端到端接线测试 — CMS 跨插件 author 校验 + 事件发布 + 订阅接收完整链路。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use sz_rust_addons_cms::controller::article::ArticleController;
use sz_rust_addons_cms::interop::{SharedUserRepo, UserRecord};
use sz_rust_addons_cms::model::article::Article;
use sz_rust_core::orm::repository::{InMemoryRepository, Repository};
use sz_rust_core::plugin::{EventBus, EventHandler, InMemoryEventBus, PluginEvent, SysUser};

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

fn make_user_repo(user_id: i64, tenant_id: i64) -> SharedUserRepo {
    let user = UserRecord(SysUser {
        id: user_id,
        tenant_id,
        username: "author".to_string(),
        display_name: "author".to_string(),
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

// ─── ArticleController::create 端到端（author_id 校验）──────────────────────────

#[tokio::test]
async fn article_create_validates_author_id() {
    let repo: Arc<InMemoryRepository<Article>> = Arc::new(InMemoryRepository::new());
    let shared_users = make_user_repo(10, 1);
    let result = ArticleController::create(
        &*repo,
        json!({"id": 0, "title": "Hello", "author_id": 10}),
        Some(&shared_users),
        1,
    )
    .await;
    assert_eq!(result["code"], 0);
}

#[tokio::test]
async fn article_create_rejects_nonexistent_author() {
    let repo: Arc<InMemoryRepository<Article>> = Arc::new(InMemoryRepository::new());
    let shared_users: SharedUserRepo = Arc::new(InMemoryRepository::new());
    let result = ArticleController::create(
        &*repo,
        json!({"id": 0, "title": "Hello", "author_id": 99}),
        Some(&shared_users),
        1,
    )
    .await;
    assert_eq!(result["code"], 422);
}

#[tokio::test]
async fn article_create_rejects_cross_tenant_author() {
    let repo: Arc<InMemoryRepository<Article>> = Arc::new(InMemoryRepository::new());
    let shared_users = make_user_repo(10, 2);
    let result = ArticleController::create(
        &*repo,
        json!({"id": 0, "title": "Hello", "author_id": 10}),
        Some(&shared_users),
        1,
    )
    .await;
    assert_eq!(result["code"], 403);
}

// ─── ArticleController::publish 端到端（事件发布）──────────────────────────────

#[tokio::test]
async fn article_publish_emits_event() {
    let repo: Arc<InMemoryRepository<Article>> = Arc::new(InMemoryRepository::new());
    repo.save(Article {
        id: 1,
        title: "Draft".to_string(),
        status: "draft".to_string(),
        ..Default::default()
    })
    .unwrap();

    let bus = Arc::new(InMemoryEventBus::new());
    let handler = Arc::new(CountHandler {
        count: Mutex::new(0),
    });
    bus.subscribe("cms.article.published", handler.clone())
        .await
        .unwrap();

    let result = ArticleController::publish(&*repo, 1, Some(&bus), 1).await;
    assert_eq!(result["code"], 0);
    assert_eq!(*handler.count.lock().unwrap(), 1);
}

#[tokio::test]
async fn article_publish_without_event_bus_still_works() {
    let repo: Arc<InMemoryRepository<Article>> = Arc::new(InMemoryRepository::new());
    repo.save(Article {
        id: 1,
        title: "Draft".to_string(),
        status: "draft".to_string(),
        ..Default::default()
    })
    .unwrap();
    let result = ArticleController::publish(&*repo, 1, None, 0).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["data"]["status"], "published");
}

// ─── CMS 事件订阅处理器端到端 ─────────────────────────────────────────────────

#[tokio::test]
async fn crm_contact_created_handler_idempotent_via_bus() {
    use sz_rust_addons_cms::event_handler::CrmContactCreatedHandler;
    let bus = InMemoryEventBus::new();
    let handler = Arc::new(CrmContactCreatedHandler::new());
    bus.subscribe("crm.contact.created", handler.clone())
        .await
        .unwrap();

    let event = PluginEvent {
        id: 0,
        tenant_id: 1,
        event_type: "crm.contact.created".to_string(),
        source_plugin: "crm".to_string(),
        payload: json!({"contact_id": 1}),
    };
    bus.publish(&event).await.unwrap();
    assert_eq!(handler.processed_count(), 1);
}

#[tokio::test]
async fn cms_permission_changed_handler_idempotent_via_bus() {
    use sz_rust_addons_cms::event_handler::PermissionChangedHandler;
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
