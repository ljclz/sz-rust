// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 Saga 分布式事务订单 E2E 集成测试
//!
//! 对应 tasks.md §5.5：端到端验证 Saga 订单全场景
//!
//! ## 测试矩阵
//!
//! | 场景 | 库存 | 支付 | 补偿 | 预期 |
//! |------|------|------|------|------|
//! | 全部成功 | 充足 | 成功 | - | Success, status=2, stock 扣减 |
//! | 库存不足 | 不足 | - | - | Compensated, failed=deduct_stock, 订单删除 |
//! | 支付失败 | 充足 | 失败 | 成功 | Compensated, failed=initiate_payment, 库存恢复+订单删除 |
//! | 补偿失败 | 充足 | 失败 | restore 失败 | ManualIntervention, failed=initiate_payment |
//!
//! 运行前确保 MySQL 9.6 运行于 127.0.0.1:3306，root/$SZ_TEST_DB_PASSWORD，sz_orm_test 数据库存在。
//! 手动运行：`cargo test -p sz-rust-sz300 --test order_saga_e2e --features v19-saga -- --ignored`

#![cfg(feature = "v19-saga")]

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use sz_rust_core::orm::{Pool, Value as DbValue};
use sz_rust_distributed_tx::DtxError;
use sz_rust_sz300::{
    config, db,
    models::{order::Order, order_item::OrderItem},
    services::saga_order::{
        create_with_saga, DbPaymentService, DbStockService, OrderCreateResult, PaymentService,
        StockService,
    },
};

mod common;

fn mysql_test_config() -> config::AppConfig {
    config::AppConfig {
        server: config::ServerConfig {
            host: "0.0.0.0".to_string(),
            port: 8300,
        },
        database: config::DatabaseConfig {
            host: "127.0.0.1".to_string(),
            port: 3306,
            username: "root".to_string(),
            password: std::env::var("SZ_TEST_DB_PASSWORD").unwrap_or_default(),
            database: "sz_orm_test".to_string(),
        },
    }
}

async fn setup_tables(pool: &Pool) -> anyhow::Result<()> {
    common::setup_all_tables(pool).await;
    Ok(())
}

/// 种子父记录：FK 链 market(1) → merchant(1) → device(1)。
///
/// 迁移 020/021 加 FK 后，good/order 引用 merchant(1)、order 引用 device(1)，
/// 必须先插入父行否则 INSERT 触发 FK 1452。用 INSERT IGNORE 防并行/残留重复。
async fn seed_fixture(pool: &Pool) -> anyhow::Result<()> {
    let mut conn = pool.acquire().await?;
    conn.execute("INSERT IGNORE INTO market (market_id, name) VALUES (1, 'saga-test-market')")
        .await?;
    conn.execute("INSERT IGNORE INTO merchant (merchant_id, market_id, name) VALUES (1, 1, 'saga-test-merchant')")
        .await?;
    conn.execute("INSERT IGNORE INTO device (device_id, merchant_id, device_sn, status) VALUES (1, 1, 'SN-SAGA-FIXTURE', 1)")
        .await?;
    Ok(())
}

async fn insert_product(pool: &Pool, good_id: i64, stock: i32) -> anyhow::Result<()> {
    let mut conn = pool.acquire().await?;
    // cat_id/ai_class_id 显式 NULL：迁移 013~019 已把哨兵 0 改 NULL 语义，
    // 且 020/021 加了 fk_good_cat/fk_good_ai_class，默认 0 在空 category/ai_category 表上触发 FK 1452。
    conn.execute_with_params(
        "INSERT INTO good (good_id, merchant_id, name, stock, price, cat_id, ai_class_id) \
         VALUES (?, 1, ?, ?, ?, NULL, NULL)",
        &[
            DbValue::I64(good_id),
            DbValue::String(format!("test-good-{}", good_id)),
            DbValue::I32(stock),
            DbValue::I64(100),
        ],
    )
    .await?;
    Ok(())
}

async fn get_order_status(pool: &Pool, order_id: i64) -> anyhow::Result<Option<i8>> {
    let mut conn = pool.acquire().await?;
    let rows = conn
        .query_with_params(
            "SELECT status FROM `order` WHERE order_id = ?",
            &[DbValue::I64(order_id)],
        )
        .await?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|r| r.get("status").and_then(|v| v.as_i64()).map(|v| v as i8)))
}

