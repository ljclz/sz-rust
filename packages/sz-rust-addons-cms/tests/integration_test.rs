//! CMS 集成测试 — 覆盖 Article / Category / Tag 模型与控制器
//!
//! 使用 `InMemoryRepository` 作为仓储实现，不依赖真实数据库。

use serde_json::json;
use std::sync::Arc;
use sz_rust_core::orm::repository::{InMemoryRepository, Repository};

use sz_rust_addons_cms::controller::article::ArticleController;
use sz_rust_addons_cms::controller::category::CategoryController;
use sz_rust_addons_cms::controller::tag::TagController;
use sz_rust_addons_cms::model::article::Article;
use sz_rust_addons_cms::model::category::Category;
use sz_rust_addons_cms::model::tag::Tag;

type ArticleRepo = Arc<InMemoryRepository<Article>>;
type CategoryRepo = Arc<InMemoryRepository<Category>>;
type TagRepo = Arc<InMemoryRepository<Tag>>;

// ============================================================================
// Model 层测试
// ============================================================================

#[test]
fn article_default_has_zero_id() {
    let a = Article::default();
    assert_eq!(a.id, 0);
    assert!(a.title.is_empty());
    assert!(a.status.is_empty());
}

#[test]
fn article_get_attribute_returns_correct_types() {
    let a = Article {
        id: 42,
        title: "Hello".to_string(),
        status: "draft".to_string(),
        category_id: 7,
        ..Default::default()
    };
    use sz_rust_core::orm::repository::EntityAttributes;
    assert_eq!(
        a.get_attribute("id"),
        Some(sz_rust_core::orm::Value::I64(42))
    );
    assert_eq!(
        a.get_attribute("title"),
        Some(sz_rust_core::orm::Value::String("Hello".to_string()))
    );
    assert_eq!(
        a.get_attribute("status"),
        Some(sz_rust_core::orm::Value::String("draft".to_string()))
    );
    assert_eq!(a.get_attribute("unknown"), None);
}

// ============================================================================
// Controller 层测试 — Article
// ============================================================================

#[tokio::test]
async fn article_create_rejects_empty_title() {
    let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
    let body = json!({"id": 0, "title": ""});
    let result = ArticleController::create(&*repo, body).await;
    assert_eq!(result["code"], 400);
    assert_eq!(result["msg"], "title is required");
}

#[tokio::test]
async fn article_create_sets_default_status_draft() {
    let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
    let body = json!({"id": 0, "title": "Test Article"});
    let result = ArticleController::create(&*repo, body).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["data"]["status"], "draft");
}

#[tokio::test]
async fn article_list_filters_by_keyword() {
    let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
    repo.save(Article {
        id: 1,
        title: "Rust Guide".to_string(),
        status: "draft".to_string(),
        ..Default::default()
    })
    .unwrap();
    repo.save(Article {
        id: 2,
        title: "Go Tutorial".to_string(),
        status: "draft".to_string(),
        ..Default::default()
    })
    .unwrap();
    let result =
        ArticleController::list(&*repo, 1, 20, Some("Rust Guide".to_string()), None, None).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["data"]["total"], 1);
}

#[tokio::test]
async fn article_list_filters_by_category_id() {
    let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
    repo.save(Article {
        id: 1,
        title: "A".to_string(),
        category_id: 5,
        status: "draft".to_string(),
        ..Default::default()
    })
    .unwrap();
    repo.save(Article {
        id: 2,
        title: "B".to_string(),
        category_id: 9,
        status: "draft".to_string(),
        ..Default::default()
    })
    .unwrap();
    let result = ArticleController::list(&*repo, 1, 20, None, Some(5), None).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["data"]["total"], 1);
}

#[tokio::test]
async fn article_list_paginates_correctly() {
    let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
    for i in 1..=5 {
        repo.save(Article {
            id: i,
            title: format!("Article {}", i),
            status: "draft".to_string(),
            ..Default::default()
        })
        .unwrap();
    }
    let result = ArticleController::list(&*repo, 1, 2, None, None, None).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["data"]["total"], 5);
    assert_eq!(result["data"]["page"], 1);
    assert_eq!(result["data"]["page_size"], 2);
}

#[tokio::test]
async fn article_list_filters_by_status() {
    let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
    repo.save(Article {
        id: 1,
        title: "A".to_string(),
        status: "published".to_string(),
        ..Default::default()
    })
    .unwrap();
    repo.save(Article {
        id: 2,
        title: "B".to_string(),
        status: "draft".to_string(),
        ..Default::default()
    })
    .unwrap();
    repo.save(Article {
        id: 3,
        title: "C".to_string(),
        status: "published".to_string(),
        ..Default::default()
    })
    .unwrap();
    let result =
        ArticleController::list(&*repo, 1, 20, None, None, Some("published".to_string())).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["data"]["total"], 2);
}

#[tokio::test]
async fn article_update_patches_fields() {
    let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
    repo.save(Article {
        id: 1,
        title: "Original".to_string(),
        status: "draft".to_string(),
        ..Default::default()
    })
    .unwrap();
    let result = ArticleController::update(
        &*repo,
        1,
        json!({"title": "Updated", "content": "New content"}),
    )
    .await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["data"]["title"], "Updated");
    assert_eq!(result["data"]["content"], "New content");
}

#[tokio::test]
async fn article_delete_returns_rows() {
    let repo: ArticleRepo = Arc::new(InMemoryRepository::new());
    repo.save(Article {
        id: 1,
        title: "To Delete".to_string(),
        status: "draft".to_string(),
        ..Default::default()
    })
    .unwrap();
    let result = ArticleController::delete(&*repo, 1).await;
    assert_eq!(result["code"], 0);
    assert_eq!(result["data"]["rows"], 1);
}

// ============================================================================
// Controller 层测试 — Category CRUD
// ============================================================================

#[tokio::test]
async fn category_crud_full_cycle() {
    let repo: CategoryRepo = Arc::new(InMemoryRepository::new());
    let created = CategoryController::create(&*repo, json!({"id": 0, "name": "Tech"})).await;
    assert_eq!(created["code"], 0);
    // 直接 save 一条带 id 的记录用于后续测试
    repo.save(Category {
        id: 10,
        name: "News".to_string(),
        ..Default::default()
    })
    .unwrap();
    let got = CategoryController::get(&*repo, 10).await;
    assert_eq!(got["data"]["name"], "News");
    let listed = CategoryController::list(&*repo, 1, 20, None).await;
    assert_eq!(listed["data"]["total"], 2);
    let deleted = CategoryController::delete(&*repo, 10).await;
    assert_eq!(deleted["code"], 0);
    let not_found = CategoryController::get(&*repo, 10).await;
    assert_eq!(not_found["code"], 404);
}

// ============================================================================
// Controller 层测试 — Tag CRUD
// ============================================================================

#[tokio::test]
async fn tag_crud_full_cycle() {
    let repo: TagRepo = Arc::new(InMemoryRepository::new());
    let created = TagController::create(&*repo, json!({"id": 0, "name": "rust"})).await;
    assert_eq!(created["code"], 0);
    // 直接 save 一条带 id 的记录用于后续测试
    repo.save(Tag {
        id: 5,
        name: "web".to_string(),
        ..Default::default()
    })
    .unwrap();
    let listed = TagController::list(&*repo).await;
    assert_eq!(listed["code"], 0);
    assert!(listed["data"].as_array().unwrap().len() >= 1);
    let deleted = TagController::delete(&*repo, 5).await;
    assert_eq!(deleted["code"], 0);
}
