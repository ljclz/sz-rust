//! # SZ-Rust CRM 模板插件
//!
//! 提供 CRM（客户关系管理）基础骨架，包含：
//!
//! - **Contact（联系人）**：客户联系人管理
//! - **Lead（线索）**：销售线索跟进
//! - **Deal（商机）**：商机阶段管理
//!
//! ## 快速开始
//!
//! ```rust,ignore
//! use sz_rust_addons_crm::register_routes;
//!
//! // 在应用路由注册时调用
//! let crm_state = CrmState::default();
//! register_routes(builder, crm_state);
//! ```
//!
//! ## 数据库表
//!
//! 运行迁移创建以下表：
//!
//! - `crm_contacts`
//! - `crm_leads`
//! - `crm_deals`

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

use crate::model::contact::Contact;
use crate::model::deal::Deal;
use crate::model::lead::Lead;

/// 联系人仓储类型
pub type ContactRepo = Arc<InMemoryRepository<Contact>>;
/// 线索仓储类型
pub type LeadRepo = Arc<InMemoryRepository<Lead>>;
/// 商机仓储类型
pub type DealRepo = Arc<InMemoryRepository<Deal>>;

/// CRM 应用共享状态
#[derive(Clone, Default)]
pub struct CrmState {
    /// 联系人仓储
    pub contacts: ContactRepo,
    /// 线索仓储
    pub leads: LeadRepo,
    /// 商机仓储
    pub deals: DealRepo,
}

// ─── Query / Path 提取器 ────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ListQuery {
    page: Option<u64>,
    page_size: Option<u64>,
    keyword: Option<String>,
    stage: Option<String>,
}

#[derive(Deserialize)]
struct IdPath {
    id: i64,
}

// ─── 路由注册 ────────────────────────────────────────────────────────────────

/// 注册 CRM 所有路由。
///
/// 通过闭包捕获 `state`，无需 `axum::extract::State` 提取器，
/// 因此可与任意应用状态类型共存。
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_addons_crm::{register_routes, CrmState};
///
/// let crm_state = CrmState::default();
/// let router = register_routes(RouterBuilder::new(), crm_state).build();
/// ```
pub fn register_routes<S>(builder: RouterBuilder<S>, state: CrmState) -> RouterBuilder<S>
where
    S: Clone + Send + Sync + 'static,
{
    let base = "/api/crm";

    // 联系人
    let builder = builder.get(&format!("{}/contacts", base), {
        let s = state.clone();
        move |q: Query<ListQuery>| async move {
            Json(
                controller::contact::ContactController::list(
                    &*s.contacts,
                    q.page.unwrap_or(1),
                    q.page_size.unwrap_or(20),
                    q.keyword.clone(),
                )
                .await,
            )
        }
    });

    let builder = builder.post(&format!("{}/contacts", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(controller::contact::ContactController::create(&*s.contacts, body.0).await)
        }
    });

    let builder = builder.get(&format!("{}/contacts/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::contact::ContactController::get(&*s.contacts, path.id).await)
        }
    });

    let builder = builder.put(&format!("{}/contacts/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>, body: Json<Value>| async move {
            Json(
                controller::contact::ContactController::update(&*s.contacts, path.id, body.0).await,
            )
        }
    });

    let builder = builder.delete(&format!("{}/contacts/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::contact::ContactController::delete(&*s.contacts, path.id).await)
        }
    });

    // 线索
    let builder = builder.get(&format!("{}/leads", base), {
        let s = state.clone();
        move |q: Query<ListQuery>| async move {
            Json(
                controller::lead::LeadController::list(
                    &*s.leads,
                    q.page.unwrap_or(1),
                    q.page_size.unwrap_or(20),
                    q.keyword.clone(),
                )
                .await,
            )
        }
    });

    let builder = builder.post(&format!("{}/leads", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(controller::lead::LeadController::create(&*s.leads, body.0).await)
        }
    });

    let builder = builder.get(&format!("{}/leads/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::lead::LeadController::get(&*s.leads, path.id).await)
        }
    });

    let builder = builder.put(&format!("{}/leads/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>, body: Json<Value>| async move {
            Json(controller::lead::LeadController::update(&*s.leads, path.id, body.0).await)
        }
    });

    let builder = builder.delete(&format!("{}/leads/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::lead::LeadController::delete(&*s.leads, path.id).await)
        }
    });

    let builder = builder.post(&format!("{}/leads/:id/convert", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::lead::LeadController::convert(&*s.leads, path.id).await)
        }
    });

    // 商机
    let builder = builder.get(&format!("{}/deals", base), {
        let s = state.clone();
        move |q: Query<ListQuery>| async move {
            Json(
                controller::deal::DealController::list(
                    &*s.deals,
                    q.page.unwrap_or(1),
                    q.page_size.unwrap_or(20),
                    q.stage.clone(),
                )
                .await,
            )
        }
    });

    let builder = builder.post(&format!("{}/deals", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(controller::deal::DealController::create(&*s.deals, body.0).await)
        }
    });

    let builder = builder.get(&format!("{}/deals/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::deal::DealController::get(&*s.deals, path.id).await)
        }
    });

    let builder = builder.put(&format!("{}/deals/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>, body: Json<Value>| async move {
            Json(controller::deal::DealController::update(&*s.deals, path.id, body.0).await)
        }
    });

    let builder = builder.delete(&format!("{}/deals/:id", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::deal::DealController::delete(&*s.deals, path.id).await)
        }
    });

    let builder = builder.get(&format!("{}/deals/pipeline", base), {
        let s = state.clone();
        move || async move { Json(controller::deal::DealController::pipeline(&*s.deals).await) }
    });

    builder
}