async fn get_product_stock(pool: &Pool, good_id: i64) -> anyhow::Result<Option<i32>> {
    let mut conn = pool.acquire().await?;
    let rows = conn
        .query_with_params(
            "SELECT stock FROM good WHERE good_id = ?",
            &[DbValue::I64(good_id)],
        )
        .await?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|r| r.get("stock").and_then(|v| v.as_i64()).map(|v| v as i32)))
}

/// 查询指定 order_no 的订单是否存在（并行隔离：只查自己创建的订单，
/// 不受其他并发测试订单影响；补偿成功应返回 false）
async fn order_exists_by_no(pool: &Pool, order_no: &str) -> anyhow::Result<bool> {
    let mut conn = pool.acquire().await?;
    let rows = conn
        .query_with_params(
            "SELECT 1 AS one FROM `order` WHERE order_no = ?",
            &[DbValue::String(order_no.to_string())],
        )
        .await?;
    Ok(!rows.is_empty())
}

fn make_order(order_no: &str, total_fen: i64) -> Order {
    Order {
        order_id: None,
        order_no: order_no.to_string(),
        merchant_id: 1,
        device_id: Some(1),
        total_fen,
        total_weight_g: 100,
        item_count: 1,
        status: 0,
        pay_method: 1,
        pay_at: None,
        offline_seq: String::new(),
        created_at: None,
        updated_at: None,
    }
}

fn make_order_item(good_id: i64, quantity: i32, price_fen: i64) -> OrderItem {
    OrderItem {
        item_id: None,
        order_id: 0,
        good_id: Some(good_id),
        good_name: format!("product-{}", good_id),
        price_fen,
        weight_g: 100,
        total_fen: price_fen * quantity as i64,
        quantity,
    }
}

fn unique_order_no(prefix: &str) -> String {
    // order_no 为 VARCHAR(32)（001_init.sql），拼接纳秒时间戳会造成超长截断错误，
    // 统一用 12 位尾号并把前缀裁剪到剩余长度内
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        % 1_000_000_000_000;
    let suffix = ts.to_string();
    let room = 32usize.saturating_sub(suffix.len() + 1);
    let head: String = prefix.chars().take(room).collect();
    format!("{}-{}", head, suffix)
}

struct FailPaymentService;

#[async_trait]
impl PaymentService for FailPaymentService {
    async fn initiate(&self, _order_id: i64, _amount_fen: i64) -> Result<String, DtxError> {
        Err(DtxError::Generic("mock 支付失败".into()))
    }
    async fn cancel(&self, _payment_id: &str) -> Result<(), DtxError> {
        Ok(())
    }
}

struct FailRestoreStockService;

#[async_trait]
impl StockService for FailRestoreStockService {
    async fn deduct(&self, _product_id: i64, _quantity: i32) -> Result<(), DtxError> {
        Ok(())
    }
    async fn restore(&self, _product_id: i64, _quantity: i32) -> Result<(), DtxError> {
        Err(DtxError::Generic("mock 恢复库存失败".into()))
    }
}

#[tokio::test]
#[ignore]
async fn test_saga_all_success() {
    let cfg = mysql_test_config();
    let pool = db::init_pool(&cfg).await.expect("init_pool");
    setup_tables(&pool).await.expect("setup_tables");
    seed_fixture(&pool).await.expect("seed_fixture");
    insert_product(&pool, 1001, 50)
        .await
        .expect("insert_product");

    let pool_arc = Arc::new(pool);
    let order = make_order(&unique_order_no("saga-ok"), 5000);
    let items = vec![make_order_item(1001, 5, 1000)];

    let stock_svc = Arc::new(DbStockService::new(pool_arc.clone()));
    let payment_svc = Arc::new(DbPaymentService::new(pool_arc.clone()));

    let result = create_with_saga(
        pool_arc.clone(),
        &order,
        &items,
        stock_svc,
        payment_svc,
        None,
    )
    .await
    .expect("create_with_saga");

    match &result {
        OrderCreateResult::Success { order_id, .. } => {
            let status = get_order_status(&pool_arc, *order_id)
                .await
                .expect("get_order_status");
            assert_eq!(status, Some(2), "订单状态应为已支付(2)");
            let stock = get_product_stock(&pool_arc, 1001)
                .await
                .expect("get_product_stock");
            assert_eq!(stock, Some(45), "库存应从 50 扣减至 45");
        }
        _ => panic!("应为 Success，实际: {:?}", result),
    }
}

