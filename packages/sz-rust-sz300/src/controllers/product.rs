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

struct ProductController;
impl SzController for ProductController {}

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

impl ProductController {
    /// 分页查询商品列表，支持按 merchant_id/cat_id/keyword 筛选
    async fn list(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = ProductController;
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
                let merchant_id = data.get("merchant_id").and_then(|v| v.as_i64());
                let cat_id = data.get("cat_id").and_then(|v| v.as_i64());
                let keyword = data.get("keyword").and_then(|v| v.as_str());
                let offset = (page - 1) * page_size;

                info!("查询商品列表: page={}, page_size={}", page, page_size);

                // 构建 WHERE 条件
                let mut conditions: Vec<String> = Vec::new();
                if let Some(mid) = merchant_id {
                    conditions.push(format!("merchant_id = {}", mid));
                }
                if let Some(cid) = cat_id {
                    conditions.push(format!("cat_id = {}", cid));
                }
                if let Some(kw) = keyword {
                    if !kw.trim().is_empty() {
                        conditions.push(format!("name LIKE '%{}%'", sql_escape(kw.trim())));
                    }
                }
                let where_clause = if conditions.is_empty() {
                    String::new()
                } else {
                    format!("WHERE {}", conditions.join(" AND "))
                };

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                // 总数查询
                let count_sql = format!("SELECT COUNT(*) as total FROM good {}", where_clause);
                let count_result = match conn.query(&count_sql).await {
                    Ok(rows) => rows,
                    Err(e) => return ctrl.render_error(&format!("查询失败: {}", e), json!({}), 0),
                };
                let total: i64 = count_result
                    .first()
                    .and_then(|row| row.get("total"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                // 列表查询
                let list_sql = format!(
                    "SELECT * FROM good {} ORDER BY good_id DESC LIMIT {} OFFSET {}",
                    where_clause, page_size, offset
                );
                let rows = match conn.query(&list_sql).await {
                    Ok(rows) => rows,
                    Err(e) => return ctrl.render_error(&format!("查询失败: {}", e), json!({}), 0),
                };

                let list: Vec<serde_json::Value> =
                    rows.iter().map(|row| row_to_json(row)).collect();

                info!("商品列表查询成功: total={}", total);
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

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                let sql = format!("SELECT * FROM good WHERE good_id = {}", good_id);
                let rows = match conn.query(&sql).await {
                    Ok(rows) => rows,
                    Err(e) => return ctrl.render_error(&format!("查询失败: {}", e), json!({}), 0),
                };

                if rows.is_empty() {
                    return ctrl.render_error("商品不存在", json!({}), 0);
                }

                let product = row_to_json(&rows[0]);
                info!("商品查询完成: good_id={}", good_id);
                ctrl.render_success(
                    "success",
                    json!({
                        "product": product
                    }),
                )
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

                let merchant_id = data
                    .get("merchant_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let cat_id = data.get("cat_id").and_then(|v| v.as_i64()).unwrap_or(0);
                let barcode = data
                    .get("barcode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let price = data.get("price").and_then(|v| v.as_i64()).unwrap_or(0);
                let unit = data
                    .get("unit")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let ai_class_id = data
                    .get("ai_class_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let image = data
                    .get("image")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let status = data.get("status").and_then(|v| v.as_i64()).unwrap_or(1);

                info!("创建商品: name={}, merchant_id={}", name, merchant_id);

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                let sql = format!(
                    "INSERT INTO good (merchant_id, cat_id, name, barcode, price, unit, ai_class_id, image, status, created_at, updated_at) VALUES ({}, {}, '{}', '{}', {}, '{}', {}, '{}', {}, NOW(), NOW())",
                    merchant_id,
                    cat_id,
                    sql_escape(&name),
                    sql_escape(&barcode),
                    price,
                    sql_escape(&unit),
                    ai_class_id,
                    sql_escape(&image),
                    status,
                );
                match conn.execute(&sql).await {
                    Ok(_) => {
                        info!("商品创建成功: name={}", name);
                        ctrl.render_success("创建成功", json!({}))
                    }
                    Err(e) => ctrl.render_error(&format!("创建失败: {}", e), json!({}), 0),
                }
            }
            Err(e) => {
                warn!("创建商品参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 更新商品信息
    async fn update(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = ProductController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let good_id = match data.get("good_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 good_id 参数", json!({}), 0),
                };

                info!("更新商品: good_id={}", good_id);

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                // 构建动态 SET 子句
                let mut set_clauses: Vec<String> = Vec::new();
                if let Some(v) = data.get("merchant_id").and_then(|v| v.as_i64()) {
                    set_clauses.push(format!("merchant_id = {}", v));
                }
                if let Some(v) = data.get("cat_id").and_then(|v| v.as_i64()) {
                    set_clauses.push(format!("cat_id = {}", v));
                }
                if let Some(v) = data.get("name").and_then(|v| v.as_str()) {
                    set_clauses.push(format!("name = '{}'", sql_escape(v)));
                }
                if let Some(v) = data.get("barcode").and_then(|v| v.as_str()) {
                    set_clauses.push(format!("barcode = '{}'", sql_escape(v)));
                }
                if let Some(v) = data.get("price").and_then(|v| v.as_i64()) {
                    set_clauses.push(format!("price = {}", v));
                }
                if let Some(v) = data.get("unit").and_then(|v| v.as_str()) {
                    set_clauses.push(format!("unit = '{}'", sql_escape(v)));
                }
                if let Some(v) = data.get("ai_class_id").and_then(|v| v.as_i64()) {
                    set_clauses.push(format!("ai_class_id = {}", v));
                }
                if let Some(v) = data.get("image").and_then(|v| v.as_str()) {
                    set_clauses.push(format!("image = '{}'", sql_escape(v)));
                }
                if let Some(v) = data.get("status").and_then(|v| v.as_i64()) {
                    set_clauses.push(format!("status = {}", v));
                }

                if set_clauses.is_empty() {
                    return ctrl.render_error("未提供需要更新的字段", json!({}), 0);
                }

                set_clauses.push("updated_at = NOW()".to_string());

                let sql = format!(
                    "UPDATE good SET {} WHERE good_id = {}",
                    set_clauses.join(", "),
                    good_id
                );
                match conn.execute(&sql).await {
                    Ok(_) => {
                        info!("商品更新成功: good_id={}", good_id);
                        ctrl.render_success("更新成功", json!({}))
                    }
                    Err(e) => ctrl.render_error(&format!("更新失败: {}", e), json!({}), 0),
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
        match ctrl.post_data(req).await {
            Ok(data) => {
                let good_id = match data.get("good_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 good_id 参数", json!({}), 0),
                };

                info!("删除商品: good_id={}", good_id);

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                let sql = format!(
                    "UPDATE good SET status = 0, updated_at = NOW() WHERE good_id = {}",
                    good_id
                );
                match conn.execute(&sql).await {
                    Ok(_) => {
                        info!("商品删除成功: good_id={}", good_id);
                        ctrl.render_success("删除成功", json!({}))
                    }
                    Err(e) => ctrl.render_error(&format!("删除失败: {}", e), json!({}), 0),
                }
            }
            Err(e) => {
                warn!("删除商品参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }
}

pub async fn list(State(state): State<AppState>, req: Request<Body>) -> Response {
    ProductController::list(&state, req).await
}

pub async fn info(State(state): State<AppState>, req: Request<Body>) -> Response {
    ProductController::info(&state, req).await
}

pub async fn create(State(state): State<AppState>, req: Request<Body>) -> Response {
    ProductController::create(&state, req).await
}

pub async fn update(State(state): State<AppState>, req: Request<Body>) -> Response {
    ProductController::update(&state, req).await
}

pub async fn delete(State(state): State<AppState>, req: Request<Body>) -> Response {
    ProductController::delete(&state, req).await
}
