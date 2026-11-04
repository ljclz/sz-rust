// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! order/list COUNT 缓存集成测试
//!
//! 验证：
//! 1. 相同筛选条件第二次查询命中缓存（stats.hits 增长），total 一致
//! 2. 不同筛选条件使用不同缓存键（互不串用，各自执行一次 COUNT）
//! 3. `OrderService::create` 成功后清空缓存，下一次查询重新执行 COUNT（miss 增长）
//!
//! 运行前确保 MySQL 运行于 127.0.0.1:3306，root/$SZ_TEST_DB_PASSWORD，sz_orm_test 数据库存在。
//! 手动运行：`cargo test -p sz-rust-sz300 --test order_count_cache_test -- --ignored`

#![forbid(unsafe_code)]

use std::sync::{Arc, LazyLock};

use sz_rust_core::orm::Pool;
use sz_rust_sz300::models::{order::Order, order_item::OrderItem};
use sz_rust_sz300::query_cache::QueryCache;
use sz_rust_sz300::services::order_service::{OrderFilters, OrderService};
use tokio::sync::Mutex;

mod common;

/// 真实 MySQL 上串行执行，避免并发迁移/清理互踩
static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn make_order(order_no: &str, device_id: i64) -> Order {
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
        offline_seq: String::new(),
        created_at: None,
        updated_at: None,
    }
}

fn make_items() -> Vec<OrderItem> {
    vec![OrderItem {
        item_id: None,
        order_id: 0,
        good_id: 1,
        good_name: "count-cache-good".to_string(),
        price_fen: 500,
        weight_g: 200,
        total_fen: 500,
        quantity: 1,
    }]
}

/// 种子数据：3 个商户 1 的订单，device_id 9200/9201/9202
async fn seed_orders(pool: &Pool) {
    for (i, device_id) in [9200i64, 9201, 9202].into_iter().enumerate() {
        let order = make_order(&format!("CNT-SEED-{:03}", i), device_id);
        OrderService::create(pool, None, &order, &make_items())
            .await
            .expect("种子下单应成功");
    }
}

#[tokio::test]
#[ignore = "requires real MySQL"]
async fn test_count_cache_hit_and_create_invalidates() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;
    seed_orders(&pool).await;

    let cache = Arc::new(QueryCache::with_defaults());

    // 第一次：未命中，total=3
    let page1 = OrderService::list(
        &pool,
        Some(&cache),
        1,
        20,
        OrderFilters::new(Some(1), None, None),
    )
    .await
    .expect("列表查询应成功");
    assert_eq!(page1.total, 3, "种子数据应统计为 3 单");
    let misses_after_first = cache.stats().misses();

    // 第二次：命中缓存，total 一致且 hits 增长
    let page2 = OrderService::list(
        &pool,
        Some(&cache),
        1,
        20,
        OrderFilters::new(Some(1), None, None),
    )
    .await
    .expect("列表查询应成功");
    assert_eq!(page2.total, 3, "命中缓存后 total 应保持一致");
    assert!(cache.stats().hits() > 0, "第二次查询应命中 COUNT 缓存");

    // create 成功后清空缓存：下一次查询重新 COUNT（miss 增长）
    let new_id = OrderService::create(
        &pool,
        Some(&cache),
        &make_order("CNT-NEW-001", 9300),
        &make_items(),
    )
    .await
    .expect("下单应成功");
    assert!(new_id > 0, "下单应返回有效 order_id");
    let page3 = OrderService::list(
        &pool,
        Some(&cache),
        1,
        20,
        OrderFilters::new(Some(1), None, None),
    )
    .await
    .expect("列表查询应成功");
    assert_eq!(page3.total, 4, "下单后 COUNT 应包含新订单");
    assert!(
        cache.stats().misses() > misses_after_first,
        "create 应清空缓存并触发重新 COUNT"
    );

    common::teardown_all(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL"]
async fn test_count_cache_key_differs_by_filter() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;
    seed_orders(&pool).await;

    let cache = Arc::new(QueryCache::with_defaults());
    let f_merchant = OrderFilters::new(Some(1), None, None);
    let f_device = OrderFilters::new(Some(1), Some(9200), None);

    let p1 = OrderService::list(&pool, Some(&cache), 1, 20, f_merchant)
        .await
        .expect("商户维度列表查询应成功");
    let p2 = OrderService::list(&pool, Some(&cache), 1, 20, f_device)
        .await
        .expect("设备维度列表查询应成功");
    assert_eq!(p1.total, 3, "商户 1 应有 3 单");
    assert_eq!(p2.total, 1, "设备 9200 应只有 1 单");
    assert_eq!(
        cache.stats().misses(),
        2,
        "两个筛选条件应各自执行一次 COUNT，不共享缓存项"
    );

    common::teardown_all(&pool).await;
    pool.close_all().await;
}
