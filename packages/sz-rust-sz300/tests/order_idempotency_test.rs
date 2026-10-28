// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 订单幂等集成测试（005_order_idempotency.sql）
//!
//! | 场景 | 预期 |
//! |------|------|
//! | 同一 (device_id, offline_seq) 重复下单 | 返回同一 order_id，库中只有 1 单 1 组明细 1 条 outbox |
//! | 在线订单 offline_seq 为空串重复下单 | 两单都成功（唯一键对 NULL 不生效），offline_seq 落库为 NULL |
//! | 不同设备使用相同 offline_seq | 互不冲突 |
//!
//! 运行前确保 MySQL 运行于 127.0.0.1:3306，root/$SZ_TEST_DB_PASSWORD，sz_orm_test 数据库存在。
//! 手动运行：`cargo test -p sz-rust-sz300 --test order_idempotency_test -- --ignored`

#![forbid(unsafe_code)]

use std::sync::LazyLock;

use sz_rust_core::orm::{Pool, Value};
use sz_rust_sz300::models::{order::Order, order_item::OrderItem};
use sz_rust_sz300::services::order_service::OrderService;
use tokio::sync::Mutex;

mod common;

/// 真实 MySQL 上串行执行，避免并发迁移/清理互踩
static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn make_order(order_no: &str, device_id: i64, offline_seq: &str) -> Order {
    Order {
        order_id: None,
        order_no: order_no.to_string(),
        merchant_id: 1,
        device_id,
        total_fen: 500,
        total_weight_g: 200,
        item_count: 1,
        status: 1,
        pay_method: 0,
        pay_at: None,
        offline_seq: offline_seq.to_string(),
        created_at: None,
        updated_at: None,
    }
}

fn make_items() -> Vec<OrderItem> {
    vec![OrderItem {
        item_id: None,
        order_id: 0,
        good_id: 1,
        good_name: "idempotency-good".to_string(),
        price_fen: 500,
        weight_g: 200,
        total_fen: 500,
        quantity: 1,
    }]
}

async fn count_where(pool: &Pool, sql: &str, params: &[Value]) -> i64 {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(sql, params)
        .await
        .unwrap_or_else(|e| panic!("COUNT 查询失败: {e}"));
    rows.first()
        .and_then(|row| row.get("c"))
        .and_then(|v| v.as_i64())
        .unwrap_or(-1)
}

#[tokio::test]
#[ignore = "requires real MySQL"]
async fn test_offline_replay_returns_existing_order() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let order = make_order("IDEM-OFFLINE-001", 9001, "SEQ-001");
    let items = make_items();

    let first_id = OrderService::create(&pool, &order, &items)
        .await
        .expect("首次下单应成功");
    assert!(first_id > 0, "首次下单应返回有效 order_id");

    // 设备断网重传：同一 (device_id, offline_seq)，order_no 由服务端重新生成也不同
    let replay = make_order("IDEM-OFFLINE-002", 9001, "SEQ-001");
    let second_id = OrderService::create(&pool, &replay, &items)
        .await
        .expect("重放应被识别为幂等并成功");
    assert_eq!(second_id, first_id, "重放必须返回原订单 ID");

    let order_count = count_where(
        &pool,
        "SELECT COUNT(*) AS c FROM `order` WHERE device_id = ? AND offline_seq = ?",
        &[Value::I64(9001), Value::String("SEQ-001".to_string())],
    )
    .await;
    assert_eq!(order_count, 1, "重放不得产生第二张订单");

    let item_count = count_where(
        &pool,
        "SELECT COUNT(*) AS c FROM order_item WHERE order_id = ?",
        &[Value::I64(first_id)],
    )
    .await;
    assert_eq!(item_count, 1, "重放不得重复写入订单明细");

    let outbox_count = count_where(
        &pool,
        "SELECT COUNT(*) AS c FROM outbox_event WHERE event_type = 'order.created' AND aggregate_id = ?",
        &[Value::String(first_id.to_string())],
    )
    .await;
    assert_eq!(outbox_count, 1, "重放不得重复投递 outbox 事件");

    common::teardown_all(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL"]
async fn test_online_orders_without_seq_do_not_collide() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let items = make_items();
    let first_id = OrderService::create(&pool, &make_order("IDEM-ONLINE-001", 9002, ""), &items)
        .await
        .expect("在线订单一应成功");
    let second_id = OrderService::create(&pool, &make_order("IDEM-ONLINE-002", 9002, ""), &items)
        .await
        .expect("同设备在线订单二也不应被唯一键误伤");
    assert_ne!(first_id, second_id, "空 offline_seq 的两张订单不得被合并");

    let stored = count_where(
        &pool,
        "SELECT COUNT(*) AS c FROM `order` WHERE device_id = ? AND offline_seq IS NULL",
        &[Value::I64(9002)],
    )
    .await;
    assert_eq!(stored, 2, "在线订单的 offline_seq 应落库为 NULL");

    common::teardown_all(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL"]
async fn test_same_seq_on_different_devices_is_independent() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let items = make_items();
    let device_a = OrderService::create(&pool, &make_order("IDEM-DEV-A", 9003, "SEQ-X"), &items)
        .await
        .expect("设备 A 下单应成功");
    let device_b = OrderService::create(&pool, &make_order("IDEM-DEV-B", 9004, "SEQ-X"), &items)
        .await
        .expect("设备 B 使用相同序列号不应冲突");
    assert_ne!(device_a, device_b, "幂等键是 (device_id, offline_seq)，跨设备必须独立");

    common::teardown_all(&pool).await;
    pool.close_all().await;
}
