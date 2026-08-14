//! 商品控制器 — 仅负责 HTTP 请求/响应处理，SQL 逻辑下沉到 [`crate::services::product_service`]
//!
//! 2026-07-25 重构（修复 P0-4：控制器分层违反）。
//!
//! ## 重构前
//!
//! 控制器内嵌 32 处直接 SQL（`format!` 拼接 + `conn.query_with_params`），违反分层架构。
//!
//! ## 重构后
//!
//! - 控制器：解析请求参数 → 调用 [`crate::services::product_service::ProductService`] → 格式化响应
//! - 服务层：构建 SQL、执行查询、返回领域数据
//! - 模型层：[`crate::models::product::Product`] 定义实体结构

use crate::controllers::common::{extract_fields_by_whitelist, parse_pagination};
use crate::services::auth_service;
use crate::services::product_service::{ProductFilters, ProductService};
use crate::services::row_to_json;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use sz_rust_core::controller::SzController;
use tracing::{info, warn};

struct ProductController;
impl SzController for ProductController {}

/// 校验商品是否归属指定商户（黑帽复核 A6 修复辅助）
///
/// 查询商品 merchant_id 并与身份值比对；商品不存在或归属不符均返回 false。
async fn product_belongs_to(state: &AppState, good_id: i64, merchant_id: i64) -> bool {
    match crate::services::product_service::ProductService::get(&state.db_pool, good_id).await {
        Ok(Some(row)) => row
            .get("merchant_id")
            .and_then(|v| v.as_i64())
            .map(|m| m == merchant_id)
            .unwrap_or(false),
        _ => false,
    }
}