#[tokio::test]
#[ignore]
async fn test_saga_stock_insufficient_compensated() {
    let cfg = mysql_test_config();
    let pool = db::init_pool(&cfg).await.expect("init_pool");
    setup_tables(&pool).await.expect("setup_tables");
    seed_fixture(&pool).await.expect("seed_fixture");
    insert_product(&pool, 2002, 1)
        .await
        .expect("insert_product");

    let pool_arc = Arc::new(pool);
    let order = make_order(&unique_order_no("saga-stock-fail"), 5000);
    let items = vec![make_order_item(2002, 5, 1000)];

    let stock_svc = Arc::new(DbStockService::new(pool_arc.clone()));
    let payment_svc = Arc::new(DbPaymentService::new(pool_arc.clone()));

    let result = create_with_saga(
        pool_arc.clone(),
        &order,
        &items,
        stock_svc,
        payment_svc,
        None,
    )
    .await
    .expect("create_with_saga");

    match &result {
        OrderCreateResult::Compensated { failed_step, .. } => {
            assert_eq!(failed_step, "deduct_stock");
            let exists = order_exists_by_no(&pool_arc, &order.order_no)
                .await
                .expect("order_exists_by_no");
            assert!(!exists, "订单 {} 应被补偿删除", order.order_no);
        }
        _ => panic!("应为 Compensated，实际: {:?}", result),
    }
}

#[tokio::test]
#[ignore]
async fn test_saga_payment_fail_compensated() {
    let cfg = mysql_test_config();
    let pool = db::init_pool(&cfg).await.expect("init_pool");
    setup_tables(&pool).await.expect("setup_tables");
    seed_fixture(&pool).await.expect("seed_fixture");
    insert_product(&pool, 3003, 50)
        .await
        .expect("insert_product");

    let pool_arc = Arc::new(pool);
    let order = make_order(&unique_order_no("saga-pay-fail"), 5000);
    let items = vec![make_order_item(3003, 5, 1000)];

    let stock_svc = Arc::new(DbStockService::new(pool_arc.clone()));
    let payment_svc = Arc::new(FailPaymentService);

    let result = create_with_saga(
        pool_arc.clone(),
        &order,
        &items,
        stock_svc,
        payment_svc,
        None,
    )
    .await
    .expect("create_with_saga");

    match &result {
        OrderCreateResult::Compensated { failed_step, .. } => {
            assert_eq!(failed_step, "initiate_payment");
            let stock = get_product_stock(&pool_arc, 3003)
                .await
                .expect("get_product_stock");
            assert_eq!(stock, Some(50), "库存应恢复至 50");
            let exists = order_exists_by_no(&pool_arc, &order.order_no)
                .await
                .expect("order_exists_by_no");
            assert!(!exists, "订单 {} 应被补偿删除", order.order_no);
        }
        _ => panic!("应为 Compensated，实际: {:?}", result),
    }
}

#[tokio::test]
#[ignore]
async fn test_saga_compensate_fail_manual_intervention() {
    let cfg = mysql_test_config();
    let pool = db::init_pool(&cfg).await.expect("init_pool");
    setup_tables(&pool).await.expect("setup_tables");
    seed_fixture(&pool).await.expect("seed_fixture");
    // order_item.good_id=4004 需父行 good(4004) 满足 fk_order_item_good
    insert_product(&pool, 4004, 50)
        .await
        .expect("insert_product");

    let pool_arc = Arc::new(pool);
    let order = make_order(&unique_order_no("saga-manual"), 5000);
    let items = vec![make_order_item(4004, 5, 1000)];

    let stock_svc = Arc::new(FailRestoreStockService);
    let payment_svc = Arc::new(FailPaymentService);

    let result = create_with_saga(
        pool_arc.clone(),
        &order,
        &items,
        stock_svc,
        payment_svc,
        None,
    )
    .await
    .expect("create_with_saga");

    match &result {
        OrderCreateResult::ManualIntervention { failed_step, .. } => {
            assert_eq!(failed_step, "initiate_payment");
        }
        _ => panic!("应为 ManualIntervention，实际: {:?}", result),
    }
}
