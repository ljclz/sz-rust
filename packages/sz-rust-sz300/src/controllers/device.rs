//! 设备控制器 — 仅负责 HTTP 请求/响应处理，SQL 逻辑下沉到 [`crate::services::device_service`]
//!
//! 2026-07-25 重构（修复 P0-4：控制器分层违反）。
//!
//! ## 重构前
//!
//! 控制器内嵌 30 处直接 SQL（`format!` 拼接 + `conn.query_with_params`），违反分层架构。
//!
//! ## 重构后
//!
//! - 控制器：解析请求参数 → 调用 [`crate::services::device_service::DeviceService`] → 格式化响应
//! - 服务层：构建 SQL、执行查询、返回领域数据
//! - 模型层：[`crate::models::device::Device`] 定义实体结构

use crate::controllers::common::parse_pagination;
use crate::services::auth_service;
use crate::services::device_service::{DeviceFilters, DeviceService};
use crate::services::row_to_json;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use sz_rust_core::controller::SzController;
use tracing::{info, warn};

struct DeviceController;
impl SzController for DeviceController {}

/// 校验设备是否归属指定商户（黑帽复核 A6 修复辅助）
///
/// 查询设备 merchant_id 并与身份值比对；设备不存在或归属不符均返回 false。
async fn device_belongs_to(state: &AppState, device_id: i64, merchant_id: i64) -> bool {
    match crate::services::device_service::DeviceService::get(&state.db_pool, device_id).await {
        Ok(Some(row)) => row
            .get("merchant_id")
            .and_then(|v| v.as_i64())
            .map(|m| m == merchant_id)
            .unwrap_or(false),
        _ => false,
    }
}

