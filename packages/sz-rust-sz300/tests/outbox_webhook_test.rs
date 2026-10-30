// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! Outbox HTTP Webhook 投递集成测试
//!
//! 验证「真实下游」语义：
//! | 场景 | 预期 |
//! |------|------|
//! | 下游返回 2xx | 事件置 status=1，载荷字段完整送达 |
//! | 下游返回 500 | 事件保持 status=0、attempts+1、写入 last_error 与 next_retry_at |
//! | 下游返回 404 | 与 5xx 同等对待（非 2xx 一律视为未送达） |
//!
//! 运行前确保 MySQL 运行于 127.0.0.1:3306，root/$SZ_TEST_DB_PASSWORD，sz_orm_test 数据库存在。
//! 手动运行：`cargo test -p sz-rust-sz300 --test outbox_webhook_test -- --ignored`

#![forbid(unsafe_code)]

use std::sync::LazyLock;

use axum::extract::Json;
use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;
use serde_json::Value as JsonValue;
use sz_rust_core::orm::{Pool, Value};
use sz_rust_sz300::models::{order::Order, order_item::OrderItem};
use sz_rust_sz300::services::order_service::OrderService;
use sz_rust_sz300::services::outbox::{dispatch_once, webhook_handler};
use tokio::sync::mpsc;
use tokio::sync::Mutex;

mod common;

