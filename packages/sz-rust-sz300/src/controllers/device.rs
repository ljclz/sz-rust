use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use std::collections::HashMap;
use sz_orm_core::{value_to_json, Value};
use sz_rust_core::controller::SzController;
use tracing::{error, info, warn};

struct DeviceController;
impl SzController for DeviceController {}

/// 将 sz-orm-core Value 的 HashMap 行转换为 serde_json::Value
fn row_to_json(row: &HashMap<String, Value>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (k, v) in row {
        obj.insert(k.clone(), value_to_json(v.clone()));
    }
    serde_json::Value::Object(obj)
}

/// SQL 字符串转义（防止注入）
fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

impl DeviceController {
    /// 分页查询设备列表，支持按 merchant_id 筛选
    async fn list(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = DeviceController;
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
                    .unwrap_or(20)
                    .clamp(1, 100);
                let merchant_id = data.get("merchant_id").and_then(|v| v.as_i64());
                let offset = (page - 1) * page_size;

                info!(
                    "查询设备列表: page={}, page_size={}, merchant_id={:?}",
                    page, page_size, merchant_id
                );

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                // 构建 WHERE 条件
                let mut conditions = Vec::new();
                if let Some(mid) = merchant_id {
                    conditions.push(format!("merchant_id = {}", mid));
                }
                let where_clause = if conditions.is_empty() {
                    String::new()
                } else {
                    format!("WHERE {}", conditions.join(" AND "))
                };

                // 总数查询
                let count_sql = format!("SELECT COUNT(*) as total FROM device {}", where_clause);
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
                    "SELECT * FROM device {} ORDER BY device_id DESC LIMIT {} OFFSET {}",
                    where_clause, page_size, offset
                );
                let rows = match conn.query(&list_sql).await {
                    Ok(rows) => rows,
                    Err(e) => return ctrl.render_error(&format!("查询失败: {}", e), json!({}), 0),
                };

                let list: Vec<serde_json::Value> =
                    rows.iter().map(|row| row_to_json(row)).collect();

