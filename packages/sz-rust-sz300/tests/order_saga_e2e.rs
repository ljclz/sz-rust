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
//! 运行前确保 MySQL 9.6 运行于 127.0.0.1:3306，root/test123，sz_orm_test 数据库存在。
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
            password: "test123".to_string(),
            database: "sz_orm_test".to_string(),
        },
    }
}

async fn setup_tables(pool: &Pool) -> anyhow::Result<()> {
    let mut conn = pool.acquire().await?;
    conn.query("DROP TABLE IF EXISTS `order_item`").await?;
    conn.query("DROP TABLE IF EXISTS `order`").await?;
    conn.query(
        "CREATE TABLE `order` (\
            `order_id` BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,\
            `order_no` VARCHAR(64) NOT NULL UNIQUE,\
            `merchant_id` INT UNSIGNED NOT NULL,\
            `device_id` INT UNSIGNED DEFAULT 0,\
            `total_fen` INT UNSIGNED NOT NULL,\
            `total_weight_g` INT UNSIGNED DEFAULT 0,\
            `item_count` SMALLINT UNSIGNED DEFAULT 0,\
            `status` TINYINT DEFAULT 0,\
            `pay_method` TINYINT DEFAULT 0,\
            `pay_at` DATETIME DEFAULT NULL,\
            `offline_seq` VARCHAR(50) DEFAULT '',\
            `created_at` DATETIME DEFAULT CURRENT_TIMESTAMP,\
            `updated_at` DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP\
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await?;
    conn.query(
        "CREATE TABLE `order_item` (\
            `item_id` BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,\
            `order_id` BIGINT UNSIGNED NOT NULL,\
            `good_id` INT UNSIGNED NOT NULL,\
            `good_name` VARCHAR(100) NOT NULL,\
            `price_fen` INT UNSIGNED NOT NULL,\
            `weight_g` INT UNSIGNED DEFAULT 0,\
            `total_fen` INT UNSIGNED NOT NULL,\
            `quantity` INT NOT NULL DEFAULT 1\
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await?;
    conn.query("DROP TABLE IF EXISTS `product`").await?;
    conn.query(
        "CREATE TABLE `product` (\
            `product_id` INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,\
            `name` VARCHAR(100) NOT NULL,\
            `stock` INT NOT NULL DEFAULT 0,\
            `price` INT UNSIGNED NOT NULL DEFAULT 0\
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await?;
    Ok(())
}

async fn insert_product(pool: &Pool, product_id: i64, stock: i32) -> anyhow::Result<()> {
    let mut conn = pool.acquire().await?;
    conn.execute_with_params(
        "INSERT INTO product (product_id, name, stock, price) VALUES (?, ?, ?, ?)",
        &[
            DbValue::I64(product_id),
            DbValue::String(format!("test-product-{}", product_id)),
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

async fn get_product_stock(pool: &Pool, product_id: i64) -> anyhow::Result<Option<i32>> {
    let mut conn = pool.acquire().await?;
    let rows = conn
        .query_with_params(
            "SELECT stock FROM product WHERE product_id = ?",
            &[DbValue::I64(product_id)],
        )
        .await?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|r| r.get("stock").and_then(|v| v.as_i64()).map(|v| v as i32)))
}

async fn count_orders(pool: &Pool) -> anyhow::Result<i64> {
    let mut conn = pool.acquire().await?;
    let rows = conn.query("SELECT COUNT(*) as cnt FROM `order`").await?;
    Ok(rows
        .first()
        .and_then(|r| r.get("cnt").and_then(|v| v.as_i64()))
        .unwrap_or(0))
}

fn make_order(order_no: &str, total_fen: i64) -> Order {
    Order {
        order_id: None,
        order_no: order_no.to_string(),
        merchant_id: 1,
        device_id: 1,
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
        good_id,
        good_name: format!("product-{}", good_id),
        price_fen,
        weight_g: 100,
        total_fen: price_fen * quantity as i64,
        quantity,
    }
}

fn unique_order_no(prefix: &str) -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{}", prefix, ts)
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
            let cnt = count_orders(&pool_arc).await.expect("count_orders");
            assert_eq!(cnt, 0, "订单应被补偿删除");
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
            let cnt = count_orders(&pool_arc).await.expect("count_orders");
            assert_eq!(cnt, 0, "订单应被补偿删除");
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
