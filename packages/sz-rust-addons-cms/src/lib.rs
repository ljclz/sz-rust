//! # SZ-Rust CMS 模板插件（可选业务模板）
//!
//! 提供 CMS（内容管理系统）基础骨架，包含：
//!
//! - **Article（文章）**：文章发布与管理
//! - **Category（分类）**：文章分类管理
//! - **Tag（标签）**：文章标签管理
//!
//! ## 快速开始
//!
//! ```rust,ignore
//! use sz_rust_addons_cms::register_routes;
//!
//! let cms_state = CmsState::default();
//! register_routes(builder, cms_state);
//! ```

#![allow(missing_docs)]

pub mod capability;
pub mod controller;
pub mod model;
pub mod service;

use std::sync::Arc;

use axum::extract::{Json, Path, Query};
use serde::Deserialize;
use serde_json::Value;
use sz_rust_core::orm::repository::InMemoryRepository;
use sz_rust_core::router::RouterBuilder;

use crate::model::article::Article;
use crate::model::category::Category;
use crate::model::tag::Tag;

pub type ArticleRepo = Arc<InMemoryRepository<Article>>;
pub type CategoryRepo = Arc<InMemoryRepository<Category>>;
pub type TagRepo = Arc<InMemoryRepository<Tag>>;

#[derive(Clone, Default)]
pub struct CmsState {
    pub articles: ArticleRepo,
    pub categories: CategoryRepo,
    pub tags: TagRepo,
}

#[derive(Deserialize)]
struct ListQuery {
    page: Option<u64>,
    page_size: Option<u64>,
    keyword: Option<String>,
    category_id: Option<i64>,
    status: Option<String>,
}

#[derive(Deserialize)]
struct IdPath {
    id: i64,
}