impl DeviceController {
    /// 分页查询设备列表，支持按 merchant_id 筛选
    async fn list(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = DeviceController;
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
                let (page, page_size) = parse_pagination(&data, 20);
                let filters = DeviceFilters {
                    merchant_id: Some(owned_merchant_id), // 服务端权威值，忽略请求体
                };

                info!(
                    "查询设备列表: page={}, page_size={}, merchant_id={:?}",
                    page, page_size, filters.merchant_id
                );

                match DeviceService::list(&state.db_pool, page, page_size, filters).await {
                    Ok(page_data) => {
                        let list: Vec<serde_json::Value> =
                            page_data.list.iter().map(row_to_json).collect();
                        info!(
                            "设备列表查询成功: page={}, page_size={}, total={}",
                            page, page_size, page_data.total
                        );
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
                warn!("设备列表查询参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }

    /// 根据 device_id 查询单个设备详情
    async fn info(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = DeviceController;
        // 安全修复 H-1：先解析服务端身份，设备必须归属当前商户
        let owned_merchant_id = match auth_service::current_user(&req).map(|u| u.id) {
            Some(uid) => match auth_service::resolve_merchant_id(uid, None).await {
                Ok(mid) => mid,
                Err(e) => return ctrl.render_error(&e, json!({}), 0),
            },
            None => return ctrl.render_error("未认证请求", json!({}), 0),
        };
        match ctrl.post_data(req).await {
            Ok(data) => {
                let device_id = match data.get("device_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 device_id 参数", json!({}), 0),
                };

                info!("查询设备详情: device_id={}", device_id);

                match DeviceService::get(&state.db_pool, device_id).await {
                    Ok(Some(row)) => {
                        // 归属校验：设备必须属于当前商户（防越权）
                        let row_merchant =
                            row.get("merchant_id").and_then(|v| v.as_i64()).unwrap_or(0);
                        if row_merchant != owned_merchant_id {
                            tracing::warn!(
                                device_id = device_id,
                                row_merchant = row_merchant,
                                owned = owned_merchant_id,
                                "越权尝试：查询非本商户设备"
                            );
                            return ctrl.render_error("设备不存在", json!({}), 0);
                        }
                        let device = row_to_json(&row);
                        info!("设备查询完成: device_id={}", device_id);
                        ctrl.render_success(
                            "success",
                            json!({
                                "device": device
                            }),
                        )
                    }
                    Ok(None) => {
                        info!("设备不存在: device_id={}", device_id);
                        ctrl.render_error("设备不存在", json!({}), 0)
                    }
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
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
        // 安全修复（黑帽复核 A6）：merchant_id 以服务端身份为准，禁止绑定到其他商户
        let owned_merchant_id = match auth_service::current_user(&req).map(|u| u.id) {
            Some(uid) => match auth_service::resolve_merchant_id(uid, None).await {
                Ok(mid) => mid,
                Err(e) => return ctrl.render_error(&e, json!({}), 0),
            },
            None => return ctrl.render_error("未认证请求", json!({}), 0),
        };
        match ctrl.post_data(req).await {
            Ok(data) => {
                let device_sn = match data.get("device_sn").and_then(|v| v.as_str()) {
                    Some(sn) if !sn.trim().is_empty() => sn.trim().to_string(),
                    _ => return ctrl.render_error("缺少有效的 device_sn 参数", json!({}), 0),
                };
                let merchant_id = owned_merchant_id; // 服务端权威值，忽略请求体

                info!(
                    "设备绑定请求: device_sn={}, merchant_id={}",
                    device_sn, merchant_id
                );

                match DeviceService::bind(&state.db_pool, &device_sn, merchant_id).await {
                    Ok(()) => {
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
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
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
        // 安全修复（黑帽复核 A6）：仅允许解绑本商户设备
        let owned_merchant_id = match auth_service::current_user(&req).map(|u| u.id) {
            Some(uid) => match auth_service::resolve_merchant_id(uid, None).await {
                Ok(mid) => mid,
                Err(e) => return ctrl.render_error(&e, json!({}), 0),
            },
            None => return ctrl.render_error("未认证请求", json!({}), 0),
        };
        match ctrl.post_data(req).await {
            Ok(data) => {
                let device_id = match data.get("device_id").and_then(|v| v.as_i64()) {
                    Some(id) if id > 0 => id,
                    _ => return ctrl.render_error("缺少有效的 device_id 参数", json!({}), 0),
                };

                info!("设备解绑请求: device_id={}", device_id);

                // 归属校验：设备必须属于当前商户
                if !device_belongs_to(state, device_id, owned_merchant_id).await {
                    return ctrl.render_error("设备不存在", json!({}), 0);
                }

                match DeviceService::unbind(&state.db_pool, device_id).await {
                    Ok(()) => {
                        info!("设备解绑成功: device_id={}", device_id);
                        ctrl.render_success(
                            "解绑成功",
                            json!({
                                "device_id": device_id
                            }),
                        )
                    }
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
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
        // 安全修复（黑帽复核 A6）：仅允许对本商户设备触发 OTA
        let owned_merchant_id = match auth_service::current_user(&req).map(|u| u.id) {
            Some(uid) => match auth_service::resolve_merchant_id(uid, None).await {
                Ok(mid) => mid,
                Err(e) => return ctrl.render_error(&e, json!({}), 0),
            },
            None => return ctrl.render_error("未认证请求", json!({}), 0),
        };
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

                // 验证设备存在且归属当前商户（防跨商户 OTA）
                match DeviceService::exists(&state.db_pool, device_id).await {
                    Ok(true) => {}
                    Ok(false) => return ctrl.render_error("设备不存在", json!({}), 0),
                    Err(msg) => return ctrl.render_error(&msg, json!({}), 0),
                }
                if !device_belongs_to(state, device_id, owned_merchant_id).await {
                    return ctrl.render_error("设备不存在", json!({}), 0);
                }

                // 查询 OTA 版本信息
                match DeviceService::get_ota_version(&state.db_pool, &ota_version).await {
                    Ok(Some(ota_row)) => {
                        let ota_info = row_to_json(&ota_row);
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
                    Ok(None) => ctrl.render_error("OTA 版本不存在或未启用", json!({}), 0),
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
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

                match DeviceService::update_status(
                    &state.db_pool,
                    device_id,
                    status,
                    signal_strength,
                    &fw_version,
                )
                .await
                {
                    Ok(()) => {
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
                    Err(msg) => ctrl.render_error(&msg, json!({}), 0),
                }
            }
            Err(e) => {
                warn!("设备状态上报参数解析失败: {}", e);
                ctrl.render_error(&e, json!({}), 0)
            }
        }
    }
}

/// 设备列表查询（对齐 PHP DeviceController::list）
#[tracing::instrument(skip(state, req))]
pub async fn list(State(state): State<AppState>, req: Request<Body>) -> Response {
    DeviceController::list(&state, req).await
}

/// 设备详情查询（对齐 PHP DeviceController::info）
#[tracing::instrument(skip(state, req))]
pub async fn info(State(state): State<AppState>, req: Request<Body>) -> Response {
    DeviceController::info(&state, req).await
}

/// 设备绑定（对齐 PHP DeviceController::bind）
#[tracing::instrument(skip(state, req))]
pub async fn bind(State(state): State<AppState>, req: Request<Body>) -> Response {
    DeviceController::bind(&state, req).await
}

/// 设备解绑（对齐 PHP DeviceController::unbind）
#[tracing::instrument(skip(state, req))]
pub async fn unbind(State(state): State<AppState>, req: Request<Body>) -> Response {
    DeviceController::unbind(&state, req).await
}

/// 触发设备 OTA 升级（对齐 PHP DeviceController::triggerOta）
#[tracing::instrument(skip(state, req))]
pub async fn trigger_ota(State(state): State<AppState>, req: Request<Body>) -> Response {
    DeviceController::trigger_ota(&state, req).await
}

/// 设备状态上报（对齐 PHP DeviceController::statusReport）
#[tracing::instrument(skip(state, req))]
pub async fn status_report(State(state): State<AppState>, req: Request<Body>) -> Response {
    DeviceController::status_report(&state, req).await
}