impl ProductController {
    /// 分页查询商品列表，支持按 merchant_id/cat_id/keyword 筛选
    async fn list(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = ProductController;
        // 安全修复 H-1：merchant_id 以服务端身份为准（用户只能查自己商户）
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
                let filters = ProductFilters {
                    merchant_id: Some(owned_merchant_id), // 服务端权威值，忽略请求体
                    cat_id: data.get("cat_id").and_then(|v| v.as_i64()),
                    keyword: data
                        .get("keyword")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                };

                info!("查询商品列表: page={}, page_size={}", page, page_size);

                // 缓存读取：key 必须含 merchant_id（修复 H-1 跨租户缓存泄露）
                let cache_key =
                    format!("product:list:{}:{}:{}", owned_merchant_id, page, page_size);
                if let Some(cache) = &state.cache {
                    if let Ok(Some(cached)) = cache.get::<String>(&cache_key) {
                        if let Ok(cached_json) = serde_json::from_str::<serde_json::Value>(&cached)
                        {
                            tracing::debug!("商品列表缓存命中: {}", cache_key);
                            return ctrl.render_success("success", cached_json);
                        }
                    }
                }

                match ProductService::list(&state.db_pool, page, page_size, filters).await {
                    Ok(page_data) => {
                        let list: Vec<serde_json::Value> =
                            page_data.list.iter().map(row_to_json).collect();
                        info!("商品列表查询成功: total={}", page_data.total);
                        let resp_data = json!({
                            "list": list,
                            "total": page_data.total,
                            "page": page,
                            "page_size": page_size
                        });
                        // 缓存写入：TTL 300 秒
                        if let Some(cache) = &state.cache {
                            if let Ok(s) = serde_json::to_string(&resp_data) {
                                let _ = cache.set(
                                    &cache_key,
                                    s,
                                    Some(std::time::Duration::from_secs(300)),
                                );
                            }
                        }
                        ctrl.render_success("success", resp_data)
                    }
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
            }
            Err(e) => {
                warn!("商品列表查询参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 根据 good_id 查询单个商品信息
    async fn info(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = ProductController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let good_id = match data.get("good_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 good_id 参数", json!({}), 0),
                };

                info!("查询商品信息: good_id={}", good_id);

                match ProductService::get(&state.db_pool, good_id).await {
                    Ok(Some(row)) => {
                        let product = row_to_json(&row);
                        info!("商品查询完成: good_id={}", good_id);
                        ctrl.render_success("success", json!({ "product": product }))
                    }
                    Ok(None) => ctrl.render_error("商品不存在", json!({}), 0),
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
            }
            Err(e) => {
                warn!("商品信息查询参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 创建商品
    async fn create(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = ProductController;
        // 安全修复 H-1：merchant_id 以服务端身份为准
        let owned_merchant_id = match auth_service::current_user(&req).map(|u| u.id) {
            Some(uid) => match auth_service::resolve_merchant_id(uid, None).await {
                Ok(mid) => mid,
                Err(e) => return ctrl.render_error(&e, json!({}), 0),
            },
            None => return ctrl.render_error("未认证请求", json!({}), 0),
        };
        match ctrl.post_data(req).await {
            Ok(data) => {
                let name = data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if name.trim().is_empty() {
                    return ctrl.render_error("商品名称不能为空", json!({}), 0);
                }

                let product = crate::models::product::Product {
                    good_id: None,
                    merchant_id: owned_merchant_id, // 服务端权威值，忽略请求体
                    cat_id: data.get("cat_id").and_then(|v| v.as_i64()).unwrap_or(0),
                    name: name.clone(),
                    barcode: data
                        .get("barcode")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    price: data.get("price").and_then(|v| v.as_i64()).unwrap_or(0),
                    unit: data
                        .get("unit")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    ai_class_id: data
                        .get("ai_class_id")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                    image: data
                        .get("image")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    status: data.get("status").and_then(|v| v.as_i64()).unwrap_or(1) as i8,
                    created_at: None,
                    updated_at: None,
                };

                info!(
                    "创建商品: name={}, merchant_id={}",
                    product.name, product.merchant_id
                );

                match ProductService::create(&state.db_pool, &product).await {
                    Ok(new_id) => {
                        info!("商品创建成功: name={}, good_id={}", name, new_id);
                        ctrl.render_success("创建成功", json!({ "good_id": new_id }))
                    }
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
            }
            Err(e) => {
                warn!("创建商品参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 更新商品信息 — 动态字段更新
    async fn update(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = ProductController;
        // 安全修复（黑帽复核 A6）：仅允许更新本商户商品
        let owned_merchant_id = match auth_service::current_user(&req).map(|u| u.id) {
            Some(uid) => match auth_service::resolve_merchant_id(uid, None).await {
                Ok(mid) => mid,
                Err(e) => return ctrl.render_error(&e, json!({}), 0),
            },
            None => return ctrl.render_error("未认证请求", json!({}), 0),
        };
        match ctrl.post_data(req).await {
            Ok(data) => {
                let good_id = match data.get("good_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 good_id 参数", json!({}), 0),
                };

                info!("更新商品: good_id={}", good_id);

                // 归属校验：商品必须属于当前商户
                if !product_belongs_to(state, good_id, owned_merchant_id).await {
                    return ctrl.render_error("商品不存在", json!({}), 0);
                }

                // 从 data 提取可更新字段（仅白名单列；merchant_id 禁止更新防越权转移）
                let allowed_keys: &[&str] = &[
                    "cat_id",
                    "name",
                    "barcode",
                    "price",
                    "unit",
                    "ai_class_id",
                    "image",
                    "status",
                ];
                let fields = extract_fields_by_whitelist(&data, allowed_keys);

                match ProductService::update(&state.db_pool, good_id, fields).await {
                    Ok(()) => {
                        info!("商品更新成功: good_id={}", good_id);
                        ctrl.render_success("更新成功", json!({}))
                    }
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
            }
            Err(e) => {
                warn!("更新商品参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 删除商品（软删除）
    async fn delete(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = ProductController;
        // 安全修复（黑帽复核 A6）：仅允许删除本商户商品
        let owned_merchant_id = match auth_service::current_user(&req).map(|u| u.id) {
            Some(uid) => match auth_service::resolve_merchant_id(uid, None).await {
                Ok(mid) => mid,
                Err(e) => return ctrl.render_error(&e, json!({}), 0),
            },
            None => return ctrl.render_error("未认证请求", json!({}), 0),
        };
        match ctrl.post_data(req).await {
            Ok(data) => {
                let good_id = match data.get("good_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 good_id 参数", json!({}), 0),
                };

                info!("删除商品: good_id={}", good_id);

                // 归属校验：商品必须属于当前商户
                if !product_belongs_to(state, good_id, owned_merchant_id).await {
                    return ctrl.render_error("商品不存在", json!({}), 0);
                }

                match ProductService::delete(&state.db_pool, good_id).await {
                    Ok(()) => {
                        info!("商品删除成功: good_id={}", good_id);
                        ctrl.render_success("删除成功", json!({}))
                    }
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
            }
            Err(e) => {
                warn!("删除商品参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }
}

/// 商品列表查询（对齐 PHP ProductController::list）
#[tracing::instrument(skip(state, req))]
pub async fn list(State(state): State<AppState>, req: Request<Body>) -> Response {
    ProductController::list(&state, req).await
}

/// 商品详情查询（对齐 PHP ProductController::info）
#[tracing::instrument(skip(state, req))]
pub async fn info(State(state): State<AppState>, req: Request<Body>) -> Response {
    ProductController::info(&state, req).await
}

/// 创建商品（对齐 PHP ProductController::create）
#[tracing::instrument(skip(state, req))]
pub async fn create(State(state): State<AppState>, req: Request<Body>) -> Response {
    ProductController::create(&state, req).await
}

/// 更新商品信息（对齐 PHP ProductController::update）
#[tracing::instrument(skip(state, req))]
pub async fn update(State(state): State<AppState>, req: Request<Body>) -> Response {
    ProductController::update(&state, req).await
}

/// 删除商品（软删除，对齐 PHP ProductController::delete）
#[tracing::instrument(skip(state, req))]
pub async fn delete(State(state): State<AppState>, req: Request<Body>) -> Response {
    ProductController::delete(&state, req).await
}
