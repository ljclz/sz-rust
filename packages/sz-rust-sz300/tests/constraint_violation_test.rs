// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 数据层 Phase 2 阶段4 约束违反集成测试
//!
//! 2026-11-05 新增（ADR 0051 §7.3 / §4.2 / §5.2）。
//!
//! ## 测试矩阵
//!
//! | 场景 | 约束类型 | 预期 MySQL error | 验证点 |
//! |------|----------|------------------|--------|
//! | 1. INSERT order 引用不存在 merchant_id | FK | 1452 | FK 拒绝孤儿行 |
//! | 2. INSERT good SET stock = -1 | CHECK | 3819 | CHECK 金额非负 |
//! | 3. INSERT order_item SET quantity = 0 | CHECK | 3819 | CHECK 数量正数 |
//! | 4. INSERT merchant SET status = 99 | CHECK | 3819 | CHECK 状态枚举 |
//! | 5. INSERT device SET merchant_id = NULL | 可空 FK | (成功) | NULLABLE 改造正确 |
//! | 6. DELETE order → order_item 自动删除 | FK CASCADE | (成功) | ON DELETE CASCADE |
//! | 7. DELETE merchant → device.merchant_id 置 NULL | FK SET NULL | (成功) | ON DELETE SET NULL |
//!
//! ## 场景2 说明：为何用 stock 而非 price
//!
//! 任务原始描述用 `good.price = -1`，但 `good.price` 是 `INT UNSIGNED NOT NULL`
//! （001_init.sql:50），插入 -1 会被 MySQL UNSIGNED 越界拒绝（error 1264 Out of
//! range），而非 CHECK 约束（error 3819）。改用 `good.stock`（`INT NOT NULL
//! DEFAULT 0`，004_good_stock.sql:15，可负），CHECK (stock >= 0)（008_amount_non_negative.sql:42）
//! 会在 stock=-1 时可靠触发 3819。两者都验证「金额非负」语义，但 stock 路径
//! 能确定性地触发 CHECK 而非 UNSIGNED 越界。
//!
//! ## 运行方式
//!
//! 需真实 MySQL 8.0.16+（CHECK 约束需 8.0.16+ 支持）：
//! ```
//! cargo test -p sz-rust-sz300 --test constraint_violation_test -- --ignored
//! ```
//! MySQL 不可达时测试静默跳过（不 fail），CI 可通过 `ensure_mysql` 预检。

#![forbid(unsafe_code)]

use std::sync::LazyLock;

use sz_rust_core::orm::{Pool, Value};
use tokio::sync::Mutex;

mod common;

/// 真实 MySQL 上串行执行，避免并发迁移/清理互踩（与其他 ignored 集成测试共享同一实例）
static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// 辅助：插入一个合法 market，返回 market_id
async fn insert_market(pool: &Pool, name: &str) -> i64 {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute_with_params(
        "INSERT INTO market (name, status) VALUES (?, 1)",
        &[Value::String(name.to_string())],
    )
    .await
    .expect("插入 market 失败");
    let rows = conn
        .query("SELECT LAST_INSERT_ID() AS id")
        .await
        .expect("查询 LAST_INSERT_ID 失败");
    rows[0]
        .get("id")
        .and_then(|v| v.as_i64())
        .expect("应包含 id 字段")
}

/// 辅助：插入一个合法 merchant，返回 merchant_id
async fn insert_merchant(pool: &Pool, market_id: i64, name: &str) -> i64 {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute_with_params(
        "INSERT INTO merchant (market_id, name, stall_no, contact_phone, category, status, bank_account, bank_name) \
         VALUES (?, ?, '', '', '', 1, '', '')",
        &[Value::I64(market_id), Value::String(name.to_string())],
    )
    .await
    .expect("插入 merchant 失败");
    let rows = conn
        .query("SELECT LAST_INSERT_ID() AS id")
        .await
        .expect("查询 LAST_INSERT_ID 失败");
    rows[0]
        .get("id")
        .and_then(|v| v.as_i64())
        .expect("应包含 id 字段")
}

/// 辅助：插入一个合法 good，返回 good_id
async fn insert_good(pool: &Pool, merchant_id: i64, name: &str, price: i64, stock: i32) -> i64 {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute_with_params(
        "INSERT INTO good (merchant_id, name, barcode, price, unit, image, status, stock) \
         VALUES (?, ?, '', ?, '斤', '', 1, ?)",
        &[
            Value::I64(merchant_id),
            Value::String(name.to_string()),
            Value::I64(price),
            Value::I32(stock),
        ],
    )
    .await
    .expect("插入 good 失败");
    let rows = conn
        .query("SELECT LAST_INSERT_ID() AS id")
        .await
        .expect("查询 LAST_INSERT_ID 失败");
    rows[0]
        .get("id")
        .and_then(|v| v.as_i64())
        .expect("应包含 id 字段")
}

