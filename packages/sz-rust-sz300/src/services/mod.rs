//! 服务层模块聚合
//!
//! 2026-07-26：提取公共 `row_to_json` 函数至本模块，消除 product/merchant/order/device
//! 四个 service 中的重复定义（DRY）。

use std::collections::HashMap;

use sz_rust_core::orm::{value_to_json, Value};

/// 认证服务模块（对齐 PHP AuthService）
pub mod auth_service;
/// 设备服务模块（封装设备 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod device_service;
/// 文件服务模块（对齐 PHP FileService）
pub mod file_service;
/// 健康检查服务模块（封装 DB 探活逻辑，2026-07-26 新增 — 修复控制器分层违反）
pub mod health_service;
/// 商户服务模块（封装商户 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod merchant_service;
/// MQTT 监听服务模块
pub mod mqtt_listener;
/// MQTT 业务服务模块
pub mod mqtt_service;
/// 订单服务模块（封装订单 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod order_service;
/// 商品服务模块（封装商品 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod product_service;

/// 将 DB 行（`HashMap<String, Value>`）转换为 JSON 对象（供控制器使用）
///
/// ## 参数
///
/// - `row`：DB 行数据（字段名 → 字段值）
///
/// ## 返回
///
/// `serde_json::Value::Object`，字段顺序保持 HashMap 迭代顺序。
///
/// ## 重复消除说明
///
/// 2026-07-26 之前，`product_service` / `merchant_service` / `order_service` /
/// `device_service` 各自定义了完全相同的 `row_to_json`，违反 DRY 原则。
/// 现统一提取至本模块，子模块通过 `super::row_to_json` 或 `crate::services::row_to_json` 引用。
pub fn row_to_json(row: &HashMap<String, Value>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (k, v) in row {
        obj.insert(k.clone(), value_to_json(v.clone()));
    }
    serde_json::Value::Object(obj)
}