pub fn register_routes<S>(builder: RouterBuilder<S>, state: CmsState) -> RouterBuilder<S>
where
    S: Clone + Send + Sync + 'static,
{
    let base = "/api/cms";

    let builder = builder.get(&format!("{}/articles", base), {
        let s = state.clone();
        move |q: Query<ListQuery>| async move {
            Json(
                controller::article::ArticleController::list(
                    &*s.articles,
                    q.page.unwrap_or(1),
                    q.page_size.unwrap_or(20),
                    q.keyword.clone(),
                    q.category_id,
                    q.status.clone(),
                )
                .await,
            )
        }
    });

    let builder = builder.post(&format!("{}/articles", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(controller::article::ArticleController::create(&*s.articles, body.0).await)
        }
    });

    let builder = builder.get(&format!("{}/articles/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::article::ArticleController::get(&*s.articles, path.id).await)
        }
    });

    let builder = builder.put(&format!("{}/articles/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>, body: Json<Value>| async move {
            Json(
                controller::article::ArticleController::update(&*s.articles, path.id, body.0).await,
            )
        }
    });

    let builder = builder.delete(&format!("{}/articles/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::article::ArticleController::delete(&*s.articles, path.id).await)
        }
    });

    let builder = builder.get(&format!("{}/categories", base), {
        let s = state.clone();
        move |q: Query<ListQuery>| async move {
            Json(
                controller::category::CategoryController::list(
                    &*s.categories,
                    q.page.unwrap_or(1),
                    q.page_size.unwrap_or(20),
                    q.keyword.clone(),
                )
                .await,
            )
        }
    });

    let builder = builder.post(&format!("{}/categories", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(controller::category::CategoryController::create(&*s.categories, body.0).await)
        }
    });

    let builder = builder.get(&format!("{}/categories/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::category::CategoryController::get(&*s.categories, path.id).await)
        }
    });

    let builder = builder.delete(&format!("{}/categories/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::category::CategoryController::delete(&*s.categories, path.id).await)
        }
    });

    let builder = builder.get(&format!("{}/tags", base), {
        let s = state.clone();
        move || async move { Json(controller::tag::TagController::list(&*s.tags).await) }
    });

    let builder = builder.post(&format!("{}/tags", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(controller::tag::TagController::create(&*s.tags, body.0).await)
        }
    });

    let builder = builder.delete(&format!("{}/tags/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::tag::TagController::delete(&*s.tags, path.id).await)
        }
    });

    builder
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::Request;
    use http_body_util::BodyExt;
    use serde_json::json;
    use sz_rust_core::orm::repository::Repository;
    use tower::ServiceExt;

    async fn body_to_json(body: Body) -> Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn make_app() -> axum::Router {
        let state = CmsState::default();
        let builder: RouterBuilder<()> = RouterBuilder::new();
        register_routes(builder, state).build()
    }

    // --- Article 路由 ---

    #[tokio::test]
    async fn route_get_articles_returns_list() {
        let app = make_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/cms/articles")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let json = body_to_json(resp.into_body()).await;
        assert_eq!(json["code"], 0);
    }

    #[tokio::test]
    async fn route_post_articles_creates_article() {
        let app = make_app();
        let body = serde_json::to_vec(&json!({"id": 0, "title": "Hello"})).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/cms/articles")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let json = body_to_json(resp.into_body()).await;
        assert_eq!(json["code"], 0);
        assert_eq!(json["data"]["title"], "Hello");
    }

    #[tokio::test]
    async fn route_get_article_by_id_returns_404_when_not_found() {
        let app = make_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/cms/articles/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let json = body_to_json(resp.into_body()).await;
        assert_eq!(json["code"], 404);
    }

    #[tokio::test]
    async fn route_delete_article_returns_404_when_not_found() {
        let app = make_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/cms/articles/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_to_json(resp.into_body()).await;
        assert_eq!(json["code"], 404);
    }

    #[tokio::test]
    async fn route_put_article_returns_404_when_not_found() {
        let app = make_app();
        let body = serde_json::to_vec(&json!({"title": "X"})).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/cms/articles/999")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_to_json(resp.into_body()).await;
        assert_eq!(json["code"], 404);
    }

    #[tokio::test]
    async fn route_get_articles_with_query_params() {
        let app = make_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/cms/articles?page=1&page_size=5&keyword=test&status=draft")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_to_json(resp.into_body()).await;
        assert_eq!(json["code"], 0);
    }

    // --- Category 路由 ---

    #[tokio::test]
    async fn route_get_categories_returns_list() {
        let app = make_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/cms/categories")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_to_json(resp.into_body()).await;
        assert_eq!(json["code"], 0);
    }

    #[tokio::test]
    async fn route_post_categories_creates_category() {
        let app = make_app();
        let body = serde_json::to_vec(&json!({"id": 0, "name": "Tech"})).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/cms/categories")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_to_json(resp.into_body()).await;
        assert_eq!(json["code"], 0);
        assert_eq!(json["data"]["name"], "Tech");
    }

    #[tokio::test]
    async fn route_get_category_by_id_returns_404_when_not_found() {
        let app = make_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/cms/categories/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_to_json(resp.into_body()).await;
        assert_eq!(json["code"], 404);
    }

    #[tokio::test]
    async fn route_delete_category_returns_404_when_not_found() {
        let app = make_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/cms/categories/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_to_json(resp.into_body()).await;
        assert_eq!(json["code"], 404);
    }

    // --- Tag 路由 ---

    #[tokio::test]
    async fn route_get_tags_returns_list() {
        let app = make_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/cms/tags")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_to_json(resp.into_body()).await;
        assert_eq!(json["code"], 0);
    }

    #[tokio::test]
    async fn route_post_tags_creates_tag() {
        let app = make_app();
        let body = serde_json::to_vec(&json!({"id": 0, "name": "rust"})).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/cms/tags")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_to_json(resp.into_body()).await;
        assert_eq!(json["code"], 0);
        assert_eq!(json["data"]["name"], "rust");
    }

    #[tokio::test]
    async fn route_delete_tag_returns_404_when_not_found() {
        let app = make_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/cms/tags/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_to_json(resp.into_body()).await;
        assert_eq!(json["code"], 404);
    }

    // --- 状态共享测试 ---

    #[tokio::test]
    async fn route_create_then_get_article_full_cycle() {
        let state = CmsState::default();
        let builder: RouterBuilder<()> = RouterBuilder::new();
        let app = register_routes(builder, state.clone()).build();

        // 创建文章
        let create_body = serde_json::to_vec(&json!({"id": 0, "title": "Cycle Test"})).unwrap();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/cms/articles")
                    .header("content-type", "application/json")
                    .body(Body::from(create_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let created = body_to_json(resp.into_body()).await;
        assert_eq!(created["code"], 0);
        let id = created["data"]["id"].as_i64().unwrap();

        // 直接通过 state 验证文章已存入仓储
        let article = state
            .articles
            .find_by_id(&sz_rust_core::orm::Value::I64(id))
            .unwrap();
        assert!(article.is_some());
        assert_eq!(article.unwrap().title, "Cycle Test");
    }
}