static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// 启动本地 mock 下游，返回 (url, 收到的载荷接收端)
async fn spawn_mock(status: u16) -> (String, mpsc::UnboundedReceiver<JsonValue>) {
    let (tx, rx) = mpsc::unbounded_channel::<JsonValue>();
    let app = Router::new().route(
        "/hook",
        post(move |Json(body): Json<JsonValue>| {
            let tx = tx.clone();
            async move {
                let _ = tx.send(body);
                (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    "ok",
                )
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock 下游端口绑定失败");
    let addr = listener.local_addr().expect("获取监听地址失败");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/hook"), rx)
}

async fn insert_event(pool: &Pool, aggregate_id: &str) -> i64 {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute_with_params(
        "INSERT INTO outbox_event (aggregate_type, aggregate_id, event_type, payload) \
         VALUES ('order', ?, 'order.created', ?)",
        &[
            Value::String(aggregate_id.to_string()),
            Value::String(format!(r#"{{"order_id":{aggregate_id},"total_fen":500}}"#)),
        ],
    )
    .await
    .expect("插入 outbox 事件失败");

    let rows = conn
        .query_with_params(
            "SELECT id FROM outbox_event WHERE aggregate_id = ? ORDER BY id DESC LIMIT 1",
            &[Value::String(aggregate_id.to_string())],
        )
        .await
        .expect("查询 outbox 事件失败");
    rows.first()
        .and_then(|r| r.get("id"))
        .and_then(|v| v.as_i64())
        .expect("应能取回事件 ID")
}

/// 取列文本：DATETIME 列解码为 `Value::DateTime`，`as_str()` 不覆盖
fn text_of(v: Option<&Value>) -> Option<String> {
    v.and_then(|v| match v {
        Value::Date(s) | Value::DateTime(s) | Value::Time(s) => Some(s.clone()),
        other => other.as_str().map(str::to_string),
    })
}

async fn event_row(pool: &Pool, id: i64) -> (i64, i64, Option<String>, Option<String>) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            "SELECT status, attempts, last_error, next_retry_at FROM outbox_event WHERE id = ?",
            &[Value::I64(id)],
        )
        .await
        .expect("查询事件状态失败");
    let row = rows.first().expect("事件应存在");
    (
        row.get("status").and_then(|v| v.as_i64()).unwrap_or(-1),
        row.get("attempts").and_then(|v| v.as_i64()).unwrap_or(-1),
        text_of(row.get("last_error")),
        text_of(row.get("next_retry_at")),
    )
}

#[tokio::test]
#[ignore = "requires real MySQL"]
async fn test_webhook_2xx_marks_delivered() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let (url, mut received) = spawn_mock(200).await;
    let event_id = insert_event(&pool, "7001").await;

    let handled = dispatch_once(&pool, &webhook_handler(url), 10, 30)
        .await
        .expect("投递轮次不应报错");
    assert_eq!(handled, 1, "应成功投递 1 条事件");

    let body = tokio::time::timeout(std::time::Duration::from_secs(2), received.recv())
        .await
        .expect("mock 下游应在 2s 内收到请求")
        .expect("mock 下游应收到载荷");
    assert_eq!(body.get("id").and_then(|v| v.as_i64()), Some(event_id));
    assert_eq!(
        body.get("event_type").and_then(|v| v.as_str()),
        Some("order.created")
    );
    assert_eq!(
        body.pointer("/payload/order_id").and_then(|v| v.as_i64()),
        Some(7001),
        "payload 应作为 JSON 对象转发，而不是字符串: {body}"
    );

    let (status, attempts, last_error, _) = event_row(&pool, event_id).await;
    assert_eq!(status, 1, "2xx 后事件应置已投递");
    assert_eq!(attempts, 1, "attempts 应记录一次尝试");
    assert!(last_error.is_none(), "成功后不应残留错误");

    common::teardown_all(&pool).await;
}

#[tokio::test]
#[ignore = "requires real MySQL"]
async fn test_order_created_event_reaches_webhook() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let (url, mut received) = spawn_mock(200).await;

    let order = Order {
        order_id: None,
        order_no: "OUTBOX-E2E-001".to_string(),
        merchant_id: 1,
        device_id: 9101,
        total_fen: 500,
        total_weight_g: 200,
        item_count: 1,
        status: 1,
        pay_method: 0,
        pay_at: None,
        offline_seq: String::new(),
        created_at: None,
        updated_at: None,
    };
    let items = vec![OrderItem {
        item_id: None,
        order_id: 0,
        good_id: 1,
        good_name: "outbox-e2e-good".to_string(),
        price_fen: 500,
        weight_g: 200,
        total_fen: 500,
        quantity: 1,
    }];
    let order_id = OrderService::create(&pool, None, &order, &items)
        .await
        .expect("下单应成功");

    let handled = dispatch_once(&pool, &webhook_handler(url), 10, 30)
        .await
        .expect("投递轮次不应报错");
    assert_eq!(handled, 1, "下单写入的 order.created 应被投递");

    let body = tokio::time::timeout(std::time::Duration::from_secs(2), received.recv())
        .await
        .expect("mock 下游应在 2s 内收到请求")
        .expect("mock 下游应收到载荷");
    assert_eq!(
        body.get("event_type").and_then(|v| v.as_str()),
        Some("order.created")
    );
    assert_eq!(
        body.get("aggregate_id").and_then(|v| v.as_str()),
        Some(order_id.to_string().as_str()),
        "聚合 ID 应为订单 ID"
    );
    assert_eq!(
        body.pointer("/payload/order_id").and_then(|v| v.as_i64()),
        Some(order_id),
        "下游应拿到完整订单载荷: {body}"
    );
    assert_eq!(
        body.pointer("/payload/total_fen").and_then(|v| v.as_i64()),
        Some(500)
    );

    let event_id = body
        .get("id")
        .and_then(|v| v.as_i64())
        .expect("载荷应带 outbox 事件 ID");
    let (status, attempts, last_error, _) = event_row(&pool, event_id).await;
    assert_eq!(status, 1, "送达后事件应置已投递");
    assert_eq!(attempts, 1);
    assert!(last_error.is_none());

    common::teardown_all(&pool).await;
}

#[tokio::test]
#[ignore = "requires real MySQL"]
async fn test_webhook_5xx_keeps_retryable() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let (url, _received) = spawn_mock(500).await;
    let event_id = insert_event(&pool, "7002").await;

    let handled = dispatch_once(&pool, &webhook_handler(url), 10, 30)
        .await
        .expect("投递轮次不应因单事件失败而报错");
    assert_eq!(handled, 0, "非 2xx 不应计入成功");

    let (status, attempts, last_error, next_retry_at) = event_row(&pool, event_id).await;
    assert_eq!(status, 0, "5xx 后事件应保持待投递");
    assert_eq!(attempts, 1, "attempts 应 +1");
    assert!(last_error.is_some(), "应记录失败原因");
    assert!(next_retry_at.is_some(), "应写入退避时间");

    common::teardown_all(&pool).await;
}

#[tokio::test]
#[ignore = "requires real MySQL"]
async fn test_webhook_404_treated_as_failure() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match common::ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    common::setup_all_tables(&pool).await;

    let (url, _received) = spawn_mock(404).await;
    let event_id = insert_event(&pool, "7003").await;

    let handled = dispatch_once(&pool, &webhook_handler(url), 10, 30)
        .await
        .expect("投递轮次不应报错");
    assert_eq!(handled, 0, "404 不得视为送达");

    let (status, attempts, last_error, _) = event_row(&pool, event_id).await;
    assert_eq!(status, 0, "404 后事件应保持待投递");
    assert_eq!(attempts, 1);
    assert!(
        last_error.as_deref().unwrap_or("").contains("404"),
        "错误信息应包含状态码，实际: {last_error:?}"
    );

    common::teardown_all(&pool).await;
}
