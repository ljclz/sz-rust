//! # SZ-Rust ERP 模板插件（可选业务模板）
//!
//! > **已填实（v0.3.2）**：23 测试，对接 InMemoryRepository，可选业务模板，非框架核心，按需启用。
//!
//! 提供 ERP（企业资源计划）基础骨架，包含：
//!
//! - **Product（商品）**：商品库存管理
//! - **Supplier（供应商）**：供应商管理
//! - **PurchaseOrder（采购单）**：采购流程管理
//!
//! ## 快速开始
//!
//! ```rust,ignore
//! use sz_rust_addons_erp::register_routes;
//!
//! let erp_state = ErpState::default();
//! register_routes(builder, erp_state);
//! ```

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

use crate::model::product::Product;
use crate::model::purchase_order::PurchaseOrder;
use crate::model::supplier::Supplier;

/// 商品仓储类型
pub type ProductRepo = Arc<InMemoryRepository<Product>>;
/// 供应商仓储类型
pub type SupplierRepo = Arc<InMemoryRepository<Supplier>>;
/// 采购单仓储类型
pub type PurchaseOrderRepo = Arc<InMemoryRepository<PurchaseOrder>>;

/// ERP 应用共享状态
#[derive(Clone, Default)]
pub struct ErpState {
    /// 商品仓储
    pub products: ProductRepo,
    /// 供应商仓储
    pub suppliers: SupplierRepo,
    /// 采购单仓储
    pub purchase_orders: PurchaseOrderRepo,
}

#[derive(Deserialize)]
struct ListQuery {
    page: Option<u64>,
    page_size: Option<u64>,
    keyword: Option<String>,
    category: Option<String>,
    status: Option<String>,
}

#[derive(Deserialize)]
struct IdPath {
    id: i64,
}

/// 注册 ERP 所有路由。
pub fn register_routes<S>(builder: RouterBuilder<S>, state: ErpState) -> RouterBuilder<S>
where
    S: Clone + Send + Sync + 'static,
{
    let base = "/api/erp";

    // 商品
    let builder = builder.get(&format!("{}/products", base), {
        let s = state.clone();
        move |q: Query<ListQuery>| async move {
            Json(
                controller::product::ProductController::list(
                    &*s.products,
                    q.page.unwrap_or(1),
                    q.page_size.unwrap_or(20),
                    q.category.clone(),
                )
                .await,
            )
        }
    });
    let builder = builder.post(&format!("{}/products", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(controller::product::ProductController::create(&*s.products, body.0).await)
        }
    });
    let builder = builder.get(&format!("{}/products/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::product::ProductController::get(&*s.products, path.id).await)
        }
    });
    let builder = builder.put(&format!("{}/products/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>, body: Json<Value>| async move {
            Json(
                controller::product::ProductController::update(&*s.products, path.id, body.0).await,
            )
        }
    });
    let builder = builder.delete(&format!("{}/products/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::product::ProductController::delete(&*s.products, path.id).await)
        }
    });

    // 供应商
    let builder = builder.get(&format!("{}/suppliers", base), {
        let s = state.clone();
        move |q: Query<ListQuery>| async move {
            Json(
                controller::supplier::SupplierController::list(
                    &*s.suppliers,
                    q.page.unwrap_or(1),
                    q.page_size.unwrap_or(20),
                    q.keyword.clone(),
                )
                .await,
            )
        }
    });
    let builder = builder.post(&format!("{}/suppliers", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(controller::supplier::SupplierController::create(&*s.suppliers, body.0).await)
        }
    });
    let builder = builder.get(&format!("{}/suppliers/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::supplier::SupplierController::get(&*s.suppliers, path.id).await)
        }
    });
    let builder = builder.put(&format!("{}/suppliers/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>, body: Json<Value>| async move {
            Json(
                controller::supplier::SupplierController::update(&*s.suppliers, path.id, body.0)
                    .await,
            )
        }
    });
    let builder = builder.delete(&format!("{}/suppliers/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(controller::supplier::SupplierController::delete(&*s.suppliers, path.id).await)
        }
    });

    // 采购单
    let builder = builder.get(&format!("{}/purchase_orders", base), {
        let s = state.clone();
        move |q: Query<ListQuery>| async move {
            Json(
                controller::purchase_order::PurchaseOrderController::list(
                    &*s.purchase_orders,
                    q.page.unwrap_or(1),
                    q.page_size.unwrap_or(20),
                    q.status.clone(),
                )
                .await,
            )
        }
    });
    let builder = builder.post(&format!("{}/purchase_orders", base), {
        let s = state.clone();
        move |body: Json<Value>| async move {
            Json(
                controller::purchase_order::PurchaseOrderController::create(
                    &*s.purchase_orders,
                    body.0,
                )
                .await,
            )
        }
    });
    let builder = builder.get(&format!("{}/purchase_orders/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(
                controller::purchase_order::PurchaseOrderController::get(
                    &*s.purchase_orders,
                    path.id,
                )
                .await,
            )
        }
    });
    let builder = builder.put(&format!("{}/purchase_orders/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>, body: Json<Value>| async move {
            Json(
                controller::purchase_order::PurchaseOrderController::update(
                    &*s.purchase_orders,
                    path.id,
                    body.0,
                )
                .await,
            )
        }
    });
    let builder = builder.delete(&format!("{}/purchase_orders/{{id}}", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(
                controller::purchase_order::PurchaseOrderController::delete(
                    &*s.purchase_orders,
                    path.id,
                )
                .await,
            )
        }
    });
    let builder = builder.post(&format!("{}/purchase_orders/{{id}}/approve", base), {
        let s = state.clone();
        move |path: Path<IdPath>| async move {
            Json(
                controller::purchase_order::PurchaseOrderController::approve(
                    &*s.purchase_orders,
                    path.id,
                )
                .await,
            )
        }
    });

    builder
}
