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

    let builder = builder.get(&format!("{}/articles/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::article::ArticleController::get(&*s.articles, path.id).await)
        }
    });

    let builder = builder.put(&format!("{}/articles/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>, body: Json<Value>| async move {
            Json(
                controller::article::ArticleController::update(&*s.articles, path.id, body.0).await,
            )
        }
    });

    let builder = builder.delete(&format!("{}/articles/:id", base), {
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

    let builder = builder.get(&format!("{}/categories/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::category::CategoryController::get(&*s.categories, path.id).await)
        }
    });

    let builder = builder.delete(&format!("{}/categories/:id", base), {
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

    let builder = builder.delete(&format!("{}/tags/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::tag::TagController::delete(&*s.tags, path.id).await)
        }
    });

    builder
}
