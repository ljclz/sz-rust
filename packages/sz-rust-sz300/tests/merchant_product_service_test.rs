// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P2 第二批：merchant_service + product_service DB 集成测试
//!
//! 覆盖 list/get/create/update/delete CRUD 路径，使用真实 MySQL 9.6。
//! 运行方式：
//! ```
//! cargo test -p sz-rust-sz300 --test merchant_product_service_test -- --ignored
//! ```
//!
//! 纯逻辑测试（ProductFilters WHERE 构建、白名单、offset 公式）已在
//! `src/services/merchant_service.rs` 和 `src/services/product_service.rs`
//! 的内嵌 `#[cfg(test)] mod tests` 中覆盖，此处不重复。

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::LazyLock;

use sz_rust_core::orm::{Pool, Value};
use sz_rust_sz300::{config, db, models::merchant::Merchant, models::product::Product, services};
use tokio::sync::Mutex;

mod common;

/// 全局序列化锁：所有 ignored 集成测试共享同一真实 MySQL，
/// 并行 DROP/CREATE 同名表会冲突，故串行执行。
static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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

async fn ensure_mysql() -> Option<Pool> {
    let cfg = mysql_test_config();
    match db::init_pool(&cfg).await {
        Ok(pool) => match pool.acquire().await {
            Ok(mut conn) => match conn.query("SELECT 1").await {
                Ok(_) => Some(pool),
                Err(_) => {
                    pool.close_all().await;
                    None
                }
            },
            Err(_) => {
                pool.close_all().await;
                None
            }
        },
        Err(_) => None,
    }
}

async fn cleanup_tables(pool: &Pool) {
    common::teardown_all(pool).await;
}

/// 种子父记录：FK 链 market(1) → merchant(1)。
/// 迁移 020/021 加 FK 后，merchant 引用 market，good 引用 merchant，
/// 必须先插入父行否则 INSERT 触发 FK 1452。
async fn seed_market(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute("INSERT IGNORE INTO market (market_id, name) VALUES (1, 'mp-test-market')")
        .await
        .expect("插入 market 失败");
}

fn make_merchant(name: &str) -> Merchant {
    Merchant {
        merchant_id: None,
        market_id: 1,
        name: name.to_string(),
        stall_no: "A-01".to_string(),
        contact_phone: "13800000000".to_string(),
        category: "水产".to_string(),
        status: 1,
        bank_account: "6222000000000000000".to_string(),
        bank_name: "工商银行".to_string(),
        created_at: None,
        updated_at: None,
    }
}

fn make_product(name: &str, merchant_id: i64) -> Product {
    Product {
        good_id: None,
        merchant_id,
        cat_id: None,
        name: name.to_string(),
        barcode: "6900000000001".to_string(),
        price: 3000,
        unit: "斤".to_string(),
        ai_class_id: None,
        image: "https://cdn.example.com/img.jpg".to_string(),
        status: 1,
        created_at: None,
        updated_at: None,
    }
}

