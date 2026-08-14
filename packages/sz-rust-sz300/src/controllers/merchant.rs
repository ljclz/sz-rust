//! 商户控制器 — 仅负责 HTTP 请求/响应处理，SQL 逻辑下沉到 [`crate::services::merchant_service`]
//!
//! 2026-07-25 重构（修复 P0-4：控制器分层违反）。
//!
//! ## 重构前
//!
//! 控制器内嵌 24 处直接 SQL（`format!` 拼接 + `conn.query_with_params`），违反分层架构。
//!
//! ## 重构后
//!
//! - 控制器：解析请求参数 → 调用 [`crate::services::merchant_service::MerchantService`] → 格式化响应
//! - 服务层：构建 SQL、执行查询、返回领域数据
//! - 模型层：[`crate::models::merchant::Merchant`] 定义实体结构

use crate::controllers::common::{extract_fields_by_whitelist, parse_pagination};
use crate::services::merchant_service::MerchantService;
use crate::services::row_to_json;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use sz_rust_core::controller::SzController;
use tracing::{info, warn};

struct MerchantController;
impl SzController for MerchantController {}

impl MerchantController {
    /// 分页查询商户列表
    async fn list(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = MerchantController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let (page, page_size) = parse_pagination(&data, 15);

                info!("查询商户列表: page={}, page_size={}", page, page_size);

                match MerchantService::list(&state.db_pool, page, page_size).await {
                    Ok(page_data) => {
                        let list: Vec<serde_json::Value> =
                            page_data.list.iter().map(row_to_json).collect();
                        info!("商户列表查询成功: total={}", page_data.total);
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
                warn!("商户列表查询参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 根据 merchant_id 查询单个商户信息
    async fn info(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = MerchantController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let merchant_id = match data.get("merchant_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 merchant_id 参数", json!({}), 0),
                };

                info!("查询商户信息: merchant_id={}", merchant_id);

                match MerchantService::get(&state.db_pool, merchant_id).await {
                    Ok(Some(row)) => {
                        let merchant = row_to_json(&row);
                        info!("商户查询完成: merchant_id={}", merchant_id);
                        ctrl.render_success(
                            "success",
                            json!({
                                "merchant": merchant
                            }),
                        )
                    }
                    Ok(None) => ctrl.render_error("商户不存在", json!({}), 0),
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
            }
            Err(e) => {
                warn!("商户信息查询参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 创建商户
    async fn create(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = MerchantController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                // 数据验证（对齐 PHP think\Validate）
                // 规则：name 必填，contact_phone 手机号格式（11 位数字）
                let mut validator = sz_rust_core::validate::Validate::new()
                    .rule("name|商户名称", "require")
                    .rule("contact_phone|联系电话", "require|regex:^1[3-9]\\d{9}$");
                if let Err(e) = validator.check(&data) {
                    return ctrl.render_error(format!("参数验证失败: {}", e), json!({}), 0);
                }

                let name = data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if name.trim().is_empty() {
                    return ctrl.render_error("商户名称不能为空", json!({}), 0);
                }

                let merchant = crate::models::merchant::Merchant {
                    merchant_id: None,
                    market_id: data.get("market_id").and_then(|v| v.as_i64()).unwrap_or(0),
                    name: name.clone(),
                    stall_no: data
                        .get("stall_no")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    contact_phone: data
                        .get("contact_phone")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    category: data
                        .get("category")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    status: data.get("status").and_then(|v| v.as_i64()).unwrap_or(1) as i8,
                    bank_account: data
                        .get("bank_account")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    bank_name: data
                        .get("bank_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    created_at: None,
                    updated_at: None,
                };

                info!("创建商户: name={}", merchant.name);

                match MerchantService::create(&state.db_pool, &merchant).await {
                    Ok(()) => {
                        info!("商户创建成功: name={}", name);
                        ctrl.render_success("创建成功", json!({}))
                    }
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
            }
            Err(e) => {
                warn!("创建商户参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 更新商户信息 — 动态字段更新
    async fn update(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = MerchantController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let merchant_id = match data.get("merchant_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 merchant_id 参数", json!({}), 0),
                };

                info!("更新商户: merchant_id={}", merchant_id);

                // 从 data 提取可更新字段（仅白名单列）
                let allowed_keys: &[&str] = &[
                    "market_id",
                    "name",
                    "stall_no",
                    "contact_phone",
                    "category",
                    "status",
                    "bank_account",
                    "bank_name",
                ];
                let fields = extract_fields_by_whitelist(&data, allowed_keys);

                match MerchantService::update(&state.db_pool, merchant_id, fields).await {
                    Ok(()) => {
                        info!("商户更新成功: merchant_id={}", merchant_id);
                        ctrl.render_success("更新成功", json!({}))
                    }
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
            }
            Err(e) => {
                warn!("更新商户参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 删除商户（软删除）
    async fn delete(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = MerchantController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let merchant_id = match data.get("merchant_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 merchant_id 参数", json!({}), 0),
                };

                info!("删除商户: merchant_id={}", merchant_id);

                match MerchantService::delete(&state.db_pool, merchant_id).await {
                    Ok(()) => {
                        info!("商户删除成功: merchant_id={}", merchant_id);
                        ctrl.render_success("删除成功", json!({}))
                    }
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
            }
            Err(e) => {
                warn!("删除商户参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }
}

/// 商户列表查询（对齐 PHP MerchantController::list）
#[tracing::instrument(skip(state, req))]
pub async fn list(State(state): State<AppState>, req: Request<Body>) -> Response {
    MerchantController::list(&state, req).await
}

/// 商户详情查询（对齐 PHP MerchantController::info）
#[tracing::instrument(skip(state, req))]
pub async fn info(State(state): State<AppState>, req: Request<Body>) -> Response {
    MerchantController::info(&state, req).await
}

/// 创建商户（对齐 PHP MerchantController::create）
#[tracing::instrument(skip(state, req))]
pub async fn create(State(state): State<AppState>, req: Request<Body>) -> Response {
    MerchantController::create(&state, req).await
}

/// 更新商户信息（对齐 PHP MerchantController::update）
#[tracing::instrument(skip(state, req))]
pub async fn update(State(state): State<AppState>, req: Request<Body>) -> Response {
    MerchantController::update(&state, req).await
}

/// 删除商户（软删除，对齐 PHP MerchantController::delete）
#[tracing::instrument(skip(state, req))]
pub async fn delete(State(state): State<AppState>, req: Request<Body>) -> Response {
    MerchantController::delete(&state, req).await
}
