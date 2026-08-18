//! # SZ-Rust IM 模板插件（可选业务模板）
//!
//! 提供即时通讯基础骨架，包含：
//!
//! - **Conversation（会话）**：聊天会话管理
//! - **Message（消息）**：消息发送与接收
//! - **UserStatus（用户状态）**：在线/离线状态管理

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

use crate::model::conversation::Conversation;
use crate::model::message::Message;
use crate::model::user_status::UserStatus;

pub type ConversationRepo = Arc<InMemoryRepository<Conversation>>;
pub type MessageRepo = Arc<InMemoryRepository<Message>>;
pub type UserStatusRepo = Arc<InMemoryRepository<UserStatus>>;

#[derive(Clone, Default)]
pub struct ImState {
    pub conversations: ConversationRepo,
    pub messages: MessageRepo,
    pub user_statuses: UserStatusRepo,
}

#[derive(Deserialize)]
struct ListQuery {
    page: Option<u64>,
    page_size: Option<u64>,
    user_id: Option<i64>,
}

#[derive(Deserialize)]
struct IdPath {
    id: i64,
}

pub fn register_routes<S>(builder: RouterBuilder<S>, state: ImState) -> RouterBuilder<S>
where
    S: Clone + Send + Sync + 'static,
{
    let base = "/api/im";

    let builder = builder.get(&format!("{}/conversations", base), {
        let s = state.clone();
        move |q: Query<ListQuery>| async move {
            Json(
                controller::conversation::ConversationController::list(
                    &*s.conversations,
                    q.user_id,
                    q.page.unwrap_or(1),
                    q.page_size.unwrap_or(20),
                )
                .await,
            )
        }
    });

    let builder = builder.post(&format!("{}/conversations", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(
                controller::conversation::ConversationController::create(&*s.conversations, body.0)
                    .await,
            )
        }
    });

    let builder = builder.get(&format!("{}/conversations/{{id}}/messages", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(
                controller::message::MessageController::list_by_conversation(&*s.messages, path.id)
                    .await,
            )
        }
    });

    let builder = builder.post(&format!("{}/conversations/{{id}}/messages", base), {
        let s = state.clone();
        move |path: Path<IdPath>, body: Json<Value>| async move {
            Json(
                controller::message::MessageController::create(&*s.messages, path.id, body.0).await,
            )
        }
    });

    let builder = builder.get(&format!("{}/users/{{id}}/status", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(
                controller::user_status::UserStatusController::get(&*s.user_statuses, path.id)
                    .await,
            )
        }
    });

    let builder = builder.put(&format!("{}/users/{{id}}/status", base), {
        let s = state.clone();
        move |path: Path<IdPath>, body: Json<Value>| async move {
            Json(
                controller::user_status::UserStatusController::update(
                    &*s.user_statuses,
                    path.id,
                    body.0,
                )
                .await,
            )
        }
    });

    builder
}