// ============================================================================
// MerchantService CRUD
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_merchant_create_then_get_round_trip() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_merchant_table(&pool).await;
    seed_market(&pool).await;

    let merchant = make_merchant("MP-CREATE-TEST");
    services::merchant_service::MerchantService::create(&pool, &merchant)
        .await
        .expect("create 失败");

    // 查询刚创建的商户
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            "SELECT merchant_id, name FROM merchant WHERE name = ?",
            &[Value::String("MP-CREATE-TEST".to_string())],
        )
        .await
        .expect("查询失败");
    assert_eq!(rows.len(), 1);
    let merchant_id = rows[0]
        .get("merchant_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    assert!(merchant_id > 0, "merchant_id 应为自增正数");

    // 用 service::get 验证
    let got = services::merchant_service::MerchantService::get(&pool, merchant_id)
        .await
        .expect("get 失败");
    assert!(got.is_some(), "刚创建的商户应能查到");
    let row = got.unwrap();
    let name = row.get("name").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(name, "MP-CREATE-TEST");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_merchant_get_nonexistent_returns_none() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_merchant_table(&pool).await;

    let got = services::merchant_service::MerchantService::get(&pool, 999_999_999)
        .await
        .expect("get 不应失败");
    assert!(got.is_none(), "不存在的 merchant_id 应返回 None");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_merchant_list_with_idor_filter_returns_only_own() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_merchant_table(&pool).await;
    seed_market(&pool).await;

    // 创建两个商户
    services::merchant_service::MerchantService::create(&pool, &make_merchant("MP-LIST-A"))
        .await
        .expect("create A 失败");
    services::merchant_service::MerchantService::create(&pool, &make_merchant("MP-LIST-B"))
        .await
        .expect("create B 失败");

    // 查询 merchant_id=1 的商户（IDOR 过滤）
    let page = services::merchant_service::MerchantService::list(&pool, 1, 100, Some(1))
        .await
        .expect("list 失败");
    assert_eq!(page.total, 1, "IDOR 过滤应只返回 1 个商户");
    assert_eq!(page.list.len(), 1);

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_merchant_list_admin_path_returns_all() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_merchant_table(&pool).await;
    seed_market(&pool).await;

    services::merchant_service::MerchantService::create(&pool, &make_merchant("MP-ADMIN-1"))
        .await
        .expect("create 1 失败");
    services::merchant_service::MerchantService::create(&pool, &make_merchant("MP-ADMIN-2"))
        .await
        .expect("create 2 失败");

    // 管理员路径：merchant_id=None，不附加 WHERE 过滤
    let page = services::merchant_service::MerchantService::list(&pool, 1, 100, None)
        .await
        .expect("list 失败");
    assert!(
        page.total >= 2,
        "管理员路径应返回所有商户, total={}",
        page.total
    );

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_merchant_update_changes_name_field() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_merchant_table(&pool).await;
    seed_market(&pool).await;

    services::merchant_service::MerchantService::create(&pool, &make_merchant("MP-UPD-BEFORE"))
        .await
        .expect("create 失败");

    // 找到刚创建的 merchant_id
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            "SELECT merchant_id FROM merchant WHERE name = ?",
            &[Value::String("MP-UPD-BEFORE".to_string())],
        )
        .await
        .expect("查询失败");
    let merchant_id = rows[0]
        .get("merchant_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    // 更新 name 字段
    let mut fields: HashMap<String, Value> = HashMap::new();
    fields.insert("name".into(), Value::String("MP-UPD-AFTER".into()));
    services::merchant_service::MerchantService::update(&pool, merchant_id, fields)
        .await
        .expect("update 失败");

    // 验证更新生效
    let rows = conn
        .query_with_params(
            "SELECT name FROM merchant WHERE merchant_id = ?",
            &[Value::I64(merchant_id)],
        )
        .await
        .expect("查询失败");
    let name = rows[0].get("name").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(name, "MP-UPD-AFTER", "name 应已更新");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_merchant_update_empty_fields_returns_error_without_db() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_merchant_table(&pool).await;

    let fields: HashMap<String, Value> = HashMap::new();
    let result = services::merchant_service::MerchantService::update(&pool, 1, fields).await;
    assert!(result.is_err(), "空 fields 应返回 Err");
    let msg = result.unwrap_err();
    assert!(msg.contains("未提供"), "msg: {msg}");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_merchant_delete_is_soft_delete_status_zero() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_merchant_table(&pool).await;
    seed_market(&pool).await;

    services::merchant_service::MerchantService::create(&pool, &make_merchant("MP-DEL-SOFT"))
        .await
        .expect("create 失败");

    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            "SELECT merchant_id FROM merchant WHERE name = ?",
            &[Value::String("MP-DEL-SOFT".to_string())],
        )
        .await
        .expect("查询失败");
    let merchant_id = rows[0]
        .get("merchant_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    services::merchant_service::MerchantService::delete(&pool, merchant_id)
        .await
        .expect("delete 失败");

    // 软删除：status 应为 0，行仍存在
    let rows = conn
        .query_with_params(
            "SELECT status FROM merchant WHERE merchant_id = ?",
            &[Value::I64(merchant_id)],
        )
        .await
        .expect("查询失败");
    assert_eq!(rows.len(), 1, "软删除后行应仍存在");
    let status = rows[0].get("status").and_then(|v| v.as_i64()).unwrap_or(-1);
    assert_eq!(status, 0, "软删除应置 status=0");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// ProductService CRUD
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_product_create_returns_good_id() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_product_table(&pool).await;
    common::setup_merchant_table(&pool).await;
    seed_market(&pool).await;
    // 插入父商户
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("INSERT IGNORE INTO merchant (merchant_id, market_id, name) VALUES (1, 1, 'mp-prod-test')")
            .await
            .expect("插入 merchant 失败");
    }

    let product = make_product("MP-PROD-CREATE", 1);
    let good_id = services::product_service::ProductService::create(&pool, &product)
        .await
        .expect("create 失败");
    assert!(
        good_id > 0,
        "create 应返回自增 good_id > 0, 实际: {good_id}"
    );

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_product_get_nonexistent_returns_none() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_product_table(&pool).await;

    let got = services::product_service::ProductService::get(&pool, 999_999_999)
        .await
        .expect("get 不应失败");
    assert!(got.is_none(), "不存在的 good_id 应返回 None");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_product_list_with_merchant_filter() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_product_table(&pool).await;
    common::setup_merchant_table(&pool).await;
    seed_market(&pool).await;
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute(
            "INSERT IGNORE INTO merchant (merchant_id, market_id, name) VALUES (1, 1, 'mp-list-1')",
        )
        .await
        .expect("插入 merchant 1 失败");
        conn.execute(
            "INSERT IGNORE INTO merchant (merchant_id, market_id, name) VALUES (2, 1, 'mp-list-2')",
        )
        .await
        .expect("插入 merchant 2 失败");
    }

    services::product_service::ProductService::create(&pool, &make_product("MP-PROD-M1-A", 1))
        .await
        .expect("create M1-A 失败");
    services::product_service::ProductService::create(&pool, &make_product("MP-PROD-M1-B", 1))
        .await
        .expect("create M1-B 失败");
    services::product_service::ProductService::create(&pool, &make_product("MP-PROD-M2-A", 2))
        .await
        .expect("create M2-A 失败");

    let filters = services::product_service::ProductFilters {
        merchant_id: Some(1),
        cat_id: None,
        keyword: None,
    };
    let page = services::product_service::ProductService::list(&pool, 1, 100, filters)
        .await
        .expect("list 失败");
    assert_eq!(page.total, 2, "merchant_id=1 过滤应返回 2 个商品");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_product_list_with_keyword_filter() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_product_table(&pool).await;
    common::setup_merchant_table(&pool).await;
    seed_market(&pool).await;
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("INSERT IGNORE INTO merchant (merchant_id, market_id, name) VALUES (1, 1, 'mp-kw-test')")
            .await
            .expect("插入 merchant 失败");
    }

    services::product_service::ProductService::create(&pool, &make_product("鲈鱼活鲜", 1))
        .await
        .expect("create 鲈鱼 失败");
    services::product_service::ProductService::create(&pool, &make_product("三文鱼冰鲜", 1))
        .await
        .expect("create 三文鱼 失败");
    services::product_service::ProductService::create(&pool, &make_product("白菜蔬菜", 1))
        .await
        .expect("create 白菜 失败");

    let filters = services::product_service::ProductFilters {
        merchant_id: None,
        cat_id: None,
        keyword: Some("鱼".to_string()),
    };
    let page = services::product_service::ProductService::list(&pool, 1, 100, filters)
        .await
        .expect("list 失败");
    assert_eq!(
        page.total, 2,
        "keyword='鱼' 应匹配 '鲈鱼活鲜' 和 '三文鱼冰鲜', total={}",
        page.total
    );

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_product_update_changes_price_field() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_product_table(&pool).await;
    common::setup_merchant_table(&pool).await;
    seed_market(&pool).await;
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("INSERT IGNORE INTO merchant (merchant_id, market_id, name) VALUES (1, 1, 'mp-upd-prod')")
            .await
            .expect("插入 merchant 失败");
    }

    let good_id =
        services::product_service::ProductService::create(&pool, &make_product("MP-PROD-UPD", 1))
            .await
            .expect("create 失败");

    let mut fields: HashMap<String, Value> = HashMap::new();
    fields.insert("price".into(), Value::I64(9999));
    services::product_service::ProductService::update(&pool, good_id, fields)
        .await
        .expect("update 失败");

    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            "SELECT price FROM good WHERE good_id = ?",
            &[Value::I64(good_id)],
        )
        .await
        .expect("查询失败");
    let price = rows[0].get("price").and_then(|v| v.as_i64()).unwrap_or(0);
    assert_eq!(price, 9999, "price 应已更新为 9999");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_product_delete_is_soft_delete_status_zero() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_product_table(&pool).await;
    common::setup_merchant_table(&pool).await;
    seed_market(&pool).await;
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("INSERT IGNORE INTO merchant (merchant_id, market_id, name) VALUES (1, 1, 'mp-del-prod')")
            .await
            .expect("插入 merchant 失败");
    }

    let good_id =
        services::product_service::ProductService::create(&pool, &make_product("MP-PROD-DEL", 1))
            .await
            .expect("create 失败");

    services::product_service::ProductService::delete(&pool, good_id)
        .await
        .expect("delete 失败");

    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            "SELECT status FROM good WHERE good_id = ?",
            &[Value::I64(good_id)],
        )
        .await
        .expect("查询失败");
    assert_eq!(rows.len(), 1, "软删除后行应仍存在");
    let status = rows[0].get("status").and_then(|v| v.as_i64()).unwrap_or(-1);
    assert_eq!(status, 0, "软删除应置 status=0");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_product_update_empty_fields_returns_error_without_db() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_product_table(&pool).await;

    let fields: HashMap<String, Value> = HashMap::new();
    let result = services::product_service::ProductService::update(&pool, 1, fields).await;
    assert!(result.is_err(), "空 fields 应返回 Err");
    let msg = result.unwrap_err();
    assert!(msg.contains("未提供"), "msg: {msg}");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}