                info!(
                    "设备列表查询成功: page={}, page_size={}, total={}",
                    page, page_size, total
                );
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
                warn!("设备列表查询参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 根据 device_id 查询单个设备详情
    async fn info(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = DeviceController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let device_id = match data.get("device_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 device_id 参数", json!({}), 0),
                };

                info!("查询设备详情: device_id={}", device_id);

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                let sql = format!("SELECT * FROM device WHERE device_id = {}", device_id);
                let rows = match conn.query(&sql).await {
                    Ok(rows) => rows,
                    Err(e) => return ctrl.render_error(&format!("查询失败: {}", e), json!({}), 0),
                };

                if rows.is_empty() {
                    info!("设备不存在: device_id={}", device_id);
                    return ctrl.render_error("设备不存在", json!({}), 0);
                }

                let device = row_to_json(&rows[0]);
                info!("设备查询完成: device_id={}", device_id);
                ctrl.render_success(
                    "success",
                    json!({
                        "device": device
                    }),
                )
            }
            Err(e) => {
                warn!("设备详情查询参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 绑定设备到商户
    async fn bind(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = DeviceController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let device_sn = match data.get("device_sn").and_then(|v| v.as_str()) {
                    Some(sn) if !sn.trim().is_empty() => sn.trim().to_string(),
                    _ => return ctrl.render_error("缺少有效的 device_sn 参数", json!({}), 0),
                };
                let merchant_id = match data.get("merchant_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 merchant_id 参数", json!({}), 0),
                };

                info!(
                    "设备绑定请求: device_sn={}, merchant_id={}",
                    device_sn, merchant_id
                );

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                // 验证设备 SN 存在
                let check_sql = format!(
                    "SELECT * FROM device WHERE device_sn = '{}'",
                    sql_escape(&device_sn)
                );
                let rows = match conn.query(&check_sql).await {
                    Ok(rows) => rows,
                    Err(e) => {
                        return ctrl.render_error(&format!("查询设备失败: {}", e), json!({}), 0)
                    }
                };
                if rows.is_empty() {
                    return ctrl.render_error("设备不存在", json!({}), 0);
                }

                // 检查是否已绑定
                let existing_merchant = rows[0]
                    .get("merchant_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if existing_merchant != 0 {
                    return ctrl.render_error("设备已绑定", json!({}), 0);
                }

                // 更新绑定
                let update_sql = format!(
                    "UPDATE device SET merchant_id = {}, bind_at = NOW() WHERE device_sn = '{}'",
                    merchant_id,
                    sql_escape(&device_sn)
                );
                if let Err(e) = conn.execute(&update_sql).await {
                    return ctrl.render_error(&format!("绑定失败: {}", e), json!({}), 0);
                }

                // 记录操作日志（best-effort）
                let log_sql = format!(
                    "INSERT INTO operate_log (merchant_id, operator, action, detail, ip) VALUES ({}, 'system', 'bind', '设备 {} 绑定到商户 {}', '')",
                    merchant_id,
                    sql_escape(&device_sn),
                    merchant_id
                );
                if let Err(e) = conn.execute(&log_sql).await {
                    warn!("记录操作日志失败: {}", e);
                }

                info!(
                    "设备绑定成功: device_sn={}, merchant_id={}",
                    device_sn, merchant_id
                );
                ctrl.render_success(
                    "绑定成功",
                    json!({
                        "device_sn": device_sn,
                        "merchant_id": merchant_id
                    }),
                )
            }
            Err(e) => {
                warn!("设备绑定参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 解绑设备（将 merchant_id 置为 0，状态置为离线）
    async fn unbind(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = DeviceController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let device_id = match data.get("device_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 device_id 参数", json!({}), 0),
                };

                info!("设备解绑请求: device_id={}", device_id);

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                let sql = format!(
                    "UPDATE device SET merchant_id = 0, bind_at = NULL, status = 0 WHERE device_id = {}",
                    device_id
                );
                if let Err(e) = conn.execute(&sql).await {
                    return ctrl.render_error(&format!("解绑失败: {}", e), json!({}), 0);
                }

                info!("设备解绑成功: device_id={}", device_id);
                ctrl.render_success(
                    "解绑成功",
                    json!({
                        "device_id": device_id
                    }),
                )
            }
            Err(e) => {
                warn!("设备解绑参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 触发 OTA 升级
    async fn trigger_ota(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = DeviceController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let device_id = match data.get("device_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 device_id 参数", json!({}), 0),
                };
                let ota_version = match data.get("ota_version").and_then(|v| v.as_str()) {
                    Some(v) if !v.trim().is_empty() => v.trim().to_string(),
                    _ => return ctrl.render_error("缺少有效的 ota_version 参数", json!({}), 0),
                };

                info!(
                    "OTA 触发请求: device_id={}, ota_version={}",
                    device_id, ota_version
                );

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                // 验证设备存在
                let dev_sql = format!("SELECT * FROM device WHERE device_id = {}", device_id);
                let dev_rows = match conn.query(&dev_sql).await {
                    Ok(rows) => rows,
                    Err(e) => {
                        return ctrl.render_error(&format!("查询设备失败: {}", e), json!({}), 0)
                    }
                };
                if dev_rows.is_empty() {
                    return ctrl.render_error("设备不存在", json!({}), 0);
                }

                // 查询 OTA 版本信息
                let ota_sql = format!(
                    "SELECT * FROM ota_version WHERE version = '{}' AND status = 1",
                    sql_escape(&ota_version)
                );
                let ota_rows = match conn.query(&ota_sql).await {
                    Ok(rows) => rows,
                    Err(e) => {
                        return ctrl.render_error(
                            &format!("查询 OTA 版本失败: {}", e),
                            json!({}),
                            0,
                        )
                    }
                };
                if ota_rows.is_empty() {
                    return ctrl.render_error("OTA 版本不存在或未启用", json!({}), 0);
                }

                let ota_info = row_to_json(&ota_rows[0]);

                info!(
                    "OTA 指令已推送: device_id={}, ota_version={}",
                    device_id, ota_version
                );
                ctrl.render_success(
                    "OTA 已触发",
                    json!({
                        "device_id": device_id,
                        "ota_version": ota_version,
                        "ota_info": ota_info
                    }),
                )
            }
            Err(e) => {
                warn!("OTA 触发参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 接收设备上报的状态数据，更新设备在线状态、信号强度、固件版本
    async fn status_report(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = DeviceController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let device_id = match data.get("device_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 device_id 参数", json!({}), 0),
                };

                let status = data.get("status").and_then(|v| v.as_i64()).unwrap_or(1);
                let signal_strength = data
                    .get("signal_strength")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let fw_version = data
                    .get("fw_version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                info!(
                    "设备状态上报: device_id={}, status={}, signal_strength={}, fw_version={}",
                    device_id, status, signal_strength, fw_version
                );

                let mut conn = match state.db_pool.acquire().await {
                    Ok(c) => c,
                    Err(e) => {
                        return ctrl.render_error(&format!("数据库连接失败: {}", e), json!({}), 0)
                    }
                };

                let sql = format!(
                    "UPDATE device SET status = {}, signal_strength = {}, fw_version = '{}', last_online_at = NOW() WHERE device_id = {}",
                    status,
                    signal_strength,
                    sql_escape(&fw_version),
                    device_id
                );
                if let Err(e) = conn.execute(&sql).await {
                    return ctrl.render_error(&format!("状态更新失败: {}", e), json!({}), 0);
                }

                info!("设备状态更新成功: device_id={}", device_id);
                ctrl.render_success(
                    "状态更新成功",
                    json!({
                        "device_id": device_id,
                        "status": status,
                        "signal_strength": signal_strength
                    }),
                )
            }
            Err(e) => {
                error!("设备状态上报参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }
}

/// 设备列表查询（对齐 PHP DeviceController::list）
pub async fn list(State(state): State<AppState>, req: Request<Body>) -> Response {
    DeviceController::list(&state, req).await
}

/// 设备详情查询（对齐 PHP DeviceController::info）
pub async fn info(State(state): State<AppState>, req: Request<Body>) -> Response {
    DeviceController::info(&state, req).await
}

/// 设备绑定（对齐 PHP DeviceController::bind）
pub async fn bind(State(state): State<AppState>, req: Request<Body>) -> Response {
    DeviceController::bind(&state, req).await
}

/// 设备解绑（对齐 PHP DeviceController::unbind）
pub async fn unbind(State(state): State<AppState>, req: Request<Body>) -> Response {
    DeviceController::unbind(&state, req).await
}

/// 触发设备 OTA 升级（对齐 PHP DeviceController::triggerOta）
pub async fn trigger_ota(State(state): State<AppState>, req: Request<Body>) -> Response {
    DeviceController::trigger_ota(&state, req).await
}

/// 设备状态上报（对齐 PHP DeviceController::statusReport）
pub async fn status_report(State(state): State<AppState>, req: Request<Body>) -> Response {
    DeviceController::status_report(&state, req).await
}
