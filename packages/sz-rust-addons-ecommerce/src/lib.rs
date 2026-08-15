//! # SZ-Rust 电商模板插件（可选业务模板）
//!
//! > **已填实（v0.3.2）**：21 测试，对接 InMemoryRepository，可选业务模板，非框架核心，按需启用。
//!
//! 提供电商基础骨架，包含：
//!
//! - **Order（订单）**：订单管理
//! - **OrderItem（订单项）**：订单商品明细
//! - **Cart（购物车）**：购物车管理
//!
//! ## 快速开始
//!
//! ```rust,ignore
//! use sz_rust_addons_ecommerce::register_routes;
//!
//! let ec_state = EcommerceState::default();
//! register_routes(builder, ec_state);
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

use crate::model::cart::CartItem;
use crate::model::order::Order;
use crate::model::order_item::OrderItem;

/// 订单仓储类型
pub type OrderRepo = Arc<InMemoryRepository<Order>>;
/// 订单项仓储类型
pub type OrderItemRepo = Arc<InMemoryRepository<OrderItem>>;
/// 购物车仓储类型
pub type CartRepo = Arc<InMemoryRepository<CartItem>>;

/// 电商应用共享状态
#[derive(Clone, Default)]
pub struct EcommerceState {
    pub orders: OrderRepo,
    pub order_items: OrderItemRepo,
    pub carts: CartRepo,
}

#[derive(Deserialize)]
struct ListQuery {
    page: Option<u64>,
    page_size: Option<u64>,
    status: Option<String>,
    order_id: Option<i64>,
    user_id: Option<i64>,
}

#[derive(Deserialize)]
struct IdPath {
    id: i64,
}

/// 注册电商所有路由。
pub fn register_routes<S>(builder: RouterBuilder<S>, state: EcommerceState) -> RouterBuilder<S>
where
    S: Clone + Send + Sync + 'static,
{
    let base = "/api/ecommerce";

    // 订单
    let builder = builder.get(&format!("{}/orders", base), {
        let s = state.clone();
        move |q: Query<ListQuery>| async move {
            Json(
                controller::order::OrderController::list(
                    &*s.orders,
                    q.page.unwrap_or(1),
                    q.page_size.unwrap_or(20),
                    q.status.clone(),
                )
                .await,
            )
        }
    });
    let builder = builder.post(&format!("{}/orders", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(controller::order::OrderController::create(&*s.orders, body.0).await)
        }
    });
    let builder = builder.get(&format!("{}/orders/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::order::OrderController::get(&*s.orders, path.id).await)
        }
    });
    let builder = builder.put(&format!("{}/orders/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>, body: Json<Value>| async move {
            Json(controller::order::OrderController::update(&*s.orders, path.id, body.0).await)
        }
    });
    let builder = builder.delete(&format!("{}/orders/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::order::OrderController::delete(&*s.orders, path.id).await)
        }
    });
    let builder = builder.post(&format!("{}/orders/{{id}}/cancel", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::order::OrderController::cancel(&*s.orders, path.id).await)
        }
    });
    let builder = builder.post(&format!("{}/orders/{{id}}/pay", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::order::OrderController::pay(&*s.orders, path.id).await)
        }
    });
    let builder = builder.post(&format!("{}/orders/{{id}}/ship", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::order::OrderController::ship(&*s.orders, path.id).await)
        }
    });
    let builder = builder.post(&format!("{}/orders/{{id}}/complete", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::order::OrderController::complete(&*s.orders, path.id).await)
        }
    });

    // 订单项
    let builder = builder.get(&format!("{}/order_items", base), {
        let s = state.clone();
        move |q: Query<ListQuery>| async move {
            Json(
                controller::order_item::OrderItemController::list(
                    &*s.order_items,
                    q.page.unwrap_or(1),
                    q.page_size.unwrap_or(20),
                    q.order_id,
                )
                .await,
            )
        }
    });
    let builder = builder.post(&format!("{}/order_items", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(controller::order_item::OrderItemController::create(&*s.order_items, body.0).await)
        }
    });
    let builder = builder.delete(&format!("{}/order_items/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(
                controller::order_item::OrderItemController::delete(&*s.order_items, path.id).await,
            )
        }
    });

    // 购物车
    let builder = builder.get(&format!("{}/cart", base), {
        let s = state.clone();
        move |q: Query<ListQuery>| async move {
            let uid = q.user_id.unwrap_or(0);
            Json(controller::cart::CartController::list(&*s.carts, uid).await)
        }
    });
    let builder = builder.post(&format!("{}/cart", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(controller::cart::CartController::add(&*s.carts, body.0).await)
        }
    });
    let builder = builder.put(&format!("{}/cart/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>, body: Json<Value>| async move {
            let qty: i64 = body.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1);
            Json(controller::cart::CartController::update_qty(&*s.carts, path.id, qty).await)
        }
    });
    let builder = builder.delete(&format!("{}/cart/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::cart::CartController::delete(&*s.carts, path.id).await)
        }
    });
    let builder = builder.delete(&format!("{}/cart/clear/{{user_id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::cart::CartController::clear(&*s.carts, path.id).await)
        }
    });

    builder
}
