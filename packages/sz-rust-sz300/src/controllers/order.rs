//! 订单控制器 — 仅负责 HTTP 请求/响应处理，SQL 逻辑下沉到 [`crate::services::order_service`]
//!
//! 2026-07-25 重构（修复 P0-4：控制器分层违反）。
//!
//! ## 重构前
//!
//! 控制器内嵌 28 处直接 SQL（`format!` 拼接 + `conn.query_with_params`），违反分层架构。
//!
//! ## 重构后
//!
//! - 控制器：解析请求参数 → 调用 [`crate::services::order_service::OrderService`] → 格式化响应
//! - 服务层：构建 SQL、执行查询、返回领域数据
//! - 模型层：[`crate::models::order::Order`] 与 [`crate::models::order_item::OrderItem`] 定义实体结构

use crate::controllers::common::parse_pagination;
use crate::services::auth_service;
use crate::services::order_service::{OrderFilters, OrderService};
use crate::services::row_to_json;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use sz_rust_core::controller::SzController;
use sz_rust_core::hooks::{HookContext, HookEvent};
use sz_rust_core::plugin::event_bus::EventBus;
use tracing::{info, warn};

struct OrderController;
impl SzController for OrderController {}

impl OrderController {
    /// 分页查询订单列表，支持按 merchant_id/device_id/status/date_range 筛选
    async fn list(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = OrderController;
        // 安全修复 H-1：先解析服务端身份，强制商户数据边界
        let owned_merchant_id = match auth_service::current_user(&req).map(|u| u.id) {
            Some(uid) => match auth_service::resolve_merchant_id(uid, None).await {
                Ok(mid) => mid,
                Err(e) => return ctrl.render_error(&e, json!({}), 0),
            },
            None => return ctrl.render_error("未认证请求", json!({}), 0),
        };
        match ctrl.post_data(req).await {
            Ok(data) => {
                let (page, page_size) = parse_pagination(&data, 15);
                let filters = OrderFilters {
                    merchant_id: Some(owned_merchant_id), // 服务端权威值，忽略请求体
                    device_id: data.get("device_id").and_then(|v| v.as_i64()),
                    status: data.get("status").and_then(|v| v.as_i64()),
                    start_date: data
                        .get("start_date")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    end_date: data
                        .get("end_date")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                };

                info!("查询订单列表: page={}, page_size={}", page, page_size);

                match OrderService::list(&state.db_pool, page, page_size, filters).await {
                    Ok(page_data) => {
                        let list: Vec<serde_json::Value> =
                            page_data.list.iter().map(row_to_json).collect();
                        info!("订单列表查询成功: total={}", page_data.total);
                        ctrl.render_success(
                            "success",
                            json!({
                                "list": list,
                                "total": page_data.total,
                                "page": page,
                                "page_size": page_size
                            }),
                        )
                    }
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
            }
            Err(e) => {
                warn!("订单列表查询参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 根据 order_id 查询订单详情（含订单项）
    async fn info(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = OrderController;
        // 安全修复 H-1：先解析服务端身份，强制商户数据边界
        let owned_merchant_id = match auth_service::current_user(&req).map(|u| u.id) {
            Some(uid) => match auth_service::resolve_merchant_id(uid, None).await {
                Ok(mid) => mid,
                Err(e) => return ctrl.render_error(&e, json!({}), 0),
            },
            None => return ctrl.render_error("未认证请求", json!({}), 0),
        };
        match ctrl.post_data(req).await {
            Ok(data) => {
                let order_id = match data.get("order_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    // C5：错误消息本地化（message_key + i18n 翻译，替代硬编码中文）
                    _ => {
                        let i18n = crate::i18n_error::order_error_i18n();
                        let err = sz_rust_http_facade::BaseException::new(
                            sz_rust_http_facade::ErrorCode::ValidateFailed,
                            "invalid order_id",
                        )
                        .with_message_key("errors.order_id_invalid");
                        return ctrl.render_error(
                            sz_rust_mvc_facade::i18n_error::localize_exception(&err, &i18n, None),
                            json!({}),
                            0,
                        );
                    }
                };

                info!("查询订单详情: order_id={}", order_id);

                match OrderService::get_with_items(&state.db_pool, order_id, owned_merchant_id)
                    .await
                {
                    Ok(Some(detail)) => {
                        let order = row_to_json(&detail.order);
                        let items: Vec<serde_json::Value> =
                            detail.items.iter().map(row_to_json).collect();
                        info!(
                            "订单详情查询完成: order_id={}, items={}",
                            order_id,
                            items.len()
                        );
                        ctrl.render_success(
                            "success",
                            json!({
                                "order": order,
                                "items": items
                            }),
                        )
                    }
                    Ok(None) => ctrl.render_error("订单不存在", json!({}), 0),
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
            }
            Err(e) => {
                warn!("订单详情查询参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 创建订单（含订单项）
    async fn create(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = OrderController;
        // 安全修复 H-1：先解析服务端身份；merchant_id 以服务端权威值为准，忽略请求体
        let owned_merchant_id = match auth_service::current_user(&req).map(|u| u.id) {
            Some(uid) => match auth_service::resolve_merchant_id(uid, None).await {
                Ok(mid) => mid,
                Err(e) => return ctrl.render_error(&e, json!({}), 0),
            },
            None => return ctrl.render_error("未认证请求", json!({}), 0),
        };
        match ctrl.post_data(req).await {
            Ok(data) => {
                let order_no = data
                    .get("order_no")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if order_no.trim().is_empty() {
                    return ctrl.render_error("订单号不能为空", json!({}), 0);
                }

                let order = crate::models::order::Order {
                    order_id: None,
                    order_no: order_no.clone(),
                    merchant_id: owned_merchant_id, // 服务端权威值，忽略请求体
                    device_id: data.get("device_id").and_then(|v| v.as_i64()).unwrap_or(0),
                    total_fen: data.get("total_fen").and_then(|v| v.as_i64()).unwrap_or(0),
                    total_weight_g: data
                        .get("total_weight_g")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                    item_count: data.get("item_count").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    status: 1, // 服务层固定为 1=待支付，此字段被忽略
                    pay_method: data.get("pay_method").and_then(|v| v.as_i64()).unwrap_or(0) as i8,
                    pay_at: None,
                    offline_seq: data
                        .get("offline_seq")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    created_at: None,
                    updated_at: None,
                };

                // 解析订单项
                let items: Vec<crate::models::order_item::OrderItem> = data
                    .get("items")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .map(|item| crate::models::order_item::OrderItem {
                                item_id: None,
                                order_id: 0, // 由服务层填充新订单 ID
                                good_id: item.get("good_id").and_then(|v| v.as_i64()).unwrap_or(0),
                                good_name: item
                                    .get("good_name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                price_fen: item
                                    .get("price_fen")
                                    .and_then(|v| v.as_i64())
                                    .unwrap_or(0),
                                weight_g: item
                                    .get("weight_g")
                                    .and_then(|v| v.as_i64())
                                    .unwrap_or(0),
                                total_fen: item
                                    .get("total_fen")
                                    .and_then(|v| v.as_i64())
                                    .unwrap_or(0),
                                quantity: item.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1)
                                    as i32,
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                info!(
                    "创建订单: order_no={}, merchant_id={}",
                    order_no, order.merchant_id
                );

                // 安全修复（黑帽审计 A16）：业务逻辑校验
                // 1. 金额/重量/数量必须非负（防负数订单记账污染）
                // 2. 明细非空时，主单金额必须等于订单项金额合计（防客户端虚报金额）
                if order.total_fen < 0 || order.total_weight_g < 0 || order.item_count < 0 {
                    return ctrl.render_error("订单金额/重量/数量不能为负", json!({}), 0);
                }
                for item in &items {
                    if item.price_fen < 0 || item.weight_g < 0 || item.quantity < 0 {
                        return ctrl.render_error("订单明细金额/重量/数量不能为负", json!({}), 0);
                    }
                }
                if !items.is_empty() {
                    let items_total: i64 = items.iter().map(|i| i.total_fen.max(0)).sum();
                    if items_total != order.total_fen {
                        return ctrl.render_error(
                            "订单总金额与明细不一致（服务端校验失败）",
                            json!({}),
                            0,
                        );
                    }
                }

                // 触发 before_insert 钩子（对齐 PHP think-orm Model 钩子）
                let hook_ctx = HookContext::new();
                state
                    .hook_registry
                    .dispatch(HookEvent::BeforeInsert, &hook_ctx)
                    .ok();

                match OrderService::create(&state.db_pool, &order, &items).await {
                    Ok(new_order_id) => {
                        info!(
                            "订单创建成功: order_id={}, order_no={}",
                            new_order_id, order_no
                        );
                        // 触发 after_insert 钩子
                        state
                            .hook_registry
                            .dispatch(HookEvent::AfterInsert, &hook_ctx)
                            .ok();
                        // 触发 order.created 事件（异步 fire-and-forget，错误不影响主流程）
                        let event = sz_rust_core::plugin::event_bus::PluginEvent {
                            id: 0,
                            tenant_id: order.merchant_id,
                            event_type: "order.created".to_string(),
                            source_plugin: "sz300".to_string(),
                            payload: json!({
                                "order_id": new_order_id,
                                "order_no": order_no,
                                "merchant_id": order.merchant_id,
                            }),
                        };
                        let bus = state.event_bus.clone();
                        tokio::spawn(async move {
                            if let Err(e) = bus.publish(&event).await {
                                tracing::warn!("order.created 事件发布失败: {}", e);
                            }
                        });
                        ctrl.render_success(
                            "下单成功",
                            json!({
                                "order_id": new_order_id,
                                "order_no": order_no
                            }),
                        )
                    }
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
            }
            Err(e) => {
                warn!("创建订单参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }
}

/// 订单列表查询（对齐 PHP OrderController::list）
#[tracing::instrument(skip(state, req))]
pub async fn list(State(state): State<AppState>, req: Request<Body>) -> Response {
    OrderController::list(&state, req).await
}

/// 订单详情查询（对齐 PHP OrderController::info）
#[tracing::instrument(skip(state, req))]
pub async fn info(State(state): State<AppState>, req: Request<Body>) -> Response {
    OrderController::info(&state, req).await
}

/// 创建订单（对齐 PHP OrderController::create）
#[tracing::instrument(skip(state, req))]
pub async fn create(State(state): State<AppState>, req: Request<Body>) -> Response {
    OrderController::create(&state, req).await
}