/// 辅助：插入一个合法 order，返回 order_id
async fn insert_order(pool: &Pool, merchant_id: i64, order_no: &str, total_fen: i64) -> i64 {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute_with_params(
        "INSERT INTO `order` (order_no, merchant_id, total_fen, total_weight_g, item_count, status, pay_method, offline_seq) \
         VALUES (?, ?, ?, 0, 1, 0, 0, '')",
        &[
            Value::String(order_no.to_string()),
            Value::I64(merchant_id),
            Value::I64(total_fen),
        ],
    )
    .await
    .expect("插入 order 失败");
    let rows = conn
        .query("SELECT LAST_INSERT_ID() AS id")
        .await
        .expect("查询 LAST_INSERT_ID 失败");
    rows[0]
        .get("id")
        .and_then(|v| v.as_i64())
        .expect("应包含 id 字段")
}

/// 辅助：插入一个合法 device，返回 device_id（merchant_id 可空）
async fn insert_device(pool: &Pool, merchant_id: Option<i64>, device_sn: &str) -> i64 {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute_with_params(
        "INSERT INTO device (merchant_id, device_sn, device_model, fw_version, status) \
         VALUES (?, ?, 'TEST', '1.0.0', 0)",
        &[
            merchant_id.map(Value::I64).unwrap_or(Value::Null),
            Value::String(device_sn.to_string()),
        ],
    )
    .await
    .expect("插入 device 失败");
    let rows = conn
        .query("SELECT LAST_INSERT_ID() AS id")
        .await
        .expect("查询 LAST_INSERT_ID 失败");
    rows[0]
        .get("id")
        .and_then(|v| v.as_i64())
        .expect("应包含 id 字段")
}

/// 辅助：断言错误文本表示 FK 违反（1452 或 foreign key constraint）
fn assert_fk_violation(err_str: &str) {
    let hit = err_str.contains("1452") || err_str.contains("foreign key constraint");
    assert!(
        hit,
        "应触发 FK 违反（1452 或 foreign key constraint fails），实际错误: {err_str}"
    );
}

/// 辅助：断言错误文本表示 CHECK 违反（3819 或 Check constraint violated）
fn assert_check_violation(err_str: &str) {
    let hit_d = err_str.contains("3819");
    let hit_p = err_str.contains("Check constraint") && err_str.contains("violated");
    assert!(
        hit_d || hit_p,
        "应触发 CHECK 违反（3819 或 Check constraint ... violated），实际错误: {err_str}"
    );
}

// ============================================================================
// 场景1：FK 违反 — INSERT order 引用不存在的 merchant_id
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 8.0.16+"]
async fn test_fk_violation_order_nonexistent_merchant() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    // 不插入任何 merchant；直接尝试 INSERT order 引用 merchant_id=99999
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let result = conn
        .execute_with_params(
            "INSERT INTO `order` (order_no, merchant_id, total_fen, total_weight_g, item_count, status, pay_method, offline_seq) \
             VALUES ('FK-VIOL-001', 99999, 100, 0, 1, 0, 0, '')",
            &[],
        )
        .await;
    assert!(
        result.is_err(),
        "INSERT order 引用不存在的 merchant_id=99999 应被 FK 拒绝"
    );
    let err_str = result.unwrap_err().to_string();
    eprintln!("场景1 FK 违反错误: {err_str}");
    assert_fk_violation(&err_str);

    common::teardown_all(&pool).await;
    pool.close_all().await;
    println!("✅ 场景1：FK 违反（order→merchant）被 DB 正确拒绝");
}

// ============================================================================
// 场景2：CHECK 违反 — INSERT good SET stock = -1（金额非负）
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 8.0.16+"]
async fn test_check_violation_negative_amount() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let market_id = insert_market(&pool, "test-market-neg-amt").await;
    let merchant_id = insert_merchant(&pool, market_id, "test-merchant-neg-amt").await;

    // good.stock 是 INT NOT NULL（可负），CHECK (stock >= 0) 会在 stock=-1 时触发 3819
    // （见文件头「场景2 说明」：good.price 是 INT UNSIGNED，-1 触发 1264 而非 3819）
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let result = conn
        .execute_with_params(
            "INSERT INTO good (merchant_id, name, barcode, price, unit, image, status, stock) \
             VALUES (?, 'neg-stock-good', '', 100, '斤', '', 1, -1)",
            &[Value::I64(merchant_id)],
        )
        .await;
    assert!(
        result.is_err(),
        "INSERT good SET stock = -1 应被 CHECK (stock >= 0) 拒绝"
    );
    let err_str = result.unwrap_err().to_string();
    eprintln!("场景2 CHECK 违反错误: {err_str}");
    assert_check_violation(&err_str);

    common::teardown_all(&pool).await;
    pool.close_all().await;
    println!("✅ 场景2：CHECK 违反（good.stock < 0）被 DB 正确拒绝");
}

