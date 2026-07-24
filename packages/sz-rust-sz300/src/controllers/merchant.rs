use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use std::collections::HashMap;
use sz_orm_core::{value_to_json, Value};
use sz_rust_core::controller::SzController;
use tracing::{info, warn};

struct MerchantController;
impl SzController for MerchantController {}

fn row_to_json(row: &HashMap<String, Value>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (k, v) in row {
        obj.insert(k.clone(), value_to_json(v.clone()));
    }
    serde_json::Value::Object(obj)
}

fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

impl MerchantController {
    /// 分页查询商户列表
    async fn list(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = MerchantController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let page = data
                    .get("page")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(1)
                    .max(1);
                let page_size = data
                    .get("page_size")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(15)
                    .clamp(1, 100);
                let offset = (page - 1) * page_size;

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                let count_sql = "SELECT COUNT(*) as total FROM merchant".to_string();
                let count_result = match conn.query(&count_sql).await {
                    Ok(rows) => rows,
                    Err(e) => return ctrl.render_error(&format!("查询失败: {}", e), json!({}), 0),
                };
                let total: i64 = count_result
                    .first()
                    .and_then(|row| row.get("total"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                let list_sql = format!(
                    "SELECT * FROM merchant ORDER BY merchant_id DESC LIMIT {} OFFSET {}",
                    page_size, offset
                );
                let rows = match conn.query(&list_sql).await {
                    Ok(rows) => rows,
                    Err(e) => return ctrl.render_error(&format!("查询失败: {}", e), json!({}), 0),
                };

                let list: Vec<serde_json::Value> =
                    rows.iter().map(|row| row_to_json(row)).collect();

                ctrl.render_success(
                    "success",
                    json!({
                        "list": list,
                        "total": total,
                        "page": page,
                        "page_size": page_size
                    }),
                )
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

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                let sql = format!("SELECT * FROM merchant WHERE merchant_id = {}", merchant_id);
                let rows = match conn.query(&sql).await {
                    Ok(rows) => rows,
                    Err(e) => return ctrl.render_error(&format!("查询失败: {}", e), json!({}), 0),
                };

                if rows.is_empty() {
                    return ctrl.render_error("商户不存在", json!({}), 0);
                }

                let merchant = row_to_json(&rows[0]);
                info!("商户查询完成: merchant_id={}", merchant_id);
                ctrl.render_success(
                    "success",
                    json!({
                        "merchant": merchant
                    }),
                )
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
                let name = data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if name.trim().is_empty() {
                    return ctrl.render_error("商户名称不能为空", json!({}), 0);
                }

                let market_id = data.get("market_id").and_then(|v| v.as_i64()).unwrap_or(0);
                let stall_no = data
                    .get("stall_no")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let contact_phone = data
                    .get("contact_phone")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let category = data
                    .get("category")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let status = data.get("status").and_then(|v| v.as_i64()).unwrap_or(1);
                let bank_account = data
                    .get("bank_account")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let bank_name = data
                    .get("bank_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                info!("创建商户: name={}", name);

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                let sql = format!(
                    "INSERT INTO merchant (market_id, name, stall_no, contact_phone, category, status, bank_account, bank_name, created_at, updated_at) VALUES ({}, '{}', '{}', '{}', '{}', {}, '{}', '{}', NOW(), NOW())",
                    market_id,
                    sql_escape(&name),
                    sql_escape(&stall_no),
                    sql_escape(&contact_phone),
                    sql_escape(&category),
                    status,
                    sql_escape(&bank_account),
                    sql_escape(&bank_name),
                );
                match conn.execute(&sql).await {
                    Ok(_) => {
                        info!("商户创建成功: name={}", name);
                        ctrl.render_success("创建成功", json!({}))
                    }
                    Err(e) => ctrl.render_error(&format!("创建失败: {}", e), json!({}), 0),
                }
            }
            Err(e) => {
                warn!("创建商户参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 更新商户信息
    async fn update(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = MerchantController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let merchant_id = match data.get("merchant_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 merchant_id 参数", json!({}), 0),
                };

                info!("更新商户: merchant_id={}", merchant_id);

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                // 构建动态 SET 子句
                let mut set_clauses: Vec<String> = Vec::new();
                if let Some(v) = data.get("name").and_then(|v| v.as_str()) {
                    set_clauses.push(format!("name = '{}'", sql_escape(v)));
                }
                if let Some(v) = data.get("market_id").and_then(|v| v.as_i64()) {
                    set_clauses.push(format!("market_id = {}", v));
                }
                if let Some(v) = data.get("stall_no").and_then(|v| v.as_str()) {
                    set_clauses.push(format!("stall_no = '{}'", sql_escape(v)));
                }
                if let Some(v) = data.get("contact_phone").and_then(|v| v.as_str()) {
                    set_clauses.push(format!("contact_phone = '{}'", sql_escape(v)));
                }
                if let Some(v) = data.get("category").and_then(|v| v.as_str()) {
                    set_clauses.push(format!("category = '{}'", sql_escape(v)));
                }
                if let Some(v) = data.get("status").and_then(|v| v.as_i64()) {
                    set_clauses.push(format!("status = {}", v));
                }
                if let Some(v) = data.get("bank_account").and_then(|v| v.as_str()) {
                    set_clauses.push(format!("bank_account = '{}'", sql_escape(v)));
                }
                if let Some(v) = data.get("bank_name").and_then(|v| v.as_str()) {
                    set_clauses.push(format!("bank_name = '{}'", sql_escape(v)));
                }

                if set_clauses.is_empty() {
                    return ctrl.render_error("未提供需要更新的字段", json!({}), 0);
                }

                set_clauses.push("updated_at = NOW()".to_string());

                let sql = format!(
                    "UPDATE merchant SET {} WHERE merchant_id = {}",
                    set_clauses.join(", "),
                    merchant_id
                );
                match conn.execute(&sql).await {
                    Ok(_) => {
                        info!("商户更新成功: merchant_id={}", merchant_id);
                        ctrl.render_success("更新成功", json!({}))
                    }
                    Err(e) => ctrl.render_error(&format!("更新失败: {}", e), json!({}), 0),
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

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                let sql = format!(
                    "UPDATE merchant SET status = 0, updated_at = NOW() WHERE merchant_id = {}",
                    merchant_id
                );
                match conn.execute(&sql).await {
                    Ok(_) => {
                        info!("商户删除成功: merchant_id={}", merchant_id);
                        ctrl.render_success("删除成功", json!({}))
                    }
                    Err(e) => ctrl.render_error(&format!("删除失败: {}", e), json!({}), 0),
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
