//! # SZ-Rust Forum 模板插件（可选业务模板）
//!
//! 提供论坛基础骨架，包含：
//!
//! - **Board（板块）**：论坛板块管理
//! - **Topic（帖子）**：发帖与回帖
//! - **Reply（回复）**：帖子回复管理

#![allow(missing_docs)]

pub mod controller;
pub mod model;
pub mod service;

use std::sync::Arc;

use axum::extract::{Json, Path, Query};
use serde::Deserialize;
use serde_json::Value;
use sz_rust_core::orm::repository::InMemoryRepository;
use sz_rust_core::router::RouterBuilder;

use crate::model::board::Board;
use crate::model::reply::Reply;
use crate::model::topic::Topic;

pub type BoardRepo = Arc<InMemoryRepository<Board>>;
pub type TopicRepo = Arc<InMemoryRepository<Topic>>;
pub type ReplyRepo = Arc<InMemoryRepository<Reply>>;

#[derive(Clone, Default)]
pub struct ForumState {
    pub boards: BoardRepo,
    pub topics: TopicRepo,
    pub replies: ReplyRepo,
}

#[derive(Deserialize)]
struct ListQuery {
    page: Option<u64>,
    page_size: Option<u64>,
    keyword: Option<String>,
    board_id: Option<i64>,
}

#[derive(Deserialize)]
struct IdPath {
    id: i64,
}

pub fn register_routes<S>(builder: RouterBuilder<S>, state: ForumState) -> RouterBuilder<S>
where
    S: Clone + Send + Sync + 'static,
{
    let base = "/api/forum";

    let builder = builder.get(&format!("{}/boards", base), {
        let s = state.clone();
        move || async move { Json(controller::board::BoardController::list(&*s.boards).await) }
    });

    let builder = builder.post(&format!("{}/boards", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(controller::board::BoardController::create(&*s.boards, body.0).await)
        }
    });

    let builder = builder.get(&format!("{}/topics", base), {
        let s = state.clone();
        move |q: Query<ListQuery>| async move {
            Json(
                controller::topic::TopicController::list(
                    &*s.topics,
                    q.page.unwrap_or(1),
                    q.page_size.unwrap_or(20),
                    q.keyword.clone(),
                    q.board_id,
                )
                .await,
            )
        }
    });

    let builder = builder.post(&format!("{}/topics", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(controller::topic::TopicController::create(&*s.topics, body.0).await)
        }
    });

    let builder = builder.get(&format!("{}/topics/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::topic::TopicController::get(&*s.topics, path.id).await)
        }
    });

    let builder = builder.delete(&format!("{}/topics/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::topic::TopicController::delete(&*s.topics, path.id).await)
        }
    });

    let builder = builder.get(&format!("{}/topics/:id/replies", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::reply::ReplyController::list_by_topic(&*s.replies, path.id).await)
        }
    });

    let builder = builder.post(&format!("{}/topics/:id/replies", base), {
        let s = state.clone();
        move |path: Path<IdPath>, body: Json<Value>| async move {
            Json(controller::reply::ReplyController::create(&*s.replies, path.id, body.0).await)
        }
    });

    builder
}