// ============================================================================
// 场景3：CHECK 违反 — INSERT order_item SET quantity = 0（数量正数）
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 8.0.16+"]
async fn test_check_violation_zero_quantity() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let market_id = insert_market(&pool, "test-market-zero-qty").await;
    let merchant_id = insert_merchant(&pool, market_id, "test-merchant-zero-qty").await;
    let order_id = insert_order(&pool, merchant_id, "ORD-ZERO-QTY-001", 100).await;

    // order_item.quantity 是 SMALLINT UNSIGNED DEFAULT 1，CHECK (quantity > 0)
    // quantity=0 是合法的 UNSIGNED 值（不触发 1264），但 CHECK (quantity > 0) 触发 3819
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let result = conn
        .execute_with_params(
            "INSERT INTO order_item (order_id, good_id, good_name, price_fen, weight_g, total_fen, quantity) \
             VALUES (?, NULL, 'zero-qty-item', 100, 0, 0, 0)",
            &[Value::I64(order_id)],
        )
        .await;
    assert!(
        result.is_err(),
        "INSERT order_item SET quantity = 0 应被 CHECK (quantity > 0) 拒绝"
    );
    let err_str = result.unwrap_err().to_string();
    eprintln!("场景3 CHECK 违反错误: {err_str}");
    assert_check_violation(&err_str);

    common::teardown_all(&pool).await;
    pool.close_all().await;
    println!("✅ 场景3：CHECK 违反（order_item.quantity = 0）被 DB 正确拒绝");
}

// ============================================================================
// 场景4：CHECK 违反 — INSERT merchant SET status = 99（状态枚举）
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 8.0.16+"]
async fn test_check_violation_invalid_status_enum() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let market_id = insert_market(&pool, "test-market-bad-status").await;

    // merchant.status 是 TINYINT DEFAULT 1，CHECK (status IN (0, 1))
    // status=99 是合法的 TINYINT（不触发越界），但 CHECK (status IN (0, 1)) 触发 3819
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let result = conn
        .execute_with_params(
            "INSERT INTO merchant (market_id, name, stall_no, contact_phone, category, status, bank_account, bank_name) \
             VALUES (?, 'bad-status-merchant', '', '', '', 99, '', '')",
            &[Value::I64(market_id)],
        )
        .await;
    assert!(
        result.is_err(),
        "INSERT merchant SET status = 99 应被 CHECK (status IN (0, 1)) 拒绝"
    );
    let err_str = result.unwrap_err().to_string();
    eprintln!("场景4 CHECK 违反错误: {err_str}");
    assert_check_violation(&err_str);

    common::teardown_all(&pool).await;
    pool.close_all().await;
    println!("✅ 场景4：CHECK 违反（merchant.status = 99）被 DB 正确拒绝");
}

// ============================================================================
// 场景5：可空 FK 允许 NULL — INSERT device SET merchant_id = NULL
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 8.0.16+"]
async fn test_nullable_fk_allows_null_device_merchant() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    // device.merchant_id 在 013_nullable_device_merchant.sql 改为 NULLABLE，
    // 021_fk_nullable.sql 加 FK ON DELETE SET NULL。NULL 应被允许（未绑定设备）
    let device_id = insert_device(&pool, None, "NULLABLE-DEVICE-001").await;
    assert!(device_id > 0, "INSERT device SET merchant_id = NULL 应成功");

    // 验证落库 merchant_id 确实是 NULL
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            "SELECT merchant_id FROM device WHERE device_id = ?",
            &[Value::I64(device_id)],
        )
        .await
        .expect("查询 device 失败");
    assert_eq!(rows.len(), 1, "应返回 1 行");
    let merchant_id_val = rows[0].get("merchant_id");
    assert!(
        merchant_id_val.is_none() || matches!(merchant_id_val, Some(Value::Null)),
        "device.merchant_id 应为 NULL，实际: {merchant_id_val:?}"
    );

    common::teardown_all(&pool).await;
    pool.close_all().await;
    println!("✅ 场景5：可空 FK 允许 NULL（device.merchant_id = NULL）验证通过");
}

