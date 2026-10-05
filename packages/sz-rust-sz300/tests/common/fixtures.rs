// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 测试数据 fixtures — 市场/商户/商品/用户/订单

#![allow(dead_code)]

use sz_rust_core::orm::{Pool, Value};

/// 插入测试商户
pub async fn insert_merchant(pool: &Pool, merchant_id: i64, name: &str, phone: &str) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute(
        format!(
            "INSERT INTO merchant (merchant_id, merchant_name, contact_phone, status) \
             VALUES ({}, '{}', '{}', 1)",
            merchant_id, name, phone
        )
        .as_str(),
    )
    .await
    .expect("插入商户失败");
}

/// 插入测试商品
pub async fn insert_product(
    pool: &Pool,
    product_id: i64,
    merchant_id: i64,
    name: &str,
    price_fen: i64,
    stock: i32,
) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute(
        format!(
            "INSERT INTO product (product_id, merchant_id, product_name, price_fen, stock, status) \
             VALUES ({}, {}, '{}', {}, {}, 1)",
            product_id, merchant_id, name, price_fen, stock
        )
        .as_str(),
    )
    .await
    .expect("插入商品失败");
}

/// 插入测试设备
pub async fn insert_device(pool: &Pool, device_id: i64, merchant_id: i64, device_sn: &str) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute(
        format!(
            "INSERT INTO device (device_id, merchant_id, device_sn, device_model, fw_version, status) \
             VALUES ({}, {}, '{}', 'TEST-MODEL', '1.0.0', 1)",
            device_id, merchant_id, device_sn
        )
        .as_str(),
    )
    .await
    .expect("插入设备失败");
}

/// 插入测试订单
pub async fn insert_order(
    pool: &Pool,
    order_id: i64,
    order_no: &str,
    merchant_id: i64,
    total_fen: i64,
    status: i32,
) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute(
        format!(
            "INSERT INTO `order` (order_id, order_no, merchant_id, total_fen, item_count, status) \
             VALUES ({}, '{}', {}, {}, 1, {})",
            order_id, order_no, merchant_id, total_fen, status
        )
        .as_str(),
    )
    .await
    .expect("插入订单失败");
}

/// 查询 operate_log 记录数
pub async fn count_operate_log(pool: &Pool) -> i64 {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query("SELECT COUNT(*) as cnt FROM operate_log")
        .await
        .expect("查询 operate_log 失败");
    if rows.is_empty() {
        return 0;
    }
    match &rows[0]["cnt"] {
        Value::I64(n) => *n,
        Value::U64(n) => *n as i64,
        _ => 0,
    }
}

/// 查询 merchant 记录数
pub async fn count_merchant(pool: &Pool) -> i64 {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query("SELECT COUNT(*) as cnt FROM merchant")
        .await
        .expect("查询 merchant 失败");
    if rows.is_empty() {
        return 0;
    }
    match &rows[0]["cnt"] {
        Value::I64(n) => *n,
        Value::U64(n) => *n as i64,
        _ => 0,
    }
}

/// 测试商户数据常量
pub mod merchant_data {
    pub const ID: i64 = 1001;
    pub const NAME: &str = "测试商户";
    pub const PHONE: &str = "13800138000";
}

/// 测试商品数据常量
pub mod product_data {
    pub const ID: i64 = 2001;
    pub const NAME: &str = "测试商品";
    pub const PRICE_FEN: i64 = 9900;
    pub const STOCK: i32 = 100;
}

/// 测试设备数据常量
pub mod device_data {
    pub const ID: i64 = 3001;
    pub const SN: &str = "TEST-DEVICE-001";
}

/// 测试订单数据常量
pub mod order_data {
    pub const ID: i64 = 4001;
    pub const NO: &str = "ORD-TEST-001";
    pub const TOTAL_FEN: i64 = 9900;
    pub const STATUS: i32 = 0;
}
