// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! GraphQL 持久化集成测试（v1.9.0 P0 — DB-2026-10-05-01 ②）
//!
//! 验证 `v19-graphql-persist` feature gate 下：
//! 1. DB-backed Schema 构建（深度/复杂度限制保持）
//! 2. DB-backed Query：merchant/product/order 查询
//! 3. DB-backed Mutation：createProduct/updateProduct/createOrder/updateOrderStatus
//! 4. 数据持久化验证（写入后可查询回）
//!
//! 运行前确保 MySQL 9.6 运行于 127.0.0.1:3306，root/test123，sz_orm_test 数据库存在。
//! 手动运行：`cargo test -p sz-rust-sz300 --test graphql_persist_test --features v19-graphql-persist -- --ignored`

#![cfg(feature = "v19-graphql-persist")]

use std::sync::Arc;
use sz_rust_sz300::{config, db, graphql::build_schema_with_db};

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

async fn try_build_db_schema() -> Option<sz_rust_sz300::graphql::GraphQLSchemaDb> {
    let cfg = mysql_test_config();
    let pool = db::init_pool(&cfg).await.ok()?;
    let mut conn = pool.acquire().await.ok()?;
    conn.query("SELECT 1").await.ok()?;

    conn.query(
        "CREATE TABLE IF NOT EXISTS `merchant` (\
            `merchant_id` INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,\
            `market_id` INT UNSIGNED NOT NULL,\
            `name` VARCHAR(100) NOT NULL,\
            `stall_no` VARCHAR(50) DEFAULT '',\
            `contact_phone` VARCHAR(20) DEFAULT '',\
            `category` VARCHAR(50) DEFAULT '',\
            `status` TINYINT DEFAULT 1,\
            `bank_account` VARCHAR(50) DEFAULT '',\
            `bank_name` VARCHAR(100) DEFAULT '',\
            `created_at` DATETIME DEFAULT CURRENT_TIMESTAMP,\
            `updated_at` DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP\
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .ok()?;
    conn.query(
        "CREATE TABLE IF NOT EXISTS `good` (\
            `good_id` INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,\
            `merchant_id` INT UNSIGNED NOT NULL,\
            `cat_id` INT UNSIGNED DEFAULT 0,\
            `name` VARCHAR(100) NOT NULL,\
            `barcode` VARCHAR(50) DEFAULT '',\
            `price` INT UNSIGNED NOT NULL,\
            `unit` VARCHAR(10) DEFAULT '斤',\
            `ai_class_id` INT UNSIGNED DEFAULT 0,\
            `image` VARCHAR(255) DEFAULT '',\
            `status` TINYINT DEFAULT 1,\
            `created_at` DATETIME DEFAULT CURRENT_TIMESTAMP,\
            `updated_at` DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP\
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .ok()?;
    conn.query("DROP TABLE IF EXISTS `order`").await.ok()?;
    conn.query(
        "CREATE TABLE `order` (\
            `order_id` BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,\
            `order_no` VARCHAR(32) NOT NULL UNIQUE,\
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
    .await
    .ok()?;

    Some(build_schema_with_db(Arc::new(pool)))
}

// ============================================================================
// Schema SDL 测试（不需要 DB 数据，但需要 Pool 构建）
// ============================================================================

#[tokio::test]
#[ignore = "需要 MySQL 9.6 运行于 127.0.0.1:3306"]

async fn test_db_schema_sdl() {
    let schema = try_build_db_schema().await;
    let Some(schema) = schema else {
        eprintln!("⚠️ MySQL 不可达，跳过 DB schema SDL 测试");
        return;
    };
    let sdl = schema.sdl();
    assert!(sdl.contains("DbQueryRoot"), "SDL 应包含 DbQueryRoot");
    assert!(sdl.contains("DbMutationRoot"), "SDL 应包含 DbMutationRoot");
    assert!(sdl.contains("MerchantGql"), "SDL 应包含 MerchantGql");
    assert!(sdl.contains("ProductGql"), "SDL 应包含 ProductGql");
    assert!(sdl.contains("OrderGql"), "SDL 应包含 OrderGql");
    assert!(sdl.contains("merchant"), "应有 merchant 查询");
    assert!(sdl.contains("createProduct"), "应有 createProduct 变更");
}

// ============================================================================
// Query 测试
// ============================================================================

#[tokio::test]
#[ignore = "需要 MySQL 9.6 运行于 127.0.0.1:3306"]
async fn test_db_query_merchant_not_found() {
    let schema = try_build_db_schema().await;
    let Some(schema) = schema else {
        eprintln!("⚠️ MySQL 不可达，跳过");
        return;
    };
    let resp = schema
        .execute(r#"{ merchant(id: 999999) { merchantId name } }"#)
        .await;
    assert!(
        resp.errors.is_empty(),
        "查询不存在的商户应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert!(data["merchant"].is_null(), "不存在的商户应返回 null");
}

#[tokio::test]
#[ignore = "需要 MySQL 9.6 运行于 127.0.0.1:3306"]
async fn test_db_query_product_not_found() {
    let schema = try_build_db_schema().await;
    let Some(schema) = schema else {
        eprintln!("⚠️ MySQL 不可达，跳过");
        return;
    };
    let resp = schema
        .execute(r#"{ product(id: 999999) { goodId name } }"#)
        .await;
    assert!(resp.errors.is_empty(), "查询应无错误: {:?}", resp.errors);
    let data = resp.data.into_json().unwrap();
    assert!(data["product"].is_null(), "不存在的商品应返回 null");
}

#[tokio::test]
#[ignore = "需要 MySQL 9.6 运行于 127.0.0.1:3306"]
async fn test_db_query_order_not_found() {
    let schema = try_build_db_schema().await;
    let Some(schema) = schema else {
        eprintln!("⚠️ MySQL 不可达，跳过");
        return;
    };
    let resp = schema
        .execute(r#"{ order(id: 999999) { orderId orderNo } }"#)
        .await;
    assert!(resp.errors.is_empty(), "查询应无错误: {:?}", resp.errors);
    let data = resp.data.into_json().unwrap();
    assert!(data["order"].is_null(), "不存在的订单应返回 null");
}

#[tokio::test]
#[ignore = "需要 MySQL 9.6 运行于 127.0.0.1:3306"]
async fn test_db_query_merchants_returns_list() {
    let schema = try_build_db_schema().await;
    let Some(schema) = schema else {
        eprintln!("⚠️ MySQL 不可达，跳过");
        return;
    };
    let resp = schema.execute(r#"{ merchants { merchantId name } }"#).await;
    assert!(
        resp.errors.is_empty(),
        "查询商户列表应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert!(data["merchants"].is_array(), "商户列表应返回数组");
}

#[tokio::test]
#[ignore = "需要 MySQL 9.6 运行于 127.0.0.1:3306"]
async fn test_db_query_products_returns_list() {
    let schema = try_build_db_schema().await;
    let Some(schema) = schema else {
        eprintln!("⚠️ MySQL 不可达，跳过");
        return;
    };
    let resp = schema.execute(r#"{ products { goodId name } }"#).await;
    assert!(
        resp.errors.is_empty(),
        "查询商品列表应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert!(data["products"].is_array(), "商品列表应返回数组");
}

#[tokio::test]
#[ignore = "需要 MySQL 9.6 运行于 127.0.0.1:3306"]
async fn test_db_query_orders_returns_list() {
    let schema = try_build_db_schema().await;
    let Some(schema) = schema else {
        eprintln!("⚠️ MySQL 不可达，跳过");
        return;
    };
    let resp = schema.execute(r#"{ orders { orderId orderNo } }"#).await;
    assert!(
        resp.errors.is_empty(),
        "查询订单列表应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert!(data["orders"].is_array(), "订单列表应返回数组");
}

// ============================================================================
// Mutation + 持久化验证测试
// ============================================================================

#[tokio::test]
#[ignore = "需要 MySQL 9.6 运行于 127.0.0.1:3306"]
async fn test_db_mutation_create_and_query_product() {
    let schema = try_build_db_schema().await;
    let Some(schema) = schema else {
        eprintln!("⚠️ MySQL 不可达，跳过");
        return;
    };
    let resp = schema
        .execute(
            r#"mutation {
                createProduct(
                    merchantId: 1, catId: 1, name: "GraphQL持久化测试商品",
                    barcode: "GPTEST001", price: 888, unit: "个"
                ) { goodId name price status }
            }"#,
        )
        .await;
    assert!(
        resp.errors.is_empty(),
        "创建商品应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();

    let good_id = data["createProduct"]["goodId"].as_i64().unwrap();
    assert!(good_id > 0, "DB 自增 ID 应 > 0");
    assert_eq!(data["createProduct"]["name"], "GraphQL持久化测试商品");
    assert_eq!(data["createProduct"]["price"], 888);
    assert_eq!(data["createProduct"]["status"], 1);

    let query = format!(
        r#"{{ product(id: {}) {{ goodId name price status }} }}"#,
        good_id
    );
    let resp = schema.execute(&query).await;
    assert!(
        resp.errors.is_empty(),
        "查询刚创建的商品应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert_eq!(data["product"]["goodId"], good_id, "查询回的商品 ID 应匹配");
    assert_eq!(
        data["product"]["name"], "GraphQL持久化测试商品",
        "数据已持久化到 DB"
    );
}

#[tokio::test]
#[ignore = "需要 MySQL 9.6 运行于 127.0.0.1:3306"]
async fn test_db_mutation_update_product() {
    let schema = try_build_db_schema().await;
    let Some(schema) = schema else {
        eprintln!("⚠️ MySQL 不可达，跳过");
        return;
    };
    let create_resp = schema
        .execute(
            r#"mutation {
                createProduct(
                    merchantId: 1, catId: 1, name: "更新测试商品",
                    barcode: "GPUPD001", price: 100, unit: "个"
                ) { goodId }
            }"#,
        )
        .await;
    assert!(create_resp.errors.is_empty());
    let data = create_resp.data.into_json().unwrap();
    let good_id = data["createProduct"]["goodId"].as_i64().unwrap();

    let update_query = format!(
        r#"mutation {{ updateProduct(id: {}, status: 0) {{ goodId status }} }}"#,
        good_id
    );
    let resp = schema.execute(&update_query).await;
    assert!(
        resp.errors.is_empty(),
        "更新商品应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert_eq!(
        data["updateProduct"]["status"], 0,
        "更新后状态应为 0（下架）"
    );

    let verify_query = format!(r#"{{ product(id: {}) {{ status }} }}"#, good_id);
    let resp = schema.execute(&verify_query).await;
    assert!(resp.errors.is_empty());
    let data = resp.data.into_json().unwrap();
    assert_eq!(data["product"]["status"], 0, "DB 中状态已持久化为 0");
}

#[tokio::test]
#[ignore = "需要 MySQL 9.6 运行于 127.0.0.1:3306"]
async fn test_db_mutation_create_and_update_order() {
    let schema = try_build_db_schema().await;
    let Some(schema) = schema else {
        eprintln!("⚠️ MySQL 不可达，跳过");
        return;
    };
    let order_no = format!(
        "GQL-ORD-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let create_query = format!(
        r#"mutation {{
            createOrder(
                orderNo: "{}",
                merchantId: 1, deviceId: 1,
                totalFen: 1500, itemCount: 3
            ) {{ orderId orderNo status payMethod }}
        }}"#,
        order_no
    );
    let create_resp = schema.execute(&create_query).await;
    assert!(
        create_resp.errors.is_empty(),
        "创建订单应无错误: {:?}",
        create_resp.errors
    );
    let data = create_resp.data.into_json().unwrap();
    let order_id = data["createOrder"]["orderId"].as_i64().unwrap();
    assert!(order_id > 0, "DB 自增 ID 应 > 0");
    assert_eq!(data["createOrder"]["orderNo"], order_no);
    assert_eq!(data["createOrder"]["status"], 1, "新订单状态应为 1");
    assert_eq!(data["createOrder"]["payMethod"], 0, "新订单支付方式应为 0");

    let update_query = format!(
        r#"mutation {{ updateOrderStatus(id: {}, status: 2, payMethod: 1) {{ orderId status payMethod }} }}"#,
        order_id
    );
    let resp = schema.execute(&update_query).await;
    assert!(
        resp.errors.is_empty(),
        "更新订单状态应无错误: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    assert_eq!(
        data["updateOrderStatus"]["status"], 2,
        "状态应为 2（已支付）"
    );
    assert_eq!(
        data["updateOrderStatus"]["payMethod"], 1,
        "支付方式应为 1（微信）"
    );

    let verify_query = format!(r#"{{ order(id: {}) {{ status payMethod }} }}"#, order_id);
    let resp = schema.execute(&verify_query).await;
    assert!(resp.errors.is_empty());
    let data = resp.data.into_json().unwrap();
    assert_eq!(data["order"]["status"], 2, "DB 中订单状态已持久化为 2");
    assert_eq!(data["order"]["payMethod"], 1, "DB 中支付方式已持久化为 1");
}

// ============================================================================
// 深度/复杂度限制测试（DB-backed schema 仍应生效）
// ============================================================================

#[tokio::test]
#[ignore = "需要 MySQL 9.6 运行于 127.0.0.1:3306"]
async fn test_db_depth_limit_exceeded() {
    let schema = try_build_db_schema().await;
    let Some(schema) = schema else {
        eprintln!("⚠️ MySQL 不可达，跳过");
        return;
    };
    let deep_query = format!(
        "query {{ {} }}",
        "merchant(id: 1) { ".repeat(15) + "merchantId " + &"}".repeat(15)
    );
    let resp = schema.execute(&deep_query).await;
    assert!(
        !resp.errors.is_empty(),
        "超过深度限制应返回错误: {:?}",
        resp.errors
    );
}

#[tokio::test]
#[ignore = "需要 MySQL 9.6 运行于 127.0.0.1:3306"]
async fn test_db_complexity_limit_exceeded() {
    let schema = try_build_db_schema().await;
    let Some(schema) = schema else {
        eprintln!("⚠️ MySQL 不可达，跳过");
        return;
    };
    let mut fields = String::new();
    for i in 0..2000 {
        fields.push_str(&format!("p{}: product(id: {}) {{ goodId }} ", i, i));
    }
    let query = format!("query {{ {} }}", fields);
    let resp = schema.execute(&query).await;
    assert!(
        !resp.errors.is_empty(),
        "超过复杂度限制应返回错误: {:?}",
        resp.errors
    );
}

#[tokio::test]
#[ignore = "需要 MySQL 9.6 运行于 127.0.0.1:3306"]
async fn test_db_depth_limit_within_bound() {
    let schema = try_build_db_schema().await;
    let Some(schema) = schema else {
        eprintln!("⚠️ MySQL 不可达，跳过");
        return;
    };
    let resp = schema
        .execute(r#"{ merchant(id: 1) { merchantId name stallNo } }"#)
        .await;
    assert!(
        resp.errors.is_empty(),
        "正常深度查询应无错误: {:?}",
        resp.errors
    );
}
