use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use std::collections::HashMap;
use tracing::{info, warn};
use sz_rust_core::controller::SzController;
use sz_orm_core::{Value, value_to_json};
use crate::state::AppState;

struct OrderController;
impl SzController for OrderController {}

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

impl OrderController {
    /// 分页查询订单列表，支持按 merchant_id/device_id/status/date_range 筛选
    async fn list(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = OrderController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let page = data.get("page").and_then(|v| v.as_i64()).unwrap_or(1).max(1);
                let page_size = data.get("page_size").and_then(|v| v.as_i64()).unwrap_or(15).clamp(1, 100);
                let merchant_id = data.get("merchant_id").and_then(|v| v.as_i64());
                let device_id = data.get("device_id").and_then(|v| v.as_i64());
                let status = data.get("status").and_then(|v| v.as_i64());
                let start_date = data.get("start_date").and_then(|v| v.as_str());
                let end_date = data.get("end_date").and_then(|v| v.as_str());
                let offset = (page - 1) * page_size;

                info!("查询订单列表: page={}, page_size={}", page, page_size);

                // 构建 WHERE 条件
                let mut conditions: Vec<String> = Vec::new();
                if let Some(mid) = merchant_id {
                    conditions.push(format!("merchant_id = {}", mid));
                }
                if let Some(did) = device_id {
                    conditions.push(format!("device_id = {}", did));
                }
                if let Some(st) = status {
                    conditions.push(format!("status = {}", st));
                }
                if let Some(sd) = start_date {
                    if !sd.trim().is_empty() {
                        conditions.push(format!("created_at >= '{}'", sql_escape(sd.trim())));
                    }
                }
                if let Some(ed) = end_date {
                    if !ed.trim().is_empty() {
                        conditions.push(format!("created_at <= '{}'", sql_escape(ed.trim())));
                    }
                }
                let where_clause = if conditions.is_empty() {
                    String::new()
                } else {
                    format!("WHERE {}", conditions.join(" AND "))
                };

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0),
                };

                // 总数查询
                let count_sql = format!("SELECT COUNT(*) as total FROM `order` {}", where_clause);
                let count_result = match conn.query(&count_sql).await {
                    Ok(rows) => rows,
                    Err(e) => return ctrl.render_error(&format!("查询失败: {}", e), json!({}), 0),
                };
                let total: i64 = count_result.first()
                    .and_then(|row| row.get("total"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                // 列表查询
                let list_sql = format!(
                    "SELECT * FROM `order` {} ORDER BY order_id DESC LIMIT {} OFFSET {}",
                    where_clause, page_size, offset
                );
                let rows = match conn.query(&list_sql).await {
                    Ok(rows) => rows,
                    Err(e) => return ctrl.render_error(&format!("查询失败: {}", e), json!({}), 0),
                };

                let list: Vec<serde_json::Value> = rows.iter().map(|row| row_to_json(row)).collect();

                info!("订单列表查询成功: total={}", total);
                ctrl.render_success("success", json!({
                    "list": list,
                    "total": total,
                    "page": page,
                    "page_size": page_size
                }))
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
        match ctrl.post_data(req).await {
            Ok(data) => {
                let order_id = match data.get("order_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 order_id 参数", json!({}), 0),
                };

                info!("查询订单详情: order_id={}", order_id);

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0),
                };

                // 查询订单主表
                let order_sql = format!("SELECT * FROM `order` WHERE order_id = {}", order_id);
                let order_rows = match conn.query(&order_sql).await {
                    Ok(rows) => rows,
                    Err(e) => return ctrl.render_error(&format!("查询失败: {}", e), json!({}), 0),
                };

                if order_rows.is_empty() {
                    return ctrl.render_error("订单不存在", json!({}), 0);
                }

                let order = row_to_json(&order_rows[0]);

                // 查询订单项
                let items_sql = format!("SELECT * FROM order_item WHERE order_id = {}", order_id);
                let items_rows = match conn.query(&items_sql).await {
                    Ok(rows) => rows,
                    Err(e) => return ctrl.render_error(&format!("查询订单项失败: {}", e), json!({}), 0),
                };

                let items: Vec<serde_json::Value> = items_rows.iter().map(|row| row_to_json(row)).collect();

                info!("订单详情查询完成: order_id={}, items={}", order_id, items.len());
                ctrl.render_success("success", json!({
                    "order": order,
                    "items": items
                }))
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
        match ctrl.post_data(req).await {
            Ok(data) => {
                let order_no = data.get("order_no").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if order_no.trim().is_empty() {
                    return ctrl.render_error("订单号不能为空", json!({}), 0);
                }

                let merchant_id = data.get("merchant_id").and_then(|v| v.as_i64()).unwrap_or(0);
                let device_id = data.get("device_id").and_then(|v| v.as_i64()).unwrap_or(0);
                let total_fen = data.get("total_fen").and_then(|v| v.as_i64()).unwrap_or(0);
                let total_weight_g = data.get("total_weight_g").and_then(|v| v.as_i64()).unwrap_or(0);
                let item_count = data.get("item_count").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let pay_method = data.get("pay_method").and_then(|v| v.as_i64()).unwrap_or(0) as i8;
                let offline_seq = data.get("offline_seq").and_then(|v| v.as_str()).unwrap_or("").to_string();

                info!("创建订单: order_no={}, merchant_id={}", order_no, merchant_id);

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0),
                };

                // 插入订单主表
                let order_sql = format!(
                    "INSERT INTO `order` (order_no, merchant_id, device_id, total_fen, total_weight_g, item_count, status, pay_method, offline_seq, created_at, updated_at) VALUES ('{}', {}, {}, {}, {}, {}, 1, {}, '{}', NOW(), NOW())",
                    sql_escape(&order_no),
                    merchant_id,
                    device_id,
                    total_fen,
                    total_weight_g,
                    item_count,
                    pay_method as i64,
                    sql_escape(&offline_seq),
                );
                if let Err(e) = conn.execute(&order_sql).await {
                    return ctrl.render_error(&format!("创建订单失败: {}", e), json!({}), 0);
                }

                // 获取新订单 ID
                let last_id_sql = "SELECT LAST_INSERT_ID() as order_id".to_string();
                let id_rows = match conn.query(&last_id_sql).await {
                    Ok(rows) => rows,
                    Err(e) => return ctrl.render_error(&format!("获取订单 ID 失败: {}", e), json!({}), 0),
                };
                let new_order_id = id_rows.first()
                    .and_then(|row| row.get("order_id"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                // 插入订单项
                if let Some(items) = data.get("items").and_then(|v| v.as_array()) {
                    for item in items {
                        let good_id = item.get("good_id").and_then(|v| v.as_i64()).unwrap_or(0);
                        let good_name = item.get("good_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let price_fen = item.get("price_fen").and_then(|v| v.as_i64()).unwrap_or(0);
                        let weight_g = item.get("weight_g").and_then(|v| v.as_i64()).unwrap_or(0);
                        let total_item_fen = item.get("total_fen").and_then(|v| v.as_i64()).unwrap_or(0);
                        let quantity = item.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1) as i32;

                        let item_sql = format!(
                            "INSERT INTO order_item (order_id, good_id, good_name, price_fen, weight_g, total_fen, quantity) VALUES ({}, {}, '{}', {}, {}, {}, {})",
                            new_order_id,
                            good_id,
                            sql_escape(&good_name),
                            price_fen,
                            weight_g,
                            total_item_fen,
                            quantity,
                        );
                        if let Err(e) = conn.execute(&item_sql).await {
                            warn!("创建订单项失败: {}", e);
                            // 不中断整体流程，继续插入其他订单项
                        }
                    }
                }

                info!("订单创建成功: order_id={}, order_no={}", new_order_id, order_no);
                ctrl.render_success("下单成功", json!({
                    "order_id": new_order_id,
                    "order_no": order_no
                }))
            }
            Err(e) => {
                warn!("创建订单参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }
}

pub async fn list(State(state): State<AppState>, req: Request<Body>) -> Response {
    OrderController::list(&state, req).await
}

pub async fn info(State(state): State<AppState>, req: Request<Body>) -> Response {
    OrderController::info(&state, req).await
}

pub async fn create(State(state): State<AppState>, req: Request<Body>) -> Response {
    OrderController::create(&state, req).await
}