// ============================================================================
// 场景6：FK CASCADE — DELETE order → order_item 自动删除
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 8.0.16+"]
async fn test_fk_cascade_delete_order_removes_items() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let market_id = insert_market(&pool, "test-market-cascade").await;
    let merchant_id = insert_merchant(&pool, market_id, "test-merchant-cascade").await;
    let good_id = insert_good(&pool, merchant_id, "cascade-good", 100, 10).await;
    let order_id = insert_order(&pool, merchant_id, "ORD-CASCADE-001", 200).await;

    // 插入 2 个 order_item 引用 order
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute_with_params(
        "INSERT INTO order_item (order_id, good_id, good_name, price_fen, weight_g, total_fen, quantity) \
         VALUES (?, ?, 'item-1', 100, 50, 100, 1)",
        &[Value::I64(order_id), Value::I64(good_id)],
    )
    .await
    .expect("插入 order_item[0] 失败");
    conn.execute_with_params(
        "INSERT INTO order_item (order_id, good_id, good_name, price_fen, weight_g, total_fen, quantity) \
         VALUES (?, ?, 'item-2', 100, 50, 100, 1)",
        &[Value::I64(order_id), Value::I64(good_id)],
    )
    .await
    .expect("插入 order_item[1] 失败");

    // 验证 order_item 有 2 行
    let rows = conn
        .query_with_params(
            "SELECT COUNT(*) AS c FROM order_item WHERE order_id = ?",
            &[Value::I64(order_id)],
        )
        .await
        .expect("COUNT order_item 失败");
    let before_count = rows[0]
        .get("c")
        .and_then(|v| v.as_i64())
        .expect("应包含 c 字段");
    assert_eq!(before_count, 2, "删除前应有 2 行 order_item");

    // DELETE order → order_item 应自动 CASCADE 删除
    conn.execute_with_params(
        "DELETE FROM `order` WHERE order_id = ?",
        &[Value::I64(order_id)],
    )
    .await
    .expect("DELETE order 失败（应触发 CASCADE）");

    // 验证 order_item 已被 CASCADE 删除
    let rows = conn
        .query_with_params(
            "SELECT COUNT(*) AS c FROM order_item WHERE order_id = ?",
            &[Value::I64(order_id)],
        )
        .await
        .expect("COUNT order_item 失败");
    let after_count = rows[0]
        .get("c")
        .and_then(|v| v.as_i64())
        .expect("应包含 c 字段");
    assert_eq!(
        after_count, 0,
        "DELETE order 后 order_item 应被 CASCADE 删除"
    );

    common::teardown_all(&pool).await;
    pool.close_all().await;
    println!("✅ 场景6：FK CASCADE（DELETE order → order_item 自动删除）验证通过");
}

// ============================================================================
// 场景7：FK SET NULL — DELETE merchant → device.merchant_id 置 NULL
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 8.0.16+"]
async fn test_fk_set_null_delete_merchant_clears_device() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let market_id = insert_market(&pool, "test-market-setnull").await;
    let merchant_id = insert_merchant(&pool, market_id, "test-merchant-setnull").await;
    let device_id = insert_device(&pool, Some(merchant_id), "SETNULL-DEVICE-001").await;

    // 验证 device.merchant_id 已绑定
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            "SELECT merchant_id FROM device WHERE device_id = ?",
            &[Value::I64(device_id)],
        )
        .await
        .expect("查询 device 失败");
    let before_mid = rows[0]
        .get("merchant_id")
        .and_then(|v| v.as_i64())
        .expect("删除前 device.merchant_id 应非 NULL");
    assert_eq!(before_mid, merchant_id, "device 应绑定到 merchant");

    // DELETE merchant → device.merchant_id 应被 SET NULL
    // 注意：merchant 被 good/order/settlement/merchant_user 引用会 RESTRICT，
    // 本测试不插入这些表，故 DELETE merchant 应成功，device.merchant_id 置 NULL
    conn.execute_with_params(
        "DELETE FROM merchant WHERE merchant_id = ?",
        &[Value::I64(merchant_id)],
    )
    .await
    .expect("DELETE merchant 失败（应触发 SET NULL on device）");

    // 验证 device.merchant_id 已被 SET NULL
    let rows = conn
        .query_with_params(
            "SELECT merchant_id FROM device WHERE device_id = ?",
            &[Value::I64(device_id)],
        )
        .await
        .expect("查询 device 失败");
    assert_eq!(rows.len(), 1, "device 行应仍存在（SET NULL 不删行）");
    let merchant_id_val = rows[0].get("merchant_id");
    assert!(
        merchant_id_val.is_none() || matches!(merchant_id_val, Some(Value::Null)),
        "DELETE merchant 后 device.merchant_id 应被 SET NULL，实际: {merchant_id_val:?}"
    );

    common::teardown_all(&pool).await;
    pool.close_all().await;
    println!("✅ 场景7：FK SET NULL（DELETE merchant → device.merchant_id 置 NULL）验证通过");
}
